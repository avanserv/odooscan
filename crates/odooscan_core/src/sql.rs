//! What a function does with database cursors: the rollbacks, commits and cache invalidations it
//! calls, and the tables its literal write statements target.

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, BoolOp, Expr, Operator, Stmt};

use crate::python::{SAME_MODEL, is_cursor, str_lit, tail_name};

/// ORM methods that drop cached values; from 16.0 the public ones flush pending writes first.
const INVALIDATIONS: &[&str] = &[
    "invalidate_model",
    "invalidate_recordset",
    "invalidate_all",
    "invalidate_cache",
    "_invalidate_cache",
];

/// Words that end the `SET` list of an `UPDATE`.
const AFTER_SET: &[&str] = &["FROM", "WHERE", "RETURNING"];

/// A literal write statement a call runs.
pub(crate) struct Write<'a> {
    pub call: &'a ast::ExprCall,
    pub verb: &'static str,
    pub table: String,
    /// The columns an `UPDATE` sets, when the text names each of them.
    pub columns: Option<Vec<String>>,
}

/// What a name the function binds holds, as far as cursors go.
#[derive(Clone, Copy)]
enum Held {
    /// The cursor the function was given, or a value read from it.
    Given,
    /// A cursor the function opens, or a value that is no cursor at all.
    Cursor,
    /// An environment, or records, on a cursor the function opens.
    Env,
    /// What a context manager yields, which may be either.
    Either,
}

/// What a function body does with cursors, with or without the functions and lambdas it defines.
/// Calls on a cursor the function opens, a transaction of its own, are left out.
pub(crate) struct CursorUse<'a> {
    nested: bool,
    /// Rollbacks the code goes on after, outside a test of whether the cursor is read-only.
    pub rollbacks: Vec<&'a ast::ExprCall>,
    pub commits: bool,
    pub invalidates: bool,
    /// The table and the column of each `create_column()` call, when they are literals.
    created: Vec<(Option<&'a str>, Option<&'a str>)>,
    pub writes: Vec<Write<'a>>,
    /// The methods it calls on `self` or `cls`.
    pub own_methods: Vec<&'a str>,
    /// The names each function binds, innermost last; a later binding shadows an earlier one.
    scopes: Vec<Vec<(&'a str, Held)>>,
    /// Whether each enclosing block ends by raising or exiting.
    leaving: Vec<bool>,
    read_only: u32,
}

fn is_invalidation(method: &str, receiver: &Expr) -> bool {
    INVALIDATIONS.contains(&method) || method == "invalidate" && tail_name(receiver) == Some("cache")
}

/// A block whose last statement raises or exits the process, which loses the transaction with or
/// without a rollback before it.
fn leaves(body: &[Stmt]) -> bool {
    match body.last() {
        Some(Stmt::Raise(_)) => true,
        Some(Stmt::Expr(e)) => matches!(&*e.value, Expr::Call(c) if is_exit(&c.func)),
        _ => false,
    }
}

fn is_exit(func: &Expr) -> bool {
    match func {
        Expr::Name(n) => n.id.as_str() == "exit",
        Expr::Attribute(a) => a.attr.as_str() == "exit" && tail_name(&a.value) == Some("sys"),
        _ => false,
    }
}

/// Whether a test reads `<cursor>.readonly`, as code does before it trades a read-only cursor,
/// which holds no write to lose, for another.
fn tests_read_only(test: &Expr) -> bool {
    match test {
        Expr::Attribute(a) => a.attr.as_str() == "readonly" && is_cursor(&a.value),
        Expr::BoolOp(b) if b.op == BoolOp::And => b.values.iter().any(tests_read_only),
        _ => false,
    }
}

impl<'a> CursorUse<'a> {
    fn new(nested: bool) -> Self {
        Self {
            nested,
            rollbacks: vec![],
            commits: false,
            invalidates: false,
            created: vec![],
            writes: vec![],
            own_methods: vec![],
            scopes: vec![vec![]],
            leaving: vec![],
            read_only: 0,
        }
    }

    /// What the body does itself; each function it nests is scanned on its own.
    pub(crate) fn scan(body: &'a [Stmt]) -> Self {
        let mut found = Self::new(false);
        found.visit_body(body);
        found
    }

