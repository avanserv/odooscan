//! Flows that cross from one method into another: a value from a source passed to a method whose
//! parameter reaches a sink, or a method that returns a source's value into a sink of its caller.

use std::path::Path;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::checks::Out;
use crate::facts::{
    ArgSlot, CallSite, FlowEdge, FlowOrigin, FlowSummary, FlowTarget, Pos, RawDiag, Receiver, RunsAs, Spanned, Step,
};
use crate::index::{FlowRef, Index, Scope};
use crate::taint;

/// How many methods deep a value is followed from the call that first passes or returns it.
const DEPTH: u8 = 4;

/// A sink that a parameter of a method reaches.
#[derive(Clone)]
struct Hit<'m> {
    code: &'static str,
    keys: bool,
    path: &'m Path,
    pos: Pos,
    /// The call that receives the value.
    callee: String,
    /// From the parameter, through the methods it is passed to, to the sink.
    steps: Vec<Step>,
    /// The locals and methods in between, for the message.
    names: Vec<String>,
}

/// What a return of a method carries: a source's value, or a parameter of the method.
#[derive(Clone)]
struct Ret {
    origin: FlowOrigin,
    keys: bool,
    /// The source or parameter, as the message names it.
    label: String,
    /// From the origin, through the methods whose return it returns, to the return.
    steps: Vec<Step>,
    /// The locals and methods in between, for the message.
    names: Vec<String>,
}

pub fn check<'m>(index: &Index<'m>, mi: usize, scope: &Scope, out: &mut Out<'m>) {
    let module = &index.modules[mi];
    let mut solver = Solver {
        index,
        scope,
        memo: FxHashMap::default(),
        returns: FxHashMap::default(),
    };
    let mut seen = FxHashSet::default();
    for file in &module.python {
        for summary in &file.facts.flows {
            let at = FlowRef {
                module: mi,
                file,
                summary,
            };
            for (path, diag) in solver.findings(at) {
                if seen.insert((path, diag.pos, diag.code)) {
                    out.diags.push((path, diag));
                }
            }
        }
    }
}

struct Solver<'a, 'm> {
    index: &'a Index<'m>,
    scope: &'a Scope,
    /// By summary, parameter, and whether the method runs as superuser.
    memo: FxHashMap<(usize, usize, bool), Vec<Hit<'m>>>,
    /// By summary.
    returns: FxHashMap<usize, Vec<Ret>>,
}

