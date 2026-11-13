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
    /// By summary, parameter, whether the method runs as superuser, and depth: a result found
    /// with fewer calls left would hide a deeper sink from a caller that has more.
    memo: FxHashMap<(usize, usize, bool, u8), Vec<Hit<'m>>>,
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

    /// The flows from a source in `at` that cross into another method.
    fn findings(&mut self, at: FlowRef<'m>) -> Vec<(&'m Path, RawDiag)> {
        let mut found = vec![];
        for edge in &at.summary.edges {
            match (edge.origin, &edge.target) {
                (origin, FlowTarget::Arg(c, slot)) if origin.is_user() => {
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
            // The `name` of a file picked by its field is that field, which the code wrote.
            let hits_of = self.hits(callee, param, depth + 1, sudo);
            let kept = hits_of.into_iter().filter(|h| !(edge.upload && h.named));
            hits.extend(kept.map(|hit| prepend(at, edge, call, hit)));
        }
        hits
    }

    /// The sinks parameter `param` of a method reaches, in it or in the methods it passes it to.
    /// What the method writes on its own records is one only when it runs as superuser.
    fn hits(&mut self, at: FlowRef<'m>, param: usize, depth: u8, sudo: bool) -> Vec<Hit<'m>> {
        let state = (std::ptr::from_ref(at.summary) as usize, param, sudo);
        let key = (state.0, param, sudo, depth);
        if let Some(hits) = self.memo.get(&key) {
            return hits.clone();
        }
        let mut hits = vec![];
        for edge in at.summary.edges.iter().filter(|e| e.origin == FlowOrigin::Param(param)) {
            match &edge.target {
                FlowTarget::Sink(code) => hits.push(sink_hit(at, edge, code)),
                FlowTarget::SelfWrite if sudo => hits.push(sink_hit(at, edge, "OD704")),
                FlowTarget::Arg(c, slot) if depth < DEPTH => hits.extend(self.passed(at, edge, *c, slot, depth, sudo)),
                _ => {}
            }
        }
        hits.retain_mut(|h| enter(&mut h.via, state));
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
    use crate::facts::RawDiag;
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

    /// The cross-method findings of `module(route, methods)`.
    fn scan_messages(route: &str, methods: &str) -> Vec<String> {
        let modules = [module(route, methods)];
        let index = Index::build(&modules, None);
        let mut out = Out { diags: vec![] };
        check(&index, 0, &index.scope(0), &mut out);
        out.diags.iter().map(shown).collect()
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
    fn one_solver_finds_for_each_route_what_a_fresh_one_does() {
        let methods = pass_chain(5, "requests.get(v, timeout=5)");
        for order in [["a", "b"], ["b", "a"]] {
            let modules = [module(&routes(order, passing), &methods)];
            let index = Index::build(&modules, None);
            let scope = index.scope(0);
            let file = &modules[0].python[0];
            let route = |name: &str| FlowRef {
                module: 0,
                file,
                summary: file.facts.flows.iter().find(|s| s.method == name).unwrap(),
            };
            let mut shared = Solver::new(&index, &scope);
            for name in order {
                let fresh = Solver::new(&index, &scope).findings(route(name));
                let found = shared.findings(route(name));
                assert_eq!(
                    found.iter().map(shown).collect::<Vec<_>>(),
                    fresh.iter().map(shown).collect::<Vec<_>>(),
                    "{order:?} {name}"
                );
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
        let modules = [module(CALLS_A, MUTUAL)];
        let index = Index::build(&modules, None);
        let mut out = Out { diags: vec![] };
        check(&index, 0, &index.scope(0), &mut out);
        let flows: Vec<Vec<&str>> = out
            .diags
            .iter()
            .map(|(_, d)| d.flow.iter().map(|s| s.label.as_str()).collect())
            .collect();
        assert_eq!(
            flows,
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
        let modules = [module(RETURNS_A, MUTUAL_RETURN)];
        let index = Index::build(&modules, None);
        let mut out = Out { diags: vec![] };
        check(&index, 0, &index.scope(0), &mut out);
        let flows: Vec<Vec<&str>> = out
            .diags
            .iter()
            .map(|(_, d)| d.flow.iter().map(|s| s.label.as_str()).collect())
            .collect();
        assert_eq!(flows, [["request.params", "_a()", "Markup()"]]);
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
}
