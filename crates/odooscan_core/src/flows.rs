//! Flows that cross from one method into another: a value from a source passed to a method whose
//! parameter reaches a sink, or a method that returns a source's value into a sink of its caller.

use std::path::Path;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::checks::Out;
use crate::facts::{
    ArgSlot, CallSite, FlowEdge, FlowOrigin, FlowSummary, FlowTarget, Pos, RawDiag, Receiver, RunsAs, Safe, Spanned,
    Step,
};
use crate::index::{FlowRef, Index, Scope};
use crate::taint;

/// How many methods deep a value is followed from the call that first passes or returns it.
/// Each nested call is one deeper, so a memo key is never re-entered while it is computed.
const DEPTH: u8 = 4;

/// A method a hit passes through: its summary, the parameter holding the value, and whether it
/// runs as superuser. A path that comes back to one is a loop, and cutting the loop leaves a
/// shorter path from the same method, with the same sinks, that is kept instead.
type HitState = (usize, usize, bool);

/// A method a return passes through, by summary and what the value is in it.
type RetState = (usize, FlowOrigin);

/// A sink that a parameter of a method reaches.
#[derive(Clone)]
struct Hit<'m> {
    code: &'static str,
    keys: bool,
    /// The codes the value is harmless for at some step, so harmless at the sink.
    safe: Safe,
    /// The value at the sink is the `name` of the parameter's own value.
    named: bool,
    path: &'m Path,
    pos: Pos,
    /// The call that receives the value.
    callee: String,
    /// From the parameter, through the methods it is passed to, to the sink.
    steps: Vec<Step>,
    /// The locals and methods in between, for the message.
    names: Vec<String>,
    via: Vec<HitState>,
    /// The path from an RPC entry crosses the RPC argument of a public method that is its own entry.
    relayed: bool,
}

/// What a return of a method carries: a source's value, or a parameter of the method.
#[derive(Clone)]
struct Ret {
    origin: FlowOrigin,
    keys: bool,
    safe: Safe,
    /// The source or parameter, as the message names it.
    label: String,
    /// From the origin, through the methods whose return it returns, to the return.
    steps: Vec<Step>,
    /// The locals and methods in between, for the message.
    names: Vec<String>,
    via: Vec<RetState>,
}

pub fn check<'m>(index: &Index<'m>, mi: usize, scope: &Scope, out: &mut Out<'m>) {
    let module = &index.modules[mi];
    let mut solver = Solver::new(index, scope);
    let mut found = vec![];
    for file in &module.python {
        for summary in &file.facts.flows {
            let at = FlowRef {
                module: mi,
                file,
                summary,
            };
            found.extend(solver.findings(at));
        }
    }
    // An RPC entry reaching a sink through another one only repeats the callee's own finding, so
    // it fills a slot that nothing else reports and never renames a finding the callee reports.
    found.sort_by_key(|f| f.0);
    let mut seen = FxHashSet::default();
    for (_, (path, diag)) in found {
        if seen.insert((path, diag.pos, diag.code)) {
            out.diags.push((path, diag));
        }
    }
}

struct Solver<'a, 'm> {
    index: &'a Index<'m>,
    scope: &'a Scope,
    /// By summary, parameter, whether the method runs as superuser, whether the entry is an RPC
    /// argument, and depth: a result found with fewer calls left would hide a deeper sink from a
    /// caller that has more.
    memo: FxHashMap<(usize, usize, bool, bool, u8), Vec<Hit<'m>>>,
    /// By summary and depth, for the same reason.
    returns: FxHashMap<(usize, u8), Vec<Ret>>,
}

impl<'a, 'm> Solver<'a, 'm> {
    fn new(index: &'a Index<'m>, scope: &'a Scope) -> Self {
        Solver {
            index,
            scope,
            memo: FxHashMap::default(),
            returns: FxHashMap::default(),
        }
    }