impl<'m> Solver<'_, 'm> {
    /// The flows from a source in `at` that cross into another method.
    fn findings(&mut self, at: FlowRef<'m>) -> Vec<(&'m Path, RawDiag)> {
        let mut found = vec![];
        for edge in &at.summary.edges {
            match (edge.origin, &edge.target) {
                (FlowOrigin::Source, FlowTarget::Arg(c, slot)) => {
                    for hit in self.passed(at, edge, *c, slot, 0, false) {
                        found.extend(self.reported(at, edge, *c, hit));
                    }
                }
                (FlowOrigin::Return(c), FlowTarget::Sink(code)) => found.extend(self.returned(at, edge, c, code)),
                _ => {}
            }
        }
        found
    }

    /// A finding at the sink when the checked module holds it, otherwise at the call that leaves
    /// the module, where its author can act.
    fn reported(&self, at: FlowRef<'m>, edge: &FlowEdge, c: usize, hit: Hit<'m>) -> Option<(&'m Path, RawDiag)> {
        if hit.code == "OD704" && !hit.keys {
            return None;
        }
        let names: Vec<&str> = hit.names.iter().map(String::as_str).collect();
        let message = taint::message(hit.code, &hit.callee, &edge.steps[0].value, &names);
        let module = &self.index.modules[at.module];
        let (path, pos) = if module.python.iter().any(|f| f.path == hit.path) {
            (hit.path, hit.pos)
        } else {
            (at.file.path.as_path(), at.summary.calls[c].pos)
        };
        Some((path, RawDiag::new(hit.code, pos, message).with_flow(hit.steps)))
    }

    /// The definitions a call may run: every override of a model's method in scope, or the
    /// method of the same Python class.
    fn callees(&self, from: FlowRef<'m>, call: &CallSite) -> Vec<FlowRef<'m>> {
        match &call.receiver {
            Receiver::Model(model) => self
                .index
                .method_flows(model, &call.method)
                .iter()
                .filter(|f| self.scope.sees(f.module))
                .copied()
                .collect(),
            Receiver::Class(_) => from
                .file
                .facts
                .flows
                .iter()
                .filter(|s| s.method == call.method && s.owner.as_ref() == Some(&call.receiver))
                .map(|summary| FlowRef { summary, ..from })
                .collect(),
        }
    }

    /// The sinks a value reaches once `edge` passes it to the call `c` of `at`, which runs as
    /// superuser when `sudo` holds.
    fn passed(
        &mut self,
        at: FlowRef<'m>,
        edge: &FlowEdge,
        c: usize,
        slot: &ArgSlot,
        depth: u8,
        sudo: bool,
    ) -> Vec<Hit<'m>> {
        let call = &at.summary.calls[c];
        let sudo = match call.runs_as {
            RunsAs::Superuser => true,
            RunsAs::AsCaller => sudo,
            RunsAs::User => false,
        };
        let mut hits = vec![];
        for callee in self.callees(at, call) {
            let Some(param) = param_index(callee.summary, slot) else {
                continue;
            };
            for hit in self.hits(callee, param, depth + 1, sudo) {
                hits.push(prepend(at, edge, call, hit));
            }
        }
        hits
    }

    /// The sinks parameter `param` of a method reaches, in it or in the methods it passes it to.
    /// What the method writes on its own records is one only when it runs as superuser.
    fn hits(&mut self, at: FlowRef<'m>, param: usize, depth: u8, sudo: bool) -> Vec<Hit<'m>> {
        let key = (std::ptr::from_ref(at.summary) as usize, param, sudo);
        if let Some(hits) = self.memo.get(&key) {
            return hits.clone();
        }
        // A method that calls itself back finds this empty entry and stops.
        self.memo.insert(key, vec![]);
        let mut hits = vec![];
        for edge in at.summary.edges.iter().filter(|e| e.origin == FlowOrigin::Param(param)) {
            match &edge.target {
                FlowTarget::Sink(code) => hits.push(sink_hit(at, edge, code)),
                FlowTarget::SelfWrite if sudo => hits.push(sink_hit(at, edge, "OD704")),
                FlowTarget::Arg(c, slot) if depth < DEPTH => hits.extend(self.passed(at, edge, *c, slot, depth, sudo)),
                _ => {}
            }
        }
        self.memo.insert(key, hits.clone());
        hits
    }

    /// Findings for a sink of `at` that receives what the call `c` returns: a source's value the
    /// callee returns, or a source's value `at` passes to it and it returns.
    fn returned(&mut self, at: FlowRef<'m>, edge: &FlowEdge, c: usize, code: &'static str) -> Vec<(&'m Path, RawDiag)> {
        let mut found = vec![];
        for callee in self.callees(at, &at.summary.calls[c]) {
            for ret in self.returns(callee, 1) {
                for arg in entries(at.summary, c, callee.summary, &ret) {
                    if arg.is_none_or(|a| a.origin == FlowOrigin::Source) {
                        found.extend(return_finding(at, arg, &ret, edge, code));
                    }
                }
            }
        }
        found
    }

    /// What each return of `at` carries, following a call's value it returns while `depth` is
    /// below `DEPTH`.
    fn returns(&mut self, at: FlowRef<'m>, depth: u8) -> Vec<Ret> {
        let key = std::ptr::from_ref(at.summary) as usize;
        if let Some(rets) = self.returns.get(&key) {
            return rets.clone();
        }
        // A method that returns its own call finds this empty entry and stops.
        self.returns.insert(key, vec![]);
        let mut rets = vec![];
        for edge in at.summary.edges.iter().filter(|e| e.target == FlowTarget::Return) {
            match edge.origin {
                FlowOrigin::Return(c) if depth < DEPTH => rets.extend(self.returned_by(at, edge, c, depth)),
                FlowOrigin::Return(_) => {}
                origin => rets.push(Ret {
                    origin,
                    keys: edge.keys,
                    label: edge.steps[0].value.clone(),
                    steps: path_steps(at, &edge.steps),
                    names: edge.steps[1..].iter().map(|s| s.value.clone()).collect(),
                }),
            }
        }
        self.returns.insert(key, rets.clone());
        rets
    }

    /// What `at` returns by `edge` of what the call `c` returns, in terms of `at`.
    fn returned_by(&mut self, at: FlowRef<'m>, edge: &FlowEdge, c: usize, depth: u8) -> Vec<Ret> {
        let mut rets = vec![];
        for callee in self.callees(at, &at.summary.calls[c]) {
            for ret in self.returns(callee, depth + 1) {
                for arg in entries(at.summary, c, callee.summary, &ret) {
                    rets.push(through(at, arg, &ret, edge));
                }
            }
        }
        rets
    }
}

