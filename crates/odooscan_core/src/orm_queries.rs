//! Queries a function runs once per record of a loop where one query for all of them would do
//! (OD134-OD137), and searches that fetch every matching id only to count them or keep the first
//! (OD138, OD139). Each function is walked on its own: the functions, classes and lambdas it nests
//! run when they are called, not in its loops.

use ruff_python_ast::helpers::any_over_expr;
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, Operator, Stmt};
use ruff_text_size::{Ranged, TextRange};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::RawDiag;
use crate::method_body::arguments;
use crate::module_state::MUTATORS;
use crate::python::{BoundNames, Locator, SAME_MODEL, str_lit, tail_name};
use crate::route_body::toggle;

/// Recordset methods whose result is records of the model they are called on.
const RECORD_METHODS: &[&str] = &[
    "browse",
    "search",
    "search_fetch",
    "filtered",
    "filtered_domain",
    "exists",
    "sorted",
    "create",
    "copy",
    "with_prefetch",
];

/// The calls that give a new value each time they run: a sequence's next number (18.0 and 20.0
/// `odoo/addons/base/models/ir_sequence.py`, `_next_do()`), and the records `create()` inserts. Not
/// `copy()`, which a dict has too.
const NEW_VALUE_CALLS: &[&str] = &["create", "next_by_code", "next_by_id"];

/// The domain operators that join the terms of a domain.
const DOMAIN_OPERATORS: &[&str] = &["&", "|", "!"];

/// The OD134-OD139 findings in `func`, whose `self` is records when `model` says it is a method of
/// a model.
pub(crate) fn check(func: &ast::StmtFunctionDef, model: bool, loc: &Locator) -> Vec<RawDiag> {
    let mut walk = Walk {
        loc,
        records: FxHashSet::default(),
        dicts: FxHashSet::default(),
        attrs: FxHashSet::default(),
        bound: func.parameters.iter().map(|p| p.name().as_str()).collect(),
        loops: vec![],
        isolated: 0,
        breaks: false,
        assigned: None,
        guards: vec![],
        fetched: None,
        kept: None,
        diags: vec![],
    };
    if model {
        walk.records.insert("self");
    }
    walk.visit_body(&func.body);
    walk.diags
}

/// A `for` over records.
struct Loop<'a> {
    /// The name the loop binds each record to, when it binds one name.
    target: Option<&'a str>,
    /// The names the statement binds, its target among them, and those it changes in place but
    /// records, which may differ on each pass.
    varying: FxHashSet<&'a str>,
    /// The records the statement changes the fields of: their values differ, their model does not.
    filled: FxHashSet<&'a str>,
    /// The names the function had bound when the loop started.
    before: FxHashSet<&'a str>,
    /// The `varying` names the statement only assigns values that do not vary, which a query may
    /// take as the same on each pass: none holds one of the `NEW_VALUE_CALLS`. A write may not take
    /// them as the same: any other call among them, such as one that makes a token, may give each
    /// pass a new value.
    steady: FxHashSet<&'a str>,
    /// The names the statement reads an attribute of, which fetches the record a name holds.
    read: FxHashSet<&'a str>,
}

impl Loop<'_> {
    fn names_varying(&self, expr: &Expr) -> bool {
        any_over_expr(
            expr,
            |e| matches!(e, Expr::Name(n) if self.varying.contains(n.id.as_str()) || self.filled.contains(n.id.as_str())),
        )
    }

    fn varies_model(&self, expr: &Expr) -> bool {
        any_over_expr(
            expr,
            |e| matches!(e, Expr::Name(n) if self.varying.contains(n.id.as_str())),
        )
    }

    fn any_varying<'e>(&self, mut exprs: impl Iterator<Item = &'e Expr>) -> bool {
        exprs.any(|e| self.names_varying(e))
    }

    /// Whether a query given `expr` may differ from one pass to the next.
    fn differs(&self, expr: &Expr) -> bool {
        any_over_expr(
            expr,
            |e| matches!(e, Expr::Name(n) if !self.steady.contains(n.id.as_str()) && self.names_varying(e)),
        )
    }

    fn any_differs<'e>(&self, mut exprs: impl Iterator<Item = &'e Expr>) -> bool {
        exprs.any(|e| self.differs(e))
    }
}

struct Walk<'a, 'l> {
    loc: &'l Locator<'l>,
    /// The names bound to records at the point of the walk.
    records: FxHashSet<&'a str>,
    /// The names last bound to a dict literal or a `dict()` call.
    dicts: FxHashSet<&'a str>,
    /// The names last bound to a field of records, which `is_model` leaves out as it leaves out the
    /// attribute, while a loop over one still counts as one over records.
    attrs: FxHashSet<&'a str>,
    /// The names the function has bound so far, its parameters among them.
    bound: FxHashSet<&'a str>,
    loops: Vec<Loop<'a>>,
    /// How many of the outer `loops` the walk is kept apart from. A query in a `try` or a savepoint
    /// handles each record's failure on its own, which one batch cannot, and one in a block that
    /// leaves the loop runs at most once each time the loop starts.
    isolated: usize,
    /// Whether a `break` at the point of the walk leaves the innermost of `loops`.
    breaks: bool,
    /// The targets and the value of the assignment whose value the walk is in.
    assigned: Option<(&'a [Expr], &'a Expr)>,
    /// The tests of the `if` and `elif` branches the walk is in, inside the innermost records loop.
    guards: Vec<&'a Expr>,
    /// The call whose result the attribute the walk is in reads.
    fetched: Option<TextRange>,
    /// The `create()` call whose result the statement drops or adds to a collection.
    kept: Option<TextRange>,
    diags: Vec<RawDiag>,
}

