//! What a method body does with its arguments and its records, for the rules on ORM overrides,
//! compute methods and caches: `python.rs` raises those that one file decides, and `checks.rs`
//! those whose outcome depends on the module's series or on every merged definition of a field. A
//! predicate that meets code it cannot follow answers so that the rule stays silent.

use std::cell::Cell;

use ruff_python_ast::comparable::ComparableExpr;
use ruff_python_ast::helpers::{Truthiness, is_constant};
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, CmpOp, Expr, ExprContext, Stmt};
use ruff_python_parser::parse_expression;
use ruff_python_stdlib::builtins::is_python_builtin;

use crate::facts::UNREAD_TYPE;
use crate::python;

/// Past every Python minor version, so that a builtin of any version counts.
const ANY_PYTHON: u8 = u8::MAX;

/// Methods a dict has and a list lacks.
const DICT_METHODS: &[&str] = &["get", "items", "keys", "values", "setdefault", "update"];

pub(crate) fn is_name(expr: &Expr, name: &str) -> bool {
    matches!(expr, Expr::Name(n) if n.id.as_str() == name)
}

fn is_str(expr: &Expr, text: &str) -> bool {
    matches!(expr, Expr::StringLiteral(s) if s.value.to_str() == text)
}

/// The values of a call, positional and keyword.
pub(crate) fn arguments(call: &ast::ExprCall) -> impl Iterator<Item = &Expr> + Clone {
    let keywords = call.arguments.keywords.iter().map(|k| &k.value);
    call.arguments.args.iter().chain(keywords)
}

/// Whether a `return` of the method itself, not of a function it nests, gives a value.
pub(crate) fn returns_value(body: &[Stmt]) -> bool {
    returned(body).iter().any(|v| !v.is_none_literal_expr())
}

/// The key `key` of a literal dict that a `return` of the method itself gives.
pub(crate) fn returned_key<'b>(body: &'b [Stmt], key: &str) -> Option<&'b Expr> {
    returned(body).into_iter().find_map(|value| dict_key(value, key))
}

/// The key of the literal dict that is the string `key`.
pub(crate) fn dict_key<'e>(expr: &'e Expr, key: &str) -> Option<&'e Expr> {
    dict_item(expr, key)?.key.as_ref()
}

/// The item of the literal dict whose key is the string `key`.
pub(crate) fn dict_item<'e>(expr: &'e Expr, key: &str) -> Option<&'e ast::DictItem> {
    let Expr::Dict(d) = expr else { return None };
    d.items
        .iter()
        .find(|item| item.key.as_ref().is_some_and(|k| is_str(k, key)))
}

/// What the `return` statements of the method itself, not of a function it nests, give.
fn returned(body: &[Stmt]) -> Vec<&Expr> {
    let mut finder = ReturnFinder::default();
    finder.visit_body(body);
    finder.values
}

#[derive(Default)]
struct ReturnFinder<'a> {
    values: Vec<&'a Expr>,
}

impl<'a> Visitor<'a> for ReturnFinder<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
            Stmt::Return(r) => self.values.extend(r.value.as_deref()),
            _ => visitor::walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, _: &'a Expr) {}
}

/// Whether the body reads `param` as one dict rather than a list of them: by a string key, a
/// mapping method, `in` with a string, or `**`. A body that rebinds it or tests its type may take
/// either shape, so it does not count.
pub(crate) fn reads_as_dict(body: &[Stmt], param: &str) -> bool {
    let mut reads = DictReads {
        param,
        reads: false,
        either_shape: false,
    };
    reads.visit_body(body);
    reads.reads && !reads.either_shape
}

struct DictReads<'p> {
    param: &'p str,
    reads: bool,
    either_shape: bool,
}

impl DictReads<'_> {
    fn is_param(&self, expr: &Expr) -> bool {
        is_name(expr, self.param)
    }

    fn reads_dict(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Subscript(s) => self.is_param(&s.value) && s.slice.is_string_literal_expr(),
            Expr::Call(c) => self.dict_call(c),
            Expr::Compare(c) => c.iter().any(|(left, op, right)| {
                matches!(op, CmpOp::In | CmpOp::NotIn) && left.is_string_literal_expr() && self.is_param(right)
            }),
            Expr::Dict(d) => d.items.iter().any(|i| i.key.is_none() && self.is_param(&i.value)),
            _ => false,
        }
    }

    fn dict_call(&self, call: &ast::ExprCall) -> bool {
        let unpacked = call
            .arguments
            .keywords
            .iter()
            .any(|k| k.arg.is_none() && self.is_param(&k.value));
        let method = match &*call.func {
            Expr::Attribute(a) if self.is_param(&a.value) => a.attr.as_str(),
            _ => return unpacked,
        };
        let pops_key = method == "pop" && call.arguments.args.first().is_some_and(Expr::is_string_literal_expr);
        unpacked || pops_key || DICT_METHODS.contains(&method)
    }

    fn takes_either_shape(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Name(n) => n.id.as_str() == self.param && n.ctx != ExprContext::Load,
            Expr::Call(c) => {
                is_name(&c.func, "isinstance") && c.arguments.args.first().is_some_and(|a| self.is_param(a))
            }
            _ => false,
        }
    }
}

impl<'a> Visitor<'a> for DictReads<'_> {
    /// A nested function may name its own parameter alike.
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if !matches!(stmt, Stmt::FunctionDef(_) | Stmt::ClassDef(_)) {
            visitor::walk_stmt(self, stmt);
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        self.reads |= self.reads_dict(expr);
        self.either_shape |= self.takes_either_shape(expr);
        if !expr.is_lambda_expr() {
            visitor::walk_expr(self, expr);
        }
    }
}

/// The names an `ormcache` key reads that are neither in `params`, bound by the key itself, nor
/// Python builtins, in order and once each; None when the key does not parse.
pub(crate) fn unknown_key_names(key: &str, params: &[&str]) -> Option<Vec<String>> {
    let parsed = parse_expression(key.trim()).ok()?;
    let mut names = KeyNames::default();
    names.visit_expr(parsed.expr());
    let mut unknown: Vec<String> = vec![];
    for name in names.loads {
        let known = params.contains(&name.as_str())
            || names.bound.contains(&name)
            || is_python_builtin(&name, ANY_PYTHON, false);
        if !known && !unknown.contains(&name) {
            unknown.push(name);
        }
    }
    Some(unknown)
}

/// The names an expression reads, and those its comprehensions, lambdas and `:=` bind anywhere in
/// it: a name bound in one part and read in another is taken as known.
#[derive(Default)]
struct KeyNames {
    loads: Vec<String>,
    bound: Vec<String>,
}

impl<'a> Visitor<'a> for KeyNames {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Name(n) = expr {
            let names = if n.ctx == ExprContext::Load {
                &mut self.loads
            } else {
                &mut self.bound
            };
            names.push(n.id.to_string());
        }
        visitor::walk_expr(self, expr);
    }

    fn visit_parameter(&mut self, parameter: &'a ast::Parameter) {
        self.bound.push(parameter.name.to_string());
        visitor::walk_parameter(self, parameter);
    }
}

/// Whether a compute method can leave `field` unassigned on a record of `self`. Its one
/// `for record in self:` loop must hold every write to the field, each on `record`; the method
/// must never read the field nor write it by a name built at runtime, and a path through the loop
/// body must skip it or leave the loop. `ftype` is the field's type, such as `Char`, which decides
/// the constants it takes without raising.
pub(crate) fn leaves_unassigned(body: &[Stmt], field: &str, ftype: &str) -> bool {
    let Some((record, records_loop)) = only_loop_over_self(body) else {
        return false;
    };
    let whole = FieldUses::scan(body, field, record);
    let in_loop = FieldUses::scan(&records_loop.body, field, record);
    if whole.reads || whole.dynamic || whole.writes != in_loop.record_writes || calls_self(body) {
        return false;
    }
    Paths::new(record, field, ftype).block(&records_loop.body) > Flow::Assigned
}

/// Whether a statement outside the loop over `self` calls a method of `self` or passes it on, so
/// that a helper may assign the field before or after the loop.
fn calls_self(body: &[Stmt]) -> bool {
    let mut calls = SelfCalls::default();
    calls.visit_body(body);
    calls.found
}

#[derive(Default)]
struct SelfCalls {
    found: bool,
}

impl<'a> Visitor<'a> for SelfCalls {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
            Stmt::For(f) if is_name(&f.iter, "self") => {}
            Stmt::Expr(e) => self.found |= matches!(&*e.value, Expr::Call(c) if may_assign(c, "self")),
            _ => visitor::walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, _: &'a Expr) {}
}

/// A method of `self` may assign the field on every record.
fn calls_self_method(call: &ast::ExprCall) -> bool {
    matches!(&*call.func, Expr::Attribute(a) if is_name(&a.value, "self"))
}

/// A method of the record or of `self`, or a call given the record, may assign the field.
fn may_assign(call: &ast::ExprCall, record: &str) -> bool {
    matches!(&*call.func, Expr::Attribute(a) if is_name(&a.value, record) || is_name(&a.value, "self"))
        || arguments(call).any(|a| is_name(a, record))
}

/// Whether a `while` test is a constant that is always true.
fn always_true(test: &Expr) -> bool {
    match test {
        Expr::BooleanLiteral(b) => b.value,
        Expr::NumberLiteral(n) => matches!(&n.value, ast::Number::Int(i) if i.as_u64().is_some_and(|v| v != 0)),
        _ => false,
    }
}

/// The body and `else` of a `for` or `while`, and whether it is a `while` that only a `break`
/// leaves.
fn loop_parts(stmt: &Stmt) -> Option<(&[Stmt], &[Stmt], bool)> {
    match stmt {
        Stmt::For(f) => Some((&f.body, &f.orelse, false)),
        Stmt::While(w) => Some((&w.body, &w.orelse, always_true(&w.test))),
        _ => None,
    }
}

/// Whether a statement is a `while` with an always true test and no `break` of its own, which only
/// a `return` or an exception leaves, so nothing after it runs.
fn never_ends(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::While(w) if always_true(&w.test) && !breaks_own(&w.body))
}

/// Whether a `break` in a loop's `body` leaves that loop. One in a nested loop's body leaves the
/// nested loop, while one in its `else` still leaves this one.
fn breaks_own(body: &[Stmt]) -> bool {
    let mut own = OwnBreak::default();
    own.visit_body(body);
    own.found
}

#[derive(Default)]
struct OwnBreak {
    found: bool,
}

impl<'a> Visitor<'a> for OwnBreak {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Break(_) => self.found = true,
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
            Stmt::For(ast::StmtFor { orelse, .. }) | Stmt::While(ast::StmtWhile { orelse, .. }) => {
                self.visit_body(orelse);
            }
            _ => visitor::walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, _: &'a Expr) {}
}

fn only_loop_over_self(body: &[Stmt]) -> Option<(&str, &ast::StmtFor)> {
    let mut loops = body.iter().filter_map(|stmt| match stmt {
        Stmt::For(f) if !f.is_async && is_name(&f.iter, "self") => Some(f),
        _ => None,
    });
    let records_loop = loops.next()?;
    if loops.next().is_some() {
        return None;
    }
    Some((records_loop.target.as_name_expr()?.id.as_str(), records_loop))
}

/// How the method reads and writes one field: `writes` counts the places that assign it, and
/// `record_writes` those that assign it on the record the loop names.
struct FieldUses<'a> {
    field: &'a str,
    record: &'a str,
    reads: bool,
    dynamic: bool,
    writes: usize,
    record_writes: usize,
}

impl<'a> FieldUses<'a> {
    fn scan(body: &[Stmt], field: &'a str, record: &'a str) -> Self {
        let mut uses = Self {
            field,
            record,
            reads: false,
            dynamic: false,
            writes: 0,
            record_writes: 0,
        };
        uses.visit_body(body);
        uses
    }

    fn is_record(&self, expr: &Expr) -> bool {
        is_name(expr, self.record) || is_name(expr, "self")
    }

    fn names_field(&self, expr: &Expr) -> bool {
        match expr {
            Expr::List(l) => l.elts.iter().any(|e| is_str(e, self.field)),
            Expr::Tuple(t) => t.elts.iter().any(|e| is_str(e, self.field)),
            e => is_str(e, self.field),
        }
    }