    /// What the body and the functions and lambdas it defines do, for code that hands its work to
    /// a closure.
    pub(crate) fn scan_nested(body: &'a [Stmt]) -> Self {
        let mut found = Self::new(true);
        found.visit_body(body);
        found
    }

    /// Whether the function adds, with `create_column()`, the columns a write sets, or a column of
    /// its table when the text does not tell which it sets: no field of the registry reads them yet.
    pub(crate) fn adds_columns_of(&self, write: &Write) -> bool {
        let added: Vec<Option<&str>> = self
            .created
            .iter()
            .filter(|(table, _)| *table == Some(write.table.as_str()))
            .map(|(_, column)| *column)
            .collect();
        match &write.columns {
            _ if added.is_empty() => false,
            Some(columns) if !added.contains(&None) => columns.iter().all(|c| added.contains(&Some(c.as_str()))),
            _ => true,
        }
    }

    fn held(&self, name: &str) -> Option<Held> {
        self.scopes
            .iter()
            .rev()
            .flat_map(|scope| scope.iter().rev())
            .find(|(bound, _)| *bound == name)
            .map(|(_, held)| *held)
    }

    /// A cursor the function opens: a name it binds to one, or the cursor of an environment on one.
    fn own_cursor(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Name(n) => matches!(self.held(n.id.as_str()), Some(Held::Cursor | Held::Either)),
            Expr::Attribute(a) => is_cursor(expr) && self.own_env(&a.value),
            _ => false,
        }
    }

    /// An environment on a cursor the function opens, or records or an environment taken from one.
    fn own_env(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Name(n) => matches!(self.held(n.id.as_str()), Some(Held::Env | Held::Either)),
            Expr::Attribute(a) => a.attr.as_str() == "env" && self.own_env(&a.value),
            Expr::Subscript(s) => self.own_env(&s.value),
            Expr::Call(c) => self.own_env_call(c),
            _ => false,
        }
    }

    /// `api.Environment(cr, ...)` or `env(cr=cr)` on an own cursor, `with_env()` of an own
    /// environment, and records an own environment returns.
    fn own_env_call(&self, call: &ast::ExprCall) -> bool {
        match &*call.func {
            Expr::Attribute(a) if a.attr.as_str() == "with_env" => {
                call.arguments.args.first().is_some_and(|e| self.own_env(e))
            }
            Expr::Attribute(a)
                if SAME_MODEL.contains(&a.attr.as_str()) || matches!(a.attr.as_str(), "browse" | "search") =>
            {
                self.own_env(&a.value)
            }
            func if matches!(tail_name(func), Some("Environment" | "env")) => {
                match call.arguments.find_argument_value("cr", 0) {
                    Some(cursor) => self.own_cursor(cursor),
                    None => self.own_env(func),
                }
            }
            _ => false,
        }
    }

    fn bind(&mut self, target: &'a Expr, held: Held) {
        if let (Expr::Name(n), Some(scope)) = (target, self.scopes.last_mut()) {
            scope.push((n.id.as_str(), held));
        }
    }

    fn assign(&mut self, target: &'a Expr, value: &Expr) {
        let held = if self.own_cursor(value) {
            Held::Cursor
        } else if is_cursor(value) {
            Held::Given
        } else if self.own_env(value) {
            Held::Env
        } else {
            Held::Cursor
        };
        self.bind(target, held);
    }

    /// Visits a nested function or lambda, whose parameters hide the names the function binds.
    fn nest(&mut self, parameters: Option<&'a ast::Parameters>, walk: impl FnOnce(&mut Self)) {
        let params = parameters
            .into_iter()
            .flat_map(|p| p.iter())
            .map(|p| (p.name().as_str(), Held::Given));
        self.scopes.push(params.collect());
        walk(self);
        self.scopes.pop();
    }

    fn call(&mut self, call: &'a ast::ExprCall) {
        let Expr::Attribute(a) = &*call.func else {
            self.create_column(call);
            return;
        };
        let (method, receiver) = (a.attr.as_str(), &*a.value);
        if is_cursor(receiver) {
            self.cursor_call(method, receiver, call);
        }
        if method == "execute_query" && tail_name(receiver) == Some("env") && !self.own_env(receiver) {
            self.write(call);
        }
        self.invalidates |= is_invalidation(method, receiver);
        self.create_column(call);
        if matches!(receiver, Expr::Name(n) if matches!(n.id.as_str(), "self" | "cls")) {
            self.own_methods.push(method);
        }
    }

    fn create_column(&mut self, call: &'a ast::ExprCall) {
        if tail_name(&call.func) == Some("create_column") {
            let literal = |i: usize| call.arguments.args.get(i).and_then(str_lit);
            self.created.push((literal(1), literal(2)));
        }
    }

    fn cursor_call(&mut self, method: &str, receiver: &Expr, call: &'a ast::ExprCall) {
        let own = self.own_cursor(receiver);
        match method {
            "rollback" if !own => self.rollback(call),
            "commit" => self.commits = true,
            "execute" if !own => self.write(call),
            _ => {}
        }
    }

    fn rollback(&mut self, call: &'a ast::ExprCall) {
        if self.read_only == 0 && !self.leaving.last().copied().unwrap_or_default() {
            self.rollbacks.push(call);
        }
    }

    fn write(&mut self, call: &'a ast::ExprCall) {
        let Some(text) = call.arguments.args.first().and_then(query_text) else {
            return;
        };
        if let Some((verb, table)) = write_target(&text) {
            self.writes.push(Write {
                call,
                verb,
                table: table.to_owned(),
                columns: set_columns(&text).map(|c| c.into_iter().map(str::to_owned).collect()),
            });
        }
    }

    fn with(&mut self, w: &'a ast::StmtWith) {
        for item in &w.items {
            if let Some(name) = &item.optional_vars {
                let given = is_cursor(&item.context_expr) && !self.own_cursor(&item.context_expr);
                self.bind(name, if given { Held::Given } else { Held::Either });
            }
        }
    }

    /// The body of `if <cursor>.readonly:` holds no rollback to report.
    fn branch(&mut self, i: &'a ast::StmtIf) {
        self.visit_expr(&i.test);
        let read_only = u32::from(tests_read_only(&i.test));
        self.read_only += read_only;
        self.visit_body(&i.body);
        self.read_only -= read_only;
        for clause in &i.elif_else_clauses {
            self.visit_elif_else_clause(clause);
        }
    }
}