impl<'a> Walk<'a, '_> {
    /// The loop a query at the point of the walk runs once per record of, if any.
    fn counted(&self) -> Option<&Loop<'a>> {
        self.loops.last().filter(|_| self.loops.len() > self.isolated)
    }

    /// Records a query is called on. A field of records, or a name last bound to one, is left out:
    /// it may be a lowercase class constant such as a compiled pattern, whose `search()` is not the
    /// ORM's. An all-caps constant is no records at all, by `holds_records`.
    fn is_model(&self, expr: &Expr) -> bool {
        let attr = match expr {
            Expr::Attribute(_) => true,
            Expr::Name(n) => self.attrs.contains(n.id.as_str()),
            _ => false,
        };
        !attr && self.is_records(expr)
    }

    fn is_records(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Name(n) => self.records.contains(n.id.as_str()),
            Expr::Subscript(s) => is_env(&s.value) || self.is_records(&s.value),
            // What an environment holds besides models, its context and cursor among them, is not
            // records, and neither is the environment itself.
            Expr::Attribute(a) if is_env(&a.value) => false,
            Expr::Attribute(a) => holds_records(a.attr.as_str()) && self.is_records(&a.value),
            Expr::Call(c) => self.records_call(c),
            _ => false,
        }
    }

    /// Only recordsets have the `SAME_MODEL` methods, whatever they are called on.
    fn records_call(&self, call: &ast::ExprCall) -> bool {
        let Expr::Attribute(a) = &*call.func else { return false };
        let method = a.attr.as_str();
        SAME_MODEL.contains(&method) || (RECORD_METHODS.contains(&method) && self.is_records(&a.value))
    }

    /// The `search()` on records that fetches every match, with no `limit`.
    fn unlimited_search(&self, expr: &Expr) -> bool {
        let Expr::Call(call) = expr else { return false };
        let Expr::Attribute(a) = &*call.func else { return false };
        a.attr.as_str() == "search"
            && self.is_model(&a.value)
            && call.arguments.find_argument_value("limit", 2).is_none()
    }

    fn bind(&mut self, target: &'a Expr, records: bool, dict: bool) {
        let single = target.is_name_expr();
        let mut names = BoundNames::default();
        names.visit_expr(target);
        for name in names.0 {
            toggle(&mut self.records, name, single && records);
            toggle(&mut self.dicts, name, single && dict);
            self.attrs.remove(name);
            self.bound.insert(name);
        }
    }

    /// Binds the names of `target` to `value` by an assignment or `:=`.
    fn bind_value(&mut self, target: &'a Expr, value: &Expr) {
        let records = self.is_records(value);
        self.bind(target, records, is_dict(value));
        if let Expr::Name(n) = target
            && records
            && value.is_attribute_expr()
        {
            self.attrs.insert(n.id.as_str());
        }
    }

    fn assign(&mut self, targets: &'a [Expr], value: &'a Expr) {
        let outer = self.assigned.replace((targets, value));
        self.visit_expr(value);
        self.assigned = outer;
        for target in targets {
            self.visit_expr(target);
            self.bind_value(target, value);
        }
    }

    fn keeping(&mut self, kept: Option<TextRange>, value: &'a Expr) {
        self.kept = kept;
        self.visit_expr(value);
        self.kept = None;
    }

    fn visit_for(&mut self, f: &'a ast::StmtFor, stmt: &'a Stmt) {
        self.visit_expr(&f.iter);
        let records = self.is_records(&f.iter);
        let before = if records {
            self.bound.clone()
        } else {
            FxHashSet::default()
        };
        self.bind(&f.target, records, false);
        let outer = std::mem::replace(&mut self.breaks, records);
        if records {
            let mut bound = BoundNames::default();
            bound.visit_stmt(stmt);
            let mut changed = Changed::default();
            changed.visit_stmt(stmt);
            let (filled, changed): (FxHashSet<_>, FxHashSet<_>) =
                changed.0.into_iter().partition(|name| self.records.contains(name));
            let mut read = Read::default();
            read.visit_stmt(stmt);
            let mut frame = Loop {
                target: f.target.as_name_expr().map(|n| n.id.as_str()),
                varying: bound.0.iter().copied().chain(changed.iter().copied()).collect(),
                filled,
                before,
                steady: FxHashSet::default(),
                read: read.0,
            };
            frame.steady = steady(stmt, &bound.0, &changed, &frame);
            self.loops.push(frame);
            let guards = std::mem::take(&mut self.guards);
            self.visit_body(&f.body);
            self.guards = guards;
            self.loops.pop();
        } else {
            self.visit_body(&f.body);
        }
        self.breaks = outer;
        self.visit_body(&f.orelse);
    }

    fn visit_while(&mut self, w: &'a ast::StmtWhile) {
        self.visit_expr(&w.test);
        let outer = std::mem::replace(&mut self.breaks, false);
        self.visit_body(&w.body);
        self.breaks = outer;
        self.visit_body(&w.orelse);
    }

    fn visit_if(&mut self, i: &'a ast::StmtIf) {
        self.visit_expr(&i.test);
        self.guarded(&i.test, &i.body);
        for clause in &i.elif_else_clauses {
            if let Some(test) = &clause.test {
                self.visit_expr(test);
                self.guarded(test, &clause.body);
            } else {
                self.visit_body(&clause.body);
            }
        }
    }

    fn guarded(&mut self, test: &'a Expr, body: &'a [Stmt]) {
        self.guards.push(test);
        self.visit_body(body);
        self.guards.pop();
    }

    /// Whether the test of an `if` the walk is in, inside the innermost records loop, names `name`.
    fn guards(&self, name: &str) -> bool {
        self.guards.iter().any(|test| reads_name(test, name))
    }

    /// Whether the assignment the walk is in stores the result of `call` in a cache the loop fills
    /// once: a name bound before the loop, when the query does not vary, or under an `if` that tests
    /// it, and an item of a collection bound before the loop under an `if` that tests the collection,
    /// once per key. Under such an `if`, any value fills the cache but one that reads it without
    /// being the result itself, such as a running sum.
    fn caches(&self, call: &ast::ExprCall, frame: &Loop, varies: bool) -> bool {
        let Some((targets, value)) = self.assigned else {
            return false;
        };
        let result = result_of(value, call);
        let fills =
            |name: &str| frame.before.contains(name) && self.guards(name) && (result || !reads_name(value, name));
        targets.iter().any(|target| match target {
            Expr::Name(n) => {
                let name = n.id.as_str();
                fills(name) || (frame.before.contains(name) && !varies && result)
            }
            Expr::Subscript(s) => root_name(&s.value).is_some_and(fills),
            _ => false,
        })
    }

    fn visit_compound(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::For(f) => self.visit_for(f, stmt),
            Stmt::If(i) => self.visit_if(i),
            Stmt::While(w) => self.visit_while(w),
            Stmt::Try(_) => self.isolate(|walk| visitor::walk_stmt(walk, stmt)),
            Stmt::With(w) => self.visit_with(w),
            _ => visitor::walk_stmt(self, stmt),
        }
    }

    /// Walks `visit` kept apart from every loop the walk is in.
    fn isolate(&mut self, visit: impl FnOnce(&mut Self)) {
        let outer = std::mem::replace(&mut self.isolated, self.loops.len());
        visit(self);
        self.isolated = outer;
    }

    fn visit_with(&mut self, w: &'a ast::StmtWith) {
        for item in &w.items {
            self.visit_expr(&item.context_expr);
            if let Some(var) = &item.optional_vars {
                self.bind(var, false, false);
            }
        }
        let savepoint = w
            .items
            .iter()
            .any(|item| matches!(&item.context_expr, Expr::Call(c) if tail_name(&c.func) == Some("savepoint")));
        if savepoint {
            self.isolate(|walk| walk.visit_body(&w.body));
        } else {
            self.visit_body(&w.body);
        }
    }

    /// Whether `body` ends by leaving the loop or the function on every path.
    fn leaves(&self, body: &[Stmt]) -> bool {
        match body.last() {
            Some(Stmt::Break(_)) => self.breaks,
            Some(Stmt::Return(_) | Stmt::Raise(_)) => true,
            Some(Stmt::With(w)) => self.leaves(&w.body),
            Some(Stmt::If(i)) => {
                i.elif_else_clauses.last().is_some_and(|c| c.test.is_none())
                    && self.leaves(&i.body)
                    && i.elif_else_clauses.iter().all(|c| self.leaves(&c.body))
            }
            _ => false,
        }
    }

    fn call(&mut self, call: &'a ast::ExprCall) {
        self.len_of_search(call);
        let Expr::Attribute(a) = &*call.func else { return };
        let Some(frame) = self.counted() else { return };
        let found = match a.attr.as_str() {
            method @ ("search" | "search_count") => self.search_in_loop(call, &a.value, method, frame),
            "browse" => self.browse_in_loop(call, &a.value, frame),
            "create" => self.create_in_loop(call, &a.value, frame),
            "write" => write_in_loop(call, &a.value, frame),
            _ => None,
        };
        if let Some((code, message)) = found {
            self.diags.push(RawDiag::new(code, self.loc.at(call), message));
        }
    }

    /// A model that the loop does not vary, so that one query on it could serve every record.
    fn fixed_model(&self, receiver: &Expr, frame: &Loop) -> bool {
        self.is_model(receiver) && !frame.varies_model(receiver)
    }

    /// OD134 on a search that runs the same query on each pass, or one keyed on a field by a
    /// literal domain whose values alone vary, which one search over every record, grouped by the
    /// field, replaces; unless its result goes to a cache.
    fn search_in_loop(
        &self,
        call: &ast::ExprCall,
        receiver: &Expr,
        method: &str,
        frame: &Loop,
    ) -> Option<(&'static str, String)> {
        if !self.fixed_model(receiver, frame) {
            return None;
        }
        let varies = frame.any_differs(arguments(call));
        if self.caches(call, frame, varies) {
            return None;
        }
        if !varies {
            let message =
                format!("{method}() runs the same query on each pass of the loop; run it once before the loop");
            return Some(("OD134", message));
        }
        let limit = call
            .arguments
            .find_argument_value("limit", if method == "search" { 2 } else { 1 });
        let domain = call.arguments.find_argument_value("domain", 0)?;
        let others = arguments(call).filter(|e| !std::ptr::eq(*e, domain));
        if limit.is_some() || frame.any_differs(others) {
            return None;
        }
        let field = keyed_field(domain, frame)?;
        let message = if method == "search" {
            format!(
                "search() runs a query for each record of the loop, keyed on {field}; search once for all of them and group the result by {field}"
            )
        } else {
            format!(
                "search_count() runs a query for each record of the loop, keyed on {field}; count them all at once with _read_group() grouped by {field}"
            )
        };
        Some(("OD134", message))
    }

    /// OD135 on a browse of one id that differs on each pass, whose record the pass then reads:
    /// it prefetches that record alone (18.0 `odoo/models.py`, 20.0 `odoo/orm/models.py`,
    /// `browse()`). A record browsed and only stored, or browsed by the same id each time, costs no
    /// query per pass; unless its result goes to a cache.
    fn browse_in_loop(&self, call: &ast::ExprCall, receiver: &Expr, frame: &Loop) -> Option<(&'static str, String)> {
        let ids = call.arguments.find_argument_value("ids", 0)?;
        if self.caches(call, frame, frame.differs(ids)) {
            return None;
        }
        let read = self.fetched == Some(call.range())
            || self.assigned.is_some_and(|(targets, value)| {
                gives(value, call)
                    && targets
                        .iter()
                        .any(|t| matches!(t, Expr::Name(n) if frame.read.contains(n.id.as_str())))
            });
        (read && one_id(ids) && frame.differs(ids) && self.fixed_model(receiver, frame)).then(|| {
            (
                "OD135",
                "browse() of one id in a loop fetches each record on its own, a query per record; browse every id at once before the loop, or use the records the id comes from".to_owned(),
            )
        })
    }

    /// OD136 on a create of one record's values, its result dropped or collected, so that one
    /// create of the list of values would return the same records.
    fn create_in_loop(&self, call: &ast::ExprCall, receiver: &Expr, frame: &Loop) -> Option<(&'static str, String)> {
        let values = call.arguments.find_argument_value("vals_list", 0)?;
        let dict = is_dict(values) || matches!(values, Expr::Name(n) if self.dicts.contains(n.id.as_str()));
        let kept = self.kept == Some(call.range());
        (kept && dict && self.fixed_model(receiver, frame)).then(|| {
            (
                "OD136",
                "create() in a loop inserts one record per query; collect the values in a list and create them all in one call".to_owned(),
            )
        })
    }

    /// OD138 on `len()` of a search, which `search_count()` counts in the database.
    fn len_of_search(&mut self, call: &ast::ExprCall) {
        let len = matches!(&*call.func, Expr::Name(n) if n.id.as_str() == "len");
        let [arg] = &*call.arguments.args else { return };
        if len && call.arguments.keywords.is_empty() && self.unlimited_search(arg) {
            self.diags.push(RawDiag::new(
                "OD138",
                self.loc.at(call),
                "len() of search() fetches every matching id only to count them; search_count() counts them in the database",
            ));
        }
    }

    /// OD139 on the first record of a search with no limit, which fetches every match to keep one.
    fn first_of_search(&mut self, s: &ast::ExprSubscript) {
        if is_first(&s.slice) && self.unlimited_search(&s.value) {
            self.diags.push(RawDiag::new(
                "OD139",
                self.loc.at(s),
                "search() without a limit fetches every matching id to keep the first; pass limit=1",
            ));
        }
    }
}