    fn use_at(&mut self, receiver: &Expr, ctx: ExprContext) {
        match ctx {
            ExprContext::Load => self.reads = true,
            _ => self.wrote(receiver),
        }
    }

    fn wrote(&mut self, receiver: &Expr) {
        self.writes += 1;
        self.record_writes += usize::from(is_name(receiver, self.record));
    }

    fn subscript(&mut self, s: &ast::ExprSubscript) {
        if is_str(&s.slice, self.field) {
            self.use_at(&s.value, s.ctx);
        } else if s.ctx == ExprContext::Store && self.is_record(&s.value) && !s.slice.is_string_literal_expr() {
            self.dynamic = true;
        }
    }

    fn call(&mut self, call: &ast::ExprCall) {
        self.dynamic |= is_name(&call.func, "setattr");
        self.reads |= arguments(call).any(|a| self.names_field(a));
        if let Expr::Attribute(a) = &*call.func
            && matches!(a.attr.as_str(), "update" | "write")
        {
            self.values_written(&a.value, call);
        }
    }

    /// `update()` and `write()` take a dict; one whose keys are not all literal may name the field.
    fn values_written(&mut self, receiver: &Expr, call: &ast::ExprCall) {
        match literal_keys(call) {
            Some(keys) if keys.contains(&self.field) => self.wrote(receiver),
            Some(_) => {}
            None => self.dynamic |= self.is_record(receiver),
        }
    }
}

/// The keys of the one dict literal a call takes, when they are all strings.
fn literal_keys(call: &ast::ExprCall) -> Option<Vec<&str>> {
    let ([Expr::Dict(d)], []) = (&*call.arguments.args, &*call.arguments.keywords) else {
        return None;
    };
    d.items
        .iter()
        .map(|i| Some(i.key.as_ref()?.as_string_literal_expr()?.value.to_str()))
        .collect()
}

impl<'a> Visitor<'a> for FieldUses<'_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if let Stmt::AugAssign(a) = stmt
            && (matches!(&*a.target, Expr::Attribute(t) if t.attr.as_str() == self.field)
                || matches!(&*a.target, Expr::Subscript(t) if is_str(&t.slice, self.field)))
        {
            self.reads = true;
        }
        visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Attribute(a) if a.attr.as_str() == self.field => self.use_at(&a.value, a.ctx),
            Expr::Subscript(s) => self.subscript(s),
            Expr::Call(c) => self.call(c),
            _ => {}
        }
        visitor::walk_expr(self, expr);
    }
}

/// How the paths through some statements of the loop body end for the field, from best to worst,
/// so that paths join to the worst of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Flow {
    /// Every path calls a method of `self`, which may assign it on every record, or goes on to the
    /// next iteration or raises, so leaving the loop over `self` after that skips none.
    Filled,
    /// Every path assigns it, or leaves by an exception that Odoo reports anyway.
    Assigned,
    /// No path leaves, and one goes on without assigning it.
    Open,
    /// In a nested loop, a path starts its next iteration by a `continue` without assigning it,
    /// so a loop that assigns in its body may still end without the field.
    Continued,
    /// A path ends the iteration, or the nested loop it is in, without assigning it.
    Escaped,
    /// A path leaves the loop over `self` from any depth, by a `return` or by a `break` after the
    /// assignment, so the records after it go unassigned.
    Left,
}

fn join(flows: impl IntoIterator<Item = Flow>) -> Flow {
    flows.into_iter().fold(Flow::Filled, Flow::max)
}

/// Whether two expressions are written alike: `super()` and `super(Class, self)` are not.
pub(crate) fn same(x: &Expr, y: &Expr) -> bool {
    ComparableExpr::from(x) == ComparableExpr::from(y)
}

/// Whether one of two tests holds exactly when the other does not: `not e` against `e`, or one
/// comparison against the same operands under the negated operator, as `==` against `!=`.
fn complementary(a: &Expr, b: &Expr) -> bool {
    let negates =
        |x: &Expr, y: &Expr| matches!(x, Expr::UnaryOp(u) if u.op == ast::UnaryOp::Not && same(&u.operand, y));
    negates(a, b) || negates(b, a) || same_comparison(a, b, true)
}

fn equivalent(a: &Expr, b: &Expr) -> bool {
    same(a, b) || same_comparison(a, b, false)
}

/// Whether two one-operator comparisons take the same operands under the same operator, or the
/// negated one when `negated`. The operands of `==`, `!=`, `is` and `is not` may come in either
/// order.
fn same_comparison(a: &Expr, b: &Expr, negated: bool) -> bool {
    let (Expr::Compare(x), Expr::Compare(y)) = (a, b) else {
        return false;
    };
    let ([p], [q], [xl, xr], [yl, yr]) = (&*x.ops, &*y.ops, &*x.operands, &*y.operands) else {
        return false;
    };
    let symmetric = matches!(p, CmpOp::Eq | CmpOp::NotEq | CmpOp::Is | CmpOp::IsNot);
    let op = if negated { p.negate() } else { *p };
    *q == op && ((same(xl, yl) && same(xr, yr)) || (symmetric && same(xl, yr) && same(xr, yl)))
}

/// A statement that, on the paths where `test` evaluates to `holds`, ends as `flow`, and elsewhere
/// goes on.
#[derive(Clone, Copy)]
struct Side<'s> {
    test: &'s Expr,
    holds: bool,
    flow: Flow,
}

impl Side<'_> {
    /// Two sides of one polarity cover every path under complementary tests, and two of opposite
    /// polarities under the same test.
    fn covers(&self, other: &Side) -> bool {
        if self.holds == other.holds {
            complementary(self.test, other.test)
        } else {
            equivalent(self.test, other.test)
        }
    }
}

/// A side assigns on every path with an earlier one of `sides` that it covers, and otherwise
/// waits for a later one.
fn pair<'s>(side: Side<'s>, sides: &mut Vec<Side<'s>>) -> Flow {
    if let Some(other) = sides.iter().find(|o| o.covers(&side)) {
        return side.flow.max(other.flow);
    }
    sides.push(side);
    Flow::Open
}

/// The side of an `if` without `elif` that assigns, given the flows of its body and `clauses`:
/// the body when its test holds, or else a lone `else` when it does not.
fn side<'s>(i: &'s ast::StmtIf, body: Flow, clauses: &[Flow]) -> Option<Side<'s>> {
    let orelse = match (&*i.elif_else_clauses, clauses) {
        ([], []) => None,
        ([c], [orelse]) if c.test.is_none() => Some(*orelse),
        _ => return None,
    };
    let (holds, flow) = if body == Flow::Open {
        (false, orelse?)
    } else {
        (true, body)
    };
    (flow <= Flow::Assigned).then_some(Side {
        test: &i.test,
        holds,
        flow,
    })
}

/// The sides each branch of `i` starts with, its body then each clause: `sides`, those of the
/// statements before `i`, then the tests. No path through a branch has an earlier test of the chain
/// true, nor its own test false, so a side `Filled` on those paths lets a statement there pair with
/// the test alone: `for line in x:` under `elif x:`, or under `if not x:` then `elif y:`.
fn guards<'s>(i: &'s ast::StmtIf, sides: &[Side<'s>]) -> Vec<Vec<Side<'s>>> {
    let filled = |test, holds| Side {
        test,
        holds,
        flow: Flow::Filled,
    };
    let tests = std::iter::once(Some(&*i.test)).chain(i.elif_else_clauses.iter().map(|c| c.test.as_ref()));
    let mut earlier = sides.to_vec();
    tests
        .map(|test| {
            let mut seed = earlier.clone();
            if let Some(test) = test {
                seed.push(filled(test, false));
                earlier.push(filled(test, true));
            }
            seed
        })
        .collect()
}

/// How statements jump before a call of a method of `self`, which may assign the field on every
/// record: each block is walked up to the first statement that fills, that call, a `raise`, or a
/// compound statement making one on every path. A `break` or `continue` in a loop among them stays
/// in it, and only a `return` or a `raise` there reaches the walk around the loop.
struct Exits<'a> {
    /// The walk it serves, which tells what assigns the field and which writes of it succeed.
    paths: Paths<'a>,
    /// In the loop over `self` itself, where a `break` leaves it and a `continue` ends the
    /// iteration; in a nested loop, both only skip the rest of its body.
    outer: bool,
    /// Inside a loop among the walked statements, where a `break` or `continue` stays; not inside a
    /// `while True:`, whose own `break` is tracked.
    in_loop: bool,
    /// A path leaves the loop over `self`.
    leaves: bool,
    /// A path goes on in the loop it is in without reaching the rest.
    skips: bool,
    /// A path leaves the loop it is in by a `break`.
    breaks: bool,
    /// A path leaves the loop it is in by a `break` with no `finally` that fills on the way out.
    unfilled_break: bool,
    /// A path stops by a `continue` of the loop over `self`, a `raise`, or an exception leaving a
    /// `while True:` loop with no `break` of its own and a body that does not fill, instead of
    /// filling, which matters only to a `finally` around it, since that still runs.
    ends: bool,
}

impl<'a> Exits<'a> {
    fn new(nested: bool, paths: Paths<'a>) -> Self {
        Self {
            paths,
            outer: !nested,
            in_loop: false,
            leaves: false,
            skips: false,
            breaks: false,
            unfilled_break: false,
            ends: false,
        }
    }

    /// Whether `stmts` may `break` the loop they are in, or `return`, before their first statement
    /// that fills.
    fn leaves_unfilled(stmts: &[Stmt], paths: Paths<'a>) -> bool {
        let mut exits = Exits::new(true, paths);
        exits.body(stmts);
        exits.breaks || exits.leaves
    }

    /// Whether a statement of `stmts` fills.
    fn body(&mut self, stmts: &[Stmt]) -> bool {
        stmts.iter().any(|s| self.fills(s))
    }

    /// A `raise` fills, since Odoo reports the exception anyway. A `with` fills when its body
    /// does, unless it swallows an exception its body may raise, which skips the call; an `if`
    /// with an `elif` or `else`, a `try` or a `match` fills when every path through it does, as
    /// `branches` and `handled` read them, and a loop as `looped` does.
    fn fills(&mut self, stmt: &Stmt) -> bool {
        match stmt {
            Stmt::Expr(e) => matches!(&*e.value, Expr::Call(c) if calls_self_method(c)),
            Stmt::Raise(_) => {
                self.ends = true;
                true
            }
            Stmt::With(w) => self.body(&w.body) && !may_swallow(w, self.paths),
            Stmt::If(i) => {
                let body = self.body(&i.body);
                self.every(i.elif_else_clauses.iter().map(|c| &c.body[..])) && body
            }
            Stmt::Try(t) => self.tried(t),
            Stmt::Match(m) => self.every(m.cases.iter().map(|c| &c.body[..])),
            Stmt::For(_) | Stmt::While(_) => self.looped(stmt),
            _ => {
                self.record_jump(stmt);
                false
            }
        }
    }

    /// A `try` fills when its body or `else` does and so does each handler, or when its `finally`
    /// does. The `finally` still runs after a path that ends early, so its jumps count then, and
    /// one that fills runs before a `break` in the `try` leaves.
    fn tried(&mut self, t: &ast::StmtTry) -> bool {
        let outer = (std::mem::take(&mut self.skips), std::mem::take(&mut self.ends));
        let unfilled_break = std::mem::take(&mut self.unfilled_break);
        let main = self.body(&t.body) || self.body(&t.orelse);
        let handlers = self.every(t.handlers.iter().map(|h| match h {
            ast::ExceptHandler::ExceptHandler(h) => &h.body[..],
        }));
        let filled = main && (handlers || t.handlers.is_empty());
        let early = self.skips || self.ends;
        self.skips |= outer.0;
        self.ends |= outer.1;
        let inner = std::mem::replace(&mut self.unfilled_break, unfilled_break);
        let finally = !(filled && !early) && self.body(&t.finalbody);
        self.unfilled_break |= inner && !finally;
        filled || finally
    }