impl<'a> Visitor<'a> for CursorUse<'a> {
    fn visit_body(&mut self, body: &'a [Stmt]) {
        self.leaving.push(leaves(body));
        visitor::walk_body(self, body);
        self.leaving.pop();
    }

    /// A class defined in the function is checked as a class of its own.
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::ClassDef(_) => return,
            Stmt::FunctionDef(f) if self.nested => {
                return self.nest(Some(&*f.parameters), |s| visitor::walk_stmt(s, stmt));
            }
            Stmt::FunctionDef(_) => return,
            Stmt::If(i) => return self.branch(i),
            Stmt::Assign(a) => a.targets.iter().for_each(|t| self.assign(t, &a.value)),
            Stmt::AnnAssign(ast::StmtAnnAssign {
                target,
                value: Some(value),
                ..
            }) => self.assign(target, value),
            Stmt::With(w) => self.with(w),
            _ => {}
        }
        visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Lambda(l) if self.nested => {
                return self.nest(l.parameters.as_deref(), |s| visitor::walk_expr(s, expr));
            }
            Expr::Lambda(_) => return,
            Expr::Call(call) => self.call(call),
            _ => {}
        }
        visitor::walk_expr(self, expr);
    }
}

/// The verb and the table of a query that starts, after comments, with `INSERT INTO`,
/// `UPDATE [ONLY]` or `DELETE FROM [ONLY]` and a plain or double-quoted table name. A name followed
/// by anything that may continue it, such as a placeholder or a schema, is no name.
pub(crate) fn write_target(text: &str) -> Option<(&'static str, &str)> {
    target(text).map(|(verb, table, _)| (verb, table))
}

/// The verb, the table, and the text after the table.
fn target(text: &str) -> Option<(&'static str, &str, &str)> {
    let mut rest = text;
    let verb = verb(&mut rest)?;
    if verb != "INSERT" {
        skip_only(&mut rest);
    }
    let (table, after) = name(skip_blank(rest))?;
    let ends = after
        .chars()
        .next()
        .is_none_or(|c| c.is_whitespace() || c == '(' || c == ';');
    ends.then_some((verb, table, after))
}