impl<'a> Visitor<'a> for Walk<'a, '_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(ast::StmtFunctionDef { name, .. }) | Stmt::ClassDef(ast::StmtClassDef { name, .. }) => {
                toggle(&mut self.records, name.as_str(), false);
                self.bound.insert(name.as_str());
            }
            Stmt::Assign(a) => self.assign(&a.targets, &a.value),
            Stmt::AnnAssign(ast::StmtAnnAssign {
                target,
                value: Some(value),
                ..
            }) => self.assign(std::slice::from_ref(&**target), value),
            Stmt::AugAssign(a) => {
                self.keeping(collected(a), &a.value);
                self.bind(&a.target, self.is_records(&a.target), false);
            }
            Stmt::Expr(e) => self.keeping(dropped(&e.value), &e.value),
            _ => self.visit_compound(stmt),
        }
    }

    fn visit_body(&mut self, body: &'a [Stmt]) {
        if self.leaves(body) {
            self.isolate(|walk| visitor::walk_body(walk, body));
        } else {
            visitor::walk_body(self, body);
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Lambda(_) => return,
            Expr::Call(c) => self.call(c),
            Expr::Subscript(s) => self.first_of_search(s),
            Expr::Attribute(a) if !matches!(a.attr.as_str(), "id" | "ids" | "env") => {
                self.fetched = Some(a.value.range());
            }
            Expr::Named(n) => {
                self.visit_expr(&n.value);
                self.bind_value(&n.target, &n.value);
                return;
            }
            _ => {}
        }
        visitor::walk_expr(self, expr);
    }
}

/// The names a statement changes in place, setting an item of them or calling a method such as
/// `append()` or `update()` on them: a list of ids or a dict of values a loop fills from each
/// record differs on each pass, though the loop never rebinds its name.
#[derive(Default)]
struct Changed<'a>(Vec<&'a str>);

impl<'a> Visitor<'a> for Changed<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        let targets = match stmt {
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => return,
            Stmt::Assign(a) => a.targets.as_slice(),
            Stmt::AugAssign(a) => std::slice::from_ref(&*a.target),
            _ => &[],
        };
        for target in targets {
            if let Expr::Subscript(s) = target
                && let Some(name) = root_name(&s.value)
            {
                self.0.push(name);
            }
        }
        visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Call(c) = expr
            && let Expr::Attribute(a) = &*c.func
            && MUTATORS.contains(&a.attr.as_str())
            && let Some(name) = root_name(&a.value)
        {
            self.0.push(name);
        }
        visitor::walk_expr(self, expr);
    }
}

/// The plain `name = value` assignments of a statement, outside the functions and classes it holds.
#[derive(Default)]
struct Assigns<'a>(Vec<(&'a str, &'a Expr)>);

impl<'a> Visitor<'a> for Assigns<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        let (targets, value) = match stmt {
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => return,
            Stmt::Assign(a) => (a.targets.as_slice(), &*a.value),
            Stmt::AnnAssign(ast::StmtAnnAssign {
                target,
                value: Some(value),
                ..
            }) => (std::slice::from_ref(&**target), &**value),
            _ => return visitor::walk_stmt(self, stmt),
        };
        for target in targets {
            if let Expr::Name(n) = target {
                self.0.push((n.id.as_str(), value));
            }
        }
        visitor::walk_stmt(self, stmt);
    }
}