    /// A loop that is not `endless` may run no iteration, so it never fills, and only such a loop
    /// runs its `else`. An endless one fills when no `break` of its own comes before the first
    /// statement of its body that fills, walked as `Paths::endless` walks it: a `continue` there
    /// restarts the body, which then fills first, and one with no such `break` either ends past a
    /// fill or is left by a `return` or an exception alone, so unless its body fills, a path ends
    /// there as at a `raise`.
    fn looped(&mut self, stmt: &Stmt) -> bool {
        let Some((body, orelse, endless)) = loop_parts(stmt) else {
            return false;
        };
        let saved = (
            self.outer,
            self.in_loop,
            std::mem::take(&mut self.skips),
            std::mem::take(&mut self.breaks),
            std::mem::take(&mut self.unfilled_break),
        );
        self.outer = false;
        self.in_loop = !endless;
        let filled = self.body(body);
        let breaks = self.breaks;
        (self.outer, self.in_loop, self.skips, self.breaks, self.unfilled_break) = saved;
        if !endless {
            self.body(orelse);
            return false;
        }
        self.ends |= !filled && !breaks_own(body);
        !breaks
    }

    /// Whether there is a body and each fills, walking every one.
    fn every<'b>(&mut self, bodies: impl Iterator<Item = &'b [Stmt]>) -> bool {
        bodies.fold(None, |all, body| Some(self.body(body) && all.unwrap_or(true))) == Some(true)
    }

    fn record_jump(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Return(_) => self.leaves = true,
            Stmt::Break(_) if !self.in_loop => {
                self.leaves |= self.outer;
                self.skips = true;
                self.breaks = true;
                self.unfilled_break = true;
            }
            Stmt::Continue(_) if !self.in_loop && self.outer => self.ends = true,
            Stmt::Continue(_) if !self.in_loop => self.skips = true,
            _ => {}
        }
    }
}

/// Whether a `with` runs its body under `contextlib.suppress`, which swallows the exceptions it
/// names.
pub(crate) fn suppresses(w: &ast::StmtWith) -> bool {
    w.items.iter().any(|item| suppress_call(&item.context_expr).is_some())
}

/// The call when `expr` is `suppress(...)` or `contextlib.suppress(...)`.
pub(crate) fn suppress_call(expr: &Expr) -> Option<&ast::ExprCall> {
    let Expr::Call(c) = expr else { return None };
    let named = is_name(&c.func, "suppress") || matches!(&*c.func, Expr::Attribute(a) if a.attr.as_str() == "suppress");
    named.then_some(c)
}

/// Whether the body of a `with` under `contextlib.suppress` may raise what it swallows: by a call,
/// an attribute or a subscript, read or written (a field write may run `write()`), binary
/// arithmetic (`+=` too), an `assert`, a bare `raise`, or a `raise` of a class a `suppress`
/// argument names, outside the body of a `try` whose handlers catch every class named. False for
/// any other `with`. A write of the field `paths` walks with a constant its type takes is taken to
/// succeed.
fn may_swallow(w: &ast::StmtWith, paths: Paths<'_>) -> bool {
    let calls = w.items.iter().filter_map(|i| suppress_call(&i.context_expr));
    let classes: Vec<&Expr> = calls.flat_map(|c| c.arguments.args.iter()).collect();
    let mut points = RaisePoints {
        classes: &classes,
        paths,
        found: false,
    };
    if !classes.is_empty() {
        points.visit_body(&w.body);
    }
    points.found
}

struct RaisePoints<'c> {
    /// The arguments of the `suppress` calls, each a class or a tuple of them.
    classes: &'c [&'c Expr],
    /// The walk of the field whose write with a constant its type takes is taken to succeed.
    paths: Paths<'c>,
    found: bool,
}

impl RaisePoints<'_> {
    /// A `raise` of a class no argument names goes past the `with`. Building the exception is
    /// taken not to raise, so `raise UserError(_('x'))` is no call.
    fn raised(&self, exc: Option<&Expr>) -> bool {
        let Some(exc) = exc else { return true };
        let class = match exc {
            Expr::Call(c) => &*c.func,
            e => e,
        };
        let name = python::tail_name(class).unwrap_or_default();
        self.classes.iter().any(|arg| python::names_error(arg, name))
    }

    /// Whether `a` writes the field with a constant that `Paths::takes` finds it takes.
    fn safe_write(&self, a: &ast::StmtAssign) -> bool {
        let field = |t: &Expr| matches!(t, Expr::Attribute(_) | Expr::Subscript(_)) && self.paths.writes(t);
        matches!(&a.targets[..], [t] if field(t)) && self.paths.takes(&a.value)
    }

    /// Whether some handler of `t` catches each named class. An argument that is no class name may
    /// hold any class, and the empty name stands for it, which only a handler of every error takes.
    fn shields(&self, t: &ast::StmtTry) -> bool {
        let mut names = vec![];
        self.classes.iter().for_each(|arg| class_names(arg, &mut names));
        names
            .iter()
            .all(|name| (t.handlers.iter()).any(|ast::ExceptHandler::ExceptHandler(h)| python::catches(h, name)))
    }
}

impl<'a> Visitor<'a> for RaisePoints<'_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
            Stmt::Raise(r) => self.found |= self.raised(r.exc.as_deref()),
            Stmt::Assert(_) | Stmt::AugAssign(_) => self.found = true,
            Stmt::Assign(a) if self.safe_write(a) => {}
            Stmt::Try(t) if self.shields(t) => {
                (t.handlers.iter()).for_each(|ast::ExceptHandler::ExceptHandler(h)| self.visit_body(&h.body));
                self.visit_body(&t.orelse);
                self.visit_body(&t.finalbody);
            }
            _ => visitor::walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Call(_) | Expr::BinOp(_) | Expr::Attribute(_) | Expr::Subscript(_) => self.found = true,
            Expr::Lambda(_) => {}
            _ => visitor::walk_expr(self, expr),
        }
    }
}

/// The names of the classes an exception class or a tuple of them gives, the empty one for any other
/// expression.
fn class_names<'e>(expr: &'e Expr, names: &mut Vec<&'e str>) {
    match expr {
        Expr::Tuple(t) => t.elts.iter().for_each(|e| class_names(e, names)),
        e => names.push(python::tail_name(e).unwrap_or_default()),
    }
}

#[derive(Clone, Copy)]
struct Paths<'a> {
    record: &'a str,
    field: &'a str,
    /// In a loop nested in the loop over `self`, where `continue` only starts its next iteration.
    nested: bool,
    /// In a `try` whose `finally` may jump, which still runs after a `continue` or a `raise`.
    jumping_finally: bool,
    /// In a `while True:` body, with no `break` or `return` before a fill after the statement
    /// walked, so an exception that a `contextlib.suppress` there swallows starts the body again.
    restarts: bool,
    /// Set when a path through the body of the `while True:` loop walked, outside any loop within
    /// it, assigns the field and then leaves the loop by a `break` before any fill.
    assigned_break: Option<&'a Cell<bool>>,
    ftype: &'a str,
}

impl<'a> Paths<'a> {
    fn new(record: &'a str, field: &'a str, ftype: &'a str) -> Self {
        Self {
            record,
            field,
            nested: false,
            jumping_finally: false,
            restarts: false,
            assigned_break: None,
            ftype,
        }
    }

    /// The paths of a statement that `rest` follows in its block.
    fn at(&self, rest: &[Stmt]) -> Self {
        Self {
            restarts: self.restarts && !Exits::leaves_unfilled(rest, *self),
            ..*self
        }
    }

    /// No later statement can make a block that may `continue` a nested loop better than
    /// `Continued`.
    fn block(&self, stmts: &[Stmt]) -> Flow {
        match self.steps(stmts, &mut vec![]) {
            (flow, true) => flow.max(Flow::Continued),
            (flow, false) => flow,
        }
    }