/// Consumes the words that open a write statement, and returns its verb.
fn verb(rest: &mut &str) -> Option<&'static str> {
    match word(rest)?.to_ascii_uppercase().as_str() {
        "INSERT" => expect(rest, "INTO").then_some("INSERT"),
        "UPDATE" => Some("UPDATE"),
        "DELETE" => expect(rest, "FROM").then_some("DELETE"),
        _ => None,
    }
}

/// Consumes the `ONLY` that may follow `UPDATE` or `DELETE FROM`.
fn skip_only(rest: &mut &str) {
    let mut after_only = *rest;
    if word(&mut after_only).is_some_and(|w| w.eq_ignore_ascii_case("ONLY")) {
        *rest = after_only;
    }
}

/// The columns an `UPDATE` sets, when its `SET` list names each one plainly and every value in it
/// can be read to its end: None for another verb, `SET (a, b) = ...`, a placeholder for a column,
/// or a value an f-string fills.
pub(crate) fn set_columns(text: &str) -> Option<Vec<&str>> {
    let (verb, _, mut rest) = target(text)?;
    if verb != "UPDATE" || !skip_alias(&mut rest) {
        return None;
    }
    let mut columns = vec![];
    loop {
        let (column, after) = name(skip_blank(rest))?;
        columns.push(column);
        let (more, after) = skip_value(skip_blank(after).strip_prefix('=')?)?;
        if !more {
            return Some(columns);
        }
        rest = after;
    }
}

/// Consumes an optional alias, with or without `AS`, and the `SET` after it.
fn skip_alias(rest: &mut &str) -> bool {
    let mut next = word(rest);
    if next.is_some_and(|w| w.eq_ignore_ascii_case("AS")) {
        word(rest);
        next = word(rest);
    } else if next.is_some_and(|w| !w.eq_ignore_ascii_case("SET")) {
        next = word(rest);
    }
    next.is_some_and(|w| w.eq_ignore_ascii_case("SET"))
}

/// Skips the value of a `SET` assignment: whether a comma follows it, with the text after the
/// comma. None where the value stops being readable: an f-string value or unbalanced brackets or
/// quotes.
fn skip_value(text: &str) -> Option<(bool, &str)> {
    let (mut depth, mut rest) = (0u32, text);
    while !rest.is_empty() {
        let token = token(rest)?;
        rest = &rest[token.len()..];
        if depth == 0 && (token == "," || AFTER_SET.iter().any(|k| token.eq_ignore_ascii_case(k))) {
            return Some((token == ",", rest));
        }
        depth = nest(depth, token)?;
    }
    (depth == 0).then_some((false, ""))
}

/// The bracket depth after a token, None past a closing bracket too many or at the `{` that ends
/// the literal head of an f-string.
fn nest(depth: u32, token: &str) -> Option<u32> {
    match token {
        "(" => Some(depth + 1),
        ")" => depth.checked_sub(1),
        "{" => None,
        _ => Some(depth),
    }
}

/// The token a non-empty text starts with: a quoted literal or name, a word, or one character.
/// None for a quote that never closes.
fn token(text: &str) -> Option<&str> {
    let first = text.chars().next()?;
    let len = match first {
        '\'' | '"' => text[1..].find(first)? + 2,
        c if is_name_char(c) => text.find(|c: char| !is_name_char(c)).unwrap_or(text.len()),
        c => c.len_utf8(),
    };
    Some(&text[..len])
}

/// The text of a query given as a literal, `SQL(literal, ...)`, `literal % ...`,
/// `literal.format(...)`, or the literal head of an f-string, which ends in `{` where the first
/// value goes.
pub(crate) fn query_text(expr: &Expr) -> Option<String> {
    match expr {
        Expr::StringLiteral(s) => Some(s.value.to_str().to_owned()),
        Expr::Call(c) if tail_name(&c.func) == Some("SQL") => c.arguments.args.first().and_then(query_text),
        Expr::Call(c) => match &*c.func {
            Expr::Attribute(a) if a.attr.as_str() == "format" => str_lit(&a.value).map(str::to_owned),
            _ => None,
        },
        Expr::BinOp(b) if b.op == Operator::Mod => str_lit(&b.left).map(str::to_owned),
        Expr::FString(f) => Some(fstring_head(f)),
        _ => None,
    }
}