/// The names of `frame` the loop statement `stmt` binds with one plain assignment alone, of a value
/// that names nothing else that varies and holds none of the `NEW_VALUE_CALLS`, so that each pass
/// gives them the same value. A name bound before the loop, or assigned in two branches, may hold
/// either value depending on the record.
/// `bound` holds a name once per binding, and `changed` the names the statement changes in place.
fn steady<'a>(stmt: &'a Stmt, bound: &[&'a str], changed: &FxHashSet<&str>, frame: &Loop) -> FxHashSet<&'a str> {
    let mut assigns = Assigns::default();
    assigns.visit_stmt(stmt);
    let mut bindings: FxHashMap<&str, usize> = FxHashMap::default();
    for name in bound {
        *bindings.entry(name).or_default() += 1;
    }
    let mut values: FxHashMap<&'a str, Vec<&'a Expr>> = FxHashMap::default();
    for (name, value) in assigns.0 {
        values.entry(name).or_default().push(value);
    }
    values.retain(|name, v| {
        v.len() == 1
            && bindings.get(name) == Some(&1)
            && !changed.contains(name)
            && !frame.before.contains(name)
            && !v.iter().any(|value| new_value(value))
    });
    let mut steady: FxHashSet<&'a str> = values.keys().copied().collect();
    loop {
        let moving: Vec<_> = steady
            .iter()
            .copied()
            .filter(|name| {
                values[name].iter().any(|value| {
                    any_over_expr(
                        value,
                        |e| matches!(e, Expr::Name(n) if !steady.contains(n.id.as_str()) && frame.names_varying(e)),
                    )
                })
            })
            .collect();
        if moving.is_empty() {
            return steady;
        }
        for name in moving {
            steady.remove(name);
        }
    }
}

/// Whether `value` holds one of the `NEW_VALUE_CALLS`.
fn new_value(value: &Expr) -> bool {
    any_over_expr(
        value,
        |e| matches!(e, Expr::Call(c) if tail_name(&c.func).is_some_and(|name| NEW_VALUE_CALLS.contains(&name))),
    )
}

/// The names a statement reads an attribute of, but its id, ids or environment.
#[derive(Default)]
struct Read<'a>(FxHashSet<&'a str>);

impl<'a> Visitor<'a> for Read<'a> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Attribute(a) = expr
            && let Expr::Name(n) = &*a.value
            && !matches!(a.attr.as_str(), "id" | "ids" | "env")
        {
            self.0.insert(n.id.as_str());
        }
        visitor::walk_expr(self, expr);
    }
}

/// Whether `value` is `call`, or an operand of `or` or `and` that is.
fn gives(value: &Expr, call: &ast::ExprCall) -> bool {
    match value {
        Expr::BoolOp(b) => b.values.iter().any(|v| gives(v, call)),
        v => v.range() == call.range(),
    }
}

/// Whether `value` is the result of `call`: `call`, an operand of `or` or `and` or a branch of
/// `if`-`else` that is, or an attribute, an item or a method call of one. Without an `if` that tests
/// the cache, a value computed from it, such as a running sum, is no cache: each pass runs the query
/// again to compute the next one.
fn result_of(value: &Expr, call: &ast::ExprCall) -> bool {
    match value {
        _ if value.range() == call.range() => true,
        Expr::BoolOp(b) => b.values.iter().any(|v| result_of(v, call)),
        Expr::If(i) => result_of(&i.body, call) || result_of(&i.orelse, call),
        Expr::Attribute(a) => result_of(&a.value, call),
        Expr::Subscript(s) => result_of(&s.value, call),
        Expr::Call(c) => matches!(&*c.func, Expr::Attribute(a) if result_of(&a.value, call)),
        _ => false,
    }
}

/// Whether `expr` reads the name `name`.
fn reads_name(expr: &Expr, name: &str) -> bool {
    any_over_expr(expr, |e| matches!(e, Expr::Name(n) if n.id.as_str() == name))
}

/// The name an item of a name, at any depth, belongs to, such as `vals` in `vals[k][0]`.
fn root_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Name(n) => Some(n.id.as_str()),
        Expr::Subscript(s) => root_name(&s.value),
        _ => None,
    }
}

/// OD137 on a write of values the loop does not vary on the record it takes, directly or through a
/// `SAME_MODEL` call, which one write on every record would make as well. A call in the values may
/// return a new value each time, such as a sequence number or a token, which one write would give
/// every record alike.
fn write_in_loop(call: &ast::ExprCall, receiver: &Expr, frame: &Loop) -> Option<(&'static str, String)> {
    let target = frame.target?;
    let fresh = arguments(call).any(|e| {
        let mut fresh = Fresh::default();
        fresh.visit_expr(e);
        fresh.0
    });
    (on_target(receiver, target, frame) && !fresh && !frame.any_varying(arguments(call))).then(|| {
        (
            "OD137",
            "write() of the same values on each record of the loop updates them one at a time; write them once on the whole recordset".to_owned(),
        )
    })
}

/// Whether an expression holds a call that may return a new value each time it runs.
#[derive(Default)]
struct Fresh(bool);

impl<'a> Visitor<'a> for Fresh {
    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Lambda(_) => {}
            Expr::Call(c) if same_each_time(c) => visitor::walk_arguments(self, &c.arguments),
            Expr::Call(_) => self.0 = true,
            _ => visitor::walk_expr(self, expr),
        }
    }
}

/// A call that gives the same value on each pass of a loop when its arguments do: the clock to the
/// second, which a loop over records seldom outlasts, a translation, a record by its XML id, or a
/// `Command` on records the loop does not vary.
fn same_each_time(call: &ast::ExprCall) -> bool {
    match &*call.func {
        Expr::Name(n) => matches!(n.id.as_str(), "_" | "_lt"),
        Expr::Attribute(a) => {
            matches!(a.attr.as_str(), "now" | "today" | "context_today" | "_" | "ref")
                || matches!(&*a.value, Expr::Name(n) if n.id.as_str() == "Command")
                    && matches!(a.attr.as_str(), "link" | "unlink" | "set" | "clear")
        }
        _ => false,
    }
}

fn on_target(expr: &Expr, target: &str, frame: &Loop) -> bool {
    match expr {
        Expr::Name(n) => n.id.as_str() == target,
        Expr::Call(c) => {
            matches!(&*c.func, Expr::Attribute(a) if SAME_MODEL.contains(&a.attr.as_str()) && on_target(&a.value, target, frame))
                && !frame.any_varying(arguments(c))
        }
        _ => false,
    }
}

/// The attributes of records that may hold records: the public ones with a lowercase letter, but
/// the ids and the environment. The check runs before the index exists, so a field is told from a
/// class constant such as `CODES` by its name alone.
fn holds_records(attr: &str) -> bool {
    !attr.starts_with('_') && !matches!(attr, "id" | "ids" | "env") && attr.bytes().any(|b| b.is_ascii_lowercase())
}

/// `env`, or an `env` attribute, such as `self.env` or `request.env`.
fn is_env(expr: &Expr) -> bool {
    match expr {
        Expr::Name(n) => n.id.as_str() == "env",
        Expr::Attribute(a) => a.attr.as_str() == "env",
        _ => false,
    }
}

fn is_dict(expr: &Expr) -> bool {
    match expr {
        Expr::Dict(_) => true,
        Expr::Call(c) => matches!(&*c.func, Expr::Name(n) if n.id.as_str() == "dict"),
        _ => false,
    }
}

/// The `create()` call an expression statement drops the result of, or adds to a list.
fn dropped(value: &Expr) -> Option<TextRange> {
    let Expr::Call(call) = value else { return None };
    match (tail_name(&call.func), &*call.arguments.args) {
        (Some("create"), _) => Some(call.range()),
        (Some("append"), [Expr::Call(arg)]) if tail_name(&arg.func) == Some("create") => Some(arg.range()),
        _ => None,
    }
}

