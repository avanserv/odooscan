//! Flows that cross from one method into another: a value from a source passed to a method whose
//! parameter reaches a sink, or a method that returns a source's value into a sink of its caller.

use std::path::Path;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::checks::Out;
use crate::facts::{
    ArgSlot, CallSite, FlowEdge, FlowOrigin, FlowSummary, FlowTarget, Pos, RawDiag, Receiver, Spanned, Step,
};
use crate::index::{FlowRef, Index, Scope};
use crate::taint;

/// How many methods deep a value is followed from the call that first passes it.
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

pub fn check<'m>(index: &Index<'m>, mi: usize, scope: &Scope, out: &mut Out<'m>) {
    let module = &index.modules[mi];
    let mut solver = Solver {
        index,
        scope,
        memo: FxHashMap::default(),
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
    memo: FxHashMap<(usize, usize), Vec<Hit<'m>>>,
}

impl<'m> Solver<'_, 'm> {
    /// The flows from a source in `at` that cross into another method.
    fn findings(&mut self, at: FlowRef<'m>) -> Vec<(&'m Path, RawDiag)> {
        let mut found = vec![];
        for edge in &at.summary.edges {
            match (edge.origin, &edge.target) {
                (FlowOrigin::Source, FlowTarget::Arg(c, slot)) => {
                    for hit in self.passed(at, edge, *c, slot, 0) {
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

    /// The sinks a value reaches once `edge` passes it to the call `c` of `at`.
    fn passed(&mut self, at: FlowRef<'m>, edge: &FlowEdge, c: usize, slot: &ArgSlot, depth: u8) -> Vec<Hit<'m>> {
        let call = &at.summary.calls[c];
        let mut hits = vec![];
        for callee in self.callees(at, call) {
            let Some(param) = param_index(callee.summary, slot) else {
                continue;
            };
            for hit in self.hits(callee, param, depth + 1) {
                hits.push(prepend(at, edge, call, hit));
            }
        }
        hits
    }

    /// The sinks parameter `param` of a method reaches, in it or in the methods it passes it to.
    fn hits(&mut self, at: FlowRef<'m>, param: usize, depth: u8) -> Vec<Hit<'m>> {
        let key = (std::ptr::from_ref(at.summary) as usize, param);
        if let Some(hits) = self.memo.get(&key) {
            return hits.clone();
        }
        // A method that calls itself back finds this empty entry and stops.
        self.memo.insert(key, vec![]);
        let mut hits = vec![];
        for edge in at.summary.edges.iter().filter(|e| e.origin == FlowOrigin::Param(param)) {
            match &edge.target {
                FlowTarget::Sink(code) => hits.push(sink_hit(at, edge, code)),
                FlowTarget::Arg(c, slot) if depth < DEPTH => hits.extend(self.passed(at, edge, *c, slot, depth)),
                _ => {}
            }
        }
        self.memo.insert(key, hits.clone());
        hits
    }

    /// Findings for a sink of `at` that receives what the call `c` returns: a source's value the
    /// callee returns, or a source's value `at` passes to it and it returns.
    fn returned(&self, at: FlowRef<'m>, edge: &FlowEdge, c: usize, code: &'static str) -> Vec<(&'m Path, RawDiag)> {
        let mut found = vec![];
        for callee in self.callees(at, &at.summary.calls[c]) {
            for ret in callee.summary.edges.iter().filter(|e| e.target == FlowTarget::Return) {
                match ret.origin {
                    FlowOrigin::Source => found.extend(return_finding(at, callee, None, ret, edge, code)),
                    FlowOrigin::Param(k) => {
                        for arg in sources_passed(at.summary, c, callee.summary, k) {
                            found.extend(return_finding(at, callee, Some(arg), ret, edge, code));
                        }
                    }
                    FlowOrigin::Return(_) => {}
                }
            }
        }
        found
    }
}

/// The edges that pass a source's value to parameter `k` of `callee` at the call `c`.
fn sources_passed<'s>(
    caller: &'s FlowSummary,
    c: usize,
    callee: &'s FlowSummary,
    k: usize,
) -> impl Iterator<Item = &'s FlowEdge> {
    caller.edges.iter().filter(move |a| {
        a.origin == FlowOrigin::Source
            && matches!(&a.target, FlowTarget::Arg(ac, slot) if *ac == c && param_index(callee, slot) == Some(k))
    })
}

/// The flow from a source, through `arg` into the callee when the callee returns its parameter,
/// out of the callee by `ret`, and on to the sink of `edge`.
fn return_finding<'m>(
    at: FlowRef<'m>,
    callee: FlowRef<'m>,
    arg: Option<&FlowEdge>,
    ret: &FlowEdge,
    edge: &FlowEdge,
    code: &'static str,
) -> Option<(&'m Path, RawDiag)> {
    let keys = arg.is_none_or(|a| a.keys) && ret.keys && edge.keys;
    if code == "OD704" && !keys {
        return None;
    }
    let mut steps = arg.map(|a| path_steps(at, &a.steps)).unwrap_or_default();
    steps.extend(path_steps(callee, &ret.steps));
    steps.extend(path_steps(at, &edge.steps));
    let (sink, after) = edge.steps.split_last()?;
    let mut names: Vec<&str> = vec![];
    if let Some(a) = arg {
        names.extend(a.steps[1..].iter().map(|s| s.value.as_str()));
    }
    names.extend(after.first().map(|s| s.value.as_str()));
    names.extend(ret.steps[1..].iter().map(|s| s.value.as_str()));
    names.extend(after.iter().skip(1).map(|s| s.value.as_str()));
    let origin = arg.map_or(&ret.steps[0].value, |a| &a.steps[0].value);
    let message = taint::message(code, &sink.value, origin, &names);
    Some((
        at.file.path.as_path(),
        RawDiag::new(code, sink.pos, message).with_flow(steps),
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