/// The literal parts of an f-string, implicitly concatenated plain strings among them, up to the
/// first value.
fn fstring_head(f: &ast::ExprFString) -> String {
    let mut head = String::new();
    for part in f.value.iter() {
        let elements = match part {
            ast::FStringPartRef::Literal(l) => {
                head.push_str(&l.value);
                continue;
            }
            ast::FStringPartRef::FString(f) => &f.elements,
        };
        for element in elements {
            match element {
                ast::InterpolatedStringElement::Literal(l) => head.push_str(&l.value),
                ast::InterpolatedStringElement::Interpolation(_) => {
                    head.push('{');
                    return head;
                }
            }
        }
    }
    head
}

/// Whitespace, `-- line` and `/* block */` comments.
fn skip_blank(mut text: &str) -> &str {
    loop {
        text = text.trim_start();
        if let Some(rest) = text.strip_prefix("--") {
            text = rest.split_once('\n').map_or("", |(_, after)| after);
        } else if let Some(rest) = text.strip_prefix("/*") {
            text = rest.split_once("*/").map_or("", |(_, after)| after);
        } else {
            return text;
        }
    }
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The next word after blanks, consumed from `text`.
fn word<'t>(text: &mut &'t str) -> Option<&'t str> {
    let rest = skip_blank(text);
    let end = rest.find(|c: char| !is_name_char(c)).unwrap_or(rest.len());
    let (word, after) = rest.split_at(end);
    *text = after;
    (!word.is_empty()).then_some(word)
}

fn expect(text: &mut &str, keyword: &str) -> bool {
    word(text).is_some_and(|w| w.eq_ignore_ascii_case(keyword))
}

/// A plain or double-quoted name at the start of the text, and the text after it.
fn name(text: &str) -> Option<(&str, &str)> {
    let (name, after) = match text.strip_prefix('"') {
        Some(quoted) => {
            let end = quoted.find('"')?;
            (&quoted[..end], &quoted[end + 1..])
        }
        None if text.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') => {
            let end = text.find(|c: char| !is_name_char(c)).unwrap_or(text.len());
            text.split_at(end)
        }
        None => return None,
    };
    (!name.is_empty()).then_some((name, after))
}

#[cfg(test)]
mod tests {
    use ruff_python_parser::{parse_expression, parse_module};

    use super::*;

    /// What the scan of a function whose body is `body` finds, through `check`.
    fn scanned<T>(body: &str, nested: bool, check: impl FnOnce(&CursorUse) -> T) -> T {
        let src = format!("def f(self, cr):\n    {}\n", body.replace('\n', "\n    "));
        let parsed = parse_module(&src).expect("valid Python");
        let Stmt::FunctionDef(func) = &parsed.syntax().body[0] else {
            panic!("not a function")
        };
        let uses = if nested {
            CursorUse::scan_nested(&func.body)
        } else {
            CursorUse::scan(&func.body)
        };
        check(&uses)
    }

    fn reported_rollbacks(body: &str) -> usize {
        scanned(body, false, |uses| uses.rollbacks.len())
    }

    #[test]
    fn counts_rollbacks_the_code_goes_on_after() {
        assert_eq!(reported_rollbacks("cr.rollback()"), 1);
        assert_eq!(
            reported_rollbacks("try:\n    pass\nexcept E:\n    self.env.cr.rollback()\n    return"),
            1
        );
        assert_eq!(
            reported_rollbacks("if a or self.env.cr.readonly:\n    self.env.cr.rollback()"),
            1
        );
        assert_eq!(reported_rollbacks("def g():\n    cr.rollback()"), 0);
        assert_eq!(reported_rollbacks("self._cr.rollback()\nraise E()"), 0);
        assert_eq!(reported_rollbacks("self.env.cr.rollback()\nsys.exit(1)"), 0);
        assert_eq!(
            reported_rollbacks("if a and self.env.cr.readonly:\n    self.env.cr.rollback()"),
            0
        );
        assert_eq!(
            reported_rollbacks("with self.pool.cursor() as cr:\n    cr.rollback()"),
            0
        );
        assert_eq!(reported_rollbacks("cr = self.env.cr\ncr.rollback()"), 1);
        assert_eq!(reported_rollbacks("savepoint.rollback()"), 0);
    }