/// The `create()` call whose records `|=` or `+=` add to the target.
fn collected(a: &ast::StmtAugAssign) -> Option<TextRange> {
    let Expr::Call(call) = &*a.value else { return None };
    (matches!(a.op, Operator::BitOr | Operator::Add) && tail_name(&call.func) == Some("create")).then(|| call.range())
}

/// A single id: `x.id`, an integer, `int()` of something, or a list or tuple of one item.
fn one_id(expr: &Expr) -> bool {
    match expr {
        Expr::Attribute(a) => a.attr.as_str() == "id",
        Expr::NumberLiteral(n) => matches!(n.value, ast::Number::Int(_)),
        Expr::Call(c) => matches!(&*c.func, Expr::Name(n) if n.id.as_str() == "int"),
        _ => matches!(elements(expr), Some([item]) if !item.is_starred_expr()),
    }
}

/// `[0]` or `[:1]`.
fn is_first(slice: &Expr) -> bool {
    match slice {
        Expr::Slice(s) => {
            s.step.is_none()
                && s.lower.as_deref().is_none_or(|e| is_int(e, 0))
                && s.upper.as_deref().is_some_and(|e| is_int(e, 1))
        }
        e => is_int(e, 0),
    }
}

fn is_int(expr: &Expr, value: u64) -> bool {
    matches!(expr, Expr::NumberLiteral(n) if matches!(&n.value, ast::Number::Int(i) if i.as_u64() == Some(value)))
}

/// The field of the one term whose value the loop varies, in a literal domain of operators and
/// `(field, operator, value)` terms whose field and operator are literal strings, when that term
/// compares a field of the model with `=` or `in`. Only then does grouping one search by the field
/// give each record its answer: `child_of`, `!=` or a range on the value, or a second varying
/// value, needs its own query, and `_read_group()` groups by no x2many path, nor by any path in
/// 18.0.
fn keyed_field<'e>(domain: &'e Expr, frame: &Loop) -> Option<&'e str> {
    let terms: Vec<_> = elements(domain)?.iter().map(domain_term).collect::<Option<_>>()?;
    let mut varying = terms.into_iter().flatten().filter(|t| frame.differs(t.2));
    let (field, operator, _) = varying.next()?;
    (varying.next().is_none() && matches!(operator, "=" | "in") && !field.contains('.')).then_some(field)
}

/// A domain operator as `Some(None)`, the field, operator and value of a term as `Some(Some(..))`,
/// and anything else, which leaves the domain unknown, as `None`.
fn domain_term(term: &Expr) -> Option<Option<(&str, &str, &Expr)>> {
    if let Some(operator) = str_lit(term) {
        return DOMAIN_OPERATORS.contains(&operator).then_some(None);
    }
    let [field, operator, value] = elements(term)? else {
        return None;
    };
    Some(Some((str_lit(field)?, str_lit(operator)?, value)))
}