/// How the value `ret` carries enters the call `c` of `at`: from a source of the callee (`None`),
/// or by each edge of `at` that passes a source's value or a parameter to the callee's parameter.
fn entries<'s>(caller: &'s FlowSummary, c: usize, callee: &FlowSummary, ret: &Ret) -> Vec<Option<&'s FlowEdge>> {
    let FlowOrigin::Param(k) = ret.origin else {
        return vec![None];
    };
    let passed = caller.edges.iter().filter(|a| {
        !matches!(a.origin, FlowOrigin::Return(_))
            && matches!(&a.target, FlowTarget::Arg(ac, slot) if *ac == c && param_index(callee, slot) == Some(k))
    });
    passed.map(Some).collect()
}

/// A value that enters `at` by `arg`, or from a source of the callee, leaves the callee as `ret`
/// carries it, and goes on by `edge` from the call.
fn through(at: FlowRef<'_>, arg: Option<&FlowEdge>, ret: &Ret, edge: &FlowEdge) -> Ret {
    let names_of = |steps: &[Spanned<String>]| steps.iter().map(|s| s.value.clone()).collect::<Vec<_>>();
    let mut steps = arg.map(|a| path_steps(at, &a.steps)).unwrap_or_default();
    steps.extend(ret.steps.iter().cloned());
    steps.extend(path_steps(at, &edge.steps));
    let mut names = arg.map(|a| names_of(&a.steps[1..])).unwrap_or_default();
    names.push(edge.steps[0].value.clone());
    names.extend(ret.names.iter().cloned());
    names.extend(names_of(&edge.steps[1..]));
    Ret {
        origin: arg.map_or(ret.origin, |a| a.origin),
        keys: arg.is_none_or(|a| a.keys) && ret.keys && edge.keys,
        label: arg.map_or_else(|| ret.label.clone(), |a| a.steps[0].value.clone()),
        steps,
        names,
    }
}

/// The flow from a source, through `arg` into the callee when the callee returns its parameter,
/// out of the callee as `ret` carries it, and on to the sink that ends `edge`.
fn return_finding<'m>(
    at: FlowRef<'m>,
    arg: Option<&FlowEdge>,
    ret: &Ret,
    edge: &FlowEdge,
    code: &'static str,
) -> Option<(&'m Path, RawDiag)> {
    let mut flow = through(at, arg, ret, edge);
    if code == "OD704" && !flow.keys {
        return None;
    }
    let sink = edge.steps.last()?;
    flow.names.pop();
    let names: Vec<&str> = flow.names.iter().map(String::as_str).collect();
    let message = taint::message(code, &sink.value, &flow.label, &names);
    Some((
        at.file.path.as_path(),
        RawDiag::new(code, sink.pos, message).with_flow(flow.steps),
    ))
}

fn param_index(summary: &FlowSummary, slot: &ArgSlot) -> Option<usize> {
    match slot {
        ArgSlot::Pos(i) => (*i < summary.params.len()).then_some(*i),
        ArgSlot::Name(name) => summary.params.iter().position(|p| p == name),
    }
}

fn path_steps(at: FlowRef<'_>, steps: &[Spanned<String>]) -> Vec<Step> {
    steps
        .iter()
        .map(|s| Step {
            path: Some(at.file.path.clone()),
            pos: s.pos,
            label: s.value.clone(),
        })
        .collect()
}

fn sink_hit<'m>(at: FlowRef<'m>, edge: &FlowEdge, code: &'static str) -> Hit<'m> {
    let last = edge.steps.len() - 1;
    Hit {
        code,
        keys: edge.keys,
        path: &at.file.path,
        pos: edge.steps[last].pos,
        callee: edge.steps[last].value.clone(),
        steps: path_steps(at, &edge.steps),
        names: edge.steps[1..last].iter().map(|s| s.value.clone()).collect(),
    }
}