    #[test]
    fn a_cursor_the_function_opens_reaches_the_environments_built_on_it() {
        for body in [
            "with self.pool.cursor() as new_cr:\n    env = api.Environment(new_cr, uid, {})\n    env.cr.rollback()",
            "with self.pool.cursor() as new_cr:\n    self = self.with_env(self.env(cr=new_cr))\n    self.env.cr.rollback()",
            "with environment() as env0:\n    env0.cr.rollback()",
            "new_cr = registry.cursor()\nenv = api.Environment(new_cr, 1, {})\nenv['x'].sudo().env.cr.rollback()",
            "new_cr = registry.cursor()\ncr = api.Environment(new_cr, 1, {}).cr\ncr.rollback()",
            "new_cr = registry.cursor()\nenv = self.env(cr=new_cr)\nenv(user=1).cr.rollback()",
        ] {
            assert_eq!(reported_rollbacks(body), 0, "{body}");
        }
        for body in [
            "env = self.env\nenv.cr.rollback()",
            "env = api.Environment(self.env.cr, 1, {})\nenv.cr.rollback()",
            "with self.pool.cursor() as new_cr:\n    pass\nself.env.cr.rollback()",
            "self.env.cr.rollback()\nwith self.pool.cursor() as cr:\n    self = self.with_env(self.env(cr=cr))",
        ] {
            assert_eq!(reported_rollbacks(body), 1, "{body}");
        }
    }

    #[test]
    fn notes_invalidations_helpers_and_new_columns() {
        let invalidates = |body: &str| scanned(body, true, |uses| uses.invalidates);
        assert!(invalidates("self.env['x'].invalidate_model()"));
        assert!(invalidates("self.env.cache.invalidate()"));
        assert!(invalidates("def g():\n    self.invalidate_recordset()"));
        assert!(!invalidates("self.flush_model()"));
        assert!(!invalidates("self.invalidate()"));
        assert!(!scanned("def g():\n    self.invalidate_recordset()", false, |uses| {
            uses.invalidates
        }));
        assert_eq!(
            scanned("self._x()\ncls._y()\nother._z()", true, |uses| uses
                .own_methods
                .join(" ")),
            "_x _y"
        );
    }

    #[test]
    fn a_new_column_covers_only_the_writes_on_it() {
        let covered = |body: &str| scanned(body, true, |uses| uses.writes.iter().all(|w| uses.adds_columns_of(w)));
        assert!(covered(
            "create_column(cr, 't', 'c', 'int4')\ncr.execute('UPDATE t SET c = 1')"
        ));
        assert!(covered(
            "sql.create_column(cr, 't', c, 'int4')\ncr.execute('UPDATE t SET d = 1')"
        ));
        assert!(covered(
            "create_column(cr, 't', 'c', 'int4')\ncr.execute(SQL('UPDATE t SET %s = 1', c))"
        ));
        assert!(!covered(
            "create_column(cr, 'u', 'c', 'int4')\ncr.execute('UPDATE t SET c = 1')"
        ));
        assert!(!covered(
            "create_column(cr, 't', 'c', 'int4')\ncr.execute('UPDATE t SET c = 1, d = 2')"
        ));
        assert!(!covered(
            "create_column(cr, table, 'c', 'int4')\ncr.execute('UPDATE t SET c = 1')"
        ));
        assert!(!covered("cr.execute('UPDATE t SET c = 1')"));
    }