    /// The first statement that assigns the field or leaves decides, but a later exit still leaves
    /// the loop over `self`: after the assignment, or in a nested loop after a `break` that only
    /// leaves that loop. The flow sums up the paths that do not `continue` a nested loop, and the
    /// block returns, beside its flow, whether such a statement occurred.
    fn steps<'s>(&self, stmts: &'s [Stmt], sides: &mut Vec<Side<'s>>) -> (Flow, bool) {
        #[cfg(test)]
        tests::BLOCK_WALKS.with(|walks| walks.set(walks.get() + 1));
        let mut continued = false;
        for (n, stmt) in stmts.iter().enumerate() {
            let (flow, continues) = self.at(&stmts[n + 1..]).stmt(stmt, sides);
            continued |= continues || flow == Flow::Continued;
            let flow = match flow {
                Flow::Open | Flow::Continued => continue,
                flow @ (Flow::Assigned | Flow::Escaped) => self.followed(stmt, &stmts[n + 1..], flow),
                flow => flow,
            };
            return (flow, continued);
        }
        (Flow::Open, continued)
    }

    /// In a nested loop, a `break` only leaves that loop.
    fn exits<'s>(&self, rest: impl IntoIterator<Item = &'s [Stmt]>, flow: Flow) -> Flow {
        let mut exits = Exits::new(self.nested, *self);
        rest.into_iter().for_each(|stmts| {
            exits.body(stmts);
        });
        if exits.leaves { Flow::Left } else { flow }
    }

    /// Nothing after a statement that never ends runs. Only a `return`, which the walk of its body
    /// already finds `Left`, or an exception, which Odoo reports, leaves it, so after an assignment
    /// it fills, unless a `finally` that may jump catches that exception; `within` reads a
    /// `contextlib.suppress` that may swallow it.
    fn followed(&self, stmt: &Stmt, rest: &[Stmt], flow: Flow) -> Flow {
        match flow {
            _ if !never_ends(stmt) => self.after(rest, flow),
            Flow::Assigned if !self.jumping_finally => Flow::Filled,
            flow => flow,
        }
    }

    /// How a statement ending as `flow` ends with `rest`, the statements after it in its block:
    /// `Left` when a path through `rest` leaves the loop over `self`, and after an assignment
    /// `Filled` when every path through `rest` fills before any jump. Under a `finally` that may
    /// jump, a path ending the iteration by a `continue` or a `raise` does not fill.
    fn after(&self, rest: &[Stmt], flow: Flow) -> Flow {
        let mut exits = Exits::new(self.nested, *self);
        let filled = exits.body(rest) && !exits.skips && !(self.jumping_finally && exits.ends);
        self.note_break(flow, &exits);
        match flow {
            _ if exits.leaves => Flow::Left,
            Flow::Assigned if filled => Flow::Filled,
            flow => flow,
        }
    }

    /// Sets `assigned_break` when the statements after one ending as `flow`, walked as `exits`,
    /// may `break` after an assignment and before any fill.
    fn note_break(&self, flow: Flow, exits: &Exits<'_>) {
        if let (Flow::Assigned, true, Some(cell)) = (flow, exits.unfilled_break, self.assigned_break) {
            cell.set(true);
        }
    }

    /// How one statement ends for the field, by its kind, as `steps` sums up a block: compound
    /// statements combine the blocks they hold, `raise` counts as assigning, and the others assign
    /// the field or go on. `sides` holds the statements before it in its block that assign on one
    /// side of a test.
    fn stmt<'s>(&self, stmt: &'s Stmt, sides: &mut Vec<Side<'s>>) -> (Flow, bool) {
        let flow = match stmt {
            Stmt::Continue(_) | Stmt::Break(_) | Stmt::Return(_) => return self.jump(stmt),
            Stmt::If(i) => return self.branches(i, sides),
            Stmt::Try(t) => return self.handled(t, sides),
            Stmt::With(w) => return self.within(w, sides),
            Stmt::Match(m) => return self.matched(m),
            Stmt::Raise(_) => Flow::Assigned,
            Stmt::For(_) | Stmt::While(_) => self.nested_loop(stmt, sides),
            _ => self.assigned(stmt),
        };
        (flow, false)
    }

    /// A `return` leaves the method from any depth, and in a nested loop a `continue` only starts
    /// its next iteration, so no path goes on past it.
    fn jump(&self, stmt: &Stmt) -> (Flow, bool) {
        match stmt {
            Stmt::Return(_) => (Flow::Left, false),
            Stmt::Continue(_) if self.nested => (Flow::Filled, true),
            _ => (Flow::Escaped, false),
        }
    }

    /// Under `contextlib.suppress`, an exception may skip the write of a side, so the body pairs
    /// only within itself, and a body that fills only does so when it raises nothing the `with`
    /// swallows, since that skips the rest of it, or when the swallowed exception `restarts` it.
    fn within<'s>(&self, w: &'s ast::StmtWith, sides: &mut Vec<Side<'s>>) -> (Flow, bool) {
        if !suppresses(w) {
            return self.steps(&w.body, sides);
        }
        match self.steps(&w.body, &mut vec![]) {
            (Flow::Filled, continued) if !self.restarts && may_swallow(w, *self) => (Flow::Assigned, continued),
            ends => ends,
        }
    }

    /// A `for` whose body assigns does so when its iterable is not empty, so it pairs as
    /// `if items:` would.
    fn nested_loop<'s>(&self, stmt: &'s Stmt, sides: &mut Vec<Side<'s>>) -> Flow {
        let Some((body, orelse, endless)) = loop_parts(stmt) else {
            return Flow::Open;
        };
        let inner = Paths {
            nested: true,
            restarts: endless,
            assigned_break: None,
            ..*self
        };
        let looped = if endless {
            inner.endless(body)
        } else {
            inner.block(body)
        };
        let flow = self.after_loop(looped, orelse, endless);
        match stmt {
            Stmt::For(f) if flow == Flow::Open && looped <= Flow::Assigned => pair(
                Side {
                    test: &f.iter,
                    holds: true,
                    flow: looped,
                },
                sides,
            ),
            _ => flow,
        }
    }

    /// How the body of a `while True:` loop ends, which only a `break`, a `return` or an exception
    /// leaves. When a `break` may come before the first statement that fills, `broken` decides.
    /// Otherwise each `break` follows a fill, and falling through or a `continue` restarts the
    /// body, so it fills, unless a `finally` that may jump catches a path that ends first. A body
    /// with no `break` never ends.
    fn endless(&self, body: &[Stmt]) -> Flow {
        let mut exits = Exits::new(true, *self);
        exits.body(body);
        if exits.breaks {
            return self.broken(body);
        }
        match self.steps(body, &mut vec![]).0 {
            Flow::Left => Flow::Left,
            _ if breaks_own(body) && !(self.jumping_finally && exits.ends) => Flow::Filled,
            flow => flow,
        }
    }

    /// How the body of a `while True:` loop that may `break` before it fills ends. `block` joins a
    /// path that assigns the field and then breaks, with no fill between and no `finally` that
    /// fills on the way out, into the path that goes on, which a later fill may complete. When
    /// `assigned_break` finds such a path, the body assigns at best.
    fn broken(&self, body: &[Stmt]) -> Flow {
        let assigned_break = Cell::new(false);
        let walk = Paths {
            assigned_break: Some(&assigned_break),
            ..*self
        };
        let flow = walk.block(body);
        if assigned_break.get() {
            flow.max(Flow::Assigned)
        } else {
            flow
        }
    }

    /// A `return` leaves the method from any depth, and a `break` that does not assign goes on
    /// after the loop. Otherwise a loop that is not endless may run no iteration, so its `else`
    /// decides, while an endless one runs no `else` and its body, as `endless` reads it, assigns at
    /// worst, since a path that goes on starts it again.
    fn after_loop(&self, looped: Flow, orelse: &[Stmt], endless: bool) -> Flow {
        match looped {
            Flow::Left => Flow::Left,
            Flow::Escaped if endless => Flow::Open,
            Flow::Escaped => join([self.block(orelse), Flow::Open]),
            flow if endless => flow.min(Flow::Assigned),
            _ => self.block(orelse),
        }
    }

    /// A chain with an `elif` but no `else` is taken to cover every case, as a `match` is. An
    /// open `if` whose body or lone `else` assigns pairs with one of `sides` to cover every path.
    fn branches<'s>(&self, i: &'s ast::StmtIf, sides: &mut Vec<Side<'s>>) -> (Flow, bool) {
        let bodies = std::iter::once(&i.body[..]).chain(i.elif_else_clauses.iter().map(|c| &c.body[..]));
        let (flows, continued) = self.arms(bodies.zip(guards(i, sides)));
        let fallthrough = i.elif_else_clauses.is_empty().then_some(Flow::Open);
        let flow = join(flows.iter().copied().chain(fallthrough));
        match side(i, flows[0], &flows[1..]) {
            Some(side) if flow == Flow::Open => (pair(side, sides), continued),
            _ => (flow, continued),
        }
    }

    /// A `match` is taken to cover every case, and a case that may `continue` a nested loop leaves
    /// the others to decide, as an `if` branch does.
    fn matched(&self, m: &ast::StmtMatch) -> (Flow, bool) {
        let (flows, continued) = self.arms(m.cases.iter().map(|c| (&c.body[..], vec![])));
        (join(flows), continued)
    }

    /// How each body ends, walked with the sides it starts with, and whether one may `continue` a
    /// nested loop.
    fn arms<'s>(&self, bodies: impl Iterator<Item = (&'s [Stmt], Vec<Side<'s>>)>) -> (Vec<Flow>, bool) {
        let mut continued = false;
        let flows = bodies
            .map(|(body, mut seed)| {
                let (flow, continues) = self.steps(body, &mut seed);
                continued |= continues;
                flow
            })
            .collect();
        (flows, continued)
    }

    /// The body then its `else`, or a handler, runs before the `finally`. A body that first writes
    /// the field a constant its type takes enters its handlers with it assigned. The body and
    /// `else` pair with `sides` only when no handler goes on unassigned, since an exception may
    /// skip the write of a side. A `continue` of a nested loop in the body skips the `else`, and
    /// is still one after a `finally` that goes on.
    fn handled<'s>(&self, t: &'s ast::StmtTry, sides: &mut Vec<Side<'s>>) -> (Flow, bool) {
        let finally = self.block(&t.finalbody);
        let tried = self.in_try(t, finally);
        let handlers = tried.excepts(t);
        let mut own = vec![];
        let sides = if handlers.iter().all(|&(h, continues)| h <= Flow::Assigned && !continues) {
            sides
        } else {
            &mut own
        };
        let (body, mut continued) = tried.steps(&t.body, sides);
        let main = match body {
            Flow::Open => {
                let (flow, continues) = tried.steps(&t.orelse, sides);
                continued |= continues;
                flow
            }
            Flow::Assigned => tried.exits([&t.orelse[..]], Flow::Assigned),
            flow => flow,
        };
        continued |= handlers.iter().any(|&(_, continues)| continues);
        let flow = join(std::iter::once(main).chain(handlers.iter().map(|&(h, _)| h)));
        match finally {
            Flow::Open => (flow, continued),
            _ if continued => (self.finally(t, flow.max(Flow::Continued), finally), false),
            _ => (self.finally(t, flow, finally), false),
        }
    }

    /// The paths to walk the body, `else` and handlers of `t` with, given how its `finally` ends.
    /// The `else` and `finally` run after the body, so they too must not leave before a fill for
    /// an exception swallowed there to start the loop again. A `finally` that fills runs before a
    /// `break` there leaves, so no such `break` is an `assigned_break`.
    fn in_try(&self, t: &ast::StmtTry, finally: Flow) -> Self {
        Self {
            jumping_finally: self.jumping_finally || finally > Flow::Open,
            restarts: self.restarts
                && !Exits::leaves_unfilled(&t.orelse, *self)
                && !Exits::leaves_unfilled(&t.finalbody, *self),
            assigned_break: self.assigned_break.filter(|_| finally != Flow::Filled),
            ..*self
        }
    }

    /// How each handler of `t` ends, as `steps` sums it up.
    fn excepts(&self, t: &ast::StmtTry) -> Vec<(Flow, bool)> {
        let first = self.writes_first(&t.body);
        (t.handlers.iter())
            .map(|h| match h {
                ast::ExceptHandler::ExceptHandler(h) if first => (self.exits([&h.body[..]], Flow::Assigned), false),
                ast::ExceptHandler::ExceptHandler(h) => self.steps(&h.body, &mut vec![]),
            })
            .collect()
    }

    /// Whether statements write the field, after nothing but `pass` and constants bound to names,
    /// with a constant its type takes. A chained or unpacking target may raise before it writes
    /// the field.
    fn writes_first(&self, stmts: &[Stmt]) -> bool {
        let mut rest = stmts.iter().skip_while(|s| match s {
            Stmt::Pass(_) => true,
            Stmt::Assign(a) => is_constant(&a.value) && a.targets.iter().all(Expr::is_name_expr),
            _ => false,
        });
        matches!(rest.next(), Some(Stmt::Assign(a))
            if matches!(&a.targets[..], [t @ Expr::Attribute(_)] if self.writes(t)) && self.takes(&a.value))
    }

    /// Whether the field takes a constant without raising. Odoo converts the value before it
    /// stores it: a `Selection` raises `ValueError` on a value outside its list, and a `One2many`
    /// or `Many2many` on any falsy value but `False` and `None`. A field whose class name has two
    /// types takes any real number, as the scan does not judge its type.
    fn takes(&self, value: &Expr) -> bool {
        let numbers = matches!(self.ftype, "Integer" | "Float" | "Monetary" | "Many2one" | UNREAD_TYPE);
        let real = |e: &Expr| matches!(e, Expr::NumberLiteral(n) if !matches!(n.value, ast::Number::Complex { .. }));
        match value {
            Expr::NoneLiteral(_) | Expr::BooleanLiteral(ast::ExprBooleanLiteral { value: false, .. }) => true,
            Expr::UnaryOp(u) if u.op == ast::UnaryOp::USub && numbers => real(&u.operand),
            value if numbers && real(value) => true,
            value => {
                !matches!(self.ftype, "One2many" | "Many2many")
                    && is_constant(value)
                    && Truthiness::from_expr(value, |_| false).into_bool() == Some(false)
            }
        }
    }

    /// A `finally` ending as `finally` runs after every path `tried` sums up, so an exit before a
    /// `finally` that assigns, or one in a `finally` after paths that assign, leaves the loop over
    /// `self` after the assignment, unless every path tried calls a method of `self` first.
    fn finally(&self, t: &ast::StmtTry, tried: Flow, finally: Flow) -> Flow {
        let handlers = t.handlers.iter().map(|h| match h {
            ast::ExceptHandler::ExceptHandler(h) => &h.body[..],
        });
        match finally {
            Flow::Open => tried,
            _ if tried == Flow::Filled => tried,
            Flow::Filled | Flow::Left => finally,
            Flow::Assigned => self.exits([&t.body[..], &t.orelse[..]].into_iter().chain(handlers), finally),
            _ if tried == Flow::Assigned => self.exits([&t.finalbody[..]], tried),
            _ => tried.max(finally),
        }
    }

    /// A method of `self` may assign the field on every record, and a method of the record, or a
    /// call given it, on that record.
    fn called(&self, expr: &Expr) -> Flow {
        match expr {
            Expr::Call(c) if calls_self_method(c) => Flow::Filled,
            Expr::Call(c) if may_assign(c, self.record) => Flow::Assigned,
            _ => Flow::Open,
        }
    }

    fn assigned(&self, stmt: &Stmt) -> Flow {
        match stmt {
            Stmt::Assign(a) if a.targets.iter().any(|t| self.writes(t)) => Flow::Assigned,
            Stmt::AnnAssign(a) if a.value.is_some() && self.writes(&a.target) => Flow::Assigned,
            Stmt::Expr(e) => self.called(&e.value),
            _ => Flow::Open,
        }
    }

    fn writes(&self, target: &Expr) -> bool {
        match target {
            Expr::Attribute(a) => a.attr.as_str() == self.field && is_name(&a.value, self.record),
            Expr::Subscript(s) => is_str(&s.slice, self.field) && is_name(&s.value, self.record),
            Expr::Tuple(t) => t.elts.iter().any(|e| self.writes(e)),
            Expr::List(l) => l.elts.iter().any(|e| self.writes(e)),
            Expr::Starred(s) => self.writes(&s.value),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use ruff_python_parser::parse_module;

    use super::*;

    thread_local! {
        /// How many blocks `Paths` walked on this thread.
        pub(super) static BLOCK_WALKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    /// The body of the one function `src` defines.
    fn with_body<T>(src: &str, check: impl FnOnce(&[Stmt]) -> T) -> T {
        let parsed = parse_module(src).expect("valid Python");
        let Stmt::FunctionDef(func) = &parsed.syntax().body[0] else {
            panic!("not a function")
        };
        check(&func.body)
    }

    fn returns(src: &str) -> bool {
        with_body(src, returns_value)
    }

    fn dict_read(src: &str) -> bool {
        with_body(src, |body| reads_as_dict(body, "vals"))
    }

    fn unassigned(body: &str) -> bool {
        unassigned_as(body, "Char")
    }

    fn unassigned_as(body: &str, ftype: &str) -> bool {
        let src = format!("def _compute_f(self):\n{}", indent(body));
        with_body(&src, |body| leaves_unassigned(body, "f", ftype))
    }

    fn indent(body: &str) -> String {
        body.lines().map(|l| format!("    {l}\n")).collect()
    }

    #[test]
    fn a_returned_key_is_one_of_a_literal_dict_the_method_itself_returns() {
        let key = |src: &str| with_body(src, |body| returned_key(body, "domain").is_some());
        assert!(key(
            "def f(self):\n    if self.a:\n        return {'domain': {}}\n    return {}\n"
        ));
        assert!(!key(
            "def f(self):\n    return {'warning': {}, 'value': {'domain': 1}}\n"
        ));
        assert!(!key("def f(self):\n    res = {'domain': {}}\n    return res\n"));
        assert!(!key(
            "def f(self):\n    def g():\n        return {'domain': {}}\n    return g\n"
        ));
    }

    #[test]
    fn a_return_gives_a_value_unless_it_is_bare_none_or_nested() {
        assert!(returns(
            "def write(self, vals):\n    res = super().write(vals)\n    return res\n"
        ));
        assert!(returns("def write(self, vals):\n    if vals:\n        return True\n"));
        assert!(!returns(
            "def write(self, vals):\n    super().write(vals)\n    return\n"
        ));
        assert!(!returns(
            "def write(self, vals):\n    super().write(vals)\n    return None\n"
        ));
        assert!(!returns(
            "def write(self, vals):\n    def inner():\n        return 1\n    super().write(vals)\n"
        ));
        assert!(!returns(
            "def write(self, vals):\n    f = lambda: 1\n    super().write(vals)\n"
        ));
    }

    #[test]
    fn reads_the_argument_as_a_dict() {
        for read in [
            "vals['name']",
            "vals['name'] = 1",
            "vals.get('name')",
            "vals.items()",
            "vals.setdefault('name', 1)",
            "vals.update(name=1)",
            "vals.pop('name')",
            "'name' in vals",
            "'name' not in vals",
            "dict(**vals)",
            "{**vals}",
        ] {
            assert!(dict_read(&format!("def create(self, vals):\n    {read}\n")), "{read}");
        }
    }

    #[test]
    fn a_list_read_or_either_shape_is_no_dict_read() {
        for read in [
            "vals[0]",
            "vals.pop()",
            "vals.append({})",
            "for v in vals:\n        v.get('name')",
            "len(vals)",
            "vals = [vals]\n    vals[0].get('name')",
            "if isinstance(vals, dict):\n        vals.get('name')",
            "def inner(vals):\n        return vals.get('name')",
            "f = lambda vals: vals.get('name')",
        ] {
            assert!(!dict_read(&format!("def create(self, vals):\n    {read}\n")), "{read}");
        }
    }

    #[test]
    fn names_free_names_of_a_cache_key() {
        let params = ["self", "lang_code", "args"];
        let unknown = |key: &str| unknown_key_names(key, &params);
        assert_eq!(unknown("lang"), Some(vec!["lang".to_owned()]));
        assert_eq!(unknown("lang, lang"), Some(vec!["lang".to_owned()]));
        assert_eq!(unknown("self.env.uid"), Some(vec![]));
        assert_eq!(unknown("frozenset(args)"), Some(vec![]));
        assert_eq!(unknown("tuple(c for c in lang_code)"), Some(vec![]));
        assert_eq!(unknown("(lambda x: x)(lang_code)"), Some(vec![]));
        assert_eq!(
            unknown("tuple(c for c in lang_codes)"),
            Some(vec!["lang_codes".to_owned()])
        );
        assert_eq!(unknown("self.env.context.get('lang'"), None);
    }

    #[test]
    fn reports_a_path_that_skips_the_field() {
        assert!(unassigned("for rec in self:\n    if rec.a:\n        rec.f = 1\n"));
        assert!(unassigned(
            "for rec in self:\n    if not rec.a:\n        continue\n    rec.f = 1\n"
        ));
        assert!(unassigned(
            "for rec in self:\n    if rec.a:\n        rec.f = 1\n    else:\n        break\n"
        ));
        assert!(unassigned("for rec in self:\n    rec.g = 1\n"));
        assert!(unassigned(
            "for rec in self:\n    try:\n        rec.f = int(rec.a)\n    except ValueError:\n        pass\n"
        ));
        assert!(unassigned(
            "for rec in self:\n    for line in rec.line_ids:\n        rec.f = line.a\n        break\n"
        ));
        assert!(unassigned("for rec in self:\n    with rec.lock():\n        pass\n"));
        assert!(unassigned(
            "logger.info('x')\nfor rec in self:\n    if rec.a:\n        rec.f = 1\n"
        ));
        assert!(unassigned(
            "records = self.filtered('a')\nfor rec in self:\n    if rec.a:\n        rec.f = 1\n"
        ));
    }

    #[test]
    fn a_nested_loop_counts_its_else_unless_a_break_skips_it() {
        let reported = [
            "for line in rec.line_ids:\n    break\nelse:\n    rec.f = 1",
            "for line in rec.line_ids:\n    if line.a:\n        break\nelse:\n    rec.f = 1",
            "while rec.a:\n    rec.f = 1\n    break",
            "while True:\n    if rec.a:\n        break\n    rec.f = 1\n    break",
            "for line in rec.line_ids:\n    try:\n        if not line.a:\n            continue\n        v = int(line.a)\n    except ValueError:\n        raise\n    else:\n        break\nelse:\n    rec.f = 1",
            "for line in rec.line_ids:\n    match line.a:\n        case 0:\n            break\n        case _:\n            pass\nelse:\n    rec.f = 1",
            "for line in rec.line_ids:\n    match line.a:\n        case 0:\n            continue\n        case _:\n            pass\n    break\nelse:\n    rec.f = 1",
        ];
        for stmts in reported {
            assert!(unassigned(&format!("for rec in self:\n{}", indent(stmts))), "{stmts}");
        }
        let assigned = [
            "for line in rec.line_ids:\n    if line.a:\n        rec.f = 1\n        break\nelse:\n    rec.f = 2",
            "for line in rec.line_ids:\n    if not line.a:\n        continue\n    rec.f = 1\n    break\nelse:\n    rec.f = 2",
            "while True:\n    rec.f = 1\n    break",
            "while 1:\n    if rec.a:\n        continue\n    rec.f = 1\n    break",
            "for line in rec.line_ids:\n    try:\n        if not line.a:\n            continue\n        rec.f = int(line.a)\n    except ValueError:\n        raise\n    break\nelse:\n    rec.f = False",
            "for line in rec.line_ids:\n    with rec.lock():\n        if not line.a:\n            continue\n        rec.f = int(line.a)\n    break\nelse:\n    rec.f = False",
            "for line in rec.line_ids:\n    if not line.a:\n        continue\n    else:\n        rec.f = line.a\n    break\nelse:\n    rec.f = False",
            "for line in rec.line_ids:\n    match line.a:\n        case 0:\n            continue\n        case _:\n            rec.f = line.a\n    break\nelse:\n    rec.f = False",
            "for line in rec.line_ids:\n    try:\n        rec.f = int(line.a)\n    except ValueError:\n        continue\n    break\nelse:\n    rec.f = False",
            "for line in rec.line_ids:\n    match line.a:\n        case 0:\n            continue\n        case 1:\n            rec.f = line.a\n    break\nelse:\n    rec.f = False",
            "for line in rec.line_ids:\n    match line.a:\n        case 0:\n            continue\n        case n if n > 1:\n            rec.f = n\n    break\nelse:\n    rec.f = False",
        ];
        for stmts in assigned {
            assert!(!unassigned(&format!("for rec in self:\n{}", indent(stmts))), "{stmts}");
        }
    }

    #[test]
    fn every_path_assigning_or_raising_is_assigned() {
        let assigned = [
            "rec.f = 1",
            "rec['f'] = 1",
            "rec.f, rec.g = 1, 2",
            "rec.update({'f': 1})",
            "rec.write({'f': 1})",
            "rec._fill()",
            "self._fill(rec)",
            "fill(record=rec)",
            "raise ValueError()",
            "if rec.a:\n    rec.f = 1\nelse:\n    rec.f = 2",
            "if rec.a:\n    rec.f = 1\nelif rec.b:\n    rec.f = 2",
            "match rec.a:\n    case 1:\n        rec.f = 1\n    case _:\n        rec.f = 2",
            "match rec.a:\n    case 1:\n        rec.f = 1\n    case 2:\n        rec.f = 2",
            "match rec.a:\n    case 1:\n        rec.f = 1",
            "try:\n    v = int(rec.a)\nexcept ValueError:\n    rec.f = 0\nelse:\n    rec.f = v",
            "try:\n    pass\nfinally:\n    rec.f = 1",
            "with rec.lock():\n    rec.f = 1",
            "if rec.a:\n    rec.f = 1\nrec.f = 2",
            "if rec.a:\n    rec.f = 1\nif not rec.a:\n    rec.f = 2",
            "if rec.t == 'a':\n    rec.f = 1\nif rec.t != 'a':\n    rec.f = 2",
            "rec.f = 1\ncontinue",
        ];
        for stmts in assigned {
            assert!(!unassigned(&format!("for rec in self:\n{}", indent(stmts))), "{stmts}");
        }
    }

    #[test]
    fn two_ifs_assign_on_every_path_only_with_complementary_tests() {
        let reported = [
            "if rec.t == 'a':\n    rec.f = 1\nif rec.u != 'a':\n    rec.f = 2",
            "if rec.t == 'a':\n    rec.f = 1\nif rec.t == 'a':\n    rec.f = 2",
            "if rec.t == 'a':\n    rec.f = 1\nif rec.t != 'b':\n    rec.f = 2",
            "if rec.t == 'a':\n    rec.f = 1\nif rec.t < 'a':\n    rec.f = 2",
            "if rec.a:\n    if rec.b:\n        rec.f = 1\nif not rec.a:\n    rec.f = 2",
            "if rec.t < 'a':\n    rec.f = 1\nif 'a' >= rec.t:\n    rec.f = 2",
            "if rec.a:\n    pass\nelse:\n    rec.f = 1\nif not rec.a:\n    rec.f = 2",
            "if rec.a:\n    pass\nelse:\n    rec.f = 1\nif rec.a:\n    pass\nelse:\n    rec.f = 2",
            "if rec.a:\n    pass\nelse:\n    rec.f = 1\nif rec.b:\n    rec.f = 2",
            "try:\n    if rec.a:\n        rec.f = int(rec.b)\nexcept ValueError:\n    pass\nif not rec.a:\n    rec.f = 2",
            "with contextlib.suppress(ValueError):\n    if rec.a:\n        rec.f = int(rec.b)\nif not rec.a:\n    rec.f = 2",
            "with suppress(ValueError):\n    if rec.a:\n        rec.f = int(rec.b)\nif not rec.a:\n    rec.f = 2",
            "if rec.a:\n    rec.f = 1\nif rec.b:\n    if rec.a:\n        rec.f = 2\nelse:\n    rec.f = 3",
        ];
        for stmts in reported {
            assert!(unassigned(&format!("for rec in self:\n{}", indent(stmts))), "{stmts}");
        }
        let assigned = [
            "if not rec.a:\n    rec.f = 1\nif rec.a:\n    rec.f = 2",
            "if rec.p is None:\n    rec.f = 1\nif rec.p is not None:\n    rec.f = 2",
            "if rec.a:\n    rec.f = 1\nv = 1\nif not rec.a:\n    rec.f = v",
            "if rec.a:\n    pass\nif not rec.a:\n    rec.f = 1\nif rec.a:\n    rec.f = 2",
            "if rec.a:\n    rec.f = 1\nelse:\n    pass\nif not rec.a:\n    rec.f = 2",
            "if rec.t == 'a':\n    rec.f = 1\nif 'a' != rec.t:\n    rec.f = 2",
            "if rec.p is None:\n    rec.f = 1\nif None is not rec.p:\n    rec.f = 2",
            "if rec.a:\n    pass\nelse:\n    rec.f = 1\nif rec.a:\n    rec.f = 2",
            "if rec.a:\n    rec.f = 1\nif rec.a:\n    pass\nelse:\n    rec.f = 2",
            "if rec.t == 'a':\n    pass\nelse:\n    rec.f = 1\nif 'a' == rec.t:\n    rec.f = 2",
            "if not rec.a:\n    v = 1\nelse:\n    v = 2\nif rec.a:\n    rec.f = v\nif not rec.a:\n    rec.f = 2",
            "with rec.lock():\n    if rec.a:\n        rec.f = 1\nif not rec.a:\n    rec.f = 2",
            "if rec.a:\n    rec.f = 1\nwith rec.lock():\n    if not rec.a:\n        rec.f = 2",
            "try:\n    if rec.a:\n        rec.f = int(rec.b)\nexcept ValueError:\n    raise\nif not rec.a:\n    rec.f = 2",
            "try:\n    v = int(rec.b)\nexcept ValueError:\n    raise\nelse:\n    if rec.a:\n        rec.f = v\nif not rec.a:\n    rec.f = 2",
            "if rec.a:\n    rec.f = 1\nif rec.b:\n    if not rec.a:\n        rec.f = 2\nelse:\n    rec.f = 3",
        ];
        for stmts in assigned {
            assert!(!unassigned(&format!("for rec in self:\n{}", indent(stmts))), "{stmts}");
        }
    }

    #[test]
    fn a_loop_and_its_empty_case_assign_on_every_path() {
        let loops = "for line in rec.line_ids:\n    rec.f = line.a";
        let reported = [
            format!("{loops}\nif not rec.other_ids:\n    rec.f = False"),
            format!("{loops}\nif rec.line_ids:\n    rec.f = False"),
            "for line in rec.line_ids:\n    if line.a:\n        rec.f = 1\nif not rec.line_ids:\n    rec.f = False"
                .to_owned(),
            "for line in rec.line_ids:\n    if not line.a:\n        continue\n    rec.f = line.a\nif not rec.line_ids:\n    rec.f = False"
                .to_owned(),
            format!("if rec.other_ids:\n{}else:\n    rec.f = False", indent(loops)),
        ];
        for stmts in reported {
            assert!(unassigned(&format!("for rec in self:\n{}", indent(&stmts))), "{stmts}");
        }
        let assigned = [
            format!("{loops}\nif not rec.line_ids:\n    rec.f = False"),
            format!("if not rec.line_ids:\n    rec.f = False\n{loops}"),
            format!("{loops}\nif rec.line_ids:\n    pass\nelse:\n    rec.f = False"),
            "for line in rec.line_ids:\n    pass\nif rec.line_ids:\n    rec.f = 1\nif not rec.line_ids:\n    rec.f = 2"
                .to_owned(),
            format!("if rec.line_ids:\n{}else:\n    rec.f = False", indent(loops)),
            format!("if not rec.line_ids:\n    rec.f = False\nelse:\n{}", indent(loops)),
            "for line in rec.line_ids:\n    if not line.a:\n        continue\n    rec.f = line.a\n    break\nelse:\n    rec.f = False"
                .to_owned(),
        ];
        for stmts in assigned {
            assert!(!unassigned(&format!("for rec in self:\n{}", indent(&stmts))), "{stmts}");
        }
    }

    #[test]
    fn an_elif_starts_with_the_earlier_tests_false() {
        let loops = indent("for line in rec.line_ids:\n    rec.f = line.a");
        let twice = indent(&loops);
        let split = format!("    if rec.b:\n{twice}    else:\n{twice}");
        let deeper = format!("    if rec.line_ids:\n{}", indent(&split));
        let assigned = [
            format!("if not rec.line_ids:\n    rec.f = False\nelif rec.a:\n    if rec.line_ids:\n{twice}"),
            format!("if not rec.line_ids:\n    rec.f = False\nelif rec.a:\n{split}"),
            format!("if not rec.line_ids:\n    rec.f = False\nelif rec.a:\n{deeper}"),
            format!("if rec.line_ids:\n{split}else:\n    rec.f = False"),
            format!("if not rec.line_ids:\n    rec.f = False\nelif rec.a:\n{loops}"),
            format!("if not rec.line_ids:\n    rec.f = False\nelif rec.a:\n    rec.f = 1\nelse:\n{loops}"),
            format!("if not rec.line_ids:\n    rec.f = False\nelif rec.a:\n    rec.f = 1\nelif rec.b:\n{loops}"),
            format!("if rec.a:\n    rec.f = 1\nelif rec.line_ids:\n{loops}"),
            "if rec.t == 'x':\n    rec.f = 1\nelif rec.a:\n    if rec.t != 'x':\n        rec.f = 2".to_owned(),
        ];
        for stmts in assigned {
            assert!(!unassigned(&format!("for rec in self:\n{}", indent(&stmts))), "{stmts}");
        }
        let reported = [
            format!("if rec.b:\n    rec.f = False\nelif rec.a:\n{loops}"),
            format!("if rec.line_ids:\n    rec.f = False\nelif rec.a:\n{loops}"),
            format!("if not rec.other_ids:\n    rec.f = False\nelif rec.a:\n    if rec.line_ids:\n{twice}"),
            format!("if not rec.other_ids:\n    rec.f = False\nelif rec.a:\n{split}"),
            format!("if not rec.other_ids:\n    rec.f = False\nelif rec.a:\n{deeper}"),
            format!("if rec.other_ids:\n{split}else:\n    rec.f = False"),
        ];
        for stmts in reported {
            assert!(unassigned(&format!("for rec in self:\n{}", indent(&stmts))), "{stmts}");
        }
    }

    #[test]
    fn leaving_the_loop_over_self_leaves_later_records_unassigned() {
        let reported = [
            "rec.f = 1\nif rec.a:\n    break",
            "rec.f = 1\nreturn",
            "if rec.a:\n    rec.f = 1\nif not rec.a:\n    rec.f = 2\nbreak",
            "for line in rec.line_ids:\n    rec.f = 1\n    if line.a:\n        return",
            "for line in rec.line_ids:\n    if line.a:\n        return\nrec.f = 1",
            "rec.f = 1\nfor line in rec.line_ids:\n    pass\nelse:\n    break",
            "for line in rec.line_ids:\n    pass\nelse:\n    rec.f = 1\n    break",
            "try:\n    rec.f = 1\nfinally:\n    return",
            "try:\n    rec.f = 1\nfinally:\n    break",
            "try:\n    rec.f = 1\nexcept ValueError:\n    raise\nelse:\n    return",
            "try:\n    return\nfinally:\n    rec.f = 1",
            "try:\n    rec.f = int(rec.a)\nexcept ValueError:\n    return\nfinally:\n    rec.f = 0",
            "rec._fill_f()\nreturn",
            "rec.f = 1\nif rec.a:\n    self._fill_f()\nreturn",
            "for line in rec.line_ids:\n    if line.a:\n        break\n    return\nrec.f = 1",
            "rec.f = 1\nwith rec.lock():\n    if rec.a:\n        return\n    self._fill_f()\nreturn",
            "while True:\n    rec.f = 1\n    if rec.a:\n        break\n    self._fill_f()\n    break\nreturn",
            "for line in rec.line_ids:\n    rec.f = 1\n    if line.a:\n        continue\n    self._fill_f()\nif not rec.line_ids:\n    self._fill_f()\nreturn",
            "if rec.a:\n    rec.f = 1\n    self._fill_f()\nreturn",
            "rec.f = 1\ntry:\n    self._fill_f()\nexcept ValueError:\n    pass\nreturn",
            "for line in rec.line_ids:\n    try:\n        if not line.a:\n            continue\n        v = int(line.a)\n    except ValueError:\n        raise\n    else:\n        return\nrec.f = 1",
            "for line in rec.line_ids:\n    try:\n        if not line.a:\n            continue\n        rec.f = 1\n    except ValueError:\n        raise\n    else:\n        return\nelse:\n    rec.f = 2",
            "try:\n    rec.f = 1\n    if rec.c:\n        continue\n    self._fill_f()\nfinally:\n    return",
            "try:\n    rec.f = 1\n    if rec.c:\n        raise ValueError()\n    self._fill_f()\nfinally:\n    return",
            "try:\n    try:\n        rec.f = 1\n        if rec.c:\n            continue\n        self._fill_f()\n    finally:\n        pass\nfinally:\n    return",
            "rec.f = 1\ntry:\n    if rec.c:\n        continue\n    self._fill_f()\nfinally:\n    return",
            "rec.f = 1\ntry:\n    if rec.c:\n        raise ValueError()\n    self._fill_f()\nfinally:\n    return",
            "rec.f = 1\ntry:\n    if rec.c:\n        continue\n    self._fill_f()\nexcept ValueError:\n    raise\nfinally:\n    return",
            "rec.f = 1\nwith contextlib.suppress(ValueError):\n    self._fill_f()\nreturn",
            "while True:\n    if rec.c:\n        continue\n    if rec.d:\n        break\n    self._fill_f()\n    break\nrec.f = 1\nreturn",
            "while True:\n    if rec.c:\n        continue\n    if rec.d:\n        rec.f = 1\n        break\n    self._fill_f()\n    break\nrec.f = 1\nreturn",
            "while True:\n    if rec.c:\n        continue\n    with rec.lock():\n        if rec.d:\n            rec.f = 1\n            break\n        self._fill_f()\n    break\nrec.f = 1\nreturn",
            "while rec.a:\n    if rec.c:\n        continue\n    self._fill_f()\n    break\nrec.f = 1\nreturn",
            "rec.f = 1\nwhile True:\n    if rec.d:\n        break\n    self._fill_f()\n    break\nreturn",
            "rec.f = 1\nwhile True:\n    if rec.d:\n        return\n    self._fill_f()\n    break",
            "rec.f = 1\nwhile True:\n    try:\n        self._fill_f()\n    except ValueError:\n        break\n    break\nreturn",
            "while True:\n    try:\n        self._fill_f()\n    except ValueError:\n        break\n    break\nreturn",
            "rec.f = 1\nwhile True:\n    if rec.c:\n        continue\n    else:\n        self._fill_f()\n    break\nreturn",
            "while True:\n    if rec.c == 1:\n        continue\n    else:\n        self._fill_f()\n    break\nrec.f = 1\nreturn",
            "while True:\n    match rec.c:\n        case 1:\n            continue\n        case _:\n            self._fill_f()\n    break\nrec.f = 1\nreturn",
            "rec.f = 1\nwhile rec.a:\n    self._fill_f()\n    break\nreturn",
            "rec.f = 1\nfor line in rec.line_ids:\n    self._fill_f()\n    break\nreturn",
        ];
        for stmts in reported {
            assert!(unassigned(&format!("for rec in self:\n{}", indent(stmts))), "{stmts}");
        }
        let assigned = [
            "rec.f = 1\nfor line in rec.line_ids:\n    if line.a:\n        break",
            "rec.f = 1\ndef inner():\n    return 1",
            "if rec.legacy:\n    self._fill_f()\n    return\nrec.f = 1",
            "with rec.lock():\n    self._fill_f()\nreturn",
            "if rec.a:\n    self._fill_f()\nif not rec.a:\n    self._fill_f()\nreturn",
            "if rec.a:\n    self._fill_f()\nelse:\n    self._fill_f(rec.b)\nreturn",
            "while True:\n    self._fill_f()\n    break\nreturn",
            "try:\n    self._fill_f()\n    return\nfinally:\n    rec.f = 1",
            "try:\n    continue\nfinally:\n    rec.f = 1",
            "try:\n    rec.f = 1\nfinally:\n    continue",
            "for line in rec.line_ids:\n    try:\n        rec.f = 1\n    finally:\n        break\nelse:\n    rec.f = 2",
            "rec.f = 1\nself._fill_f()\nreturn",
            "try:\n    self._fill_f()\nfinally:\n    return",
            "rec.f = 1\nwith rec.lock():\n    self._fill_f()\nreturn",
            "if rec.a:\n    rec.f = 1\n    self._fill_f()\nelse:\n    self._fill_f()\nreturn",
            "rec.f = 1\ntry:\n    self._fill_f()\nexcept ValueError:\n    self._fill_f()\nreturn",
            "try:\n    rec.f = 1\n    self._fill_f()\nexcept ValueError:\n    raise\nelse:\n    return",
            "rec.f = 1\nmatch rec.a:\n    case 1:\n        self._fill_f()\n    case _:\n        self._fill_f()\nreturn",
            "with rec.lock():\n    rec.f = 1\n    if rec.c:\n        continue\n    self._fill_f()\nreturn",
            "try:\n    rec.f = 1\n    if rec.c:\n        continue\n    self._fill_f()\nfinally:\n    pass\nreturn",
            "rec.f = 1\ntry:\n    self._fill_f()\nexcept ValueError:\n    raise\nreturn",
            "rec.f = 1\ntry:\n    self._fill_f()\nexcept ValueError:\n    raise\nelse:\n    return",
            "rec.f = 1\nif rec.a:\n    raise ValueError()\nelse:\n    self._fill_f()\nreturn",
            "for line in rec.line_ids:\n    with rec.lock():\n        if not line.a:\n            continue\n        self._fill_f()\n    return\nrec.f = 1",
            "while True:\n    if rec.c:\n        continue\n    self._fill_f()\n    break\nrec.f = 1\nreturn",
            "while 1:\n    if rec.c:\n        continue\n    self._fill_f()\n    if rec.d:\n        rec.f = 1\n        break\n    break\nrec.f = 1\nreturn",
            "rec.f = 1\nwhile True:\n    self._fill_f()\n    break\nreturn",
            "rec.f = 1\nwhile True:\n    self._fill_f()\n    break\nbreak",
            "rec.f = 1\nwhile True:\n    if rec.c:\n        continue\n    self._fill_f()\n    break\nreturn",
            "rec.f = 1\nwhile True:\n    for line in rec.line_ids:\n        if line.a:\n            break\n    self._fill_f()\n    break\nreturn",
            "rec.f = 1\nwhile True:\n    self._fill_f()\nreturn",
            "while True:\n    if rec.c:\n        continue\n    while True:\n        self._fill_f()\n        break\n    break\nrec.f = 1\nreturn",
            "rec.f = 1\ntry:\n    while True:\n        if rec.c:\n            break\n    self._fill_f()\nfinally:\n    break",
            "try:\n    rec.f = 1\n    while True:\n        if rec.c:\n            break\n    self._fill_f()\nfinally:\n    return",
            "rec.f = 1\ntry:\n    while True:\n        with suppress(ConnectionError):\n            self._fetch()\n            break\n    self._fill_f()\nfinally:\n    break",
            "rec.f = 1\ntry:\n    while True:\n        if rec.c:\n            self._fill_f()\n            break\n    self._fill_f()\nfinally:\n    break",
        ];
        for stmts in assigned {
            assert!(!unassigned(&format!("for rec in self:\n{}", indent(stmts))), "{stmts}");
        }
    }

    #[test]
    fn an_endless_loop_with_no_break_never_falls_through() {
        let assigned = [
            "rec.f = 1\nwhile True:\n    pass\nreturn",
            "rec.f = 1\nwhile True:\n    pass\nbreak",
            "rec.f = 1\nwhile True:\n    if rec.a:\n        rec.g = 1\nreturn",
            "while True:\n    if rec.a:\n        rec.g = 1\nreturn",
            "rec.f = 1\nwhile True:\n    for line in rec.line_ids:\n        break\nreturn",
            "rec.f = 1\nwhile 1:\n    pass\nreturn",
            "rec.f = 1\nwhile True:\n    pass\nelse:\n    break",
            "rec.f = 1\nwhile True:\n    self._fill_f()\n    break\nelse:\n    return",
            "while True:\n    pass\nrec.f = 1\nreturn",
            "while True:\n    pass\nbreak",
            "while True:\n    if rec.a:\n        rec.f = 1\nreturn",
            "while True:\n    rec.f = 1\n    if rec.a:\n        break",
            "rec.f = 1\ntry:\n    while True:\n        self._fill_f()\nfinally:\n    break",
            "with suppress(ValueError):\n    while True:\n        rec.f = int(rec.a)",
            "with suppress(ValueError):\n    rec.f = 1\n    while True:\n        foo()",
        ];
        for stmts in assigned {
            assert!(!unassigned(&format!("for rec in self:\n{}", indent(stmts))), "{stmts}");
        }
        let reported = [
            "while True:\n    rec.f = 1\n    if rec.a:\n        break\nreturn",
            "while True:\n    if rec.a:\n        break\n    rec.f = 1",
            "rec.f = 1\nwhile True:\n    if rec.a:\n        return",
            "while True:\n    if rec.a:\n        return",
            "while True:\n    rec.f = 1\n    if rec.a:\n        return",
            "try:\n    while True:\n        rec.f = 1\n        if rec.a:\n            raise ValueError()\nfinally:\n    break",
            "rec.f = 1\ntry:\n    while True:\n        raise ValueError()\nfinally:\n    break",
            "rec.f = 1\ntry:\n    while True:\n        foo()\nfinally:\n    break",
            "try:\n    rec.f = 1\n    while True:\n        foo()\nfinally:\n    break",
            "with suppress(ValueError):\n    while True:\n        rec.f = int(rec.a)\nreturn",
            "with contextlib.suppress(ValueError):\n    rec.g = 1\n    while True:\n        rec.f = int(rec.a)\nbreak",
            "rec.f = 1\nwhile rec.a:\n    pass\nreturn",
            "rec.f = 1\nwhile True:\n    for line in rec.line_ids:\n        pass\n    else:\n        break\nreturn",
        ];
        for stmts in reported {
            assert!(unassigned(&format!("for rec in self:\n{}", indent(stmts))), "{stmts}");
        }
    }

    #[test]
    fn a_suppressed_exception_skips_the_rest_of_its_with() {
        let reported = [
            "with contextlib.suppress(ValueError):\n    rec.f = 1\n    self._fill_f()\nreturn",
            "with suppress(ValueError):\n    self._fill_f()\nreturn",
            "with suppress(ValueError):\n    rec.f = 1\n    while True:\n        foo()\nreturn",
            "with suppress(ValueError):\n    rec.f = 1\n    while True:\n        foo()\nbreak",
            "with suppress(ValueError):\n    rec.f = 1\n    with rec.lock():\n        while True:\n            foo()\nreturn",
            "with suppress(ValueError):\n    rec.f = 1\n    raise ValueError('x')\nreturn",
            "with suppress(ValueError):\n    rec.f = 1\n    raise\nreturn",
            "with suppress(Exception):\n    rec.f = 1\n    raise UserError('x')\nreturn",
            "with suppress(ValueError):\n    rec.f = 1\n    raise UserError('x')\nreturn",
            "with suppress(ValueError):\n    while True:\n        rec.f = 1\nreturn",
            "with suppress(KeyError):\n    rec.f = 1\n    if rec.a:\n        continue\n    self._fill_f()\nreturn",
            "with suppress(ValueError):\n    rec.f = 1\n    try:\n        while True:\n            foo()\n    except Exception:\n        self._fill_f()\nreturn",
            "with suppress(ValueError):\n    rec.f = 1\n    try:\n        while True:\n            foo()\n    except KeyError:\n        raise UserError('x')\nreturn",
            "while True:\n    with suppress(ValueError):\n        self._fill_f()\n        break\n    break\nrec.f = 1\nreturn",
            "while True:\n    try:\n        with suppress(ValueError):\n            self._fill_f()\n            break\n    finally:\n        break\nrec.f = 1\nreturn",
            "while True:\n    with suppress(ValueError):\n        self._fill_f()\n        break\n    if rec.a:\n        return",
        ];
        for stmts in reported {
            assert!(unassigned(&format!("for rec in self:\n{}", indent(stmts))), "{stmts}");
        }
        let assigned = [
            "with rec.lock():\n    rec.f = 1\n    self._fill_f()\nreturn",
            "with suppress(ValueError):\n    rec.f = 1\n    if rec.a:\n        self._fill_f()\n    else:\n        self._fill_f()\nif rec.c:\n    rec.f = 2",
            "with suppress(ValueError):\n    rec.f = 1\n    match rec.a:\n        case 1:\n            self._fill_f()\n        case _:\n            self._fill_f()\nif rec.c:\n    rec.f = 2",
            "with suppress(ValueError):\n    rec.f = 1\n    try:\n        self._fill_f()\n    except ValueError:\n        self._fill_f()\nif rec.c:\n    rec.f = 2",
            "with suppress(KeyError):\n    rec.f = 1\n    if rec.a:\n        continue\nself._fill_f()\nreturn",
            "with suppress(ValueError):\n    rec.f = False\n    raise UserError('x')\nreturn",
            "with suppress(ValueError):\n    rec.f = False\n    try:\n        while True:\n            foo()\n    except Exception:\n        raise UserError('x')\nreturn",
            "with suppress(ValueError):\n    while True:\n        rec.f = False\nreturn",
            "with suppress(ValueError):\n    rec.f = int(rec.a)",
            "rec.f = 1\nwith suppress(ValueError):\n    raise UserError('x')\nreturn",
        ];
        for stmts in assigned {
            assert!(!unassigned(&format!("for rec in self:\n{}", indent(stmts))), "{stmts}");
        }
    }

    #[test]
    fn a_suppressed_exception_in_a_while_true_loop_retries() {
        let retry = indent("with suppress(ValueError):\n    self._fill_f()\n    break");
        let assigned = [
            format!("while True:\n{retry}rec.f = 1\nreturn"),
            format!("while True:\n{retry}    if rec.attempts > 3:\n        raise UserError('x')\nrec.f = 1\nreturn"),
            format!(
                "while True:\n    try:\n{}    finally:\n        rec.g = 1\nrec.f = 1\nreturn",
                indent(&retry)
            ),
            format!("while True:\n    with rec.lock():\n{}rec.f = 1\nreturn", indent(&retry)),
            format!("rec.f = 1\nwhile True:\n{retry}return"),
        ];
        for stmts in assigned {
            assert!(!unassigned(&format!("for rec in self:\n{}", indent(&stmts))), "{stmts}");
        }
    }

    #[test]
    fn a_while_true_loop_restarts_until_a_break_leaves_it() {
        let retry = "try:\n    self._fill_f()\n    break\nexcept ValueError:\n    pass";
        let suppressed = "with suppress(ValueError):\n    self._fill_f()\n    break";
        let reported = [
            format!("{retry}\nrec.f = 1\nreturn"),
            format!("while rec.a:\n{}rec.f = 1\nreturn", indent(retry)),
            format!("for line in rec.line_ids:\n{}rec.f = 1\nreturn", indent(retry)),
            format!("rec.f = False\n{suppressed}\nreturn"),
            format!("while rec.a:\n    rec.f = False\n{}return", indent(suppressed)),
            format!("while True:\n{}    if rec.tries > 3:\n        break\nrec.f = 1\nreturn", indent(retry)),
            format!("while True:\n    rec.f = False\n    if rec.tries > 3:\n        break\n{}return", indent(suppressed)),
            format!("while True:\n    if rec.tries > 3:\n        rec.f = False\n        break\n{}return", indent(suppressed)),
            "while True:\n    if rec.tries > 3:\n        rec.f = False\n        break\n    self._fill_f()\n    break\nreturn"
                .to_owned(),
            format!("while True:\n    if rec.tries > 3:\n        rec.f = False\n        break\n{}return", indent(retry)),
            format!(
                "while True:\n    if rec.tries > 3:\n        if rec.a:\n            rec.f = 1\n        else:\n            rec.f = 2\n        break\n{}return",
                indent(suppressed)
            ),
            format!("while True:\n    if rec.tries > 3:\n        with rec.lock():\n            rec.f = 1\n        break\n{}return", indent(suppressed)),
            format!(
                "while True:\n    if rec.tries > 3:\n        try:\n            rec.f = 1\n        except ValueError:\n            rec.f = 2\n        break\n{}return",
                indent(suppressed)
            ),
            format!(
                "while True:\n    if rec.tries > 3:\n        try:\n            rec.f = False\n        except ValueError:\n            pass\n        break\n{}return",
                indent(suppressed)
            ),
            "while True:\n    try:\n        with suppress(ValueError):\n            self._fill_f()\n            break\n        foo()\n    except KeyError:\n        break\nreturn"
                .to_owned(),
            format!("while True:\n    if rec.c:\n        return\n{}", indent(retry)),
            "try:\n    while True:\n        rec.f = 1\n        try:\n            self._fill_f()\n            break\n        except ValueError:\n            raise UserError('x')\nfinally:\n    break"
                .to_owned(),
            "try:\n    while True:\n        rec.f = 1\n        try:\n            foo()\n        except ValueError:\n            pass\nfinally:\n    break"
                .to_owned(),
            "try:\n    while True:\n        rec.f = 1\n        if rec.c:\n            raise ValueError()\n        self._fill_f()\n        break\nfinally:\n    break"
                .to_owned(),
        ];
        for stmts in reported {
            assert!(unassigned(&format!("for rec in self:\n{}", indent(&stmts))), "{stmts}");
        }
        let assigned = [
            format!("while True:\n{}rec.f = 1\nreturn", indent(retry)),
            format!("while True:\n    rec.f = False\n{}return", indent(suppressed)),
            format!("while True:\n    rec.f = False\n{}return", indent(retry)),
            "while True:\n    if rec.ready:\n        self._fill_f()\n        break\nrec.f = 1\nreturn".to_owned(),
            format!("while True:\n{}    rec.tries += 1\nrec.f = 1\nreturn", indent(retry)),
            "while True:\n    try:\n        self._fill_f()\n        break\n    except ValueError:\n        raise UserError('x')\nrec.f = 1\nreturn"
                .to_owned(),
            format!(
                "while True:\n    for line in rec.line_ids:\n        if line.a:\n            break\n{}rec.f = 1\nreturn",
                indent(retry)
            ),
            format!("try:\n    while True:\n{}finally:\n    break", indent(&indent(retry))),
            "while True:\n    try:\n        if rec.c:\n            break\n    finally:\n        self._fill_f()\nif rec.d:\n    return"
                .to_owned(),
            "while True:\n    if rec.a:\n        self._fill_f()\n    if not rec.a:\n        self._fill_f()\n    break\nreturn"
                .to_owned(),
            "while True:\n    if rec.a:\n        self._fill_f()\n    with rec.lock():\n        if not rec.a:\n            self._fill_f()\n    break\nreturn"
                .to_owned(),
            "while True:\n    for line in rec.line_ids:\n        self._fill_f()\n    if not rec.line_ids:\n        self._fill_f()\n    break\nreturn"
                .to_owned(),
            "while True:\n    if rec.line_ids:\n        for line in rec.line_ids:\n            self._fill_f()\n    else:\n        self._fill_f()\n    break\nreturn"
                .to_owned(),
            "while True:\n    if rec.a:\n        self._fill_f()\n    try:\n        if not rec.a:\n            self._fill_f()\n    finally:\n        rec.g = 1\n    break\nreturn"
                .to_owned(),
            "while True:\n    if rec.c:\n        rec.f = 1\n    if rec.a:\n        self._fill_f()\n    with rec.lock():\n        if not rec.a:\n            self._fill_f()\n    break\nreturn"
                .to_owned(),
            "while True:\n    try:\n        rec.f = 1\n        if rec.c:\n            break\n    finally:\n        self._fill_f()\nreturn"
                .to_owned(),
            format!(
                "while True:\n    if rec.tries > 3:\n        try:\n            rec.f = False\n            break\n        finally:\n            self._fill_f()\n{}return",
                indent(suppressed)
            ),
        ];
        let paired = [
            "if rec.a:\n    self._fill_f()\nif not rec.a:\n    self._fill_f()",
            "for line in rec.line_ids:\n    self._fill_f()\nif not rec.line_ids:\n    self._fill_f()",
            "if rec.a:\n    self._fill_f()\nwith rec.lock():\n    if not rec.a:\n        self._fill_f()",
        ];
        let then = ["rec.f = False", "_logger.info('%s', rec)", "rec.message_post()"];
        let filled_first = paired
            .iter()
            .flat_map(|fill| then.map(|then| format!("{fill}\n{then}\nbreak")));
        let seeded = "if rec.line_ids:\n    for line in rec.line_ids:\n        self._fill_f()\n    rec.f = False\n    break\nelse:\n    self._fill_f()\n    break";
        let assigned = assigned.into_iter().chain(
            filled_first
                .chain([seeded.to_owned()])
                .map(|body| format!("while True:\n{}return", indent(&body))),
        );
        for stmts in assigned {
            assert!(!unassigned(&format!("for rec in self:\n{}", indent(&stmts))), "{stmts}");
        }
    }

    #[test]
    fn an_attribute_subscript_arithmetic_or_assert_may_raise_under_suppress() {
        let reported = [
            "with suppress(IndexError):\n    while True:\n        rec.f = rec.vals[i]\n        i += 1\nreturn",
            "with suppress(IndexError):\n    while True:\n        rec.f = 1\n        v = vals[i]\nreturn",
            "with suppress(AttributeError):\n    while True:\n        rec.f = 1\n        v = rec.partner.name\nreturn",
            "with suppress(ZeroDivisionError):\n    while True:\n        rec.f = 1\n        v = 1 / i\nreturn",
            "with suppress(ZeroDivisionError):\n    while True:\n        rec.f = 1\n        i /= 0\nreturn",
            "with suppress(AssertionError):\n    while True:\n        rec.f = 1\n        assert i\nreturn",
            "with suppress(AttributeError):\n    while True:\n        rec.f = False\n        rec.g = 2\nreturn",
            "with suppress(KeyError):\n    while True:\n        rec.f = False\n        vals[i] = 2\nreturn",
            "with suppress(ValueError):\n    while True:\n        rec.f = 'draft'\n        v = i\nreturn",
            "with suppress(ValueError):\n    while True:\n        rec['f'] = 'draft'\n        v = i\nreturn",
            "with suppress(ValueError):\n    while True:\n        rec.f, v = False, i\nreturn",
        ];
        for stmts in reported {
            assert!(unassigned(&format!("for rec in self:\n{}", indent(stmts))), "{stmts}");
        }
        let assigned = [
            "with suppress(IndexError):\n    while True:\n        rec.f = False\n        v = i\nreturn",
            "with suppress(KeyError):\n    while True:\n        rec['f'] = False\n        v = i\nreturn",
        ];
        for stmts in assigned {
            assert!(!unassigned(&format!("for rec in self:\n{}", indent(stmts))), "{stmts}");
        }
    }

    #[test]
    fn a_handler_after_a_constant_write_finds_the_field_assigned() {
        let handled = |first: &str, handler: &str| {
            format!("try:\n{first}    rec.f = int(rec.a)\nexcept ValueError:\n    {handler}")
        };
        let assigned = [
            handled("    rec.f = False\n", "pass"),
            handled("    pass\n    rec.f = 0\n", "pass"),
            handled("    v = 0\n    rec.f = 0\n", "pass"),
            handled("    rec.f = False\n", "continue"),
            handled("    rec.f = None\n", "pass"),
            handled("    rec.f = ''\n", "pass"),
        ];
        for stmts in assigned {
            assert!(!unassigned(&format!("for rec in self:\n{}", indent(&stmts))), "{stmts}");
        }
        let reported = [
            handled("    v = int(rec.a)\n    rec.f = 0\n", "pass"),
            handled("    rec.g = False\n    rec.f = 0\n", "pass"),
            handled("    rec.f = False\n", "return"),
            handled("    rec.f = False\n", "break"),
            handled("    rec.f = 'archived'\n", "pass"),
            handled("    rec.f = 5\n", "pass"),
            handled("    rec.f, rec.g = (0,)\n", "pass"),
            handled("    rec.f, rec.g = ()\n", "pass"),
            handled("    rec.g = rec.f = 0\n", "pass"),
        ];
        for stmts in reported {
            assert!(unassigned(&format!("for rec in self:\n{}", indent(&stmts))), "{stmts}");
        }
        let typed = |first: &str, ftype: &str| {
            unassigned_as(&format!("for rec in self:\n{}", indent(&handled(first, "pass"))), ftype)
        };
        let numbered = |first: &str| typed(first, "Integer");
        assert!(!numbered("    rec.f = 5\n") && !numbered("    rec.f = 2.5\n") && !numbered("    rec.f = -1\n"));
        assert!(numbered("    rec.f = 'archived'\n") && numbered("    rec.f = 1j\n") && numbered("    rec.f = -1j\n"));
        assert!(!typed("    rec.f = False\n", "Many2many") && !typed("    rec.f = None\n", "One2many"));
        assert!(typed("    rec.f = 0\n", "Many2many") && typed("    rec.f = ''\n", "One2many"));
        let unread = |first: &str| typed(first, UNREAD_TYPE);
        assert!(!unread("    rec.f = 5\n") && !unread("    rec.f = -1\n") && !unread("    rec.f = 0\n"));
        assert!(unread("    rec.f = 1j\n"));
    }

    /// Walking a body twice would take 2^depth walks here.
    #[test]
    fn nested_bodies_take_one_walk_each() {
        fn nest(depth: usize) -> String {
            let inner = match depth {
                0 => return "rec.f = 1".to_owned(),
                _ => indent(&nest(depth - 1)),
            };
            match depth % 6 {
                0 => format!("if rec.a{depth}:\n{inner}"),
                1 => format!("if rec.a{depth}:\n    pass\nelse:\n{inner}"),
                2 => format!("try:\n{inner}except ValueError:\n    pass"),
                3 => format!("try:\n    pass\nexcept ValueError:\n{inner}"),
                4 => format!("try:\n    pass\nfinally:\n{inner}"),
                _ => format!("for line in rec.line_ids:\n{inner}"),
            }
        }
        let depth = 18;
        BLOCK_WALKS.with(|walks| walks.set(0));
        assert!(unassigned(&format!("for rec in self:\n{}", indent(&nest(depth)))));
        let walks = BLOCK_WALKS.with(std::cell::Cell::get);
        assert!(walks <= 3 * depth, "{walks} walks");
    }

    #[test]
    fn a_body_it_cannot_follow_is_not_reported() {
        let skipped = [
            "self.f = False\nfor rec in self:\n    if rec.a:\n        rec.f = 1",
            "self._reset_f()\nfor rec in self:\n    if rec.a:\n        rec.f = 1",
            "for rec in self:\n    if rec.a:\n        rec.f = 1\nif not all(self.mapped('a')):\n    self._fill_f()",
            "for rec in self:\n    if rec.a:\n        self.f = 1",
            "for rec in self:\n    if rec.a:\n        rec.parent_id.f = 1",
            "for rec in self:\n    if not rec.f:\n        rec.f = 1",
            "for rec in self:\n    if rec.mapped('f'):\n        continue",
            "for rec in self:\n    if rec.a:\n        rec.f += 1",
            "for rec in self:\n    if rec.a:\n        setattr(rec, 'f', 1)",
            "for rec in self:\n    if rec.a:\n        rec[name] = 1",
            "for rec in self:\n    if rec.a:\n        rec.update(values)",
            "for rec in self:\n    if rec.a:\n        rec.update({name: 1})",
            "for rec in self.filtered('a'):\n    rec.f = 1",
            "for rec in self:\n    rec.f = 1\nfor rec in self:\n    pass",
            "for rec, other in zip(self, others):\n    pass",
            "rec = self[0]\nrec.f = 1",
        ];
        for body in skipped {
            assert!(!unassigned(body), "{body}");
        }
    }
}
