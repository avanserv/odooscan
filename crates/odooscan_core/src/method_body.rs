//! What a method body does with its arguments and its records, for the rules `python.rs` raises on
//! ORM overrides, compute methods and caches. A predicate that meets code it cannot follow answers
//! so that the rule stays silent.

use ruff_python_ast::comparable::ComparableExpr;
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, CmpOp, Expr, ExprContext, Stmt};
use ruff_python_parser::parse_expression;
use ruff_python_stdlib::builtins::is_python_builtin;

/// Past every Python minor version, so that a builtin of any version counts.
const ANY_PYTHON: u8 = u8::MAX;

/// Methods a dict has and a list lacks.
const DICT_METHODS: &[&str] = &["get", "items", "keys", "values", "setdefault", "update"];

fn is_name(expr: &Expr, name: &str) -> bool {
    matches!(expr, Expr::Name(n) if n.id.as_str() == name)
}

fn is_str(expr: &Expr, text: &str) -> bool {
    matches!(expr, Expr::StringLiteral(s) if s.value.to_str() == text)
}

fn arguments(call: &ast::ExprCall) -> impl Iterator<Item = &Expr> {
    let keywords = call.arguments.keywords.iter().map(|k| &k.value);
    call.arguments.args.iter().chain(keywords)
}

/// Whether a `return` of the method itself, not of a function it nests, gives a value.
pub(crate) fn returns_value(body: &[Stmt]) -> bool {
    let mut finder = ReturnFinder::default();
    finder.visit_body(body);
    finder.found
}

#[derive(Default)]
struct ReturnFinder {
    found: bool,
}

impl<'a> Visitor<'a> for ReturnFinder {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
            Stmt::Return(r) => self.found |= r.value.as_deref().is_some_and(|v| !v.is_none_literal_expr()),
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
/// body must skip it or leave the loop.
pub(crate) fn leaves_unassigned(body: &[Stmt], field: &str) -> bool {
    let Some((record, records_loop)) = only_loop_over_self(body) else {
        return false;
    };
    let whole = FieldUses::scan(body, field, record);
    let in_loop = FieldUses::scan(&records_loop.body, field, record);
    if whole.reads || whole.dynamic || whole.writes != in_loop.record_writes || calls_self(body) {
        return false;
    }
    Paths::new(record, field).block(&records_loop.body) > Flow::Assigned
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
    /// Every path calls a method of `self`, which may assign it on every record, so leaving the
    /// loop over `self` after that skips none.
    Filled,
    /// Every path assigns it, or leaves by an exception that Odoo reports anyway.
    Assigned,
    /// No path leaves, and one goes on without assigning it.
    Open,
    /// A path ends the iteration, or the nested loop it is in, without assigning it.
    Escaped,
    /// A path leaves the loop over `self` from any depth, by a `return` or by a `break` after the
    /// assignment, so the records after it go unassigned.
    Left,
}

fn join(flows: impl IntoIterator<Item = Flow>) -> Flow {
    flows.into_iter().fold(Flow::Filled, Flow::max)
}

fn same(x: &Expr, y: &Expr) -> bool {
    ComparableExpr::from(x) == ComparableExpr::from(y)
}

/// Whether one of two tests holds exactly when the other does not: `not e` against `e`, or one
/// comparison against the same operands under the negated operator, as `==` against `!=`.
fn complementary(a: &Expr, b: &Expr) -> bool {
    let negates =
        |x: &Expr, y: &Expr| matches!(x, Expr::UnaryOp(u) if u.op == ast::UnaryOp::Not && same(&u.operand, y));
    negates(a, b) || negates(b, a) || compared(a, b, true)
}

fn equivalent(a: &Expr, b: &Expr) -> bool {
    same(a, b) || compared(a, b, false)
}

/// Whether two one-operator comparisons take the same operands under the same operator, or the
/// negated one when `negated`. The operands of `==`, `!=`, `is` and `is not` may come in either
/// order.
fn compared(a: &Expr, b: &Expr, negated: bool) -> bool {
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

/// A statement that assigns the field, ending as `flow`, on the paths where `test` is `holds`, and
/// goes on without it on the others.
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

/// Whether statements leave the loop over `self`: by a `return`, or by a `break` when `breaks`
/// holds and the `break` is not in the body of a loop among them. A block stops at a call of a
/// method of `self`, which may assign the field on every record.
struct Exits {
    breaks: bool,
    found: bool,
}

fn fills(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::Expr(e) if matches!(&*e.value, Expr::Call(c) if calls_self_method(c)))
}

impl<'a> Visitor<'a> for Exits {
    fn visit_body(&mut self, body: &'a [Stmt]) {
        body.iter().take_while(|s| !fills(s)).for_each(|s| self.visit_stmt(s));
    }

    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
            Stmt::Return(_) => self.found = true,
            Stmt::Break(_) => self.found |= self.breaks,
            Stmt::For(ast::StmtFor { body, orelse, .. }) | Stmt::While(ast::StmtWhile { body, orelse, .. }) => {
                let breaks = std::mem::replace(&mut self.breaks, false);
                self.visit_body(body);
                self.breaks = breaks;
                self.visit_body(orelse);
            }
            _ => visitor::walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, _: &'a Expr) {}
}

#[derive(Clone, Copy)]
struct Paths<'a> {
    record: &'a str,
    field: &'a str,
    /// In a loop nested in the loop over `self`, where `continue` only starts its next iteration.
    nested: bool,
}

