//! What a method body does with its arguments and its records, for the rules `python.rs` raises on
//! ORM overrides, compute methods and caches. A predicate that meets code it cannot follow answers
//! so that the rule stays silent.

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
/// body must skip it.
pub(crate) fn leaves_unassigned(body: &[Stmt], field: &str) -> bool {
    let Some((record, records_loop)) = only_loop_over_self(body) else {
        return false;
    };
    let whole = FieldUses::scan(body, field, record);
    let in_loop = FieldUses::scan(&records_loop.body, field, record);
    if whole.reads || whole.dynamic || whole.writes != in_loop.record_writes || calls_self(body) {
        return false;
    }
    Paths::new(record, field).block(&records_loop.body) != Flow::Assigned
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

/// How the paths through some statements of the loop body end for the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    /// Every path assigns it, or leaves by an exception that Odoo reports anyway.
    Assigned,
    /// A path leaves the iteration, or the nested loop it is in, without assigning it.
    Escaped,
    /// No path leaves, and one goes on without assigning it.
    Open,
}

fn join(flows: impl IntoIterator<Item = Flow>) -> Flow {
    let mut joined = Flow::Assigned;
    for flow in flows {
        match flow {
            Flow::Escaped => return Flow::Escaped,
            Flow::Open => joined = Flow::Open,
            Flow::Assigned => {}
        }
    }
    joined
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

    /// The first statement that assigns the field or leaves decides. Two that each assign it on
    /// some paths may together cover every path, as `if x` and `if not x` do, so they count as
    /// assigning it.
    fn block(&self, stmts: &[Stmt]) -> Flow {
        let mut partial = 0;
        for stmt in stmts {
            match self.stmt(stmt) {
                Flow::Open => partial += usize::from(self.writes_in(stmt)),
                flow => return flow,
            }
            if partial > 1 {
                return Flow::Assigned;
            }
        }
        Flow::Open
    }

    fn writes_in(&self, stmt: &Stmt) -> bool {
        FieldUses::scan(std::slice::from_ref(stmt), self.field, self.record).record_writes > 0
    }

    /// How one statement ends for the field, by its kind: compound statements combine the blocks
    /// they hold, `raise` counts as assigning, and the others assign the field or go on.
    fn stmt(&self, stmt: &Stmt) -> Flow {
        match stmt {
            Stmt::Raise(_) => Flow::Assigned,
            Stmt::Continue(_) | Stmt::Break(_) | Stmt::Return(_) => self.jump(stmt),
            Stmt::If(i) => self.branches(i),
            Stmt::Try(t) => self.handled(t),
            Stmt::With(w) => self.block(&w.body),
            Stmt::For(_) | Stmt::While(_) => self.nested_loop(stmt),
            Stmt::Match(m) => join(m.cases.iter().map(|c| self.block(&c.body))),
            stmt if self.assigns(stmt) => Flow::Assigned,
            _ => Flow::Open,
        }
    }

    /// In a nested loop, `continue` only starts its next iteration.
    fn jump(&self, stmt: &Stmt) -> Flow {
        if self.nested && stmt.is_continue_stmt() {
            Flow::Open
        } else {
            Flow::Escaped
        }
    }

    /// A nested loop may run no iteration, and its `else` runs unless a `break` leaves it, so the
    /// body only counts when every `break` assigns first: then the `else` decides, or for an
    /// endless `while`, which only a `break` leaves, the body. A `break` or `return` that does not
    /// assign is taken to go on after the loop.
    fn nested_loop(&self, stmt: &Stmt) -> Flow {
        let (body, orelse, endless) = match stmt {
            Stmt::For(f) => (&f.body, &f.orelse, false),
            Stmt::While(w) => (&w.body, &w.orelse, always_true(&w.test)),
            _ => return Flow::Open,
        };
        let inner = Paths { nested: true, ..*self };
        match inner.block(body) {
            Flow::Escaped if endless => Flow::Open,
            Flow::Escaped => join([self.block(orelse), Flow::Open]),
            _ if endless => Flow::Assigned,
            _ => self.block(orelse),
        }
    }

    /// A chain with an `elif` but no `else` is taken to cover every case, as a `match` is.
    fn branches(&self, i: &ast::StmtIf) -> Flow {
        let clauses = i.elif_else_clauses.iter().map(|c| self.block(&c.body));
        let fallthrough = i.elif_else_clauses.is_empty().then_some(Flow::Open);
        join(std::iter::once(self.block(&i.body)).chain(clauses).chain(fallthrough))
    }

    fn handled(&self, t: &ast::StmtTry) -> Flow {
        if self.block(&t.finalbody) == Flow::Assigned {
            return Flow::Assigned;
        }
        let main = match self.block(&t.body) {
            Flow::Open => self.block(&t.orelse),
            flow => flow,
        };
        let handlers = t.handlers.iter().map(|h| match h {
            ast::ExceptHandler::ExceptHandler(h) => self.block(&h.body),
        });
        join(std::iter::once(main).chain(handlers))
    }

    fn assigns(&self, stmt: &Stmt) -> bool {
        match stmt {
            Stmt::Assign(a) => a.targets.iter().any(|t| self.writes(t)),
            Stmt::AnnAssign(a) => a.value.is_some() && self.writes(&a.target),
            Stmt::Expr(e) => matches!(&*e.value, Expr::Call(c) if may_assign(c, self.record)),
            _ => false,
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
        ];
        for stmts in assigned {
            assert!(!unassigned(&format!("for rec in self:\n{}", indent(stmts))), "{stmts}");
        }
    }

    #[test]
    fn a_body_it_cannot_follow_is_not_reported() {
        let skipped = [
            "self.f = False\nfor rec in self:\n    if rec.a:\n        rec.f = 1",
            "self._reset_f()\nfor rec in self:\n    if rec.a:\n        rec.f = 1",
            "for rec in self:\n    if rec.a:\n        rec.f = 1\nif not all(self.mapped('a')):\n    self._fill_f()",
            "for rec in self:\n    if rec.t == 'a':\n        rec.f = 1\n    if rec.t != 'a':\n        rec.f = 2",
            "for rec in self:\n    if rec.a:\n        rec.f = 1\n    if not rec.a:\n        rec.f = 2",
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