/// A hit in a callee, seen from the caller that passes it the value through `edge`.
fn prepend<'m>(at: FlowRef<'m>, edge: &FlowEdge, call: &CallSite, mut hit: Hit<'m>) -> Hit<'m> {
    let mut steps = path_steps(at, &edge.steps);
    steps.push(Step {
        path: Some(at.file.path.clone()),
        pos: call.pos,
        label: format!("{}()", call.method),
    });
    steps.append(&mut hit.steps);
    let mut names: Vec<String> = edge.steps[1..].iter().map(|s| s.value.clone()).collect();
    names.push(format!("{}()", call.method));
    names.append(&mut hit.names);
    Hit {
        keys: hit.keys && edge.keys,
        steps,
        names,
        ..hit
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::check;
    use crate::checks::Out;
    use crate::index::Index;
    use crate::manifest;
    use crate::mentions::Mentions;
    use crate::project::{Module, SourceFile};
    use crate::python;

    /// The cross-method findings of one module holding the route `route` and the model `x.y` with
    /// the methods `methods`, as `file:line code message` up to the advice.
    fn findings(route: &str, methods: &str) -> Vec<String> {
        let route = format!("class C(http.Controller):\n    @http.route('/r')\n{route}");
        let model = format!("class X(models.Model):\n    _name = 'x.y'\n\n{methods}");
        let file = |name: &str, src: &str| SourceFile {
            path: PathBuf::from(name),
            rel: name.to_owned(),
            rank: None,
            demo: false,
            facts: python::analyze(src, None),
            suppressions: Default::default(),
            mentions: Mentions::scan(src),
        };
        let manifest = manifest::parse("{'version': '18.0.1.0'}").unwrap();
        let module = Module {
            name: "main".to_owned(),
            dir: PathBuf::new(),
            manifest_path: PathBuf::new(),
            series: manifest.series(),
            manifest,
            checked: true,
            python: vec![file("route.py", &route), file("model.py", &model)],
            data: vec![],
            js: vec![],
            templates: vec![],
            sql_views: vec![],
            lost_text: false,
        };
        let modules = [module];
        let index = Index::build(&modules, None);
        let mut out = Out { diags: vec![] };
        check(&index, 0, &index.scope(0), &mut out);
        out.diags
            .iter()
            .map(|(path, d)| {
                let message = d.message.split(';').next().unwrap_or_default();
                let message = message.split(", so ").next().unwrap_or_default();
                format!("{}:{} {} {message}", path.display(), d.pos.line, d.code)
            })
            .collect()
    }

    #[test]
    fn a_method_called_under_sudo_writes_its_records_as_superuser() {
        let apply = "    def _apply(self, vals):\n        self.write(vals)\n";
        let call = |receiver: &str| {
            findings(
                &format!("    def r(self, **post):\n        {receiver}._apply(post)\n"),
                apply,
            )
        };
        assert_eq!(
            call("request.env['x.y'].sudo()"),
            ["model.py:5 OD704 route parameter 'post' reaches self.write() through '_apply()'"]
        );
        assert!(call("request.env['x.y']").is_empty());
        assert!(call("request.env['x.y'].sudo().with_user(1)").is_empty());
    }

    #[test]
    fn sudo_reaches_the_methods_called_on_self() {
        let methods =
            "    def _apply(self, vals):\n        CALL\n\n    def _store(self, vals):\n        self.write(vals)\n";
        let call = |call: &str| {
            findings(
                "    def r(self, **post):\n        request.env['x.y'].sudo()._apply(post)\n",
                &methods.replace("CALL", call),
            )
        };
        assert_eq!(
            call("self._store(vals)"),
            ["model.py:8 OD704 route parameter 'post' reaches self.write() through '_apply()', '_store()'"]
        );
        assert!(call("self.with_user(1)._store(vals)").is_empty());
    }

    /// Methods `_m1` to `_m<n>` of `x.y`, each returning the next and the last `last`.
    fn chain(n: usize, last: &str) -> String {
        let method = |i: usize| match i {
            i if i == n => format!("    def _m{i}(self, v):\n        return {last}\n"),
            i => format!("    def _m{i}(self, v):\n        return self._m{}(v)\n", i + 1),
        };
        (1..=n).map(method).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn a_source_is_returned_through_three_methods() {
        let route = "    def r(self):\n        return Markup(request.env['x.y']._m1(1))\n";
        assert_eq!(
            findings(route, &chain(3, "request.params.get('x')")),
            ["route.py:4 OD703 request.params reaches Markup() through '_m1()', '_m2()', '_m3()' unescaped"]
        );
    }

    #[test]
    fn a_parameter_is_returned_through_two_methods() {
        let route = "    def r(self, term):\n        return Markup(request.env['x.y']._m1(term))\n";
        assert_eq!(
            findings(route, &chain(2, "'<p>%s</p>' % v")),
            ["route.py:4 OD703 route parameter 'term' reaches Markup() through '_m1()', '_m2()' unescaped"]
        );
    }

    #[test]
    fn returns_are_followed_four_calls_deep() {
        let route = "    def r(self):\n        return Markup(request.env['x.y']._m1(1))\n";
        assert_eq!(findings(route, &chain(4, "request.params.get('x')")).len(), 1);
        assert!(findings(route, &chain(5, "request.params.get('x')")).is_empty());
        assert!(findings(route, &chain(1, "self._m1(v)")).is_empty());
    }
}