impl<'a> Paths<'a> {
    fn new(record: &'a str, field: &'a str) -> Self {
        Self {
            record,
            field,
            nested: false,
        }
    }

    /// The first statement that assigns the field or leaves decides, but a later exit still leaves
    /// the loop over `self`: after the assignment, or in a nested loop after a `break` that only
    /// leaves that loop.
    fn block(&self, stmts: &[Stmt]) -> Flow {
        #[cfg(test)]
        tests::BLOCKS.with(|walks| walks.set(walks.get() + 1));
        let mut sides = vec![];
        for (n, stmt) in stmts.iter().enumerate() {
            match self.stmt(stmt, &mut sides) {
                Flow::Open => {}
                flow @ (Flow::Assigned | Flow::Escaped) => return self.exits([&stmts[n + 1..]], flow),
                flow => return flow,
            }
        }
        Flow::Open
    }

    /// In a nested loop, a `break` only leaves that loop.
    fn exits<'s>(&self, rest: impl IntoIterator<Item = &'s [Stmt]>, flow: Flow) -> Flow {
        let mut exits = Exits {
            breaks: !self.nested,
            found: false,
        };
        rest.into_iter().for_each(|stmts| exits.visit_body(stmts));
        if exits.found { Flow::Left } else { flow }
    }

    /// How one statement ends for the field, by its kind: compound statements combine the blocks
    /// they hold, `raise` counts as assigning, and the others assign the field or go on. `sides`
    /// holds the statements before it in its block that assign on one side of a test.
    fn stmt<'s>(&self, stmt: &'s Stmt, sides: &mut Vec<Side<'s>>) -> Flow {
        match stmt {
            Stmt::Raise(_) => Flow::Assigned,
            Stmt::Continue(_) | Stmt::Break(_) | Stmt::Return(_) => self.jump(stmt),
            Stmt::If(i) => self.branches(i, sides),
            Stmt::Try(t) => self.handled(t),
            Stmt::With(w) => self.block(&w.body),
            Stmt::For(_) | Stmt::While(_) => self.nested_loop(stmt, sides),
            Stmt::Match(m) => join(m.cases.iter().map(|c| self.block(&c.body))),
            _ => self.assigned(stmt),
        }
    }

    /// A `return` leaves the method from any depth, and in a nested loop `continue` only starts
    /// its next iteration.
    fn jump(&self, stmt: &Stmt) -> Flow {
        match stmt {
            Stmt::Return(_) => Flow::Left,
            Stmt::Continue(_) if self.nested => Flow::Open,
            _ => Flow::Escaped,
        }
    }

    /// A `for` whose body assigns does so when its iterable is not empty, so it pairs as
    /// `if items:` would.
    fn nested_loop<'s>(&self, stmt: &'s Stmt, sides: &mut Vec<Side<'s>>) -> Flow {
        let (body, orelse, endless) = match stmt {
            Stmt::For(f) => (&f.body, &f.orelse, false),
            Stmt::While(w) => (&w.body, &w.orelse, always_true(&w.test)),
            _ => return Flow::Open,
        };
        let looped = Paths { nested: true, ..*self }.block(body);
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

    /// A nested loop may run no iteration, and its `else` runs unless a `break` leaves it, so the
    /// body only counts when every `break` assigns first: then the `else` decides, or for an
    /// endless `while`, which only a `break` leaves, the body. A `break` that does not assign is
    /// taken to go on after the loop, and a `return` leaves the method from any depth.
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
    fn branches<'s>(&self, i: &'s ast::StmtIf, sides: &mut Vec<Side<'s>>) -> Flow {
        let body = self.block(&i.body);
        let clauses: Vec<_> = i.elif_else_clauses.iter().map(|c| self.block(&c.body)).collect();
        let fallthrough = i.elif_else_clauses.is_empty().then_some(Flow::Open);
        let flow = join(std::iter::once(body).chain(clauses.iter().copied()).chain(fallthrough));
        match side(i, body, &clauses) {
            Some(side) if flow == Flow::Open => pair(side, sides),
            _ => flow,
        }
    }

    /// The body then its `else`, or a handler, runs before the `finally`.
    fn handled(&self, t: &ast::StmtTry) -> Flow {
        let main = match self.block(&t.body) {
            Flow::Open => self.block(&t.orelse),
            Flow::Assigned => self.exits([&t.orelse[..]], Flow::Assigned),
            flow => flow,
        };
        let handlers = t.handlers.iter().map(|h| match h {
            ast::ExceptHandler::ExceptHandler(h) => self.block(&h.body),
        });
        self.finally(t, join(std::iter::once(main).chain(handlers)))
    }

    /// A `finally` runs after every path `tried` sums up, so an exit before a `finally` that
    /// assigns, or one in a `finally` after paths that assign, leaves the loop over `self` after
    /// the assignment, unless every path tried calls a method of `self` first.
    fn finally(&self, t: &ast::StmtTry, tried: Flow) -> Flow {
        let finally = self.block(&t.finalbody);
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
        pub(super) static BLOCKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
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
        let src = format!("def _compute_f(self):\n{}", indent(body));
        with_body(&src, |body| leaves_unassigned(body, "f"))
    }

    fn indent(body: &str) -> String {
        body.lines().map(|l| format!("    {l}\n")).collect()
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
        ];
        for stmts in reported {
            assert!(unassigned(&format!("for rec in self:\n{}", indent(stmts))), "{stmts}");
        }
        let assigned = [
            "for line in rec.line_ids:\n    if line.a:\n        rec.f = 1\n        break\nelse:\n    rec.f = 2",
            "for line in rec.line_ids:\n    if not line.a:\n        continue\n    rec.f = 1\n    break\nelse:\n    rec.f = 2",
            "while True:\n    rec.f = 1\n    break",
            "while 1:\n    if rec.a:\n        continue\n    rec.f = 1\n    break",
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
        ];
        for stmts in assigned {
            assert!(!unassigned(&format!("for rec in self:\n{}", indent(&stmts))), "{stmts}");
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
        ];
        for stmts in assigned {
            assert!(!unassigned(&format!("for rec in self:\n{}", indent(stmts))), "{stmts}");
        }
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
        BLOCKS.with(|walks| walks.set(0));
        assert!(unassigned(&format!("for rec in self:\n{}", indent(&nest(depth)))));
        let walks = BLOCKS.with(std::cell::Cell::get);
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