/// The items of a list or tuple literal.
fn elements(expr: &Expr) -> Option<&[Expr]> {
    match expr {
        Expr::List(ast::ExprList { elts, .. }) | Expr::Tuple(ast::ExprTuple { elts, .. }) => Some(elts),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use ruff_python_parser::parse_module;

    use super::*;

    /// The findings on the method `body`, in a model unless `model` is false.
    fn diags(body: &str, model: bool) -> Vec<RawDiag> {
        let src = format!("def f(self, ids, key, text):\n{body}");
        let parsed = parse_module(&src).expect("valid Python");
        let Stmt::FunctionDef(func) = &parsed.syntax().body[0] else {
            panic!("a function")
        };
        check(func, model, &Locator::new(&src))
    }

    /// The codes and lines of the findings on the method `body`, in a model unless `model` is
    /// false.
    fn found(body: &str, model: bool) -> Vec<(&'static str, u32)> {
        diags(body, model).into_iter().map(|d| (d.code, d.pos.line)).collect()
    }

    fn messages(body: &str) -> Vec<String> {
        diags(body, true).into_iter().map(|d| d.message).collect()
    }

    fn codes(body: &str) -> Vec<&'static str> {
        found(body, true).into_iter().map(|(code, _)| code).collect()
    }

    fn looped(line: &str) -> Vec<&'static str> {
        codes(&format!("    for rec in self:\n        {line}\n"))
    }

    const NONE: [&str; 0] = [];

    #[test]
    fn counting_or_taking_the_first_of_a_search_is_reported() {
        for (body, code) in [
            ("    return len(self.search([]))\n", "OD138"),
            (
                "    return len(self.env['res.partner'].search([('x', '=', 1)]))\n",
                "OD138",
            ),
            ("    return self.search([])[0]\n", "OD139"),
            ("    return self.env['a'].sudo().search([], order='id')[:1]\n", "OD139"),
            ("    return self.env['a'].search([])[0:1]\n", "OD139"),
            ("    rec = self.browse(ids)\n    return rec.search([])[0]\n", "OD139"),
        ] {
            assert_eq!(codes(body), [code], "{body}");
        }
        assert_eq!(found("    return self.env['a'].search([])[0]\n", false), [("OD139", 2)]);
    }

    #[test]
    fn counting_a_limited_search_or_other_calls_is_not_reported() {
        for body in [
            "    return len(self.search([], limit=10))\n",
            "    return self.search([], 0, 1)[0]\n",
            "    return self.search([], limit=1)[0]\n",
            "    return self.search([])[1]\n",
            "    return self.search([])[:2]\n",
            "    return self.search([])[1:2]\n",
            "    return re.search('a', text)[0]\n",
            "    return len(re.search('a', text))\n",
            "    return self.id.search([])[0]\n",
            "    return self.env.search([])[0]\n",
            "    return self._origin.search([])[0]\n",
            "    return self.EMAIL_RE.search(text)[0]\n",
            "    return self.partner_id.child_ids.search([])[0]\n",
            "    return (lambda: self.search([])[0])()\n",
            "    def g():\n        return self.search([])[0]\n",
            "    class C:\n        x = self.search([])[0]\n",
        ] {
            assert_eq!(codes(body), NONE, "{body}");
        }
        assert_eq!(found("    return self.search([])[0]\n", false), []);
    }

    #[test]
    fn a_name_holds_records_until_rebound() {
        assert_eq!(
            codes("    x = self.env['a']\n    x = 1\n    return x.search([])[0]\n"),
            NONE
        );
        assert_eq!(codes("    self = 1\n    return self.search([])[0]\n"), NONE);
        assert_eq!(
            codes("    if (x := self.env['a']):\n        return x.search([])[0]\n"),
            ["OD139"]
        );
    }

    #[test]
    fn searches_a_loop_repeats_are_reported() {
        for line in [
            "self.env['res.partner'].search([('name', '=', 'Main')])",
            "self.env['res.partner'].search_count([])",
            "self.env['res.partner'].search([('name', '=', key)], limit=1)",
            "partner = self.env['res.partner'].search([])",
            "self.env['res.partner'].search(domain=[('a', '=', 1)], order='id')",
        ] {
            assert_eq!(looped(line), ["OD134"], "{line}");
        }
        assert_eq!(
            codes(
                "    for rec in self:\n        for line in rec.line_ids:\n            self.env['a'].search([('o', '=', rec.id)])\n"
            ),
            ["OD134"]
        );
    }

    #[test]
    fn searches_keyed_on_a_field_name_it() {
        for (line, field) in [
            (
                "self.env['a'].search(['|', ('state', '=', 'done'), ('order_id', '=', rec.id)])",
                "order_id",
            ),
            (
                "self.env['a'].search([('date', '>=', key), ('order_id', '=', rec.id)])",
                "order_id",
            ),
            ("self.env['a'].search_count([('order_id', '=', rec.id)])", "order_id"),
            (
                "self.env['a'].search([('line_id', 'in', rec.line_ids.ids)], order='id')",
                "line_id",
            ),
            ("self.env['a'].search(domain=(('order_id', '=', rec.id),))", "order_id"),
        ] {
            let found = messages(&format!("    for rec in self:\n        {line}\n"));
            assert_eq!(found.len(), 1, "{line}");
            assert!(found[0].contains(&format!("keyed on {field};")), "{}", found[0]);
        }
    }

    #[test]
    fn searches_keyed_by_a_comparison_one_search_cannot_group_are_not_reported() {
        for operator in ["child_of", "parent_of", "!=", "not in", "<", ">=", "ilike", "=like"] {
            let line = format!("self.env['a'].search([('order_id', '{operator}', rec.id)])");
            assert_eq!(looped(&line), NONE, "{line}");
        }
        let two = "self.env['a'].search([('order_id', '=', rec.id), ('date', '<', rec.date)])";
        assert_eq!(looped(two), NONE);
        let both = "self.env['a'].search([('order_id', '=', rec.id), ('user_id', '=', rec.user_id.id)])";
        assert_eq!(looped(both), NONE);
        for line in [
            "self.env['a'].search_count([('line_ids.product_id', '=', rec.id)])",
            "self.env['a'].search([('config_id.team_id', 'in', rec.ids)])",
        ] {
            assert_eq!(looped(line), NONE, "{line}");
        }
        let fixed_path = "self.env['a'].search([('line_ids.state', '=', 'done'), ('order_id', '=', rec.id)])";
        assert_eq!(looped(fixed_path), ["OD134"]);
    }

    #[test]
    fn searches_each_pass_needs_are_not_reported() {
        for line in [
            "self.env['a'].search([('order_id', '=', rec.id)], limit=1)",
            "self.env['a'].search([('order_id', '=', rec.id)], 0, 1)",
            "self.env['a'].search_count([('order_id', '=', rec.id)], 1)",
            "self.env['a'].search([('order_id', '=', rec.id)], order=rec.order)",
            "self.env['a'].search([(rec.field, '=', 1)])",
            "self.env['a'].search([('a', rec.op, 1)])",
            "self.env['a'].search([('a', '=', 1), rec.term])",
            "self.env['a'].search([('a', '=', rec.x), 'x'])",
            "self.env['a'].search(rec.domain)",
            "self.env[rec.model].search([])",
            "rec.env['a'].search([])",
            "rec.child_ids.search([])",
            "self.env['a'].with_company(rec.company_id).search([])",
            "re.search('a', text)",
            "(lambda: self.env['a'].search([]))()",
        ] {
            assert_eq!(looped(line), NONE, "{line}");
        }
    }

    #[test]
    fn a_cache_filled_in_the_loop_is_not_reported() {
        let cache = "    partner = None\n    for rec in self:\n        if not partner:\n            partner = self.env['a'].search([], limit=1)\n";
        assert_eq!(codes(cache), NONE);
        let param = "    for rec in self:\n        key = key or self.env['a'].search([])\n";
        assert_eq!(codes(param), NONE);
        let inner = "    for rec in self:\n        partner = None\n        partner = self.env['a'].search([])\n";
        assert_eq!(codes(inner), ["OD134"]);
        let keyed = "    partner = None\n    for rec in self:\n        partner = self.env['a'].search([('order_id', '=', rec.id)])\n";
        assert_eq!(codes(keyed), ["OD134"]);
    }

    #[test]
    fn a_cache_an_if_tests_is_not_reported_whatever_it_searches() {
        for body in [
            "    cache = {}\n    for rec in self:\n        if rec.x not in cache:\n            cache[rec.x] = self.env['a'].search([('x', '=', rec.x)]).y\n",
            "    cache = {}\n    for rec in self:\n        for k in rec.keys:\n            if k not in cache:\n                cache[k] = self.env['a'].search([('x', '=', k)])\n",
            "    found = None\n    for rec in self:\n        if found is None:\n            found = self.env['a'].search([('x', 'in', rec.x)])\n",
            "    found = None\n    for rec in self:\n        if rec.y:\n            pass\n        elif not found:\n            d = rec.d\n            found = self.env['a'].search([('x', '=', d)])\n",
        ] {
            assert_eq!(codes(body), NONE, "{body}");
        }
        for body in [
            "    cache = {}\n    for rec in self:\n        cache[rec.x] = self.env['a'].search([('x', '=', rec.x)])\n",
            "    found = None\n    for rec in self:\n        if rec.y:\n            found = self.env['a'].search([('x', '=', rec.x)])\n",
            "    cache = {}\n    if not cache:\n        for rec in self:\n            cache[rec.x] = self.env['a'].search([('x', '=', rec.x)])\n",
            "    for rec in self:\n        found = None\n        if found is None:\n            found = self.env['a'].search([('x', '=', rec.x)])\n",
            "    for rec in self:\n        vals = rec.vals()\n        if vals.get('x'):\n            vals['y'] = self.env['a'].search([('x', '=', rec.x)])\n",
        ] {
            assert_eq!(codes(body), ["OD134"], "{body}");
        }
    }

    #[test]
    fn a_name_the_loop_assigns_the_same_value_does_not_key_a_search() {
        let same = "    for rec in self:\n        f = self.env['f'].search([], limit=1)\n        self.env['a'].search([('f', '=', f.id)])\n";
        let found = messages(same);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[1].contains("same query"), "{}", found[1]);
        for body in [
            "    for rec in self:\n        f = rec.f\n        self.env['a'].search([('f', '=', f)])\n",
            "    for rec in self:\n        f = 1\n        f = rec.f\n        self.env['a'].search([('f', '=', f)])\n",
            "    for rec in self:\n        f = []\n        f.append(rec.id)\n        self.env['a'].search([('f', 'in', f)])\n",
            "    for rec in self:\n        for f in ids:\n            self.env['a'].search([('f', '=', f)])\n",
            "    for rec in self:\n        g = rec.g\n        f = g\n        self.env['a'].search([('f', '=', f)])\n",
            "    for rec in self:\n        if rec.x:\n            f = 1\n        else:\n            f = 2\n        self.env['a'].search([('f', '=', f)])\n",
            "    f = 0\n    for rec in self:\n        if rec.x:\n            f = 1\n        self.env['a'].search([('f', '=', f)])\n",
        ] {
            let found = messages(body);
            assert!(found.iter().all(|m| !m.contains("same query")), "{body}");
        }
    }

    #[test]
    fn a_name_holding_a_new_value_on_each_pass_keys_a_search() {
        for (body, field) in [
            (
                "    for rec in self:\n        code = self.env['ir.sequence'].next_by_code('x')\n        self.env['a'].search([('name', '=', code)])\n",
                "name",
            ),
            (
                "    for rec in self:\n        line = self.env['l'].create({})\n        self.env['a'].search([('line_id', '=', line.id)])\n",
                "line_id",
            ),
            (
                "    for rec in self:\n        number = self.env['ir.sequence'].browse(1).next_by_id()\n        ref = 'R' + number\n        self.env['a'].search_count([('ref', '=', ref)])\n",
                "ref",
            ),
        ] {
            let found = messages(body);
            assert_eq!(found.len(), 1, "{body}");
            assert!(found[0].contains(&format!("keyed on {field};")), "{}", found[0]);
        }
    }

    #[test]
    fn a_copied_dict_is_no_new_value() {
        let body = "    for rec in self:\n        v = vals.copy()\n        self.env['a'].search([('name', '=', v)])\n";
        let found = messages(body);
        assert_eq!(found.len(), 1);
        assert!(found[0].contains("same query"), "{}", found[0]);
    }

    #[test]
    fn a_value_computed_from_a_search_without_a_guard_is_not_a_cache() {
        for body in [
            "    total = 0\n    for rec in self:\n        total = total + self.env['a'].search_count([])\n",
            "    total = 0\n    for rec in self:\n        total = self.env['a'].search_count([]) + total\n",
            "    total = 0\n    for rec in self:\n        if total < 10:\n            total = total + self.env['a'].search_count([('x', '=', rec.x)])\n",
            "    found = None\n    for rec in self:\n        found = key(self.env['a'].search([]))\n",
        ] {
            assert_eq!(codes(body), ["OD134"], "{body}");
        }
    }

    #[test]
    fn a_search_result_or_a_guarded_value_from_it_is_a_cache() {
        for value in [
            "sum(self.env['a'].search([('x', '=', rec.x)]).mapped('y'))",
            "bool(self.env['a'].search_count([('x', '=', rec.x)]))",
            "set(self.env['a'].search([('x', '=', rec.x)]).ids)",
            "self.env['a'].search_count([('x', '=', rec.x)]) > 0",
            "{r.id: r for r in self.env['a'].search([('x', '=', rec.x)])}",
        ] {
            let body = format!(
                "    cache = {{}}\n    for rec in self:\n        if rec.x not in cache:\n            cache[rec.x] = {value}\n"
            );
            assert_eq!(codes(&body), NONE, "{body}");
        }
        let guarded = "    found = None\n    for rec in self:\n        if found is None:\n            found = key(self.env['a'].search([('x', '=', rec.x)]))\n";
        assert_eq!(codes(guarded), NONE);
        for value in [
            "self.env['a'].search([]).ids",
            "self.env['a'].search([], limit=2)[:1]",
            "self.env['a'].search([]).filtered('x')",
            "found or self.env['a'].search([])",
            "found if found else self.env['a'].search([], limit=1)",
            "self.env['a'].search([], limit=1) if not found else found",
        ] {
            let body = format!("    found = None\n    for rec in self:\n        found = {value}\n");
            assert_eq!(codes(&body), NONE, "{body}");
        }
    }

    #[test]
    fn a_browse_a_cache_holds_is_not_reported() {
        for body in [
            "    partner = None\n    for rec in self:\n        if partner is None:\n            partner = self.env['a'].browse(rec.x.id)\n        key.append(partner.name)\n",
            "    names = {}\n    for rec in self:\n        if rec.x.id not in names:\n            names[rec.x.id] = self.env['a'].browse(rec.x.id).name\n",
        ] {
            assert_eq!(codes(body), NONE, "{body}");
        }
        let unguarded = "    partner = None\n    for rec in self:\n        partner = self.env['a'].browse(rec.x.id)\n        key.append(partner.name)\n";
        assert_eq!(codes(unguarded), ["OD135"]);
        let rebuilt = "    for rec in self:\n        vals = rec.vals()\n        if vals.get('x'):\n            vals['y'] = self.env['a'].browse(int(vals['x'])).name\n";
        assert_eq!(codes(rebuilt), ["OD135"]);
    }

    #[test]
    fn a_name_bound_to_an_attribute_is_no_model_but_still_records() {
        for body in [
            "    pattern = self.x_mail_pattern\n    return pattern.search(text)[0]\n",
            "    if (pattern := self.x_mail_pattern):\n        return len(pattern.search(text))\n",
            "    partner = self.partner_id\n    for rec in self:\n        partner.search([])\n",
        ] {
            assert_eq!(codes(body), NONE, "{body}");
        }
        let rebound = "    x = self.partner_id\n    x = self.env['a']\n    return x.search([])[0]\n";
        assert_eq!(codes(rebound), ["OD139"]);
        let looped = "    lines = self.line_ids\n    for line in lines:\n        self.env['a'].search([])\n";
        assert_eq!(codes(looped), ["OD134"]);
    }

    #[test]
    fn an_all_caps_class_constant_holds_no_records() {
        for body in [
            "    for c in self.CODES:\n        self.env['a'].search([('code', '=', c)])\n",
            "    for rec in self:\n        for c in rec.CODES:\n            c.write({'a': 1})\n",
            "    x = self.env['a']\n    for c in x.CODES:\n        self.env['b'].search([('code', '=', c)])\n",
            "    codes = self.CODES\n    for c in codes:\n        self.env['a'].search([('code', '=', c)])\n",
            "    return self.CODES[0].search([])[0]\n",
        ] {
            assert_eq!(codes(body), NONE, "{body}");
        }
        for body in [
            "    for c in self.x_codes:\n        self.env['a'].search([])\n",
            "    for l in self.line_ids:\n        self.env['a'].search([])\n",
        ] {
            assert_eq!(codes(body), ["OD134"], "{body}");
        }
    }

    #[test]
    fn only_a_loop_over_records_counts() {
        for body in [
            "    for i in ids:\n        self.env['a'].search([('id', '=', i)])\n",
            "    for i in self.ids:\n        self.env['a'].browse(i)\n",
            "    for k in self.env.context:\n        self.env['a'].search([])\n",
            "    for n in self.mapped('name'):\n        self.env['a'].search([('name', '=', n)])\n",
            "    while self:\n        self.env['a'].search([])\n",
            "    [self.env['a'].search([]) for rec in self]\n",
            "    for rec in self:\n        def g():\n            self.env['a'].search([])\n",
        ] {
            assert_eq!(codes(body), NONE, "{body}");
        }
        for body in [
            "    for rec in self.env['a'].search([]):\n        self.env['b'].search([])\n",
            "    for rec in self.line_ids.filtered('x'):\n        self.env['b'].search([])\n",
            "    for rec in self.sudo():\n        self.env['b'].search([])\n",
            "    for rec in self:\n        for k in ids:\n            self.env['b'].search([])\n",
        ] {
            assert_eq!(codes(body), ["OD134"], "{body}");
        }
        let plain = "    for rec in self:\n        self.env['b'].search([])\n";
        assert_eq!(found(plain, false), []);
        let env = "    for rec in self.env['a'].browse(ids):\n        self.env['b'].search([])\n";
        assert_eq!(found(env, false), [("OD134", 3)]);
    }

    #[test]
    fn a_try_or_a_savepoint_keeps_each_record_apart() {
        for body in [
            "    for rec in self:\n        try:\n            self.env['a'].search([])\n        except Exception:\n            pass\n",
            "    for rec in self:\n        with self.env.cr.savepoint():\n            self.env['a'].search([])\n",
            "    for rec in self:\n        try:\n            for x in ids:\n                self.env['a'].search([])\n        finally:\n            pass\n",
            "    for rec in self:\n        try:\n            pass\n        except Exception:\n            self.env['a'].search([])\n",
            "    for rec in self:\n        try:\n            pass\n        except Exception:\n            rec.write({'error': True})\n",
            "    for rec in self:\n        try:\n            pass\n        except Exception:\n            pass\n        else:\n            self.env['a'].search([])\n        finally:\n            self.env['a'].search([])\n",
        ] {
            assert_eq!(codes(body), NONE, "{body}");
        }
        for body in [
            "    try:\n        for rec in self:\n            self.env['a'].search([])\n    except Exception:\n        pass\n",
            "    for rec in self:\n        try:\n            for line in rec.line_ids:\n                self.env['a'].search([])\n        except Exception:\n            pass\n",
            "    for rec in self:\n        with open(text):\n            self.env['a'].search([])\n",
        ] {
            assert_eq!(codes(body), ["OD134"], "{body}");
        }
    }

    #[test]
    fn a_call_in_a_block_that_leaves_the_loop_runs_once() {
        for body in [
            "    for rec in self:\n        if rec.x:\n            rec.write({'a': 1})\n            break\n",
            "    for rec in self:\n        if rec.x:\n            return self.env['a'].search([])\n",
            "    for rec in self:\n        self.env['a'].search([])\n        raise ValueError()\n",
            "    for rec in self:\n        if rec.x:\n            rec.write({'a': 1})\n            break\n        else:\n            return\n",
            "    for rec in self:\n        for i in ids:\n            self.env['a'].search([])\n            return\n",
        ] {
            assert_eq!(codes(body), NONE, "{body}");
        }
        for body in [
            "    for rec in self:\n        for i in ids:\n            rec.write({'a': 1})\n            break\n",
            "    for rec in self:\n        while key:\n            self.env['a'].search([])\n            break\n",
            "    for rec in self:\n        if rec.x:\n            rec.write({'a': 1})\n            continue\n",
            "    for rec in self:\n        if rec.x:\n            rec.write({'a': 1})\n            break\n        rec.write({'b': 1})\n",
            "    for rec in self:\n        if rec.x:\n            break\n        self.env['a'].search([])\n",
        ] {
            assert_eq!(codes(body).len(), 1, "{body}");
        }
    }

    #[test]
    fn browsing_one_id_in_a_loop_and_reading_it_is_reported() {
        for line in [
            "self.env['a'].browse(rec.partner_id.id).name",
            "self.env['a'].browse(int(rec.ref)).exists()",
            "self.env['a'].browse([rec.partner_id.id]).name",
            "self.env['a'].browse(ids=(rec.x,)).name",
        ] {
            assert_eq!(looped(line), ["OD135"], "{line}");
        }
        for value in [
            "self.env['a'].browse(rec.x.id)",
            "rec.a or self.env['a'].browse(rec.x.id)",
        ] {
            let named = format!("    for rec in self:\n        a = {value}\n        key.append(a.name)\n");
            assert_eq!(found(&named, true), [("OD135", 3)], "{named}");
        }
        for line in [
            "self.env['a'].browse(ids).name",
            "self.env['a'].browse(rec.line_ids.ids).name",
            "self.env['a'].browse([1, 2]).name",
            "self.env['a'].browse([*ids]).name",
            "self.env['a'].browse().name",
            "rec.env['a'].browse(rec.partner_id.id).name",
            "self.env['a'].browse(7).name",
            "self.env['a'].browse(int(key)).name",
            "self.env['a'].browse(rec.partner_id.id)",
            "self.env['a'].browse(rec.partner_id.id).id",
            "rec.partner_id = self.env['a'].browse(rec.id)",
            "a = self.env['a'].browse(rec.id)",
            "key.append(self.env['a'].browse(rec.id))",
        ] {
            assert_eq!(looped(line), NONE, "{line}");
        }
        let steady = "    for rec in self:\n        a = int(key)\n        self.env['a'].browse(a).name\n";
        assert_eq!(codes(steady), NONE);
    }

    #[test]
    fn creating_one_record_a_pass_is_reported() {
        for line in [
            "self.env['a'].create({'order_id': rec.id})",
            "self.env['a'].create(dict(order_id=rec.id))",
            "self.env['a'].create(vals_list={'order_id': rec.id})",
        ] {
            assert_eq!(looped(line), ["OD136"], "{line}");
        }
        for collect in ["out |= ", "out += "] {
            let body =
                format!("    out = self.env['a']\n    for rec in self:\n        {collect}self.env['a'].create({{}})\n");
            assert_eq!(codes(&body), ["OD136"], "{body}");
        }
        let append = "    out = []\n    for rec in self:\n        out.append(self.env['a'].create({}))\n";
        assert_eq!(codes(append), ["OD136"]);
        let named = "    for rec in self:\n        vals = {'a': rec.id}\n        vals['b'] = 1\n        self.env['a'].create(vals)\n";
        assert_eq!(found(named, true), [("OD136", 5)]);
    }

    #[test]
    fn creating_records_the_pass_uses_is_not_reported() {
        for line in [
            "new = self.env['a'].create({'order_id': rec.id})",
            "rec.line_id = self.env['a'].create({})",
            "self.env['a'].create({}).post()",
            "self.env['a'].create([{'x': 1}, {'x': 2}])",
            "self.env['a'].create(rec._prepare_line())",
            "out -= self.env['a'].create({})",
            "rec.env['a'].create({})",
            "self.env['a'].create({}) if rec.x else None",
        ] {
            assert_eq!(looped(line), NONE, "{line}");
        }
        let rebound = "    for rec in self:\n        vals = {'a': rec.id}\n        vals = rec.fix(vals)\n        self.env['a'].create(vals)\n";
        assert_eq!(codes(rebound), NONE);
    }

    #[test]
    fn writing_the_same_values_on_each_record_is_reported() {
        for line in [
            "rec.write({'state': 'done'})",
            "rec.sudo().write({'state': 'done'})",
            "rec.with_context(tracking_disable=True).write({'state': 'done'})",
            "rec.write(vals={'date': key})",
            "rec.write({'date': fields.Datetime.now(), 'day': fields.Date.context_today(self)})",
            "rec.write({'note': _('Done'), 'stage_id': self.env.ref('a.b').id})",
            "rec.write({'tag_ids': [Command.link(key)]})",
        ] {
            assert_eq!(looped(line), ["OD137"], "{line}");
        }
        let lines = "    lines = self.line_ids\n    for line in lines:\n        line.write({'amount': 0})\n";
        assert_eq!(found(lines, true), [("OD137", 4)]);
        assert_eq!(found(lines, false), []);
    }

    #[test]
    fn writing_values_of_the_record_is_not_reported() {
        for line in [
            "rec.write({'name': rec.partner_id.name})",
            "rec.with_company(rec.company_id).write({'a': 1})",
            "rec.partner_id.write({'a': 1})",
            "self.write({'a': 1})",
            "rec.write(rec._values())",
            "rec.write({'name': self.env['ir.sequence'].next_by_code('a')})",
            "rec.write({'token': str(uuid.uuid4())})",
            "rec.write({'date': fields.Datetime.now() + timedelta(key)})",
        ] {
            assert_eq!(looped(line), NONE, "{line}");
        }
        let computed = "    for rec in self:\n        vals = {'a': 1}\n        rec.write(vals)\n";
        assert_eq!(codes(computed), NONE);
        for change in [
            "vals['b'] = rec.b",
            "vals['b']['c'] = rec.b",
            "vals.update(b=rec.b)",
            "vals.setdefault('b', rec.b)",
        ] {
            let filled =
                format!("    vals = {{'a': 1}}\n    for rec in self:\n        {change}\n        rec.write(vals)\n");
            assert_eq!(codes(&filled), NONE, "{filled}");
        }
        let fixed = "    vals = {'a': 1}\n    for rec in self:\n        other['b'] = rec.b\n        rec.write(vals)\n";
        assert_eq!(codes(fixed), ["OD137"]);
        let tupled = "    for i, rec in enumerate(self):\n        rec.write({'a': 1})\n";
        assert_eq!(codes(tupled), NONE);
    }

    #[test]
    fn a_list_the_loop_fills_varies() {
        let seen = "    seen = []\n    for rec in self:\n        self.env['a'].search([('id', 'not in', seen)])\n        seen.append(rec.id)\n";
        assert_eq!(codes(seen), NONE);
    }

    #[test]
    fn records_the_loop_changes_still_name_their_model() {
        for change in ["self.update({'a': rec.a})", "self['a'] = rec.a"] {
            let body = format!("    for rec in self:\n        {change}\n        self.env['a'].search([])\n");
            assert_eq!(codes(&body), ["OD134"], "{body}");
        }
    }
}