    #[test]
    fn finds_writes_on_the_cursor_or_through_the_environment() {
        let tables = |body: &str| {
            scanned(body, true, |uses| {
                uses.writes
                    .iter()
                    .map(|w| format!("{} {}", w.verb, w.table))
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(tables("cr.execute('DELETE FROM a')"), ["DELETE a"]);
        assert_eq!(
            tables("self.env.execute_query(SQL('INSERT INTO b VALUES (1)'))"),
            ["INSERT b"]
        );
        assert!(tables("other.execute('DELETE FROM a')").is_empty());
        assert!(tables("with registry.cursor() as cr:\n    cr.execute('DELETE FROM a')").is_empty());
        assert!(
            tables("with registry.cursor() as cr:\n    api.Environment(cr, 1, {}).execute_query(SQL('DELETE FROM a'))")
                .is_empty()
        );
    }

    #[test]
    fn a_closure_binds_its_own_names_and_a_class_is_left_out() {
        let tables = |body: &str| scanned(body, true, |uses| uses.writes.len());
        let closure_opens = "cr = self.env.cr\ndef log():\n    with self.pool.cursor() as cr:\n        \
                             cr.execute('INSERT INTO a VALUES (1)')\ncr.execute('UPDATE b SET c = 1')";
        assert_eq!(tables(closure_opens), 1);
        let param_hides = "cr = self.pool.cursor()\ndef g(cr):\n    cr.execute('UPDATE b SET c = 1')";
        assert_eq!(tables(param_hides), 1);
        let reads_outer = "cr = self.pool.cursor()\ndef g():\n    cr.execute('UPDATE b SET c = 1')";
        assert_eq!(tables(reads_outer), 0);
        assert_eq!(
            tables("class Inner:\n    def g(self):\n        self.env.cr.execute('UPDATE b SET c = 1')"),
            0
        );
    }

    #[test]
    fn reads_the_verb_and_table_of_a_write() {
        assert_eq!(
            write_target("UPDATE res_partner SET x = 1"),
            Some(("UPDATE", "res_partner"))
        );
        assert_eq!(write_target("update only x_data set a = 1"), Some(("UPDATE", "x_data")));
        assert_eq!(
            write_target("\n  INSERT INTO x_rel(a, b) VALUES (1, 2)"),
            Some(("INSERT", "x_rel"))
        );
        assert_eq!(
            write_target("DELETE FROM ONLY \"x data\" WHERE id = 1"),
            Some(("DELETE", "x data"))
        );
        assert_eq!(write_target("DELETE FROM x_log"), Some(("DELETE", "x_log")));
        assert_eq!(
            write_target("-- reset\n/* all of them */ UPDATE only_rows SET a = 1"),
            Some(("UPDATE", "only_rows"))
        );
    }

    #[test]
    fn reads_the_columns_an_update_sets() {
        let columns = |text: &str| set_columns(text).map(|c| c.join(" "));
        assert_eq!(columns("UPDATE t SET a = 1").as_deref(), Some("a"));
        assert_eq!(
            columns("update t as x set \"a\"=coalesce(b, 'x, y'), c = (1, 2) where id = 1").as_deref(),
            Some("a c")
        );
        assert_eq!(columns("UPDATE t x SET a = %s FROM u WHERE b, c").as_deref(), Some("a"));
        assert_eq!(columns("UPDATE t SET a = 1 RETURNING id, b").as_deref(), Some("a"));
        for text in [
            "DELETE FROM t",
            "UPDATE t SET (a, b) = (1, 2)",
            "UPDATE t SET %s = 1",
            "UPDATE t SET a = {",
            "UPDATE t SET a = (1",
            "UPDATE t SET a = 'x",
            "UPDATE t SET a",
            "UPDATE t WHERE a = 1",
        ] {
            assert_eq!(set_columns(text), None, "{text}");
        }
    }

    #[test]
    fn a_read_an_unknown_name_or_a_cte_is_no_write() {
        for text in [
            "SELECT id FROM res_partner",
            "WITH t AS (SELECT 1) UPDATE x SET a = 1",
            "UPDATE %s SET a = 1",
            "UPDATE {} SET a = 1",
            "UPDATE x_{",
            "UPDATE public.x SET a = 1",
            "INSERT x VALUES (1)",
            "DELETE x",
            "UPDATE \"x",
            "-- only a comment",
            "",
        ] {
            assert_eq!(write_target(text), None, "{text}");
        }
    }

    #[test]
    fn reads_the_text_the_code_spells_out() {
        let text = |src: &str| query_text(parse_expression(src).unwrap().expr());
        assert_eq!(text("'UPDATE a SET b = 1'").as_deref(), Some("UPDATE a SET b = 1"));
        assert_eq!(
            text("SQL('DELETE FROM a WHERE id = %s', 1)").as_deref(),
            Some("DELETE FROM a WHERE id = %s")
        );
        assert_eq!(text("'UPDATE %s' % t").as_deref(), Some("UPDATE %s"));
        assert_eq!(text("'UPDATE {}'.format(t)").as_deref(), Some("UPDATE {}"));
        assert_eq!(
            text("f'UPDATE a SET b = {v} WHERE c'").as_deref(),
            Some("UPDATE a SET b = {")
        );
        assert_eq!(text("query"), None);
        assert_eq!(text("build()"), None);
    }
}