    /// The flows from a source in `at` that cross into another method, each with whether it is an
    /// RPC entry's flow through another RPC entry, which `check` keeps only when nothing else
    /// reports its sink.
    fn findings(&mut self, at: FlowRef<'m>) -> Vec<(bool, (&'m Path, RawDiag))> {
        let mut found = vec![];
        for edge in &at.summary.edges {
            match (edge.origin, &edge.target) {
                (origin, FlowTarget::Arg(c, _)) if origin.is_user() => {
                    let rpc = matches!(origin, FlowOrigin::Rpc(_));
                    for hit in self.passed(at, edge, 0, false, rpc) {
                        let relayed = hit.relayed;
                        found.extend(self.reported(at, edge, *c, hit).map(|f| (relayed, f)));
                    }
                }
                (FlowOrigin::Return(c), FlowTarget::Sink(code)) => {
                    let returned = self.returned(at, edge, c, code);
                    found.extend(returned.into_iter().map(|f| (false, f)));
                }
                _ => {}
            }
        }
        found
    }

    /// A finding at the sink when the checked module holds it, otherwise at the call that leaves
    /// the module, where its author can act.
    fn reported(&self, at: FlowRef<'m>, edge: &FlowEdge, c: usize, hit: Hit<'m>) -> Option<(&'m Path, RawDiag)> {
        if (hit.code == "OD704" && !hit.keys) || hit.safe.has(hit.code) {
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

    /// The sinks a value reaches once `edge` passes it to a call of `at`, which runs as superuser
    /// when `sudo` holds. `rpc` holds when the value entered as an RPC argument.
    fn passed(&mut self, at: FlowRef<'m>, edge: &FlowEdge, depth: u8, sudo: bool, rpc: bool) -> Vec<Hit<'m>> {
        let FlowTarget::Arg(c, slot) = &edge.target else {
            return vec![];
        };
        let call = &at.summary.calls[*c];
        let sudo = match call.runs_as {
            RunsAs::Superuser => true,
            RunsAs::AsCaller => sudo,
            RunsAs::User => false,
        };
        // The call's own `SelfWrite` or sink already reports the mapping a superuser `write()` or
        // `create()` receives, so what its overrides do with it would report it twice.
        let mapped = sudo && matches!(call.method.as_str(), "write" | "create");
        let mut hits = vec![];
        for callee in self.callees(at, call) {
            let Some(param) = param_index(callee.summary, slot) else {
                continue;
            };
            // The `name` of a file picked by its field is that field, which the code wrote.
            let hits_of = self.hits(callee, param, depth + 1, sudo, rpc);
            let kept = hits_of
                .into_iter()
                .filter(|h| !(edge.upload && h.named) && !(mapped && h.code == "OD704"));
            hits.extend(kept.map(|hit| prepend(at, edge, call, hit)));
        }
        hits
    }

    /// The sinks parameter `param` of a method reaches, in it or in the methods it passes it to.
    /// What the method writes on its own records is one only when it runs as superuser. A public
    /// method's parameter is followed too, for the Python callers that fill it. `rpc` holds when
    /// the value entered as an RPC argument, and then a hit through a public method's own
    /// parameter is `relayed`.
    fn hits(&mut self, at: FlowRef<'m>, param: usize, depth: u8, sudo: bool, rpc: bool) -> Vec<Hit<'m>> {
        let state = (std::ptr::from_ref(at.summary) as usize, param, sudo);
        let key = (state.0, param, sudo, rpc, depth);
        if let Some(hits) = self.memo.get(&key) {
            return hits.clone();
        }
        let mut hits = vec![];
        for edge in at.summary.edges.iter().filter(|e| e.origin.param() == Some(param)) {
            let from = hits.len();
            match &edge.target {
                FlowTarget::Sink(code) => hits.push(sink_hit(at, edge, code)),
                FlowTarget::SelfWrite if sudo => hits.push(sink_hit(at, edge, "OD704")),
                FlowTarget::Arg(..) if depth < DEPTH => hits.extend(self.passed(at, edge, depth, sudo, rpc)),
                _ => {}
            }
            // Only an RPC entry tells a relayed path apart; from any other the shortest path wins.
            if rpc && matches!(edge.origin, FlowOrigin::Rpc(_)) {
                hits[from..].iter_mut().for_each(|h| h.relayed = true);
            }
        }
        hits.retain_mut(|h| enter(&mut h.via, state));
        // A relayed path must not displace one that is not, which `check` prefers.
        let same = |a: &Hit, b: &Hit| {
            (a.code, a.path, a.pos, a.keys, a.safe, a.named, a.relayed)
                == (b.code, b.path, b.pos, b.keys, b.safe, b.named, b.relayed)
        };
        let hits = distinct(hits, same, |h| h.via.len());
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
                    if arg.is_none_or(|a| a.origin.is_user()) {
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
        let summary = std::ptr::from_ref(at.summary) as usize;
        let key = (summary, depth);
        if let Some(rets) = self.returns.get(&key) {
            return rets.clone();
        }
        let mut rets = vec![];
        for edge in at.summary.edges.iter().filter(|e| e.target == FlowTarget::Return) {
            match edge.origin {
                FlowOrigin::Return(c) if depth < DEPTH => rets.extend(self.returned_by(at, edge, c, depth)),
                FlowOrigin::Return(_) => {}
                origin => rets.push(Ret {
                    origin,
                    keys: edge.keys,
                    safe: edge.safe,
                    label: edge.steps[0].value.clone(),
                    steps: path_steps(at, &edge.steps),
                    names: edge.steps[1..].iter().map(|s| s.value.clone()).collect(),
                    via: vec![],
                }),
            }
        }
        rets.retain_mut(|r| enter(&mut r.via, (summary, r.origin)));
        let same = |a: &Ret, b: &Ret| (a.origin, &a.label, a.keys, a.safe) == (b.origin, &b.label, b.keys, b.safe);
        let rets = distinct(rets, same, |r| r.via.len());
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

/// Adds `state` to the methods a path passes through and returns true, or returns false when the
/// path already passed through it, so is a loop.
fn enter<S: PartialEq>(via: &mut Vec<S>, state: S) -> bool {
    let fresh = !via.contains(&state);
    if fresh {
        via.push(state);
    }
    fresh
}

/// Keeps one of the items `same` finds alike, the first unless a later one passes through fewer
/// methods. A value joined from several calls has an edge for each, so without it the paths to one
/// sink grow with the product of the calls at each depth.
fn distinct<T>(items: Vec<T>, same: impl Fn(&T, &T) -> bool, via: impl Fn(&T) -> usize) -> Vec<T> {
    let mut kept: Vec<T> = vec![];
    for item in items {
        match kept.iter_mut().find(|k| same(k, &item)) {
            Some(k) if via(&item) < via(k) => *k = item,
            Some(_) => {}
            None => kept.push(item),
        }
    }
    kept
}

/// How the value `ret` carries enters the call `c` of `caller`: from a source of the callee
/// (`None`), or by each edge of `caller` that passes a source's value or a parameter to the
/// callee's parameter. A public method's parameter is one here too: a Python call, not a client,
/// sets it.
fn entries<'s>(caller: &'s FlowSummary, c: usize, callee: &FlowSummary, ret: &Ret) -> Vec<Option<&'s FlowEdge>> {
    let Some(k) = ret.origin.param() else {
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
        safe: arg.map_or(ret.safe, |a| a.safe.union(ret.safe)).union(edge.safe),
        label: arg.map_or_else(|| ret.label.clone(), |a| a.steps[0].value.clone()),
        steps,
        names,
        via: ret.via.clone(),
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
    if (code == "OD704" && !flow.keys) || flow.safe.has(code) {
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
        safe: edge.safe,
        named: edge.named,
        path: &at.file.path,
        pos: edge.steps[last].pos,
        callee: edge.steps[last].value.clone(),
        steps: path_steps(at, &edge.steps),
        names: edge.steps[1..last].iter().map(|s| s.value.clone()).collect(),
        via: vec![],
        relayed: false,
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
        safe: hit.safe.union(edge.safe),
        // Only the parameter's own value, passed on, makes the callee's `name` that of the caller's.
        named: edge.named || (hit.named && edge.own),
        steps,
        names,
        ..hit
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Solver, check};
    use crate::checks::Out;
    use crate::facts::{PyFacts, RawDiag};
    use crate::index::{FlowRef, Index};
    use crate::mentions::Mentions;
    use crate::project::{Module, SourceFile};
    use crate::python;

    /// A checked module holding the route `route` and the model `x.y` with the methods `methods`.
    fn module(route: &str, methods: &str) -> Module {
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
        let mut module = crate::project::module("main", "{'version': '18.0.1.0'}");
        module.python = vec![file("route.py", &route), file("model.py", &model)];
        module
    }

    /// A finding as `file:line code message` up to the advice.
    fn shown((path, d): &(&Path, RawDiag)) -> String {
        let message = d.message.split(';').next().unwrap_or_default();
        let message = message.split(", so ").next().unwrap_or_default();
        format!("{}:{} {} {message}", path.display(), d.pos.line, d.code)
    }

    /// Each cross-method finding of `module(route, methods)`, mapped by `each`.
    fn scanned<T>(route: &str, methods: &str, each: impl Fn(&(&Path, RawDiag)) -> T) -> Vec<T> {
        let modules = [module(route, methods)];
        let index = Index::build(&modules, None);
        let mut out = Out { diags: vec![] };
        check(&index, 0, &index.scope(0), &mut out);
        out.diags.iter().map(each).collect()
    }

    /// The cross-method findings of `module(route, methods)`.
    fn scan_messages(route: &str, methods: &str) -> Vec<String> {
        scanned(route, methods, shown)
    }

    /// The findings of `main` and of a module `dep` it depends on, which holds only `dep_file`, each
    /// module's as `file:line code origin`.
    fn scan_with_dep(mut main: Module, dep_file: SourceFile<PyFacts>) -> Vec<Vec<String>> {
        let mut dep = crate::project::module("dep", "{'version': '18.0.1.0'}");
        dep.python = vec![dep_file];
        main.manifest = crate::manifest::parse("{'version': '18.0.1.0', 'depends': ['dep']}").unwrap();
        let modules = [main, dep];
        let index = Index::build(&modules, None);
        (0..modules.len())
            .map(|mi| {
                let mut out = Out { diags: vec![] };
                check(&index, mi, &index.scope(mi), &mut out);
                out.diags.iter().map(|d| origin(&shown(d))).collect()
            })
            .collect()
    }

    /// The flow step labels of each cross-method finding of `module(route, methods)`.
    fn flows(route: &str, methods: &str) -> Vec<Vec<String>> {
        scanned(route, methods, |(_, d)| {
            d.flow.iter().map(|s| s.label.clone()).collect()
        })
    }

    #[test]
    fn a_method_called_under_sudo_writes_its_records_as_superuser() {
        let apply = "    def _apply(self, vals):\n        self.write(vals)\n";
        let call = |receiver: &str| {
            scan_messages(
                &format!("    def r(self, **post):\n        {receiver}._apply(post)\n"),
                apply,
            )
        };
        assert_eq!(
            call("request.env['x.y'].sudo()"),
            ["model.py:5 OD704 route parameter 'post' reaches self.write() through '_apply()'"]
        );
        assert!(call("request.env['x.y']").is_empty());
        assert!(call("request.env['x.y'].sudo().with_user(request.env.user)").is_empty());
        assert_eq!(
            call("request.env['x.y'].sudo().with_user(SUPERUSER_ID)"),
            ["model.py:5 OD704 route parameter 'post' reaches self.write() through '_apply()'"]
        );
        assert_eq!(
            call("request.env['x.y'].with_user(request.env.user).sudo()"),
            ["model.py:5 OD704 route parameter 'post' reaches self.write() through '_apply()'"]
        );
        // Only a parameter's origin leaves the callee, so `docs/rules/index.md` (OD7: data flows)
        // says a mapping the method reads from the request itself is not followed.
        let own = scan_messages(
            "    def r(self, **post):\n        request.env['x.y'].sudo()._own()\n",
            "    def _own(self):\n        self.write(request.params)\n",
        );
        assert!(own.is_empty());
    }

    #[test]
    fn sudo_reaches_the_methods_called_on_self() {
        let methods =
            "    def _apply(self, vals):\n        CALL\n\n    def _store(self, vals):\n        self.write(vals)\n";
        let call = |call: &str| {
            scan_messages(
                "    def r(self, **post):\n        request.env['x.y'].sudo()._apply(post)\n",
                &methods.replace("CALL", call),
            )
        };
        assert_eq!(
            call("self._store(vals)"),
            ["model.py:8 OD704 route parameter 'post' reaches self.write() through '_apply()', '_store()'"]
        );
        assert!(call("self.with_user(self.env.user)._store(vals)").is_empty());
        assert_eq!(
            call("self.with_user(SUPERUSER_ID)._store(vals)"),
            ["model.py:8 OD704 route parameter 'post' reaches self.write() through '_apply()', '_store()'"]
        );
    }

    /// Methods `_m1` to `_m<n>` of `x.y`, each returning the next and the last `last`.
    fn chain(n: usize, last: &str) -> String {
        let method = |i: usize| match i {
            i if i == n => format!("    def _m{i}(self, v):\n        return {last}\n"),
            i => format!("    def _m{i}(self, v):\n        return self._m{}(v)\n", i + 1),
        };
        (1..=n).map(method).collect::<Vec<_>>().join("\n")
    }

    /// Methods `_m1` to `_m<n>` of `x.y`, each passing `v` to the next and the last running `last`.
    fn pass_chain(n: usize, last: &str) -> String {
        let method = |i: usize| match i {
            i if i == n => format!("    def _m{i}(self, v):\n        {last}\n"),
            i => format!("    def _m{i}(self, v):\n        self._m{}(v)\n", i + 1),
        };
        (1..=n).map(method).collect::<Vec<_>>().join("\n")
    }

    /// Route `a` calling `_m1` and route `b` calling `_m4`, in `order`, each written by `route`
    /// from its name and the number of the method it calls.
    fn routes(order: [&str; 2], route: impl Fn(&str, usize) -> String) -> String {
        let route = |name: &str| route(name, if name == "a" { 1 } else { 4 });
        format!("{}\n    @http.route('/s')\n{}", route(order[0]), route(order[1]))
    }

    /// A route passing its parameter to `_m<m>`.
    fn passing(name: &str, m: usize) -> String {
        format!("    def {name}(self, {name}):\n        request.env['x.y']._m{m}({name})\n")
    }

    #[test]
    fn a_sink_reached_from_a_shallow_and_a_deep_call_is_found_in_either_order() {
        let methods = pass_chain(5, "requests.get(v, timeout=5)");
        for order in [["a", "b"], ["b", "a"]] {
            assert_eq!(
                scan_messages(&routes(order, passing), &methods),
                ["model.py:17 OD706 route parameter 'b' reaches the URL of requests.get() through '_m4()', '_m5()'"],
                "{order:?}"
            );
        }
    }

    #[test]
    fn a_return_reached_from_a_shallow_and_a_deep_call_is_found_in_either_order() {
        let methods = chain(5, "request.params.get('x')");
        for (order, line) in [(["a", "b"], 8), (["b", "a"], 4)] {
            assert_eq!(
                scan_messages(
                    &routes(order, |name, m| format!(
                        "    def {name}(self):\n        return Markup(request.env['x.y']._m{m}(1))\n"
                    )),
                    &methods
                ),
                [format!(
                    "route.py:{line} OD703 request.params reaches Markup() through '_m4()', '_m5()' unescaped"
                )],
                "{order:?}"
            );
        }
    }

    #[test]
    fn one_solver_finds_for_each_entry_what_a_fresh_one_does() {
        let sink = "requests.get(v, timeout=5)";
        let methods = format!(
            "{}\n    def outer(self, m):\n        self.relay(m)\n\n{}",
            pass_chain(5, sink),
            relay(sink, "v")
        );
        // Route `c` and the RPC entry `outer` reach `relay` in the same state but for `rpc`, so
        // only whether a hit is relayed tells their memo entries apart.
        let c = "\n    @http.route('/c')\n    def c(self, c):\n        request.env['x.y'].relay(c)\n";
        for order in [["a", "b"], ["b", "a"]] {
            for last in [["c", "outer"], ["outer", "c"]] {
                let modules = [module(&(routes(order, passing) + c), &methods)];
                let index = Index::build(&modules, None);
                let scope = index.scope(0);
                let entry = |name: &str| {
                    let (file, summary) = modules[0]
                        .python
                        .iter()
                        .find_map(|file| Some((file, file.facts.flows.iter().find(|s| s.method == name)?)))
                        .unwrap();
                    FlowRef {
                        module: 0,
                        file,
                        summary,
                    }
                };
                let pairs = |found: Vec<(bool, (&Path, RawDiag))>| {
                    found
                        .iter()
                        .map(|(relayed, f)| (*relayed, shown(f)))
                        .collect::<Vec<_>>()
                };
                let mut shared = Solver::new(&index, &scope);
                for name in order.into_iter().chain(last) {
                    let fresh = Solver::new(&index, &scope).findings(entry(name));
                    let found = shared.findings(entry(name));
                    assert_eq!(pairs(found), pairs(fresh), "{order:?} {last:?} {name}");
                }
            }
        }
    }

    #[test]
    fn a_method_calling_itself_is_reported_without_the_loop() {
        let methods = "    def _m1(self, v):\n        self._m1(v)\n        requests.get(v, timeout=5)\n";
        assert_eq!(
            scan_messages("    def r(self, t):\n        request.env['x.y']._m1(t)\n", methods),
            ["model.py:6 OD706 route parameter 't' reaches the URL of requests.get() through '_m1()'"]
        );
        let methods = "    def _m1(self, v):\n        if v:\n            return self._m1(v)\n        return request.params.get('x')\n";
        let route = "    def r(self):\n        return Markup(request.env['x.y']._m1(1))\n";
        assert_eq!(
            scan_messages(route, methods),
            ["route.py:4 OD703 request.params reaches Markup() through '_m1()' unescaped"]
        );
    }

    /// `_a` passing `v` to `_b`, which passes it back, then sending it to `requests.get()`.
    const MUTUAL: &str = "    def _a(self, v):\n        self._b(v)\n        requests.get(v, timeout=5)\n\n    def _b(self, v):\n        self._a(v)\n";

    /// The route that passes its parameter `t` to `_a`.
    const CALLS_A: &str = "    def r(self, t):\n        request.env['x.y']._a(t)\n";

    #[test]
    fn methods_calling_each_other_are_reported_without_the_loop() {
        assert_eq!(
            scan_messages(CALLS_A, MUTUAL),
            ["model.py:6 OD706 route parameter 't' reaches the URL of requests.get() through '_a()'"]
        );
    }

    #[test]
    fn the_flow_through_methods_calling_each_other_leaves_out_the_loop() {
        assert_eq!(
            flows(CALLS_A, MUTUAL),
            [["route parameter 't'", "_a()", "parameter 'v'", "requests.get()"]]
        );
    }

    /// `_a` returning what `_b` returns, which returns what `_a` returns, or else a source.
    const MUTUAL_RETURN: &str = "    def _a(self, v):\n        if v:\n            return self._b(v)\n        return request.params.get('x')\n\n    def _b(self, v):\n        return self._a(v)\n";

    /// The route that marks safe what `_a` returns.
    const RETURNS_A: &str = "    def r(self):\n        return Markup(request.env['x.y']._a(1))\n";

    #[test]
    fn methods_returning_each_other_are_reported_without_the_loop() {
        assert_eq!(
            scan_messages(RETURNS_A, MUTUAL_RETURN),
            ["route.py:4 OD703 request.params reaches Markup() through '_a()' unescaped"]
        );
    }

    #[test]
    fn the_flow_through_methods_returning_each_other_leaves_out_the_loop() {
        assert_eq!(
            flows(RETURNS_A, MUTUAL_RETURN),
            [["request.params", "_a()", "Markup()"]]
        );
    }

    #[test]
    fn a_method_calling_itself_under_sudo_is_followed_as_superuser() {
        let methods = "    def _a(self, vals):\n        self.sudo()._a(vals)\n        self.write(vals)\n";
        assert_eq!(
            scan_messages(
                "    def r(self, **post):\n        request.env['x.y']._a(post)\n",
                methods
            ),
            ["model.py:6 OD704 route parameter 'post' reaches self.write() through '_a()'"]
        );
    }

    #[test]
    fn a_method_called_back_with_the_value_in_another_parameter_is_followed() {
        let methods = "    def _a(self, p, q):\n        self._b(p)\n        requests.get(q, timeout=5)\n\n    def _b(self, v):\n        self._a(1, v)\n";
        assert_eq!(
            scan_messages("    def r(self, t):\n        request.env['x.y']._a(t, 1)\n", methods),
            ["model.py:6 OD706 route parameter 't' reaches the URL of requests.get() through '_a()', '_b()', '_a()'"]
        );
    }

    #[test]
    fn a_method_returning_what_it_is_called_back_with_in_another_parameter_is_followed() {
        let methods = "    def _a(self, p, q):\n        if p:\n            return self._b(q)\n        return p\n\n    def _b(self, v):\n        return self._a(v, 1)\n";
        let route = "    def r(self):\n        return Markup(request.env['x.y']._a(1, request.params.get('x')))\n";
        assert_eq!(
            scan_messages(route, methods),
            ["route.py:4 OD703 request.params reaches Markup() through '_a()', '_b()', '_a()' unescaped"]
        );
    }

    #[test]
    fn a_source_is_returned_through_three_methods() {
        let route = "    def r(self):\n        return Markup(request.env['x.y']._m1(1))\n";
        assert_eq!(
            scan_messages(route, &chain(3, "request.params.get('x')")),
            ["route.py:4 OD703 request.params reaches Markup() through '_m1()', '_m2()', '_m3()' unescaped"]
        );
    }

    #[test]
    fn a_parameter_is_returned_through_two_methods() {
        let route = "    def r(self, term):\n        return Markup(request.env['x.y']._m1(term))\n";
        assert_eq!(
            scan_messages(route, &chain(2, "'<p>%s</p>' % v")),
            ["route.py:4 OD703 route parameter 'term' reaches Markup() through '_m1()', '_m2()' unescaped"]
        );
    }

    #[test]
    fn a_url_whose_start_the_caller_picks_reaches_no_host() {
        let call = |arg: &str| {
            scan_messages(
                &format!(
                    "    def r(self, t):\n        return self._get({arg})\n\n    def _get(self, url):\n        return requests.get(url, timeout=5)\n"
                ),
                "",
            )
        };
        assert!(call("f\"https://x/{t}\"").is_empty());
        assert_eq!(
            call("t"),
            ["route.py:7 OD706 route parameter 't' reaches the URL of requests.get() through '_get()'"]
        );
    }

    #[test]
    fn a_returned_value_harmless_for_the_sink_is_not_reported() {
        let route = "    def r(self, t):\n        return open(request.env['x.y']._m1(t))\n";
        assert!(scan_messages(route, &chain(1, "os.path.basename(v)")).is_empty());
        assert_eq!(scan_messages(route, &chain(1, "v")).len(), 1);
    }

    #[test]
    fn a_public_method_called_from_python_returns_what_its_caller_passes() {
        let methods = "    def get(self, key, default=False):\n        return self._find(key) or default\n";
        let call = |args: &str| {
            scan_messages(
                &format!("    def r(self, t):\n        return requests.get(request.env['x.y'].get({args}))\n"),
                methods,
            )
        };
        assert!(call("'k'").is_empty());
        assert!(call("'k', 'https://x/'").is_empty());
        assert_eq!(
            call("'k', t"),
            ["route.py:4 OD706 route parameter 't' reaches the URL of requests.get() through 'get()'"]
        );
    }

    #[test]
    fn a_source_joined_to_a_public_method_parameter_is_returned_whatever_the_caller_passes() {
        let route = "    def r(self):\n        return Markup(request.env['x.y'].label('a'))\n";
        for body in [
            "prefix + request.params.get('n')",
            "f\"{prefix}{request.params.get('n')}\"",
            "prefix or request.params.get('n')",
        ] {
            let methods = format!("    def label(self, prefix):\n        return {body}\n");
            assert_eq!(scan_messages(route, &methods).len(), 1, "{body}");
        }
    }

    #[test]
    fn a_call_joined_to_a_public_method_parameter_is_returned_whatever_the_caller_passes() {
        let route = "    def r(self):\n        return Markup(request.env['x.y'].label('x'))\n";
        let methods = "    def label(self, prefix):\n        return prefix + self._suffix()\n\n    def _suffix(self):\n        return request.params.get('n')\n";
        assert_eq!(
            scan_messages(route, methods),
            ["route.py:4 OD703 request.params reaches Markup() through 'label()', '_suffix()' unescaped"]
        );
    }

    #[test]
    fn each_parameter_a_public_method_joins_is_returned() {
        let route = "    def r(self, t):\n        return Markup(request.env['x.y'].label('x', t))\n";
        let methods = "    def label(self, a, b):\n        return a + b\n";
        assert_eq!(
            scan_messages(route, methods),
            ["route.py:4 OD703 route parameter 't' reaches Markup() through 'label()' unescaped"]
        );
    }

    /// What `_m1` of `x.y` returns and the sinks its parameter reaches, by label and by code, of
    /// methods `_m1` to `_m4` where the last runs `last` and each other `body` with `#` the next.
    fn solved(body: &str, last: &str) -> (Vec<String>, Vec<&'static str>) {
        let method = |i: usize| match i {
            4 => format!("    def _m4(self, v):\n        {last}\n"),
            i => format!(
                "    def _m{i}(self, v):\n        {}\n",
                body.replace('#', &format!("_m{}", i + 1))
            ),
        };
        let methods: Vec<String> = (1..=4).map(method).collect();
        let modules = [module("    def r(self):\n        pass\n", &methods.join("\n"))];
        let index = Index::build(&modules, None);
        let scope = index.scope(0);
        let file = &modules[0].python[1];
        let m1 = FlowRef {
            module: 0,
            file,
            summary: file.facts.flows.iter().find(|s| s.method == "_m1").unwrap(),
        };
        let mut solver = Solver::new(&index, &scope);
        let rets = solver.returns(m1, 1).into_iter().map(|r| r.label).collect();
        let hits = solver
            .hits(m1, 0, 1, false, false)
            .into_iter()
            .map(|h| h.code)
            .collect();
        (rets, hits)
    }

    #[test]
    fn a_value_through_two_calls_at_each_depth_is_followed_once() {
        let (rets, _) = solved("return self.#(v) + self.#(v)", "return request.params.get('x')");
        assert_eq!(rets, ["request.params"]);
        let (_, hits) = solved("self.#(v)\n        self.#(v)", "requests.get(v, timeout=5)");
        assert_eq!(hits, ["OD706"]);
    }

    #[test]
    fn returns_are_followed_four_calls_deep() {
        let route = "    def r(self):\n        return Markup(request.env['x.y']._m1(1))\n";
        assert_eq!(scan_messages(route, &chain(4, "request.params.get('x')")).len(), 1);
        assert!(scan_messages(route, &chain(5, "request.params.get('x')")).is_empty());
        assert!(scan_messages(route, &chain(1, "self._m1(v)")).is_empty());
    }

    #[test]
    fn a_helper_storing_a_route_upload_publicly_is_reported() {
        let store = "    def _store(self, data):\n        self.env['ir.attachment'].sudo().create({'raw': data, 'public': True})\n";
        let call = |arg: &str| {
            scan_messages(
                &format!("    def r(self, upload):\n        request.env['x.y']._store({arg})\n"),
                store,
            )
        };
        assert_eq!(
            call("upload.read()"),
            [
                "model.py:5 OD717 route parameter 'upload' reaches the content of sudo().create() through '_store()', which public=True lets anyone download"
            ]
        );
        assert!(call("b'logo'").is_empty());
    }

    #[test]
    fn a_helper_publishing_values_its_caller_names_is_not_reported() {
        for publish in ["vals['public'] = True", "vals |= {'public': True}"] {
            let store = format!(
                "    def _store(self, vals):\n        {publish}\n        return self.env['ir.attachment'].sudo().create(vals)\n"
            );
            let route =
                "    def r(self, **kw):\n        request.env['x.y']._store({'name': kw.get('name'), 'raw': b'pdf'})\n";
            assert!(scan_messages(route, &store).is_empty(), "{publish}");
            // A public method reports what an RPC sends it, not what a Python caller picks.
            let store = store.replace("_store", "store");
            let route = route.replace("_store", "store");
            assert!(scan_messages(&route, &store).is_empty(), "public {publish}");
        }
    }

    /// A sink of each rule but OD704, which needs a mapping, and OD716, which a route reaches,
    /// receiving `#` in a method of `x.y`.
    const METHOD_SINK_OF: &[(&str, &str)] = &[
        ("OD701", "self.env.cr.execute(#)"),
        ("OD702", "safe_eval(#)"),
        ("OD703", "Markup(#)"),
        ("OD705", "request.redirect(#, local=False)"),
        ("OD706", "requests.get(#, timeout=5)"),
        ("OD707", "open(#)"),
        ("OD708", "os.system(#)"),
        ("OD709", "pickle.loads(#)"),
        ("OD710", "etree.fromstring(#, etree.XMLParser(resolve_entities=True))"),
        ("OD711", "request.make_response('', [('X-Name', #)])"),
        ("OD712", "self.env[#].sudo().search([])"),
        ("OD713", "getattr(self, #)()"),
        ("OD714", "self.sudo()[#]"),
        ("OD715", "self.sudo().search(#)"),
        (
            "OD717",
            "self.env['ir.attachment'].sudo().create({'raw': #, 'public': True})",
        ),
    ];

    #[test]
    fn a_callee_reading_the_name_of_a_file_picked_by_its_field_reads_no_user_data() {
        let route = "    def r(self):\n        for f in request.httprequest.files.getlist('ufile'):\n            request.env['x.y']._a(f)\n";
        let codes = |methods: &str| {
            let found = scan_messages(route, methods);
            found
                .iter()
                .map(|f| f.split(' ').nth(1).unwrap_or_default().to_owned())
                .collect::<Vec<_>>()
        };
        for (code, sink) in METHOD_SINK_OF {
            let direct = |value: &str| format!("    def _a(self, file):\n        {}\n", sink.replace('#', value));
            let deeper = |value: &str| {
                format!(
                    "    def _a(self, file):\n        self._b(file)\n\n    def _b(self, file):\n        {}\n",
                    sink.replace('#', value)
                )
            };
            let passed = format!(
                "    def _a(self, file):\n        self._b(getattr(file, 'name', None))\n\n    def _b(self, v):\n        {}\n",
                sink.replace('#', "v")
            );
            for methods in [direct("getattr(file, 'name', None)"), deeper("file.name"), passed] {
                assert!(codes(&methods).is_empty(), "{code}: {methods}");
            }
            for methods in [
                direct("file.filename"),
                direct("file.stream.name"),
                direct("getattr(file, 'name', None) or file.filename"),
                deeper("getattr(file, 'filename')"),
            ] {
                assert_eq!(codes(&methods), [*code], "{code}: {methods}");
            }
        }
        let both = "    def _a(self, file):\n        open(getattr(file, 'name', None))\n        open(file.filename)\n";
        assert_eq!(
            scan_messages(route, both),
            ["model.py:6 OD707 request.httprequest.files reaches the path of open() through 'f', '_a()'"]
        );
        let archive = "    def r(self):\n        request.env['x.y']._a(request.httprequest.files.get('archive'))\n";
        let members = "    def _a(self, file):\n        tar = tarfile.open(fileobj=file)\n        for m in tar.getmembers():\n            open(os.path.join(DEST, m.name), 'wb')\n";
        assert_eq!(
            scan_messages(archive, members),
            [
                "model.py:7 OD707 request.httprequest.files reaches the path of os.path.join() through '_a()', 'tar', 'm'"
            ]
        );
        let deeper = "    def _a(self, file):\n        for m in tarfile.open(fileobj=file).getmembers():\n            self._b(m)\n\n    def _b(self, member):\n        open(os.path.join(DEST, member.name), 'wb')\n";
        assert_eq!(
            scan_messages(archive, deeper),
            [
                "model.py:9 OD707 request.httprequest.files reaches the path of os.path.join() through '_a()', 'm', '_b()'"
            ]
        );
    }

    /// A route taking `**post` whose body is `call`.
    fn applying(call: &str) -> String {
        format!("    def r(self, **post):\n        {call}\n")
    }

    #[test]
    fn a_public_method_called_under_sudo_writes_its_records_as_superuser() {
        let sudo = "request.env['x.y'].sudo()";
        for name in ["_apply", "apply"] {
            let methods = format!("    def {name}(self, vals):\n        self.write(vals)\n");
            assert_eq!(
                scan_messages(&applying(&format!("{sudo}.{name}(post)")), &methods),
                [format!(
                    "model.py:5 OD704 route parameter 'post' reaches self.write() through '{name}()'"
                )],
                "{name}"
            );
        }
        let apply = "    def apply(self, vals):\n        self.write(vals)\n";
        for call in [
            "request.env['x.y'].apply(post)",
            "request.env['x.y'].sudo().with_user(request.env.user).apply(post)",
            "request.env['x.y'].sudo().apply({'name': 'x'})",
            "return post",
        ] {
            assert!(scan_messages(&applying(call), apply).is_empty(), "{call}");
        }
        // A sink in a public method's own body reports the RPC argument there and keeps no edge.
        let own = "    def apply(self, vals):\n        self.sudo().write(vals)\n";
        assert!(scan_messages(&applying(&format!("{sudo}.apply(post)")), own).is_empty());
    }

    #[test]
    fn sudo_reaches_a_write_between_public_and_private_methods() {
        let route = applying("request.env['x.y'].sudo()._apply(post)");
        for (first, second) in [("apply", "_store"), ("_apply", "store")] {
            let methods = format!(
                "    def {first}(self, vals):\n        self.{second}(vals)\n\n    def {second}(self, vals):\n        self.write(vals)\n"
            );
            assert_eq!(
                scan_messages(&route.replace("_apply", first), &methods),
                [format!(
                    "model.py:8 OD704 route parameter 'post' reaches self.write() through '{first}()', '{second}()'"
                )],
                "{first}"
            );
        }
    }

    #[test]
    fn a_write_override_adds_nothing_to_the_mapping_its_caller_reports() {
        let sync = "    def write(self, vals):\n        self._sync(vals)\n        return super().write(vals)\n\n    def _sync(self, vals):\n        self.create(vals)\n";
        let methods = format!("    def apply(self, vals):\n        self.write(vals)\n\n{sync}");
        assert_eq!(
            scan_messages(&applying("request.env['x.y'].sudo().apply(post)"), &methods),
            ["model.py:5 OD704 route parameter 'post' reaches self.write() through 'apply()'"]
        );
        // The route's own `sudo().write()` reports the mapping, outside the flows across methods.
        assert!(scan_messages(&applying("request.env['x.y'].sudo().write(post)"), sync).is_empty());
    }

    /// A public `relay` of `x.y` passing `v` to `_a`, which runs `sink` on `value`.
    fn relay(sink: &str, value: &str) -> String {
        format!(
            "    def relay(self, v):\n        self._a(v)\n\n    def _a(self, v):\n        {}\n",
            sink.replace('#', value)
        )
    }

    /// A finding `shown` gives, up to its origin.
    fn origin(finding: &str) -> String {
        finding.split(" reaches ").next().unwrap_or_default().to_owned()
    }

    /// The findings of `module(route, methods)` as `file:line code origin`.
    fn origins(route: &str, methods: &str) -> Vec<String> {
        scan_messages(route, methods).iter().map(|f| origin(f)).collect()
    }

    #[test]
    fn a_helper_sink_behind_a_public_method_is_reached_from_a_route() {
        for (code, sink) in METHOD_SINK_OF {
            let methods = relay(sink, "v");
            assert_eq!(
                origins("    def r(self, t):\n        request.env['x.y'].relay(t)\n", &methods),
                [format!("model.py:8 {code} route parameter 't'")],
                "{code}"
            );
            assert_eq!(
                origins("    def r(self):\n        request.env['x.y'].relay('x')\n", &methods),
                [format!("model.py:8 {code} RPC argument 'v'")],
                "{code}"
            );
        }
    }

    #[test]
    fn a_helper_sink_behind_a_public_method_of_a_dependency_is_reported_at_the_route_call() {
        let route = "    def r(self, t):\n        request.env['x.y'].relay(t)\n";
        for (code, sink) in METHOD_SINK_OF {
            let mut main = module(route, &relay(sink, "v"));
            let model = main.python.remove(1);
            assert_eq!(
                scan_with_dep(main, model),
                [
                    [format!("route.py:4 {code} route parameter 't'")],
                    [format!("model.py:8 {code} RPC argument 'v'")]
                ],
                "{code}"
            );
        }
    }

    #[test]
    fn an_rpc_entry_relaying_to_another_keeps_the_message_of_the_other() {
        for (code, sink) in METHOD_SINK_OF {
            let methods = format!("    def outer(self, m):\n        self.relay(m)\n\n{}", relay(sink, "v"));
            assert_eq!(
                origins("    def r(self):\n        pass\n", &methods),
                [format!("model.py:11 {code} RPC argument 'v'")],
                "{code}"
            );
        }
    }

    #[test]
    fn an_rpc_entry_names_its_own_longer_path_over_one_through_another() {
        for (code, sink) in METHOD_SINK_OF {
            let methods = format!(
                "    def outer(self, m):\n        self._x(m)\n\n    def _x(self, v):\n        self.relay(v)\n        self._b(v)\n\n    def _b(self, v):\n        self._c(v)\n\n    def _c(self, v):\n        self._a(v)\n\n{}",
                relay(sink, "v")
            );
            assert_eq!(
                origins("    def r(self):\n        pass\n", &methods),
                [format!("model.py:21 {code} RPC argument 'm'")],
                "{code}"
            );
        }
    }

    #[test]
    fn a_route_reaching_a_sink_directly_and_through_a_public_method_names_the_shorter_path() {
        let route = "    def r(self, t):\n        request.env['x.y']._x(t)\n";
        for (code, sink) in METHOD_SINK_OF {
            let methods = format!(
                "    def _x(self, v):\n        self.relay(v)\n        self._a(v)\n\n{}",
                relay(sink, "v")
            );
            let found = scan_messages(route, &methods);
            assert_eq!(found.len(), 1, "{code}: {found:?}");
            assert!(
                found[0].starts_with(&format!("model.py:12 {code} route parameter 't'"))
                    && found[0].contains(" through '_x()', '_a()'"),
                "{code}: {found:?}"
            );
        }
    }

    #[test]
    fn an_rpc_entry_reaching_a_sink_only_through_another_reports_it() {
        let methods = "    def outer(self, vals):\n        self.sudo().apply(vals)\n\n    def apply(self, vals):\n        self.write(vals)\n";
        assert_eq!(
            scan_messages("    def r(self):\n        pass\n", methods),
            ["model.py:8 OD704 RPC argument 'vals' reaches self.write() through 'apply()'"]
        );
        let outer = "    def outer(self, m):\n        self.relay(m)\n";
        for (code, sink) in METHOD_SINK_OF {
            let main = module("    def r(self):\n        pass\n", outer);
            let mut dep_file = module("", &relay(sink, "v")).python.remove(1);
            dep_file.path = PathBuf::from("dep.py");
            dep_file.rel = "dep.py".to_owned();
            assert_eq!(
                scan_with_dep(main, dep_file),
                [
                    [format!("model.py:5 {code} RPC argument 'm'")],
                    [format!("dep.py:8 {code} RPC argument 'v'")]
                ],
                "{code}"
            );
        }
    }

    #[test]
    fn a_public_method_passing_on_the_name_of_a_file_picked_by_its_field_reports_only_its_rpc_argument() {
        let route = "    def r(self):\n        for f in request.httprequest.files.getlist('ufile'):\n            request.env['x.y'].relay(f)\n";
        for (code, sink) in METHOD_SINK_OF {
            assert_eq!(
                origins(route, &relay(sink, "getattr(v, 'name', None)")),
                [format!("model.py:8 {code} RPC argument 'v'")],
                "{code}"
            );
        }
    }

    #[test]
    fn a_public_method_returning_records_read_as_superuser_adds_nothing() {
        let methods = "    def show(self, i):\n        return self.sudo().browse(i)\n";
        let route = "    def r(self, t):\n        return request.env['x.y'].show(t)\n";
        assert!(scan_messages(route, methods).is_empty());
    }
}
