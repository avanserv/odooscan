//! What a function does with database cursors: the rollbacks, commits and cache invalidations it
//! calls, and the tables its literal write statements target, the CTE writes of a `WITH` query
//! among them. `QueryVars` reads a query held in a name the function binds once, and `parse`
//! reads a query's text into its writes and the columns it names on each table.

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, BoolOp, Expr, Operator, Stmt};
use ruff_text_size::TextSize;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::python::{SAME_MODEL, exits, is_cursor, str_lit, tail_name};

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

/// Words after the `ADD` of `ALTER TABLE` that add a constraint, not a column.
const ADDED_CONSTRAINTS: &[&str] = &["CONSTRAINT", "PRIMARY", "UNIQUE", "CHECK", "FOREIGN", "EXCLUDE"];

/// A literal write statement a call runs.
pub(crate) struct Write<'a> {
    pub call: &'a ast::ExprCall,
    pub verb: &'static str,
    pub table: String,
    /// The columns an `UPDATE` sets, when the text names each of them.
    pub columns: Option<Vec<String>>,
}

/// What a name the function binds holds, as far as cursors go.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Held {
    /// The cursor the function was given, or a value read from it.
    Given,
    /// A cursor the function opens, or a value that is no cursor at all.
    Cursor,
    /// An environment, or records, on a cursor the function opens.
    Env,
    /// What a context manager yields, which may be either.
    Either,
    /// `None` or `False`, no cursor at all; code tests such a name before it uses it, so at a meet
    /// the other path decides.
    Unset,
}

impl Held {
    /// What a name holds after two paths that bound it to `self` and to `other`: an own cursor or
    /// environment only when both paths may hold one.
    fn meet(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unset, held) | (held, Self::Unset) => held,
            (Self::Either, held) | (held, Self::Either) => held,
            (Self::Cursor, Self::Cursor) => Self::Cursor,
            (Self::Env, Self::Env) => Self::Env,
            _ => Self::Given,
        }
    }
}

/// A name, what it holds, and whether the value comes from a `.cursor()` call.
type Bound<'a> = (&'a str, Held, bool);

/// What two paths leave in a name, `opened` taken from the path that is not unset.
fn meet((held, opened): (Held, bool), (other, other_opened): (Held, bool)) -> (Held, bool) {
    match (held, other) {
        (Held::Unset, _) => (other, other_opened),
        (_, Held::Unset) => (held, opened),
        _ => (held.meet(other), opened && other_opened),
    }
}

/// A function the scanned one defines, the frames on the paths reaching its `def`, and the length
/// of `bound` then.
type Def<'a> = (&'a Stmt, Vec<Vec<Bound<'a>>>, usize);

/// A loop being visited: the length of the innermost frame before it, and the bindings since then
/// on the paths leaving its body by `break` and by `continue`, wherever they sit in it.
#[derive(Default)]
struct Loop<'a> {
    start: usize,
    breaks: Vec<Vec<Bound<'a>>>,
    continues: Vec<Vec<Bound<'a>>>,
}

/// How many findings and bindings a scan holds, to drop those of a run of a loop body that the
/// next run, from a narrower loop head, replaces.
struct Mark {
    rollbacks: usize,
    commits: bool,
    own_commits: usize,
    invalidates: bool,
    created: usize,
    writes: usize,
    own_methods: usize,
    defs: usize,
    bound: usize,
}

/// What a function body does with cursors, with or without the functions and lambdas it defines.
/// Rollbacks and queries on a cursor the function opens, a transaction of its own, are left out;
/// commits on a cursor it opens with a `.cursor()` call are kept apart, for `opened()`.
pub(crate) struct CursorUse<'a> {
    nested: bool,
    /// Rollbacks the code goes on after, outside a test of whether the cursor is read-only.
    pub rollbacks: Vec<&'a ast::ExprCall>,
    /// Whether it commits a cursor it did not open with a `.cursor()` call.
    pub commits: bool,
    /// The commits on cursors it opens with a `.cursor()` call.
    own_commits: Vec<&'a ast::ExprCall>,
    pub invalidates: bool,
    /// The table and the column of each `create_column()` call, when they are literals.
    created: Vec<(Option<&'a str>, Option<&'a str>)>,
    pub writes: Vec<Write<'a>>,
    /// The methods it calls on `self` or `cls`.
    pub own_methods: Vec<&'a str>,
    vars: QueryVars<'a>,
    /// The names each function binds on the paths reaching the statement visited, innermost last;
    /// a later binding shadows an earlier one. The flag says the value comes from a `.cursor()`
    /// call, not from the guess `Held` may hold.
    scopes: Vec<Vec<Bound<'a>>>,
    /// Every binding of the visited function's own frame in source order: a closure may run after
    /// any of those that follow its `def`.
    bound: Vec<Bound<'a>>,
    defs: Vec<Def<'a>>,
    loops: Vec<Loop<'a>>,
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
    fn new(nested: bool, func: &'a ast::StmtFunctionDef, mut scopes: Vec<Vec<Bound<'a>>>) -> Self {
        let params = func.parameters.iter().map(|p| (p.name().as_str(), Held::Given, false));
        scopes.push(params.collect());
        Self {
            nested,
            rollbacks: vec![],
            commits: false,
            own_commits: vec![],
            invalidates: false,
            created: vec![],
            writes: vec![],
            own_methods: vec![],
            vars: QueryVars::of(func),
            bound: vec![],
            defs: vec![],
            loops: vec![],
            scopes,
            leaving: vec![],
            read_only: 0,
        }
    }

    /// What the function does itself; each function it nests is scanned on its own, with the scan
    /// of the function that encloses it.
    pub(crate) fn scan(func: &'a ast::StmtFunctionDef, enclosing: Option<&CursorUse<'a>>) -> Self {
        let scopes = enclosing.map(|e| e.closure_scopes(func)).unwrap_or_default();
        let mut found = Self::new(false, func, scopes);
        found.visit_body(&func.body);
        found
    }

    /// What the function and the functions and lambdas it defines do, for code that hands its work
    /// to a closure.
    pub(crate) fn scan_nested(func: &'a ast::StmtFunctionDef) -> Self {
        let mut found = Self::new(true, func, vec![]);
        found.visit_body(&func.body);
        found.visit_closures();
        found
    }

    /// The frames a function the scanned one defines sees: those on the paths reaching its `def`,
    /// the bindings that follow the `def` shadowing them, since the closure may run after any. A
    /// function no `def` of the body defines, a method of a class in it, sees those at its end.
    fn closure_scopes(&self, func: &ast::StmtFunctionDef) -> Vec<Vec<Bound<'a>>> {
        let def = self
            .defs
            .iter()
            .find(|(stmt, ..)| matches!(stmt, Stmt::FunctionDef(f) if std::ptr::eq(f, func)));
        let Some((_, scopes, mark)) = def else {
            return self.scopes.clone();
        };
        let mut scopes = scopes.clone();
        if let Some(own) = scopes.last_mut() {
            own.extend_from_slice(&self.bound[*mark..]);
        }
        scopes
    }

    /// Visits each function the visited one defines, once the whole body has given its bindings,
    /// then the functions each of them defines.
    fn visit_closures(&mut self) {
        let stmts: Vec<&'a Stmt> = self.defs.iter().map(|(stmt, ..)| *stmt).collect();
        for stmt in stmts {
            let Stmt::FunctionDef(func) = stmt else { continue };
            let scopes = self.closure_scopes(func);
            let scopes = std::mem::replace(&mut self.scopes, scopes);
            let (defs, bound) = (std::mem::take(&mut self.defs), std::mem::take(&mut self.bound));
            self.nest(Some(&*func.parameters), |s| {
                visitor::walk_stmt(s, stmt);
                s.visit_closures();
            });
            (self.defs, self.bound, self.scopes) = (defs, bound, scopes);
        }
    }

    /// Whether `call` commits a cursor the function opens with a `.cursor()` call.
    pub(crate) fn opened(&self, call: &ast::ExprCall) -> bool {
        self.own_commits.iter().any(|own| std::ptr::eq(*own, call))
    }

    /// Whether the function adds, with `create_column()`, the columns an `UPDATE` sets, or a column
    /// of its table when the text does not tell which it sets: no field of the registry reads them
    /// yet. An `INSERT` or a `DELETE` changes whole rows, which records in the cache may hold.
    pub(crate) fn adds_columns_of(&self, write: &Write) -> bool {
        if write.verb != "UPDATE" {
            return false;
        }
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

    fn held(&self, name: &str) -> Option<(Held, bool)> {
        self.scopes
            .iter()
            .rev()
            .flat_map(|scope| scope.iter().rev())
            .find(|(bound, ..)| *bound == name)
            .map(|(_, held, opened)| (*held, *opened))
    }

    /// A cursor the function opens: a `.cursor()` call, a name it binds to one, or the cursor of an
    /// environment on one.
    fn own_cursor(&self, expr: &Expr) -> bool {
        self.owns_cursor(expr, false)
    }

    fn own_env(&self, expr: &Expr) -> bool {
        self.owns_env(expr, false)
    }

    /// With `opened`, only a cursor a `.cursor()` call returns counts, not a name `Held` guesses.
    fn owns_cursor(&self, expr: &Expr, opened: bool) -> bool {
        match expr {
            Expr::Name(n) => {
                matches!(self.held(n.id.as_str()), Some((Held::Cursor | Held::Either | Held::Unset, o)) if o || !opened)
            }
            Expr::Attribute(a) => is_cursor(expr) && self.owns_env(&a.value, opened),
            Expr::Call(c) => matches!(&*c.func, Expr::Attribute(a) if a.attr.as_str() == "cursor"),
            _ => false,
        }
    }

    /// An environment on a cursor the function opens, or records or an environment taken from one.
    fn owns_env(&self, expr: &Expr, opened: bool) -> bool {
        match expr {
            Expr::Name(n) if opened => matches!(self.held(n.id.as_str()), Some((Held::Env, true))),
            Expr::Name(n) => matches!(self.held(n.id.as_str()), Some((Held::Env | Held::Either, _))),
            Expr::Attribute(a) => a.attr.as_str() == "env" && self.owns_env(&a.value, opened),
            Expr::Subscript(s) => self.owns_env(&s.value, opened),
            Expr::Call(c) => self.owns_env_call(c, opened),
            _ => false,
        }
    }

    /// `api.Environment(cr, ...)` or `env(cr=cr)` on an own cursor, `with_env()` of an own
    /// environment, and records an own environment returns.
    fn owns_env_call(&self, call: &ast::ExprCall, opened: bool) -> bool {
        match &*call.func {
            Expr::Attribute(a) if a.attr.as_str() == "with_env" => {
                call.arguments.args.first().is_some_and(|e| self.owns_env(e, opened))
            }
            Expr::Attribute(a)
                if SAME_MODEL.contains(&a.attr.as_str()) || matches!(a.attr.as_str(), "browse" | "search") =>
            {
                self.owns_env(&a.value, opened)
            }
            func if matches!(tail_name(func), Some("Environment" | "env")) => {
                match call.arguments.find_argument_value("cr", 0) {
                    Some(cursor) => self.owns_cursor(cursor, opened),
                    None => self.owns_env(func, opened),
                }
            }
            _ => false,
        }
    }

    /// A statement binds into the innermost frame, which is the visited function's own, since a
    /// nested function is visited after the body that defines it, and a `lambda` binds no name.
    fn bind(&mut self, target: &'a Expr, held: Held, opened: bool) {
        if let (Expr::Name(n), Some(scope)) = (target, self.scopes.last_mut()) {
            scope.push((n.id.as_str(), held, opened));
            self.bound.push((n.id.as_str(), held, opened));
        }
    }

    fn assign(&mut self, target: &'a Expr, value: &Expr) {
        let (held, opened) = if self.own_cursor(value) {
            (Held::Cursor, self.owns_cursor(value, true))
        } else if is_cursor(value) {
            (Held::Given, false)
        } else if self.own_env(value) {
            (Held::Env, self.owns_env(value, true))
        } else if value.is_none_literal_expr() || matches!(value, Expr::BooleanLiteral(b) if !b.value) {
            (Held::Unset, false)
        } else {
            (Held::Cursor, false)
        };
        self.bind(target, held, opened);
    }

    /// Visits a nested function or lambda, whose parameters hide the names the function binds.
    fn nest(&mut self, parameters: Option<&'a ast::Parameters>, walk: impl FnOnce(&mut Self)) {
        let params = parameters
            .into_iter()
            .flat_map(|p| p.iter())
            .map(|p| (p.name().as_str(), Held::Given, false));
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
            "commit" if self.owns_cursor(receiver, true) => self.own_commits.push(call),
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

    /// The write a query runs, or for a query opening with `WITH`, the writes of its statement and
    /// of its CTEs.
    fn write(&mut self, call: &'a ast::ExprCall) {
        let Some(text) = call
            .arguments
            .args
            .first()
            .and_then(|arg| query_text(self.vars.value(arg)))
        else {
            return;
        };
        let targets = match write_target(&text) {
            Some((verb, table)) => vec![Target {
                verb,
                table: table.to_owned(),
                columns: set_columns(&text).map(|c| c.into_iter().map(str::to_owned).collect()),
            }],
            None if opens_with(&text) => parse(&text).map(|query| query.writes).unwrap_or_default(),
            None => vec![],
        };
        self.writes.extend(targets.into_iter().map(|t| Write {
            call,
            verb: t.verb,
            table: t.table,
            columns: t.columns,
        }));
    }

    fn with(&mut self, w: &'a ast::StmtWith) {
        for item in &w.items {
            if let Some(name) = &item.optional_vars {
                let given = is_cursor(&item.context_expr) && !self.own_cursor(&item.context_expr);
                let opened = self.owns_cursor(&item.context_expr, true);
                self.bind(name, if given { Held::Given } else { Held::Either }, opened);
            }
        }
    }

    /// Each arm runs from the bindings before the `if`, and one that returns, raises, exits, breaks
    /// or continues does not reach the statement after it. The body of `if <cursor>.readonly:`
    /// holds no rollback to report.
    fn branch(&mut self, i: &'a ast::StmtIf) {
        self.visit_expr(&i.test);
        let read_only = u32::from(tests_read_only(&i.test));
        self.read_only += read_only;
        let start = self.frame_len();
        let mut paths: Vec<_> = self.path(start, &i.body).into_iter().collect();
        self.read_only -= read_only;
        for clause in &i.elif_else_clauses {
            if let Some(test) = &clause.test {
                self.visit_expr(test);
            }
            paths.extend(self.path(start, &clause.body));
        }
        if i.elif_else_clauses.last().is_none_or(|c| c.test.is_some()) {
            paths.push(vec![]);
        }
        self.merge(&paths);
    }

    /// Each run of a loop body starts from the loop head: the meet of the bindings before the loop
    /// and of those at the end of a run and at each `continue`, narrowed until it holds still.
    /// Only the run from the final head counts. A `break` leaves with the bindings it sees; any
    /// other way out leaves from the head, through the `else`, except from `while True:`, which
    /// only a `break` leaves.
    fn repeat(&mut self, body: &'a [Stmt], orelse: &'a [Stmt], always: bool) {
        let start = self.frame_len();
        let mut head = vec![];
        let mut paths = loop {
            let mark = self.mark();
            self.loops.push(Loop {
                start,
                breaks: vec![],
                continues: vec![],
            });
            self.extend_frame(&head);
            let end = self.path(start, body);
            let Loop { breaks, continues, .. } = self.loops.pop().unwrap_or_default();
            let mut runs = vec![vec![], head.clone()];
            runs.extend(end);
            runs.extend(continues);
            let next = self.met(&runs);
            if next == head {
                break breaks;
            }
            self.undo(mark);
            head = next;
        };
        self.extend_frame(&head);
        let past = self.path(start, orelse);
        if !always {
            paths.extend(past);
        }
        self.merge(&paths);
    }

    /// Hands the loop the bindings since it began, on a `break` or a `continue` in its body or in
    /// a block within it, though not in a nested loop's body, which hands them to that loop.
    fn jump(&mut self, breaks: bool) {
        let (Some(lp), Some(frame)) = (self.loops.last_mut(), self.scopes.last()) else {
            return;
        };
        let path = frame.get(lp.start..).unwrap_or_default().to_vec();
        if breaks { &mut lp.breaks } else { &mut lp.continues }.push(path);
    }

    fn mark(&self) -> Mark {
        Mark {
            rollbacks: self.rollbacks.len(),
            commits: self.commits,
            own_commits: self.own_commits.len(),
            invalidates: self.invalidates,
            created: self.created.len(),
            writes: self.writes.len(),
            own_methods: self.own_methods.len(),
            defs: self.defs.len(),
            bound: self.bound.len(),
        }
    }

    fn undo(&mut self, mark: Mark) {
        self.rollbacks.truncate(mark.rollbacks);
        self.commits = mark.commits;
        self.own_commits.truncate(mark.own_commits);
        self.invalidates = mark.invalidates;
        self.created.truncate(mark.created);
        self.writes.truncate(mark.writes);
        self.own_methods.truncate(mark.own_methods);
        self.defs.truncate(mark.defs);
        self.bound.truncate(mark.bound);
    }

    fn frame_len(&self) -> usize {
        self.scopes.last().map_or(0, Vec::len)
    }

    fn extend_frame(&mut self, bindings: &[Bound<'a>]) {
        if let Some(frame) = self.scopes.last_mut() {
            frame.extend_from_slice(bindings);
        }
    }

    /// Visits a block and takes back the bindings the innermost frame gained since `start`, the
    /// path the statement after it sees unless the block does not get there.
    fn path(&mut self, start: usize, body: &'a [Stmt]) -> Option<Vec<Bound<'a>>> {
        self.visit_body(body);
        let added = self.scopes.last_mut()?.split_off(start);
        let ends = exits(body) || leaves(body) || matches!(body.last(), Some(Stmt::Break(_)));
        (!ends).then_some(added)
    }

    fn merge(&mut self, paths: &[Vec<Bound<'a>>]) {
        let merged = self.met(paths);
        self.extend_frame(&merged);
    }

    /// Binds each name a path rebinds to the meet of what the paths leave in it, an empty path
    /// keeping the bindings of the innermost frame. A path on which the frame never bound the name
    /// adds nothing: Python makes it local, so it is unbound there.
    fn met(&self, paths: &[Vec<Bound<'a>>]) -> Vec<Bound<'a>> {
        let frame = self.scopes.last().map_or(&[][..], Vec::as_slice);
        let last = |bindings: &[Bound<'a>], name: &str| {
            bindings
                .iter()
                .rev()
                .find(|(bound, ..)| *bound == name)
                .map(|(_, held, opened)| (*held, *opened))
        };
        let mut names: Vec<&'a str> = vec![];
        for &(name, ..) in paths.iter().flatten() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        let mut merged = vec![];
        for name in names {
            let before = last(frame, name);
            let reaching = paths.iter().filter_map(|path| last(path, name).or(before));
            if let Some((held, opened)) = reaching.reduce(meet) {
                merged.push((name, held, opened));
            }
        }
        merged
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
            Stmt::FunctionDef(_) => return self.defs.push((stmt, self.scopes.clone(), self.bound.len())),
            Stmt::If(i) => return self.branch(i),
            Stmt::For(f) => {
                self.visit_expr(&f.target);
                self.visit_expr(&f.iter);
                return self.repeat(&f.body, &f.orelse, false);
            }
            Stmt::While(w) => {
                self.visit_expr(&w.test);
                return self.repeat(
                    &w.body,
                    &w.orelse,
                    matches!(&*w.test, Expr::BooleanLiteral(b) if b.value),
                );
            }
            Stmt::Assign(a) => a.targets.iter().for_each(|t| self.assign(t, &a.value)),
            Stmt::AnnAssign(ast::StmtAnnAssign {
                target,
                value: Some(value),
                ..
            }) => self.assign(target, value),
            Stmt::With(w) => self.with(w),
            Stmt::Break(_) | Stmt::Continue(_) => self.jump(stmt.is_break_stmt()),
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

/// The names a function binds exactly once, with a plain `=`, and the value each holds: a query
/// passed by name is read from that value. A binding in a function it nests counts too, and any
/// other binding, such as a parameter, `+=` or a loop target, leaves the name without a value.
pub(crate) struct QueryVars<'a> {
    names: FxHashMap<&'a str, Option<&'a Expr>>,
}

impl<'a> QueryVars<'a> {
    pub(crate) fn of(func: &'a ast::StmtFunctionDef) -> Self {
        let mut vars = Self {
            names: FxHashMap::default(),
        };
        vars.visit_parameters(&func.parameters);
        vars.visit_body(&func.body);
        vars
    }

    /// The value of a name the function binds once, or else the expression itself.
    pub(crate) fn value(&self, expr: &'a Expr) -> &'a Expr {
        match expr {
            Expr::Name(n) => self.names.get(n.id.as_str()).copied().flatten().unwrap_or(expr),
            _ => expr,
        }
    }

    fn bind(&mut self, name: &'a str, value: Option<&'a Expr>) {
        self.names.entry(name).and_modify(|v| *v = None).or_insert(value);
    }

    fn bind_target(&mut self, target: &'a Expr, value: Option<&'a Expr>) {
        match target {
            Expr::Name(n) => self.bind(n.id.as_str(), value),
            Expr::Tuple(t) => t.elts.iter().for_each(|e| self.bind_target(e, None)),
            Expr::List(l) => l.elts.iter().for_each(|e| self.bind_target(e, None)),
            Expr::Starred(s) => self.bind_target(&s.value, None),
            _ => {}
        }
    }

    fn assigned(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Assign(a) => a.targets.iter().for_each(|t| self.bind_target(t, Some(&a.value))),
            Stmt::AnnAssign(ast::StmtAnnAssign {
                target,
                value: Some(value),
                ..
            }) => self.bind_target(target, Some(value)),
            Stmt::AugAssign(a) => self.bind_target(&a.target, None),
            Stmt::For(f) => self.bind_target(&f.target, None),
            Stmt::Delete(d) => d.targets.iter().for_each(|t| self.bind_target(t, None)),
            _ => self.declared(stmt),
        }
    }

    fn declared(&mut self, stmt: &'a Stmt) {
        let names: Vec<&'a str> = match stmt {
            Stmt::FunctionDef(f) => vec![f.name.as_str()],
            Stmt::ClassDef(c) => vec![c.name.as_str()],
            Stmt::Global(g) => g.names.iter().map(|n| n.as_str()).collect(),
            Stmt::Nonlocal(n) => n.names.iter().map(|n| n.as_str()).collect(),
            _ => vec![],
        };
        names.into_iter().for_each(|name| self.bind(name, None));
    }
}

impl<'a> Visitor<'a> for QueryVars<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        self.assigned(stmt);
        visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Named(n) = expr {
            self.bind_target(&n.target, None);
        }
        visitor::walk_expr(self, expr);
    }

    fn visit_comprehension(&mut self, comprehension: &'a ast::Comprehension) {
        self.bind_target(&comprehension.target, None);
        visitor::walk_comprehension(self, comprehension);
    }

    fn visit_except_handler(&mut self, handler: &'a ast::ExceptHandler) {
        let ast::ExceptHandler::ExceptHandler(h) = handler;
        if let Some(name) = &h.name {
            self.bind(name.as_str(), None);
        }
        visitor::walk_except_handler(self, handler);
    }

    fn visit_parameter(&mut self, parameter: &'a ast::Parameter) {
        self.bind(parameter.name.as_str(), None);
        visitor::walk_parameter(self, parameter);
    }

    fn visit_with_item(&mut self, item: &'a ast::WithItem) {
        if let Some(target) = &item.optional_vars {
            self.bind_target(target, Some(&item.context_expr));
        }
        visitor::walk_with_item(self, item);
    }

    fn visit_alias(&mut self, alias: &'a ast::Alias) {
        let name = alias.asname.as_ref().unwrap_or(&alias.name).as_str();
        self.bind(name.split('.').next().unwrap_or(name), None);
    }

    fn visit_pattern(&mut self, pattern: &'a ast::Pattern) {
        let name = match pattern {
            ast::Pattern::MatchAs(p) => p.name.as_ref(),
            ast::Pattern::MatchStar(p) => p.name.as_ref(),
            ast::Pattern::MatchMapping(p) => p.rest.as_ref(),
            _ => None,
        };
        if let Some(name) = name {
            self.bind(name.as_str(), None);
        }
        visitor::walk_pattern(self, pattern);
    }
}

/// Whether `expr` is a cursor on the database Odoo serves: the `_cr` of records, the `cr` of an
/// environment or of `request`, one a registry opens, `closing()` of one, `self.cr` where
/// `self_cr` says the class holds one there, or a name the function binds once to one of those. A
/// cursor the function is given, or opens on another connection, may read another database.
pub(crate) fn odoo_cursor<'a>(expr: &'a Expr, vars: Option<&QueryVars<'a>>, self_cr: bool) -> bool {
    match bound(expr, vars) {
        Expr::Attribute(a) if a.attr.as_str() == "_cr" => true,
        Expr::Attribute(a) if a.attr.as_str() == "cr" => odoo_env(&a.value, vars) || self_cr && is_self(&a.value),
        Expr::Call(c) if tail_name(&c.func) == Some("closing") => {
            c.arguments.args.first().is_some_and(|e| odoo_cursor(e, vars, self_cr))
        }
        Expr::Call(c) => registry_cursor(c),
        _ => false,
    }
}

/// The value a name holds when the function binds it once, followed a few names deep, since
/// `a = b` and `b = a` would loop.
fn bound<'a>(mut expr: &'a Expr, vars: Option<&QueryVars<'a>>) -> &'a Expr {
    for _ in 0..4 {
        expr = vars.map_or(expr, |v| v.value(expr));
    }
    expr
}

/// What holds Odoo's cursor as `cr`: `env`, the `env` of records, `request`, a call of an
/// environment such as `self.env(su=True)` or `api.Environment(...)`, or a name bound once to one.
fn odoo_env<'a>(expr: &'a Expr, vars: Option<&QueryVars<'a>>) -> bool {
    match bound(expr, vars) {
        Expr::Name(n) => matches!(n.id.as_str(), "env" | "request"),
        Expr::Attribute(a) => matches!(a.attr.as_str(), "env" | "request"),
        Expr::Call(c) => matches!(tail_name(&c.func), Some("env" | "Environment")),
        _ => false,
    }
}

fn is_self(expr: &Expr) -> bool {
    matches!(expr, Expr::Name(n) if matches!(n.id.as_str(), "self" | "cls"))
}

/// Whether the methods of a class read Odoo's cursor as `self.cr`: a test case, which a base
/// named like Odoo's `TransactionCase` or `HttpCase` makes it, or a class a method of which binds
/// `self.cr` to one. `unittest.TestCase` holds no cursor.
pub(crate) fn self_cursor(class: &ast::StmtClassDef) -> bool {
    let test_case = |base: &Expr| tail_name(base).is_some_and(|n| n.ends_with("Case") && n != "TestCase");
    class.bases().iter().any(test_case)
        || class
            .body
            .iter()
            .filter_map(Stmt::as_function_def_stmt)
            .any(binds_self_cursor)
}

fn binds_self_cursor(func: &ast::StmtFunctionDef) -> bool {
    let mut finder = SelfCursor {
        vars: QueryVars::of(func),
        found: false,
    };
    finder.visit_body(&func.body);
    finder.found
}

/// Finds a `self.cr = <Odoo cursor>` in a method.
struct SelfCursor<'a> {
    vars: QueryVars<'a>,
    found: bool,
}

impl<'a> Visitor<'a> for SelfCursor<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if let Stmt::Assign(a) = stmt {
            let to_self = |t: &Expr| matches!(t, Expr::Attribute(t) if t.attr.as_str() == "cr" && is_self(&t.value));
            self.found |= a.targets.iter().any(to_self) && odoo_cursor(&a.value, Some(&self.vars), false);
        }
        visitor::walk_stmt(self, stmt);
    }
}

/// `registry.cursor()`, `self.pool.cursor()` or `Registry(db).cursor()`.
fn registry_cursor(call: &ast::ExprCall) -> bool {
    let Expr::Attribute(a) = &*call.func else {
        return false;
    };
    let registry = match &*a.value {
        Expr::Call(c) => tail_name(&c.func),
        other => tail_name(other),
    };
    a.attr.as_str() == "cursor" && matches!(registry, Some("registry" | "Registry" | "pool"))
}

/// Whether a query opens, after comments, with `WITH`.
fn opens_with(text: &str) -> bool {
    let mut rest = text;
    word(&mut rest).is_some_and(|w| w.eq_ignore_ascii_case("WITH"))
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

/// The table and the column each `ADD [COLUMN] [IF NOT EXISTS] <name>` of an
/// `ALTER TABLE [IF EXISTS] [ONLY] <table>` query adds, with plain or double-quoted names. Empty
/// for another query, or a table followed by anything that may continue it, such as a schema.
pub(crate) fn added_columns(text: &str) -> Vec<(String, String)> {
    let mut rest = text;
    if !(expect(&mut rest, "ALTER") && expect(&mut rest, "TABLE")) {
        return vec![];
    }
    skip_words(&mut rest, &["IF", "EXISTS"]);
    skip_only(&mut rest);
    let Some((table, mut rest)) =
        folded_name(skip_blank(rest)).filter(|(_, after)| after.starts_with(char::is_whitespace))
    else {
        return vec![];
    };
    let mut added = vec![];
    loop {
        added.extend(added_column(rest).map(|column| (table.clone(), column)));
        match skip_value(rest) {
            Some((true, after)) => rest = after,
            _ => return added,
        }
    }
}

/// The column an `ADD` action of `ALTER TABLE` names, unless it adds a constraint.
fn added_column(mut rest: &str) -> Option<String> {
    if !expect(&mut rest, "ADD") {
        return None;
    }
    let mut next = rest;
    match word(&mut next) {
        Some(w) if w.eq_ignore_ascii_case("COLUMN") => rest = next,
        Some(w) if ADDED_CONSTRAINTS.iter().any(|c| w.eq_ignore_ascii_case(c)) => return None,
        _ => {}
    }
    skip_words(&mut rest, &["IF", "NOT", "EXISTS"]);
    folded_name(skip_blank(rest)).map(|(column, _)| column)
}

/// Consumes `words` when the text goes on with all of them.
fn skip_words(rest: &mut &str, words: &[&str]) {
    let mut next = *rest;
    if words.iter().all(|w| expect(&mut next, w)) {
        *rest = next;
    }
}

/// A name as PostgreSQL reads it, which folds an unquoted one to lowercase, and the text after it.
fn folded_name(text: &str) -> Option<(String, &str)> {
    let (name, after) = name(text)?;
    let folded = if text.starts_with('"') {
        name.to_owned()
    } else {
        name.to_ascii_lowercase()
    };
    Some((folded, after))
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
    query_source(expr).map(|source| source.text)
}

/// A query's text and the literal that spells it out.
pub(crate) struct QuerySource<'a> {
    pub text: String,
    pub literal: &'a Expr,
}

/// The text of a query as `query_text` reads it, with its literal.
pub(crate) fn query_source(expr: &Expr) -> Option<QuerySource<'_>> {
    match expr {
        Expr::StringLiteral(_) => literal_source(expr),
        Expr::Call(c) if tail_name(&c.func) == Some("SQL") => c.arguments.args.first().and_then(query_source),
        Expr::Call(c) => match &*c.func {
            Expr::Attribute(a) if a.attr.as_str() == "format" => literal_source(&a.value),
            _ => None,
        },
        Expr::BinOp(b) if b.op == Operator::Mod => literal_source(&b.left),
        Expr::FString(f) => Some(QuerySource {
            text: fstring_head(f),
            literal: expr,
        }),
        _ => None,
    }
}

/// The text a query starts with as far as literals spell it: the text `query_text` reads, or the
/// head of the left operand of a `+`, before `%` or `.format()` too, with a `{` for the value that
/// follows. A `SQL()` call is read where it is called.
pub(crate) fn query_head(expr: &Expr) -> Option<String> {
    match expr {
        Expr::BinOp(b) if b.op == Operator::Add => query_head(&b.left).map(|head| head + "{"),
        Expr::BinOp(b) if b.op == Operator::Mod => query_head(&b.left),
        Expr::Call(c) => match &*c.func {
            Expr::Attribute(a) if a.attr.as_str() == "format" => query_head(&a.value),
            _ => None,
        },
        _ => query_text(expr),
    }
}

fn literal_source(expr: &Expr) -> Option<QuerySource<'_>> {
    str_lit(expr).map(|text| QuerySource {
        text: text.to_owned(),
        literal: expr,
    })
}

/// Where byte `at` of a literal's text sits in the source, when the literal is a single part that
/// spells its text as is: an escape makes the source longer than the text.
pub(crate) fn source_offset(literal: &Expr, at: usize) -> Option<TextSize> {
    let Expr::StringLiteral(s) = literal else {
        return None;
    };
    let [part] = s.value.as_slice() else {
        return None;
    };
    let content = part.content_range();
    if content.len() != TextSize::of(&*part.value) {
        return None;
    }
    Some(content.start() + TextSize::try_from(at).ok()?)
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

/// The query levels a reference looks through for its qualifier, past which it is left unresolved.
const MAX_SCOPES: usize = 32;

/// Words that end a `FROM` or `USING` list.
const LIST_ENDS: &[&str] = &[
    "where",
    "group",
    "having",
    "order",
    "limit",
    "offset",
    "fetch",
    "window",
    "returning",
    "for",
    "do",
];

/// Words that follow a table in a `FROM` list or a write without being its alias.
const NOT_ALIASES: &[&str] = &[
    "on",
    "using",
    "join",
    "left",
    "right",
    "full",
    "inner",
    "cross",
    "natural",
    "outer",
    "where",
    "group",
    "having",
    "order",
    "limit",
    "offset",
    "fetch",
    "window",
    "union",
    "intersect",
    "except",
    "returning",
    "for",
    "set",
    "tablesample",
    "with",
    "do",
    "values",
    "select",
    "lateral",
    "and",
    "or",
    "not",
];

/// A write a parsed query runs on a table its text names.
pub(crate) struct Target {
    pub verb: &'static str,
    pub table: String,
    /// The columns an `UPDATE` sets, when the text names each of them.
    pub columns: Option<Vec<String>>,
}

/// A `table.column` or `alias.column` reference, on the table the query binds the qualifier to.
pub(crate) struct ColumnRef {
    pub table: String,
    pub column: String,
    /// The reference as written.
    pub text: String,
    /// The byte offset of the column in the text.
    pub at: usize,
}

/// What a query's text writes, and the columns it names on the tables it binds.
pub(crate) struct Query {
    pub writes: Vec<Target>,
    pub columns: Vec<ColumnRef>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Blank,
    /// An unquoted name or keyword.
    Word,
    /// A double-quoted name.
    Quoted,
    /// A string constant: `'..'`, `E'..'` or `$tag$..$tag$`.
    Literal,
    /// A value Python or PostgreSQL fills in: `%s`, `%(name)s`, `{..}` or `$1`, alone or in a
    /// double-quoted name.
    Placeholder,
    Open,
    Close,
    Other,
}

struct Token {
    kind: Kind,
    start: usize,
    end: usize,
    /// A word lowercased, or a quoted name unquoted.
    name: Option<String>,
}

/// The name an item of a `FROM` list or a write binds.
enum Binding {
    Unnamed,
    Named(String),
    /// A placeholder, which may be any name.
    Any,
}

/// A query level that binds names: a statement, a subquery, or a branch of a set operation. Each
/// name binds a table, or None for a subquery, a function, a placeholder or a CTE, whose columns
/// are unknown.
#[derive(Default)]
struct Scope {
    parent: Option<usize>,
    /// How many times each name is bound, with the table of its first binding.
    names: FxHashMap<String, (u32, Option<String>)>,
    /// Whether a placeholder binds a name, which may be any.
    any: bool,
}

/// The tokens of a query, None for a quote, comment or placeholder that never closes.
fn tokenize(text: &str) -> Option<Vec<Token>> {
    let (tokens, whole) = leading_tokens(text);
    whole.then_some(tokens)
}

/// The tokens of a text up to the first quote, comment or placeholder that never closes, such as
/// the `{` that ends the literal head of an f-string, and whether they cover the whole text.
fn leading_tokens(text: &str) -> (Vec<Token>, bool) {
    let mut tokens = vec![];
    let mut i = 0;
    while i < text.len() {
        let Some((kind, end)) = lex(text, i) else {
            return (tokens, false);
        };
        if kind != Kind::Blank {
            let name = token_name(kind, &text[i..end]);
            tokens.push(Token {
                kind,
                start: i,
                end,
                name,
            });
        }
        i = end;
    }
    (tokens, true)
}

fn token_name(kind: Kind, text: &str) -> Option<String> {
    match kind {
        Kind::Word => Some(text.to_ascii_lowercase()),
        Kind::Quoted => Some(text[1..text.len() - 1].replace("\"\"", "\"")),
        _ => None,
    }
}

/// The kind and the end of the token at byte `i`, which starts a character.
fn lex(text: &str, i: usize) -> Option<(Kind, usize)> {
    let bytes = text.as_bytes();
    match (bytes[i], bytes.get(i + 1).copied()) {
        (b'-', Some(b'-')) | (b'/', Some(b'*')) => Some((Kind::Blank, comment_end(text, i)?)),
        (b'e' | b'E', Some(b'\'')) => Some((Kind::Literal, quoted(bytes, i + 1, true)?)),
        (b'\'' | b'"' | b'$' | b'%' | b'{', _) => enclosed(text, i),
        _ => Some(plain(bytes, i)),
    }
}

/// The end of a `-- line` or `/* block */` comment.
fn comment_end(text: &str, i: usize) -> Option<usize> {
    if text.as_bytes()[i] == b'-' {
        return Some(text[i..].find('\n').map_or(text.len(), |e| i + e + 1));
    }
    Some(i + 2 + text[i + 2..].find("*/")? + 2)
}

/// A token that runs to a closing character: a string, a quoted name or a placeholder.
fn enclosed(text: &str, i: usize) -> Option<(Kind, usize)> {
    let bytes = text.as_bytes();
    match bytes[i] {
        b'\'' => Some((Kind::Literal, quoted(bytes, i, false)?)),
        b'"' => {
            let end = quoted(bytes, i, false)?;
            let filled = holds_placeholder(&text[i + 1..end - 1]);
            Some((if filled { Kind::Placeholder } else { Kind::Quoted }, end))
        }
        b'$' => dollar(text, i),
        b'%' => percent(text, i),
        _ => Some((Kind::Placeholder, i + text[i..].find('}')? + 1)),
    }
}

/// Whether a quoted name holds a `%s`, `%(name)s` or `{..}` Python fills in, which makes it any
/// name, as in `p."%s"`.
fn holds_placeholder(name: &str) -> bool {
    let bytes = name.as_bytes();
    name.contains('{')
        || name
            .match_indices('%')
            .any(|(at, _)| bytes.get(at + 1).is_some_and(|c| *c == b'(' || c.is_ascii_alphabetic()))
}

fn plain(bytes: &[u8], i: usize) -> (Kind, usize) {
    let word_end = || i + bytes[i..].iter().take_while(|c| is_word_byte(**c)).count();
    match bytes[i] {
        c if c.is_ascii_whitespace() => (Kind::Blank, i + 1),
        b'(' | b'[' => (Kind::Open, i + 1),
        b')' | b']' => (Kind::Close, i + 1),
        c if c.is_ascii_digit() => (Kind::Other, word_end()),
        c if is_word_byte(c) => (Kind::Word, word_end()),
        _ => (Kind::Other, i + 1),
    }
}

/// Bytes of an unquoted name: PostgreSQL takes `$` after the first and any letter beyond ASCII.
fn is_word_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || !c.is_ascii()
}

/// The end of the quoted text that opens at `open`, where a doubled quote, or a backslash in an
/// `E'..'` string, escapes the next character.
fn quoted(bytes: &[u8], open: usize, backslash: bool) -> Option<usize> {
    let quote = bytes[open];
    let mut j = open + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' if backslash => j += 2,
            c if c == quote && bytes.get(j + 1) == Some(&quote) => j += 2,
            c if c == quote => return Some(j + 1),
            _ => j += 1,
        }
    }
    None
}

/// A `$tag$..$tag$` string, a `$1` parameter, or a lone `$`.
fn dollar(text: &str, i: usize) -> Option<(Kind, usize)> {
    let bytes = text.as_bytes();
    let tag_end = i
        + 1
        + bytes[i + 1..]
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric() || **c == b'_')
            .count();
    let numbered = bytes.get(i + 1).is_some_and(u8::is_ascii_digit);
    match bytes.get(tag_end) {
        _ if numbered => Some((Kind::Placeholder, tag_end)),
        Some(b'$') => {
            let tag = &text[i..=tag_end];
            Some((Kind::Literal, tag_end + 1 + text[tag_end + 1..].find(tag)? + tag.len()))
        }
        _ => Some((Kind::Other, i + 1)),
    }
}

/// A `%s` or `%(name)s` placeholder of Python formatting, or `%%` and `%`, which are none.
fn percent(text: &str, i: usize) -> Option<(Kind, usize)> {
    let bytes = text.as_bytes();
    match bytes.get(i + 1) {
        Some(b'%') => Some((Kind::Other, i + 2)),
        Some(b'(') => {
            let close = i + text[i..].find(')')?;
            let typed = bytes.get(close + 1).is_some_and(u8::is_ascii_alphabetic);
            Some((Kind::Placeholder, close + 1 + usize::from(typed)))
        }
        Some(c) if c.is_ascii_alphabetic() => Some((Kind::Placeholder, i + 2)),
        _ => Some((Kind::Other, i + 1)),
    }
}

/// The closing token of each opening one, None when a bracket is left open or closes another kind.
fn pairs(tokens: &[Token], bytes: &[u8]) -> Option<Vec<usize>> {
    let mut close = vec![0; tokens.len()];
    let mut open = vec![];
    for (i, token) in tokens.iter().enumerate() {
        match token.kind {
            Kind::Open => open.push(i),
            Kind::Close => {
                let o = open.pop()?;
                if (bytes[tokens[o].start] == b'(') != (bytes[token.start] == b')') {
                    return None;
                }
                close[o] = i;
            }
            _ => {}
        }
    }
    open.is_empty().then_some(close)
}

/// Parses a query's text: its writes, and its qualified column references resolved to a table.
/// None when a quote, a comment, a placeholder or a bracket is left open. The text may hold
/// several statements; a level opening with anything but `SELECT`, `WITH`, `VALUES` or a write,
/// such as a fragment, binds no name.
pub(crate) fn parse(text: &str) -> Option<Query> {
    let tokens = tokenize(text)?;
    let close = pairs(&tokens, text.as_bytes())?;
    let mut parser = Parser {
        text,
        tables: vec![false; tokens.len()],
        tokens,
        close,
        ctes: FxHashSet::default(),
        scopes: vec![],
        refs: vec![],
        writes: vec![],
    };
    let mut levels = vec![(0, parser.tokens.len(), None)];
    while let Some((start, end, scope)) = levels.pop() {
        parser.level(start, end, scope, &mut levels);
    }
    Some(parser.query())
}

struct Parser<'t> {
    text: &'t str,
    tokens: Vec<Token>,
    /// The closing token of each opening one.
    close: Vec<usize>,
    /// Tokens that name a table, which are no column references.
    tables: Vec<bool>,
    /// The names the query's `WITH` clauses define, wherever they are.
    ctes: FxHashSet<String>,
    scopes: Vec<Scope>,
    /// The scope and the qualifier token of each `qualifier.column` reference.
    refs: Vec<(usize, usize)>,
    writes: Vec<Target>,
}

/// A level of brackets to parse: its first and end tokens, and the scope it sits in.
type Level = (usize, usize, Option<usize>);

impl Parser<'_> {
    fn keyword(&self, i: usize) -> Option<&str> {
        let token = self.tokens.get(i)?;
        (token.kind == Kind::Word).then_some(token.name.as_deref()?)
    }

    fn keyword_at(&self, segment: &[usize], k: usize) -> Option<&str> {
        self.keyword(*segment.get(k)?)
    }

    fn kind(&self, i: usize) -> Option<Kind> {
        self.tokens.get(i).map(|t| t.kind)
    }

    fn kind_at(&self, segment: &[usize], k: usize) -> Option<Kind> {
        self.kind(*segment.get(k)?)
    }

    fn is_char(&self, i: usize, c: u8) -> bool {
        self.tokens
            .get(i)
            .is_some_and(|t| t.kind == Kind::Other && t.end == t.start + 1 && self.text.as_bytes()[t.start] == c)
    }

    /// Whether token `j` starts where token `i` ends, as `x_` and `{}` do in `x_{}`.
    fn glued(&self, i: usize, j: usize) -> bool {
        self.tokens.get(j).is_some_and(|t| t.start == self.tokens[i].end)
    }

    /// Splits a level into the branches of its set operations and statements, each a scope of its
    /// own when it binds names.
    fn level(&mut self, start: usize, end: usize, parent: Option<usize>, levels: &mut Vec<Level>) {
        let mut segment = vec![];
        let mut i = start;
        while i < end {
            if matches!(self.keyword(i), Some("union" | "intersect" | "except")) || self.is_char(i, b';') {
                self.segment(&std::mem::take(&mut segment), parent, levels);
            } else {
                segment.push(i);
            }
            i = if self.tokens[i].kind == Kind::Open {
                self.close[i] + 1
            } else {
                i + 1
            };
        }
        self.segment(&segment, parent, levels);
    }

    /// A branch, as the tokens of its level with each bracket standing for its contents.
    fn segment(&mut self, segment: &[usize], parent: Option<usize>, levels: &mut Vec<Level>) {
        let scope = if self.binds(segment) {
            self.scopes.push(Scope {
                parent,
                ..Scope::default()
            });
            let scope = self.scopes.len() - 1;
            self.bind_segment(segment, scope);
            Some(scope)
        } else {
            parent
        };
        for &i in segment {
            if self.tokens[i].kind == Kind::Open {
                levels.push((i + 1, self.close[i], scope));
            } else if let Some(scope) = scope.filter(|_| self.is_ref(i)) {
                self.refs.push((scope, i));
            }
        }
    }

    fn binds(&self, segment: &[usize]) -> bool {
        let first = segment
            .iter()
            .find(|&&i| !matches!(self.keyword(i), Some("all" | "distinct")));
        first.is_some_and(|&i| {
            matches!(
                self.keyword(i),
                Some("select" | "with" | "values" | "insert" | "update" | "delete")
            )
        })
    }

    fn bind_segment(&mut self, segment: &[usize], scope: usize) {
        let mut k = self.skip_with(segment);
        k = self.statement(segment, k, scope);
        while k < segment.len() {
            k = match self.keyword_at(segment, k) {
                Some("from") if k == 0 || self.keyword_at(segment, k - 1) != Some("distinct") => {
                    self.table_list(segment, k + 1, scope)
                }
                Some("using") => self.table_list(segment, k + 1, scope),
                _ => k + 1,
            };
        }
    }

    /// Skips a `WITH` clause, noting the names it defines, and returns where its statement starts.
    fn skip_with(&mut self, segment: &[usize]) -> usize {
        let mut k = usize::from(matches!(self.keyword_at(segment, 0), Some("all" | "distinct")));
        if self.keyword_at(segment, k) != Some("with") {
            return k;
        }
        k += 1 + usize::from(self.keyword_at(segment, k + 1) == Some("recursive"));
        while let Some(name) = segment.get(k).and_then(|&i| self.tokens[i].name.clone()) {
            self.ctes.insert(name);
            k += 1 + usize::from(self.kind_at(segment, k + 1) == Some(Kind::Open));
            for word in ["as", "not", "materialized"] {
                k += usize::from(self.keyword_at(segment, k) == Some(word));
            }
            k += usize::from(self.kind_at(segment, k) == Some(Kind::Open));
            if !segment.get(k).is_some_and(|&i| self.is_char(i, b',')) {
                return k;
            }
            k += 1;
        }
        k
    }

    /// Binds the table an `INSERT INTO`, `UPDATE` or `DELETE FROM` at `k` writes, and notes the
    /// write. Returns where the rest of the statement starts.
    fn statement(&mut self, segment: &[usize], k: usize, scope: usize) -> usize {
        let (verb, mut at) = match (self.keyword_at(segment, k), self.keyword_at(segment, k + 1)) {
            (Some("insert"), Some("into")) => ("INSERT", k + 2),
            (Some("update"), _) => ("UPDATE", k + 1),
            (Some("delete"), Some("from")) => ("DELETE", k + 2),
            _ => return k,
        };
        at += usize::from(verb != "INSERT" && self.keyword_at(segment, at) == Some("only"));
        let Some((table, name, after)) = self.source(segment, at, false) else {
            return at;
        };
        let (alias, after) = self.alias(segment, after, verb != "INSERT");
        self.bind(scope, alias.unwrap_or(name), table.clone());
        if let Some(table) = table {
            if verb == "INSERT" {
                self.bind(scope, Binding::Named("excluded".to_owned()), Some(table.clone()));
            }
            let columns = if verb == "UPDATE" {
                self.set_list(segment, after)
            } else {
                None
            };
            self.writes.push(Target { verb, table, columns });
        }
        after
    }

    /// The columns of the `SET` list at `k`, when it names each one plainly: None for
    /// `SET (a, b) = ...` or a placeholder for a column. A bracket in a value is one token of the
    /// branch, so the list is read without what it holds.
    fn set_list(&self, segment: &[usize], mut k: usize) -> Option<Vec<String>> {
        if self.keyword_at(segment, k) != Some("set") {
            return None;
        }
        let mut columns = vec![];
        loop {
            let column = *segment.get(k + 1)?;
            let assigned = segment.get(k + 2).is_some_and(|&i| self.is_char(i, b'='));
            if !assigned || !matches!(self.kind(column), Some(Kind::Word | Kind::Quoted)) {
                return None;
            }
            columns.push(self.tokens[column].name.clone()?);
            match self.next_assignment(segment, k + 3) {
                Some(comma) => k = comma,
                None => return Some(columns),
            }
        }
    }

    /// The comma after the value at `k` of a `SET` list, None when the list ends first.
    fn next_assignment(&self, segment: &[usize], k: usize) -> Option<usize> {
        let ends =
            |i: &usize| self.is_char(*i, b',') || matches!(self.keyword(*i), Some("from" | "where" | "returning"));
        let at = k + segment.get(k..)?.iter().position(ends)?;
        self.is_char(segment[at], b',').then_some(at)
    }

    /// Binds each item of a `FROM` or `USING` list, and returns where the list ends.
    fn table_list(&mut self, segment: &[usize], mut k: usize, scope: usize) -> usize {
        loop {
            k = self.table_item(segment, k, scope);
            loop {
                let Some(&i) = segment.get(k) else {
                    return k;
                };
                if self.ends_list(i) {
                    return k;
                }
                k += 1;
                if self.is_char(i, b',') || self.keyword(i) == Some("join") {
                    break;
                }
            }
        }
    }

    fn ends_list(&self, i: usize) -> bool {
        self.keyword(i).is_some_and(|w| LIST_ENDS.contains(&w))
            || self.keyword(i) == Some("on") && self.keyword(i + 1) == Some("conflict")
    }

    fn table_item(&mut self, segment: &[usize], mut k: usize, scope: usize) -> usize {
        while matches!(self.keyword_at(segment, k), Some("lateral" | "only")) {
            k += 1;
        }
        let Some((mut table, name, mut after)) = self.source(segment, k, true) else {
            return k;
        };
        if self.keyword_at(segment, after) == Some("with") && self.keyword_at(segment, after + 1) == Some("ordinality")
        {
            after += 2;
        }
        let (alias, mut after) = self.alias(segment, after, true);
        if alias.is_some() && self.kind_at(segment, after) == Some(Kind::Open) {
            table = None;
            after += 1;
        }
        self.bind(scope, alias.unwrap_or(name), table);
        after
    }

    fn bind(&mut self, scope: usize, name: Binding, table: Option<String>) {
        let scope = &mut self.scopes[scope];
        match name {
            Binding::Unnamed => {}
            Binding::Named(name) => scope.names.entry(name).or_insert((0, table)).0 += 1,
            Binding::Any => scope.any = true,
        }
    }

    /// The table an item of a `FROM` list or a write names at `k`, the name it binds without an
    /// alias, and where it ends. A bracket, a placeholder, a function when `calls`, a CTE or a name
    /// in another schema than `public` is no table the text tells.
    fn source(&mut self, segment: &[usize], k: usize, calls: bool) -> Option<(Option<String>, Binding, usize)> {
        let i = *segment.get(k)?;
        match self.tokens[i].kind {
            Kind::Open => Some((None, Binding::Unnamed, k + 1)),
            Kind::Placeholder => Some((None, Binding::Any, self.glued_end(segment, k))),
            Kind::Word | Kind::Quoted => Some(self.named_source(segment, k, calls)),
            _ => None,
        }
    }

    /// Where a run of names and placeholders written without a space, such as `x_{}`, ends.
    fn glued_end(&self, segment: &[usize], mut k: usize) -> usize {
        while segment.get(k + 1).is_some_and(|&j| {
            self.glued(segment[k], j) && matches!(self.tokens[j].kind, Kind::Word | Kind::Quoted | Kind::Placeholder)
        }) {
            k += 1;
        }
        k + 1
    }

    fn named_source(&mut self, segment: &[usize], k: usize, calls: bool) -> (Option<String>, Binding, usize) {
        let mut parts = vec![];
        let mut end = k;
        loop {
            let i = segment[end];
            self.tables[i] = true;
            parts.push(self.tokens[i].name.clone().unwrap_or_default());
            let more = segment.get(end + 1).is_some_and(|&d| self.is_char(d, b'.'))
                && segment
                    .get(end + 2)
                    .is_some_and(|&n| matches!(self.tokens[n].kind, Kind::Word | Kind::Quoted));
            if !more {
                break;
            }
            end += 2;
        }
        let after = self.glued_end(segment, end);
        let last = parts.pop().unwrap_or_default();
        if after > end + 1 || segment.get(after).is_some_and(|&d| self.is_char(d, b'.')) {
            return (None, Binding::Any, after);
        }
        if calls && self.kind_at(segment, after) == Some(Kind::Open) {
            return (None, Binding::Named(last), after + 1);
        }
        let plain = parts.is_empty() || parts == ["public"];
        let table = (plain && !self.ctes.contains(&last)).then(|| last.clone());
        (table, Binding::Named(last), after)
    }

    /// The alias at `k`, after `AS` or, when `bare`, alone, and where it ends.
    fn alias(&self, segment: &[usize], k: usize, bare: bool) -> (Option<Binding>, usize) {
        let Some(&i) = segment.get(k) else {
            return (None, k);
        };
        if self.keyword(i) == Some("as") {
            return match self.alias_name(segment, k + 1) {
                Some((name, after)) => (Some(name), after),
                None => (None, k + 1),
            };
        }
        let reserved = self.keyword(i).is_some_and(|w| NOT_ALIASES.contains(&w));
        match self.alias_name(segment, k).filter(|_| bare && !reserved) {
            Some((name, after)) => (Some(name), after),
            None => (None, k),
        }
    }

    fn alias_name(&self, segment: &[usize], k: usize) -> Option<(Binding, usize)> {
        let i = *segment.get(k)?;
        let after = self.glued_end(segment, k);
        match self.tokens[i].kind {
            Kind::Word | Kind::Quoted if after == k + 1 => Some((Binding::Named(self.tokens[i].name.clone()?), after)),
            Kind::Word | Kind::Quoted | Kind::Placeholder => Some((Binding::Any, after)),
            _ => None,
        }
    }

    /// Whether token `i` qualifies a column, as `a` in `a.b`: neither part of a longer dotted name
    /// nor of a call, a table name, or written against a placeholder.
    fn is_ref(&self, i: usize) -> bool {
        let named = |j: usize| matches!(self.kind(j), Some(Kind::Word | Kind::Quoted));
        let dotted_before = i.checked_sub(1).is_some_and(|b| self.is_char(b, b'.'));
        named(i)
            && self.is_char(i + 1, b'.')
            && named(i + 2)
            && !self.tables[i]
            && !dotted_before
            && !self.is_char(i + 3, b'.')
            && self.kind(i + 3) != Some(Kind::Open)
            && !self.touches_placeholder(i)
    }

    fn touches_placeholder(&self, i: usize) -> bool {
        let before = i
            .checked_sub(1)
            .is_some_and(|b| self.kind(b) == Some(Kind::Placeholder) && self.glued(b, i));
        let after = self.kind(i + 3) == Some(Kind::Placeholder) && self.glued(i + 2, i + 3);
        before || after
    }

    /// The table `qualifier` names in `scope` or the nearest scope around it that binds it. None
    /// when it is unbound, bound twice in that scope, or bound to no table the text tells.
    fn resolve(&self, scope: usize, qualifier: &str) -> Option<&str> {
        let mut at = Some(scope);
        for _ in 0..MAX_SCOPES {
            let scope = &self.scopes[at?];
            match scope.names.get(qualifier) {
                _ if scope.any => return None,
                Some((1, table)) => return table.as_deref(),
                Some(_) => return None,
                None => at = scope.parent,
            }
        }
        None
    }

    fn query(self) -> Query {
        let columns = self
            .refs
            .iter()
            .filter_map(|&(scope, i)| {
                let (qualifier, column) = (&self.tokens[i], &self.tokens[i + 2]);
                Some(ColumnRef {
                    table: self.resolve(scope, qualifier.name.as_deref()?)?.to_owned(),
                    column: column.name.clone()?,
                    text: self.text[qualifier.start..column.end].to_owned(),
                    at: column.start,
                })
            })
            .collect();
        Query {
            writes: self.writes,
            columns,
        }
    }
}

/// Words that may sit between `CREATE` and `VIEW`.
const VIEW_PREFIXES: &[&str] = &["or", "replace", "temp", "temporary", "recursive", "materialized"];

/// Words that end the select list of a branch, or start the next branch.
const SELECT_ENDS: &[&str] = &[
    "from",
    "into",
    "where",
    "group",
    "having",
    "window",
    "order",
    "limit",
    "offset",
    "fetch",
    "for",
    "union",
    "intersect",
    "except",
];

/// Words that end no expression they follow, so the name after them is no bare alias, and words
/// that are no alias themselves.
const NOT_BARE_ALIASES: &[&str] = &[
    "all",
    "and",
    "any",
    "array",
    "as",
    "asc",
    "at",
    "between",
    "by",
    "case",
    "cast",
    "collate",
    "current_date",
    "current_time",
    "current_timestamp",
    "desc",
    "distinct",
    "else",
    "end",
    "escape",
    "false",
    "filter",
    "ilike",
    "in",
    "is",
    "isnull",
    "like",
    "localtime",
    "localtimestamp",
    "not",
    "notnull",
    "null",
    "or",
    "over",
    "similar",
    "then",
    "true",
    "when",
    "within",
];

/// Words that end an expression though `NOT_BARE_ALIASES` holds them, as in `CASE ... END total`.
const OPERAND_ENDS: &[&str] = &["end", "null", "true", "false"];

/// Keywords of constraint and index definitions, which name no column.
const DEFINITION_WORDS: &[&str] = &[
    "all",
    "and",
    "any",
    "array",
    "as",
    "asc",
    "asymmetric",
    "between",
    "both",
    "cascade",
    "case",
    "check",
    "collate",
    "constraint",
    "current_date",
    "current_time",
    "current_timestamp",
    "current_user",
    "default",
    "deferrable",
    "deferred",
    "desc",
    "distinct",
    "else",
    "end",
    "escape",
    "exclude",
    "exists",
    "false",
    "for",
    "foreign",
    "from",
    "ilike",
    "immediate",
    "in",
    "include",
    "initially",
    "is",
    "isnull",
    "leading",
    "like",
    "localtime",
    "localtimestamp",
    "match",
    "no",
    "not",
    "notnull",
    "null",
    "nulls",
    "on",
    "or",
    "overlaps",
    "placing",
    "primary",
    "references",
    "restrict",
    "session_user",
    "set",
    "similar",
    "some",
    "symmetric",
    "tablespace",
    "then",
    "trailing",
    "true",
    "unique",
    "using",
    "valid",
    "when",
    "where",
    "with",
];

/// Words of a definition that only `DEFINITION_WORDS` before them make keywords, by that word:
/// `key` is a column of `ir.ui.view`, so only `PRIMARY KEY` and `FOREIGN KEY` make it a keyword.
const FOLLOWING_WORDS: &[(&str, &[&str])] = &[
    ("primary", &["key"]),
    ("foreign", &["key"]),
    ("no", &["action", "inherit"]),
    ("is", &["unknown"]),
    ("not", &["unknown"]),
    ("with", &["time"]),
    ("on", &["delete", "update"]),
    ("match", &["full", "partial", "simple"]),
    ("nulls", &["first", "last"]),
    ("similar", &["to"]),
    ("at", &["time"]),
];

/// Words of a definition after which a name is no column: an index method, a collation, the table
/// a foreign key references, the type of a `CAST`, or a tablespace.
const NAMING_WORDS: &[&str] = &["using", "collate", "references", "as", "tablespace"];

/// A view a `CREATE VIEW` statement creates.
pub(crate) struct View {
    /// The view's name, None for a placeholder.
    pub table: Option<String>,
    /// The names of its columns, from its column list or else the select list of the first branch
    /// of its query: None when the text does not spell each one.
    pub names: Option<Vec<String>>,
}

/// The view the first `CREATE [OR REPLACE] [MATERIALIZED] VIEW` statement of a text creates. None
/// when the text has none, leaves a quote or a bracket open, or names a view of another schema.
pub(crate) fn view(text: &str) -> Option<View> {
    let tokens = tokenize(text)?;
    let close = pairs(&tokens, text.as_bytes())?;
    let reader = Reader {
        text,
        tokens: &tokens,
        close: &close,
    };
    let mut start = 0;
    while start < tokens.len() {
        if let Some(view) = (reader.keyword(start) == Some("create"))
            .then(|| reader.create_view(start + 1))
            .flatten()
        {
            return Some(view);
        }
        start = reader.statement_end(start) + 1;
    }
    None
}

/// The view each `CREATE [OR REPLACE] [MATERIALIZED] VIEW` statement of a text creates, read up to
/// its name, so that whatever follows may be unreadable, as in the literal head of an f-string or
/// of a concatenation: None for a name Python fills in or that the head stops before.
pub(crate) fn created_views(text: &str) -> Vec<Option<String>> {
    let (tokens, _) = leading_tokens(text);
    let reader = Reader {
        text,
        tokens: &tokens,
        close: &[],
    };
    (0..tokens.len())
        .filter(|&k| reader.keyword(k) == Some("create"))
        .filter_map(|k| reader.created_view(k + 1))
        .collect()
}

/// The names of the columns of a view with a column list: those of the list, then PostgreSQL
/// names the rest after the select items. None when an item it names has no name the text spells,
/// or the list is longer than the select list.
fn merged_names(listed: Vec<String>, items: Vec<Option<String>>) -> Option<Vec<String>> {
    let rest: Vec<String> = items.get(listed.len()..)?.iter().cloned().collect::<Option<_>>()?;
    Some(listed.into_iter().chain(rest).collect())
}

/// The columns a constraint or index definition names, such as `name` and `company_id` in
/// `UNIQUE (name, company_id)`, with their byte offsets. Keywords, calls, casts, typed literals,
/// qualified names, index methods, collations, operator classes, storage parameters, tablespaces
/// and the table and columns a foreign key references are no columns. None when a placeholder, a
/// quote or a bracket is left unread.
pub(crate) fn definition_columns(text: &str) -> Option<Vec<(String, usize)>> {
    let tokens = tokenize(text)?;
    let close = pairs(&tokens, text.as_bytes())?;
    if tokens.iter().any(|t| t.kind == Kind::Placeholder) {
        return None;
    }
    let reader = Reader {
        text,
        tokens: &tokens,
        close: &close,
    };
    let unread = reader.unread();
    let columns = (0..tokens.len())
        .filter(|&i| !unread[i] && reader.is_definition_column(i))
        .filter_map(|i| Some((tokens[i].name.clone()?, tokens[i].start)));
    Some(columns.collect())
}

/// Reads a statement's tokens, each bracket with the token that closes it.
struct Reader<'t> {
    text: &'t str,
    tokens: &'t [Token],
    close: &'t [usize],
}

impl Reader<'_> {
    fn keyword(&self, i: usize) -> Option<&str> {
        let token = self.tokens.get(i)?;
        (token.kind == Kind::Word).then_some(token.name.as_deref()?)
    }

    fn kind(&self, i: usize) -> Option<Kind> {
        self.tokens.get(i).map(|t| t.kind)
    }

    fn is_char(&self, i: usize, c: u8) -> bool {
        self.tokens
            .get(i)
            .is_some_and(|t| t.kind == Kind::Other && t.end == t.start + 1 && self.text.as_bytes()[t.start] == c)
    }

    /// The token after token `i` on its level: past the bracket `i` opens.
    fn next(&self, i: usize) -> usize {
        if self.kind(i) == Some(Kind::Open) {
            self.close[i] + 1
        } else {
            i + 1
        }
    }

    /// The `;` that ends the statement starting at `k`, or the end of the text.
    fn statement_end(&self, mut k: usize) -> usize {
        while k < self.tokens.len() && !self.is_char(k, b';') {
            k = self.next(k);
        }
        k
    }

    /// Where the name of the view a `CREATE` statement creates starts, read after its `CREATE`.
    /// None when the statement creates no view.
    fn view_start(&self, mut k: usize) -> Option<usize> {
        while self.keyword(k).is_some_and(|w| VIEW_PREFIXES.contains(&w)) {
            k += 1;
        }
        (self.keyword(k) == Some("view")).then(|| k + 1 + 3 * usize::from(self.keyword(k + 1) == Some("if")))
    }

    /// The view of a `CREATE` statement, read after its `CREATE`.
    fn create_view(&self, k: usize) -> Option<View> {
        let (table, mut k) = self.view_name(self.view_start(k)?)?;
        let listed = (self.kind(k) == Some(Kind::Open)).then(|| self.name_list(k));
        if listed.is_some() {
            k = self.next(k);
        }
        if self.keyword(k) == Some("with") {
            k = self.next(k + 1);
        }
        if self.keyword(k) != Some("as") {
            return None;
        }
        let items = self.query_items(k + 1, self.statement_end(k));
        let names = match listed {
            Some(listed) => listed
                .zip(items)
                .and_then(|(listed, items)| merged_names(listed, items)),
            None => items.and_then(|items| items.into_iter().collect()),
        };
        Some(View { table, names })
    }

    /// The name of a view at `k`, None for a placeholder or a name glued to one, and where it ends.
    /// A `public.` schema is read past; None for a name of another schema.
    fn view_name(&self, k: usize) -> Option<(Option<String>, usize)> {
        if self.name(k).as_deref() == Some("public") && self.is_char(k + 1, b'.') {
            return self.view_name(k + 2);
        }
        let token = self.tokens.get(k)?;
        let mut end = k + 1;
        while self.tokens.get(end).is_some_and(|t| {
            t.start == self.tokens[end - 1].end && matches!(t.kind, Kind::Word | Kind::Quoted | Kind::Placeholder)
        }) {
            end += 1;
        }
        if self.is_char(end, b'.') {
            return None;
        }
        match token.kind {
            Kind::Word | Kind::Quoted if end == k + 1 => Some((token.name.clone(), end)),
            Kind::Word | Kind::Quoted | Kind::Placeholder => Some((None, end)),
            _ => None,
        }
    }

    /// The name of the view a `CREATE` statement creates, read after its `CREATE`: None inside
    /// when Python fills it in or the text ends before it, and None when the statement creates no
    /// view or one of another schema.
    fn created_view(&self, k: usize) -> Option<Option<String>> {
        let start = self.view_start(k)?;
        if start >= self.tokens.len() {
            return Some(None);
        }
        self.view_name(start).map(|(name, _)| name)
    }

    /// The names of the bracket at `open` when it lists only names.
    fn name_list(&self, open: usize) -> Option<Vec<String>> {
        let items = &self.tokens[open + 1..self.close[open]];
        let mut names = vec![];
        for (n, token) in items.iter().enumerate() {
            let expected = if n % 2 == 0 {
                matches!(token.kind, Kind::Word | Kind::Quoted)
            } else {
                self.is_char(open + 1 + n, b',')
            };
            if !expected {
                return None;
            }
            names.extend(token.name.clone().filter(|_| n % 2 == 0));
        }
        (items.len() % 2 == 1).then_some(names)
    }

    /// The items of the select list of the first branch of the query from `k` to `end`, inside
    /// the brackets that enclose it and past its `WITH` clause, as `select_list` reads them.
    fn query_items(&self, mut k: usize, mut end: usize) -> Option<Vec<Option<String>>> {
        loop {
            k = self.skip_with(k)?;
            if self.kind(k) != Some(Kind::Open) {
                break;
            }
            end = self.close[k];
            k += 1;
        }
        if self.keyword(k) != Some("select") {
            return None;
        }
        self.select_list(k + 1, end)
    }

    /// Where the statement after a `WITH` clause at `k` starts, or `k` without one. None when the
    /// clause does not read as one.
    fn skip_with(&self, mut k: usize) -> Option<usize> {
        if self.keyword(k) != Some("with") {
            return Some(k);
        }
        k += 1 + usize::from(self.keyword(k + 1) == Some("recursive"));
        loop {
            k += 1;
            if self.kind(k) == Some(Kind::Open) {
                k = self.next(k);
            }
            if self.keyword(k) != Some("as") {
                return None;
            }
            k += 1;
            while matches!(self.keyword(k), Some("not" | "materialized")) {
                k += 1;
            }
            if self.kind(k) != Some(Kind::Open) {
                return None;
            }
            k = self.next(k);
            if !self.is_char(k, b',') {
                return Some(k);
            }
            k += 1;
        }
    }

    /// The name of each item of a select list from `k`, after `SELECT`, to `end`, None for an
    /// item without one. None when an item may give several columns: `*`, `t.*` or a placeholder.
    fn select_list(&self, mut k: usize, end: usize) -> Option<Vec<Option<String>>> {
        while let Some(word @ ("all" | "distinct")) = self.keyword(k) {
            let on = word == "distinct" && self.keyword(k + 1) == Some("on");
            k = if on { self.next(k + 2) } else { k + 1 };
        }
        let mut items = vec![vec![]];
        while k < end && !self.ends_select(k) {
            if self.is_char(k, b',') {
                items.push(vec![]);
            } else {
                items.last_mut()?.push(k);
            }
            k = self.next(k);
        }
        items
            .iter()
            .map(|item| self.single_column(item).then(|| self.item_name(item)))
            .collect()
    }

    /// Whether a select item gives one column: it is not empty, `*` or `t.*`, and has no
    /// placeholder outside brackets but its alias.
    fn single_column(&self, item: &[usize]) -> bool {
        let Some((&last, rest)) = item.split_last() else {
            return false;
        };
        let star = self.is_char(last, b'*') && rest.last().is_none_or(|&before| self.is_char(before, b'.'));
        let aliased = rest.last().is_some_and(|&before| self.keyword(before) == Some("as"));
        let expression = if aliased { rest } else { item };
        !star && !expression.iter().any(|&i| self.kind(i) == Some(Kind::Placeholder))
    }

    fn ends_select(&self, k: usize) -> bool {
        self.keyword(k).is_some_and(|w| SELECT_ENDS.contains(&w)) || self.is_char(k, b';')
    }

    /// The name PostgreSQL gives the column of a select item: its alias, after `AS` or bare, or the
    /// column it reads, alone, qualified or cast. None for anything else, such as `*` or a call.
    fn item_name(&self, item: &[usize]) -> Option<String> {
        let (&last, rest) = item.split_last()?;
        if rest.last().is_some_and(|&before| self.keyword(before) == Some("as")) {
            return self.name(last);
        }
        let cast = item
            .windows(2)
            .position(|pair| self.is_char(pair[0], b':') && self.is_char(pair[1], b':'));
        match cast {
            Some(at) if at + 3 == item.len() && self.kind(last) == Some(Kind::Word) => self.column_name(&item[..at]),
            Some(_) => None,
            None => self.column_name(item).or_else(|| self.bare_alias(item)),
        }
    }

    /// The column a select item reads when it is a name, alone or qualified.
    fn column_name(&self, item: &[usize]) -> Option<String> {
        match *item {
            [name] => self
                .name(name)
                .filter(|n| self.kind(name) == Some(Kind::Quoted) || !NOT_BARE_ALIASES.contains(&n.as_str())),
            [qualifier, dot, name] if self.name(qualifier).is_some() && self.is_char(dot, b'.') => self.name(name),
            _ => None,
        }
    }

    /// The alias a select item ends in without `AS`, after the expression it names.
    fn bare_alias(&self, item: &[usize]) -> Option<String> {
        let &[.., before, last] = item else {
            return None;
        };
        let alias = self.name(last)?;
        let reserved = |i: usize| self.keyword(i).is_some_and(|w| NOT_BARE_ALIASES.contains(&w));
        let ends_operand = match self.kind(before)? {
            Kind::Open | Kind::Literal | Kind::Quoted => true,
            Kind::Word => !reserved(before) || self.keyword(before).is_some_and(|w| OPERAND_ENDS.contains(&w)),
            Kind::Other => self.text.as_bytes()[self.tokens[before].start].is_ascii_digit(),
            _ => false,
        };
        (ends_operand && !reserved(last)).then_some(alias)
    }

    fn name(&self, i: usize) -> Option<String> {
        let token = self.tokens.get(i)?;
        matches!(token.kind, Kind::Word | Kind::Quoted)
            .then(|| token.name.clone())
            .flatten()
    }

    /// The tokens of a definition that name no column of the table: the table and the bracket of
    /// columns a `REFERENCES` names, and the storage parameters in the bracket after `WITH`.
    fn unread(&self) -> Vec<bool> {
        let mut unread = vec![false; self.tokens.len()];
        let mut i = 0;
        while i < self.tokens.len() {
            let end = match self.keyword(i) {
                Some("references") => self.reference_end(i),
                Some("with") if self.kind(i + 1) == Some(Kind::Open) => self.close[i + 1] + 1,
                _ => {
                    i += 1;
                    continue;
                }
            };
            let end = end.min(self.tokens.len());
            unread[i + 1..end].iter_mut().for_each(|r| *r = true);
            i = end;
        }
        unread
    }

    /// Where what the `REFERENCES` at `i` names ends: past its table, qualified or not, and the
    /// bracket of its columns.
    fn reference_end(&self, i: usize) -> usize {
        let mut end = i + 2;
        while self.is_char(end, b'.') {
            end += 2;
        }
        if self.kind(end) == Some(Kind::Open) {
            end = self.close[end] + 1;
        }
        end
    }

    /// Whether token `i` of a definition names a column of the table.
    fn is_definition_column(&self, i: usize) -> bool {
        let Some(name) = self.name(i) else {
            return false;
        };
        let word = self.kind(i) == Some(Kind::Word);
        !(word && DEFINITION_WORDS.contains(&name.as_str()))
            && !self.is_char(i + 1, b'.')
            && !matches!(self.kind(i + 1), Some(Kind::Open))
            && !(word && self.kind(i + 1) == Some(Kind::Literal))
            && i.checked_sub(1)
                .is_none_or(|before| !self.names_no_column(before, &name))
    }

    /// Whether token `before` makes the name after it no column: a qualifier, a cast, a keyword
    /// that names something else or makes it a keyword, the end of an operand that an operator
    /// class or a type name follows, or the bracket of `EXTRACT`.
    fn names_no_column(&self, before: usize, name: &str) -> bool {
        if let Some(word) = self.keyword(before) {
            let follows = FOLLOWING_WORDS
                .iter()
                .any(|(first, then)| *first == word && then.contains(&name));
            return follows || NAMING_WORDS.contains(&word) || !DEFINITION_WORDS.contains(&word);
        }
        let extract = self.kind(before) == Some(Kind::Open)
            && before
                .checked_sub(1)
                .is_some_and(|call| self.keyword(call) == Some("extract"));
        extract
            || self.is_char(before, b'.')
            || self.is_char(before, b':')
            || matches!(self.kind(before), Some(Kind::Quoted | Kind::Close | Kind::Literal))
    }
}

#[cfg(test)]
mod tests {
    use ruff_python_parser::{parse_expression, parse_module};

    use super::*;

    /// What the scan of a function whose body is `body` finds, through `check`.
    fn scanned<T>(body: &str, nested: bool, check: impl FnOnce(&CursorUse) -> T) -> T {
        scanned_with("self, cr", body, nested, check)
    }

    /// What the scan finds of a function with the parameters `params` and the body `body`.
    fn scanned_with<T>(params: &str, body: &str, nested: bool, check: impl FnOnce(&CursorUse) -> T) -> T {
        let src = format!("def f({params}):\n    {}\n", body.replace('\n', "\n    "));
        let parsed = parse_module(&src).expect("valid Python");
        let Stmt::FunctionDef(func) = &parsed.syntax().body[0] else {
            panic!("not a function")
        };
        let uses = if nested {
            CursorUse::scan_nested(func)
        } else {
            CursorUse::scan(func, None)
        };
        check(&uses)
    }

    /// The first function the body defines, at any depth of its blocks.
    struct FirstDef<'a>(Option<&'a ast::StmtFunctionDef>);

    impl<'a> Visitor<'a> for FirstDef<'a> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            match stmt {
                Stmt::FunctionDef(f) if self.0.is_none() => self.0 = Some(f),
                _ => visitor::walk_stmt(self, stmt),
            }
        }
    }

    /// What the scan of the innermost function `body` defines finds, following the first `def` of
    /// each level, with each level scanned around the next as `PyVisitor` does.
    fn closure_scanned<T>(body: &str, check: impl FnOnce(&CursorUse) -> T) -> T {
        let src = format!("def f(self, cr):\n    {}\n", body.replace('\n', "\n    "));
        let parsed = parse_module(&src).expect("valid Python");
        let Stmt::FunctionDef(outer) = &parsed.syntax().body[0] else {
            panic!("not a function")
        };
        let mut func = outer;
        let mut uses = CursorUse::scan(func, None);
        let mut depth = 0;
        loop {
            let mut first = FirstDef(None);
            first.visit_body(&func.body);
            let Some(inner) = first.0 else { break };
            uses = CursorUse::scan(inner, Some(&uses));
            func = inner;
            depth += 1;
        }
        assert!(depth > 0, "no nested function in {body}");
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
            "env = api.Environment(self.pool.cursor(), uid, {})\nenv.cr.rollback()",
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
    fn a_closure_sees_the_cursors_the_enclosing_function_opens() {
        for body in [
            "with self.pool.cursor() as cr:\n    def step():\n        cr.rollback()\n        cr.commit()",
            "def step():\n    env.cr.rollback()\nnew_cr = registry.cursor()\nenv = api.Environment(new_cr, 1, {})",
            "with self.pool.cursor() as cr:\n    def outer():\n        def step():\n            cr.rollback()",
        ] {
            closure_scanned(body, |uses| {
                assert!(uses.rollbacks.is_empty() && !uses.commits, "{body}");
            });
        }
        for body in [
            "with self.pool.cursor() as cr:\n    def step(cr):\n        cr.rollback()\n        cr.commit()",
            "with self.pool.cursor() as cr:\n    def step():\n        self.env.cr.rollback()\n        self.env.cr.commit()",
            "def step():\n    cr.rollback()\n    cr.commit()",
        ] {
            closure_scanned(body, |uses| {
                assert!(uses.rollbacks.len() == 1 && uses.commits, "{body}");
            });
        }
        let body = "cr = new_cr or self.env.cr\ndef step():\n    cr.commit()";
        assert!(closure_scanned(body, |uses| uses.commits));
    }

    #[test]
    fn keeps_apart_commits_on_cursors_the_function_opens() {
        let src = "def f(self):\n    with self.pool.cursor() as cr:\n        cr.commit()\n    self.env.cr.commit()\n";
        let parsed = parse_module(src).expect("valid Python");
        let Stmt::FunctionDef(func) = &parsed.syntax().body[0] else {
            panic!("not a function")
        };
        let uses = CursorUse::scan(func, None);
        let call = |i: usize| {
            let stmt = match i {
                0 => &func.body[0].as_with_stmt().expect("a with").body[0],
                _ => &func.body[1],
            };
            let Stmt::Expr(e) = stmt else { panic!("not a call") };
            e.value.as_call_expr().expect("a call")
        };
        assert!(uses.opened(call(0)));
        assert!(!uses.opened(call(1)));
        assert!(uses.commits);
        let commits = |body: &str| scanned(body, false, |uses| uses.commits);
        for body in [
            "with self.pool.cursor() as cr:\n    cr.commit()",
            "cr = self.env.registry.cursor()\ncr.commit()",
            "with odoo.sql_db.db_connect(name).cursor() as cr:\n    cr.commit()",
            "with self.pool.cursor() as log_cr:\n    env = api.Environment(log_cr, 1, {})\n    env.cr.commit()",
            "with self.pool.cursor() as new_cr:\n    self = self.with_env(self.env(cr=new_cr))\n    self.env.cr.commit()",
        ] {
            assert!(!commits(body), "{body}");
        }
        for body in [
            "new_cr = None\ncr = new_cr or self.env.cr\ncr.commit()",
            "new_cr = None\ncr = new_cr if new_cr else self.env.cr\ncr.commit()",
            "new_cr = None\nenv = self.env(cr=new_cr or self.env.cr)\nenv.cr.commit()",
            "with self.env.cr as cr:\n    cr.commit()",
        ] {
            assert!(commits(body), "{body}");
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
        for added in ["'c'", "c"] {
            for write in ["DELETE FROM t", "INSERT INTO t (c) VALUES (1)"] {
                let body = format!("create_column(cr, 't', {added}, 'int4')\ncr.execute('{write}')");
                assert!(!covered(&body), "{body}");
            }
        }
    }

    #[test]
    fn meets_what_two_paths_hold() {
        use Held::{Cursor, Either, Env, Given, Unset};
        assert_eq!(Either.meet(Cursor), Cursor);
        assert_eq!(Unset.meet(Env), Env);
        assert_eq!(Given.meet(Unset), Given);
        assert_eq!(meet((Unset, false), (Cursor, true)), (Cursor, true));
        assert_eq!(meet((Cursor, true), (Cursor, false)), (Cursor, false));
        assert_eq!(Env.meet(Either), Env);
        assert_eq!(Either.meet(Either), Either);
        assert_eq!(Cursor.meet(Cursor), Cursor);
        assert_eq!(Env.meet(Env), Env);
        assert_eq!(Cursor.meet(Env), Given);
        assert_eq!(Given.meet(Either), Given);
        assert_eq!(Cursor.meet(Given), Given);
    }

    #[test]
    fn an_own_environment_holds_only_on_the_paths_that_build_it() {
        let writes = |body: &str| scanned(body, true, |uses| uses.writes.len());
        let switch = "self = self.with_env(self.env(cr=registry.cursor()))";
        let update = "self.env.cr.execute('UPDATE a SET b = 1')";
        for body in [
            format!("if new:\n    {switch}\n{update}"),
            format!("if new:\n    {switch}\nelif other:\n    pass\nelse:\n    {switch}\n{update}"),
            format!("if a:\n    if b:\n        {switch}\n    else:\n        return\n{update}"),
            format!("for x in y:\n    {switch}\n{update}"),
            format!("for x in y:\n    {switch}\n    break\n{update}"),
            format!("while x:\n    {switch}\n{update}"),
            format!("while x:\n    if a:\n        {switch}\n        continue\n    {update}"),
            format!("for x in y:\n    if a:\n        {switch}\n        break\n{update}"),
            format!("while True:\n    if a:\n        {switch}\n        continue\n    break\n{update}"),
            format!("while True:\n    if a:\n        {switch}\n        continue\n    if b:\n        break\n{update}"),
            format!(
                "{switch}\nwhile True:\n    for x in y:\n        pass\n    else:\n        self = records\n        break\n{update}"
            ),
            format!("if new:\n    {switch}\ndef g():\n    {update}"),
        ] {
            assert_eq!(writes(&body), 1, "{body}");
        }
        for body in [
            format!("if new:\n    {switch}\n    {update}"),
            format!("if new:\n    {switch}\nelse:\n    {switch}\n{update}"),
            format!("if new:\n    {switch}\nelif other:\n    raise E()\nelse:\n    return\n{update}"),
            format!(
                "if a:\n    if b:\n        {switch}\n    else:\n        {switch}\nelse:\n    sys.exit(1)\n{update}"
            ),
            format!("for x in y:\n    {switch}\nelse:\n    {switch}\n{update}"),
            format!("for x in y:\n    {switch}\n    {update}"),
            format!("def g():\n    if new:\n        {switch}\n    {update}"),
            format!("while True:\n    if a:\n        {switch}\n        break\n{update}"),
            format!("while True:\n    {switch}\n    if done:\n        break\n{update}"),
            format!("while True:\n    try:\n        {switch}\n        break\n    except E:\n        time.sleep(1)\n{update}"),
            format!("while True:\n    with lock:\n        {switch}\n        break\n{update}"),
            format!("while True:\n    match x:\n        case 1:\n            {switch}\n            break\n{update}"),
            format!("while True:\n    {switch}\n    for x in y:\n        break\n    else:\n        break\n{update}"),
            format!("while True:\n    for x in y:\n        break\n    {switch}\n    break\n{update}"),
            format!("while True:\n    if a:\n        self = records\n        continue\n    {switch}\n    break\n{update}"),
            format!("for x in y:\n    if a:\n        {switch}\n        break\nelse:\n    return\n{update}"),
            format!("def g():\n    {update}\n{switch}"),
            format!("if new:\n    {switch}\n    def g():\n        {update}\n    g()\n    return"),
            "env = None\nif new:\n    env = api.Environment(self.pool.cursor(), uid, {})\nif env:\n    env.cr.execute('UPDATE a SET b = 1')".to_owned(),
        ] {
            assert_eq!(writes(&body), 0, "{body}");
        }
    }

    #[test]
    fn an_own_cursor_holds_only_on_the_paths_that_open_it() {
        let commits = |body: &str| scanned_with("self", body, false, |uses| uses.commits);
        for body in [
            "if new:\n    cr = self.pool.cursor()\ncr.commit()",
            "if new:\n    cr = self.pool.cursor()\nelse:\n    cr = registry.cursor()\ncr.commit()",
            "if new:\n    cr = self.pool.cursor()\nelse:\n    return\ncr.commit()",
            "if new:\n    cr = self.pool.cursor()\nelse:\n    cr = None\ncr.commit()",
            "cr = None\nif new:\n    cr = self.pool.cursor()\ncr.commit()",
            "for x in y:\n    cr = self.pool.cursor()\n    break\ncr.commit()",
            "while True:\n    try:\n        cr = self.pool.cursor()\n        break\n    except E:\n        time.sleep(1)\ncr.commit()",
            "cr = False\nif apart:\n    cr = self.pool.cursor()\nif cr:\n    cr.commit()",
            "cr = self.pool.cursor()\nwhile x:\n    cr.commit()\n    cr = registry.cursor()",
        ] {
            assert!(!commits(body), "{body}");
        }
        for body in [
            "cr = self.env.cr\nif new:\n    cr = self.pool.cursor()\ncr.commit()",
            "cr = None\ncr.commit()",
            "cr = self.env.cr\nfor x in y:\n    cr = self.pool.cursor()\ncr.commit()",
            "cr = self.pool.cursor()\nfor rec in self:\n    if rec.x:\n        cr = self.env.cr\n        break\ncr.commit()",
            "cr = self.pool.cursor()\nfor rec in self:\n    if rec.x:\n        cr = self.env.cr\n        continue\ncr.commit()",
            "cr = self.pool.cursor()\nwhile True:\n    if not self._x_batch(cr):\n        break\n    cr = self.env.cr\ncr.commit()",
            "cr = self.pool.cursor()\nfor x in y:\n    cr.commit()\n    cr = self.env.cr",
            "cr = self.pool.cursor()\nwhile True:\n    a = 1\n    if a:\n        break\n    b = 2\n    if b:\n        cr = self.env.cr\ncr.commit()",
        ] {
            assert!(commits(body), "{body}");
        }
        assert_eq!(
            reported_rollbacks(
                "if new:\n    self = self.with_env(self.env(cr=registry.cursor()))\nself.env.cr.rollback()"
            ),
            1
        );
        let body = "if new:\n    cr = self.pool.cursor()\n    def step():\n        cr.commit()\n    step()\n    return";
        assert!(!closure_scanned(body, |uses| uses.commits));
    }

    #[test]
    fn a_closure_sees_the_paths_reaching_its_def_and_the_bindings_after_it() {
        let commits = |body: &str| closure_scanned(body, |uses| uses.commits);
        for body in [
            "cr = self.env.cr\nif new:\n    cr = self.pool.cursor()\ndef step():\n    cr.commit()",
            "cr = self.pool.cursor()\ndef step():\n    cr.commit()\ncr = self.env.cr",
        ] {
            assert!(commits(body), "{body}");
        }
        for body in [
            "if new:\n    cr = self.pool.cursor()\nelse:\n    cr = registry.cursor()\ndef step():\n    cr.commit()",
            "cr = self.env.cr\ndef step():\n    cr.commit()\ncr = self.pool.cursor()",
        ] {
            assert!(!commits(body), "{body}");
        }
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
    fn reads_a_query_held_in_a_name_bound_once_and_the_writes_of_a_with_query() {
        let tables = |body: &str| {
            scanned(body, true, |uses| {
                uses.writes
                    .iter()
                    .map(|w| format!("{} {}", w.verb, w.table))
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(tables("query = 'UPDATE a SET b = 1'\ncr.execute(query)"), ["UPDATE a"]);
        assert_eq!(
            tables("query: str = SQL('DELETE FROM a')\nself.env.execute_query(query)"),
            ["DELETE a"]
        );
        assert_eq!(
            tables("cr.execute('WITH t AS (SELECT 1) UPDATE a SET b = 1')"),
            ["UPDATE a"]
        );
        assert_eq!(
            tables("cr.execute('/* x */ WITH t AS (DELETE FROM a RETURNING id) INSERT INTO b SELECT id FROM t')"),
            ["INSERT b", "DELETE a"]
        );
        for body in [
            "query = 'UPDATE a SET b = 1'\nquery += ' WHERE c'\ncr.execute(query)",
            "query = 'UPDATE a SET b = 1'\nquery = 'UPDATE c SET d = 1'\ncr.execute(query)",
            "def g():\n    query = 'SELECT 1'\nquery = 'UPDATE a SET b = 1'\ncr.execute(query)",
            "for query in ['UPDATE a SET b = 1']:\n    cr.execute(query)",
            "query, other = 'UPDATE a SET b = 1', 1\ncr.execute(query)",
            "cr = 'UPDATE a SET b = 1'\nself.env.cr.execute(cr)",
            "cr.execute(f'WITH t AS (SELECT {x}) UPDATE a SET b = 1')",
        ] {
            assert!(tables(body).is_empty(), "{body}");
        }
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

    /// The `table.column` pairs a query names, in the order they are found.
    fn columns(text: &str) -> Vec<String> {
        let mut found: Vec<String> = parse(text)
            .expect("a parsable query")
            .columns
            .iter()
            .map(|c| format!("{}.{}", c.table, c.column))
            .collect();
        found.sort();
        found
    }

    #[test]
    fn resolves_qualified_columns_to_the_tables_the_query_binds() {
        assert_eq!(
            columns("SELECT l.product_uom FROM sale_order_line l"),
            ["sale_order_line.product_uom"]
        );
        assert_eq!(
            columns("select res_partner.name from res_partner"),
            ["res_partner.name"]
        );
        assert_eq!(
            columns("SELECT p.x FROM public.res_partner AS p WHERE p.y = %s"),
            ["res_partner.x", "res_partner.y"]
        );
        assert_eq!(
            columns("SELECT a.x FROM t a JOIN u b ON a.id = b.t_id LEFT OUTER JOIN v c USING (id) WHERE c.z"),
            ["t.id", "t.x", "u.t_id", "v.z"]
        );
        assert_eq!(
            columns(
                "SELECT o.id FROM sale_order o WHERE EXISTS \
                 (SELECT 1 FROM sale_order_line l WHERE l.order_id = o.id AND o.state = 'sale')"
            ),
            [
                "sale_order.id",
                "sale_order.id",
                "sale_order.state",
                "sale_order_line.order_id"
            ]
        );
        assert_eq!(
            columns("SELECT \"P\".\"Name\" FROM res_partner \"P\""),
            ["res_partner.Name"]
        );
        assert_eq!(
            columns("SELECT EXTRACT(year FROM o.day), o.a IS DISTINCT FROM o.b FROM t o"),
            ["t.a", "t.b", "t.day"]
        );
        assert_eq!(
            columns("-- a.x\nSELECT 'a.y', $$ a.z $$, $q$ a.w $q$, E'a\\'.v' /* a.u */ FROM t a"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn writes_bind_their_table_and_excluded() {
        assert_eq!(
            columns("UPDATE t x SET a = 1 FROM u y WHERE x.id = y.id AND y.z"),
            ["t.id", "u.id", "u.z"]
        );
        assert_eq!(
            columns("DELETE FROM ONLY t USING u WHERE t.id = u.id"),
            ["t.id", "u.id"]
        );
        assert_eq!(
            columns("INSERT INTO x_rel (a, b) VALUES (%s, %s) ON CONFLICT (a) DO UPDATE SET b = EXCLUDED.b"),
            ["x_rel.b"]
        );
        assert_eq!(
            columns("INSERT INTO t AS n (a) SELECT s.a FROM s ON CONFLICT DO UPDATE SET a = n.a"),
            ["s.a", "t.a"]
        );
    }

    #[test]
    fn each_branch_of_a_set_operation_or_statement_binds_its_own_names() {
        assert_eq!(
            columns("SELECT a.x FROM t a UNION ALL SELECT a.y FROM u a; SELECT a.z FROM v a"),
            ["t.x", "u.y", "v.z"]
        );
        assert_eq!(
            columns("(SELECT a.x FROM t a) EXCEPT (SELECT a.y FROM u a)"),
            ["t.x", "u.y"]
        );
    }

    #[test]
    fn leaves_out_what_the_text_does_not_tell() {
        for text in [
            "WITH c AS (SELECT 1 AS id) SELECT c.id FROM c",
            "WITH RECURSIVE t AS (SELECT 1) SELECT t.id FROM t",
            "SELECT d.x FROM (SELECT 1 AS x) d",
            "SELECT a.x FROM %s a",
            "SELECT a.x FROM {} a",
            "SELECT a.x FROM {table} AS a",
            "SELECT a.x FROM res_partner_{} a",
            "SELECT a.x FROM t %s",
            "SELECT a.x FROM t a_{}",
            "a.x = %s AND b.y IN %s",
            "JOIN res_partner p ON p.id = x.partner_id",
            "SELECT a.perm_{} FROM t a",
            "SELECT a.%s FROM t a",
            "SELECT {}.x FROM t a",
            "SELECT public.t.x FROM t",
            "SELECT a.x FROM t a, u a",
            "SELECT a.f(1), pg_catalog.now() FROM t a",
            "SELECT v.a FROM t AS v(a, b)",
            "SELECT f.x FROM unnest(%s) f",
            "SELECT s.x FROM other.t s",
            "SELECT p.x FROM res_partner",
            "SELECT res_partner.x FROM res_partner p",
            "CREATE INDEX ON t (a.x)",
            "SELECT p.\"%s\" FROM res_partner p",
            "SELECT p.\"%(col)s\" FROM res_partner p",
            "SELECT p.\"{}\", p.\"x_{0}\" FROM res_partner p",
            "SELECT p.x FROM \"%s\" p",
        ] {
            assert_eq!(columns(text), Vec::<String>::new(), "{text}");
        }
    }

    #[test]
    fn lateral_and_ordinality_items_bind_their_alias() {
        assert_eq!(
            columns("SELECT x.a, z.b FROM t x CROSS JOIN LATERAL (SELECT y.b FROM u y WHERE y.t_id = x.id) z"),
            ["t.a", "t.id", "u.b", "u.t_id"]
        );
        assert_eq!(
            columns("SELECT 1 FROM t u WHERE EXISTS (SELECT u.val FROM t x, LATERAL unnest(x.ids) u)"),
            ["t.ids"]
        );
        assert_eq!(
            columns("SELECT 1 FROM t u WHERE EXISTS (SELECT u.val FROM unnest(%s) WITH ORDINALITY AS u(val, n))"),
            Vec::<String>::new()
        );
        assert_eq!(
            columns("SELECT o.n FROM t x JOIN LATERAL unnest(x.ids) WITH ORDINALITY o(id, n) ON true"),
            ["t.ids"]
        );
    }

    #[test]
    fn reads_the_writes_of_a_query_with_ctes() {
        let writes = |text: &str| {
            parse(text)
                .expect("a parsable query")
                .writes
                .iter()
                .map(|w| format!("{} {} {:?}", w.verb, w.table, w.columns))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            writes("WITH moved AS (UPDATE t SET a = 1 RETURNING id) DELETE FROM u WHERE id IN (SELECT id FROM moved)"),
            ["DELETE u None", "UPDATE t Some([\"a\"])"]
        );
        assert_eq!(
            writes("WITH low AS (SELECT id FROM t) UPDATE t SET a = 0, b = 1 WHERE id IN (SELECT id FROM low)"),
            ["UPDATE t Some([\"a\", \"b\"])"]
        );
        assert_eq!(writes("UPDATE public.t SET a = 0"), ["UPDATE t Some([\"a\"])"]);
        assert_eq!(
            writes("INSERT INTO t (a) VALUES (1) ON CONFLICT (a) DO UPDATE SET a = EXCLUDED.a"),
            ["INSERT t None"]
        );
        assert_eq!(
            writes("WITH n AS (SELECT 1 AS a) INSERT INTO t (a) SELECT a FROM n ON CONFLICT DO UPDATE SET a = 2"),
            ["INSERT t None"]
        );
        for text in [
            "SELECT id FROM t WHERE id = 1 FOR UPDATE",
            "SELECT id FROM t FOR NO KEY UPDATE",
            "INSERT INTO %s (a) VALUES (1)",
            "UPDATE {} SET a = 1",
            "DELETE FROM x_{} WHERE id = 1",
            "WITH c AS (SELECT 1) UPDATE c SET a = 1",
        ] {
            assert!(writes(text).is_empty(), "{text}");
        }
    }

    #[test]
    fn places_each_column_in_the_text() {
        let text = "SELECT x.name\n  FROM res_partner x";
        let query = parse(text).expect("a parsable query");
        assert_eq!(query.columns[0].at, 9);
        assert_eq!(query.columns[0].text, "x.name");
    }

    #[test]
    fn an_unclosed_quote_comment_or_bracket_gives_no_query() {
        for text in [
            "SELECT a.x FROM t a WHERE b = 'x",
            "SELECT \"a.x FROM t a",
            "SELECT a.x FROM t a /* note",
            "SELECT $q$ a.x FROM t a",
            "SELECT a.x FROM t a WHERE b IN (1",
            "SELECT a.x FROM t a WHERE b = 1)",
            "SELECT a[1) FROM t a",
            "SELECT a.x FROM t a WHERE b = {",
            "SELECT a.x FROM t a WHERE b = %(b",
            "SELECT E'a\\' FROM t",
        ] {
            assert!(parse(text).is_none(), "{text}");
        }
        for text in [
            "",
            ";",
            "$",
            "%",
            "-- only a comment",
            "SELECT",
            "FROM",
            "a.",
            ".b",
            "UPDATE",
            "INSERT INTO",
        ] {
            assert!(parse(text).is_some(), "{text}");
        }
    }

    #[test]
    fn long_and_deep_queries_stay_linear() {
        let deep = format!("{}{}", "(".repeat(100_000), ")".repeat(100_000));
        assert!(parse(&deep).is_some());
        let nested = 2_000;
        let mut text = String::new();
        for _ in 0..nested {
            text.push_str("SELECT a.x FROM t a WHERE a.y IN (");
        }
        text.push_str("SELECT 1");
        text.push_str(&")".repeat(nested));
        assert_eq!(parse(&text).expect("a parsable query").columns.len(), 2 * nested);
        let wide = format!("SELECT {} FROM t a", vec!["a.x"; 50_000].join(", "));
        assert_eq!(parse(&wide).expect("a parsable query").columns.len(), 50_000);
        let unclosed = format!("SELECT '{}", "x".repeat(200_000));
        assert!(parse(&unclosed).is_none());
        let far = format!("SELECT {} FROM t a", "b.x, ".repeat(20_000));
        let nested_far = format!("{}{far}{}", "SELECT 1 FROM t b WHERE c IN (".repeat(50), ")".repeat(50));
        assert!(parse(&nested_far).expect("a parsable query").columns.len() == 20_000);
        let far_out = format!(
            "SELECT 1 FROM t z WHERE {}true{}",
            "a IN (SELECT z.x FROM u WHERE ".repeat(5_000),
            ")".repeat(5_000)
        );
        assert!(parse(&far_out).expect("a parsable query").columns.len() <= MAX_SCOPES);
        let names = 50_000;
        let refs: Vec<String> = (0..names).map(|i| format!("a{i}.x")).collect();
        let items: Vec<String> = (0..names).map(|i| format!("t a{i}")).collect();
        let wide_from = format!("SELECT {} FROM {}", refs.join(", "), items.join(", "));
        assert_eq!(parse(&wide_from).expect("a parsable query").columns.len(), names);
        let writes = 20_000;
        let nested_writes = format!("{}1{}", "UPDATE t SET a = (".repeat(writes), ")".repeat(writes));
        assert_eq!(parse(&nested_writes).expect("a parsable query").writes.len(), writes);
    }

    #[test]
    fn reads_the_columns_alter_table_adds_by_name() {
        let added = |text: &str| added_columns(text);
        let pair = |t: &str, c: &str| (t.to_owned(), c.to_owned());
        assert_eq!(
            added("ALTER TABLE res_users ADD COLUMN totp_secret varchar"),
            [pair("res_users", "totp_secret")]
        );
        assert_eq!(
            added(
                "alter table if exists only \"Box\" add column if not exists Code varchar default 'a, b', \
                 ADD CONSTRAINT box_code UNIQUE (code), DROP COLUMN old, ADD weight numeric(10, 2), \
                 ADD \"Label\" text"
            ),
            [pair("Box", "code"), pair("Box", "weight"), pair("Box", "Label")]
        );
        assert!(added("ALTER TABLE public.box ADD COLUMN code varchar, ADD COLUMN label varchar").is_empty());
        assert!(added("ALTER TABLE box_%s ADD COLUMN code varchar, ADD COLUMN label varchar").is_empty());
        assert!(added("ALTER TABLE box ADD PRIMARY KEY (id)").is_empty());
        assert!(added("ALTER INDEX box_idx RENAME TO crate_idx").is_empty());
        assert!(added("SELECT 1 FROM box").is_empty());
    }

    /// The table and the names of the view a text creates, as `table: a b c` with `?` for unknowns.
    fn viewed(text: &str) -> String {
        let Some(view) = view(text) else {
            return "none".to_owned();
        };
        let names = view.names.map_or("?".to_owned(), |n| n.join(" "));
        format!("{}: {names}", view.table.as_deref().unwrap_or("?"))
    }

    #[test]
    fn reads_the_name_and_the_columns_of_a_view() {
        assert_eq!(
            viewed("CREATE OR replace VIEW x_box_report AS(SELECT b.id AS id, b.depot_id, count(*) AS total FROM b)"),
            "x_box_report: id depot_id total"
        );
        assert_eq!(
            viewed("CREATE MATERIALIZED VIEW IF NOT EXISTS \"Box\" AS SELECT 0 n, \"Q\", w::date, b.w::text FROM b"),
            "Box: n Q w w"
        );
        assert_eq!(viewed("create view %s as (select 1 as id)"), "?: id");
        assert_eq!(viewed("CREATE VIEW x_{} AS SELECT 1 AS id"), "?: id");
        assert_eq!(
            viewed("DROP VIEW v; CREATE TABLE t (a int); CREATE VIEW v AS SELECT a FROM t"),
            "v: a"
        );
        assert_eq!(
            viewed("CREATE VIEW v (id, total) AS SELECT b.id, count(*) FROM b"),
            "v: id total"
        );
        assert_eq!(
            viewed("CREATE VIEW public.v (id) AS SELECT b.id, b.total, 1 AS n FROM b"),
            "v: id total n"
        );
        assert_eq!(
            viewed("CREATE VIEW v WITH (security_barrier) AS SELECT id FROM b"),
            "v: id"
        );
        assert_eq!(
            viewed(
                "CREATE VIEW v AS (WITH RECURSIVE c (x) AS NOT MATERIALIZED (SELECT 1 AS x), d AS (SELECT 2) \
                 SELECT DISTINCT ON (c.x) c.x AS id, row_number() OVER (ORDER BY c.x) rank, CASE WHEN true \
                 THEN 1 END flag FROM c UNION SELECT 1, 2, 3)"
            ),
            "v: id rank flag"
        );
        assert_eq!(
            viewed("CREATE VIEW v AS ((SELECT 1 AS id) UNION ALL (SELECT 2))"),
            "v: id"
        );
    }

    #[test]
    fn leaves_the_columns_of_a_view_unknown_unless_each_item_names_one() {
        for text in [
            "CREATE VIEW v AS SELECT * FROM b",
            "CREATE VIEW v AS SELECT b.* FROM b",
            "CREATE VIEW v AS SELECT b.id, count(*) FROM b",
            "CREATE VIEW v AS SELECT %s FROM b",
            "CREATE VIEW v AS SELECT b.id AS %s FROM b",
            "CREATE VIEW v AS SELECT NULL FROM b",
            "CREATE VIEW v AS SELECT CASE WHEN a THEN b END FROM t",
            "CREATE VIEW v AS SELECT a AND b FROM t",
            "CREATE VIEW v AS SELECT x::double precision FROM t",
            "CREATE VIEW v AS SELECT x::numeric(10, 2) FROM t",
            "CREATE VIEW v AS VALUES (1)",
            "CREATE VIEW v AS WITH c SELECT 1",
            "CREATE VIEW v AS SELECT {} FROM b",
            "CREATE VIEW v (a, b + 1) AS SELECT 1, 2",
            "CREATE VIEW v (a) AS SELECT 1, count(*) FROM b",
            "CREATE VIEW v (a) AS SELECT b.* FROM b",
            "CREATE VIEW v (a, b, c) AS SELECT 1, 2",
        ] {
            assert_eq!(viewed(text), "v: ?", "{text}");
        }
        for text in [
            "SELECT 1 AS id",
            "CREATE TABLE v AS SELECT 1 AS id",
            "CREATE VIEW reports.v AS SELECT 1 AS id",
            "CREATE VIEW v SELECT 1 AS id",
            "CREATE VIEW v AS (SELECT 1",
        ] {
            assert_eq!(viewed(text), "none", "{text}");
        }
    }

    #[test]
    fn reads_the_name_of_each_view_a_text_creates_whatever_follows_it() {
        let created = |text: &str| -> Vec<String> {
            created_views(text)
                .into_iter()
                .map(|name| name.unwrap_or_else(|| "?".to_owned()))
                .collect()
        };
        assert_eq!(created("CREATE OR REPLACE VIEW x_report AS ({"), ["x_report"]);
        assert_eq!(created("CREATE VIEW public.x_report AS {"), ["x_report"]);
        assert_eq!(
            created(
                "DROP VIEW IF EXISTS a; CREATE VIEW a AS SELECT 1; CREATE MATERIALIZED VIEW IF NOT EXISTS \"B\" AS x"
            ),
            ["a", "B"]
        );
        assert_eq!(created("CREATE VIEW %s AS SELECT 1"), ["?"]);
        assert_eq!(created("CREATE VIEW x_{} AS SELECT 1"), ["?"]);
        assert_eq!(created("CREATE VIEW {"), ["?"]);
        assert_eq!(created("CREATE OR REPLACE VIEW "), ["?"]);
        assert!(created("CREATE VIEW reports.x AS SELECT 1").is_empty());
        assert!(created("CREATE TABLE x AS SELECT 1; SELECT 'CREATE VIEW y AS'").is_empty());
    }

    fn defined(text: &str) -> Option<Vec<String>> {
        definition_columns(text).map(|columns| columns.into_iter().map(|(name, _)| name).collect())
    }

    #[test]
    fn reads_the_columns_a_definition_names() {
        let names = |text: &str| defined(text).expect("a readable definition").join(" ");
        assert_eq!(names("unique(name, company_id)"), "name company_id");
        assert_eq!(
            names("CHECK (key IS NOT NULL OR \"Mode\" = 'x' AND size>=0)"),
            "key Mode size"
        );
        assert_eq!(
            names("CHECK(sort IS NULL OR jsonb_typeof(sort::jsonb) = 'array' OR CAST(x AS integer) > 0)"),
            "sort sort x"
        );
        assert_eq!(
            names("CHECK(date_from <= date_to + interval '1 day' AND EXTRACT(year FROM day) > 2000)"),
            "date_from date_to day"
        );
        assert_eq!(
            names("FOREIGN KEY (box_id) REFERENCES parcel_box (id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED"),
            "box_id"
        );
        assert_eq!(
            names("EXCLUDE USING gist (tsrange(start, stop, '()') WITH &&, crate_id WITH =) WHERE (active)"),
            "start stop crate_id active"
        );
        assert_eq!(
            names("(lower(name) text_pattern_ops, code COLLATE \"C\" DESC NULLS LAST) INCLUDE (ref) WHERE x"),
            "name code ref x"
        );
        assert_eq!(names("PRIMARY KEY (id)"), "id");
        assert_eq!(names("(a) NULLS NOT DISTINCT"), "a");
        assert_eq!(names("CHECK (t.a > 0 AND current_date > b AT TIME ZONE 'UTC')"), "b");
        assert_eq!(names("CHECK (x::timestamp with time zone > y)"), "x y");
        assert_eq!(names("CHECK (CAST(x AS timestamp with time zone) > y)"), "x y");
        assert_eq!(names("CHECK (a > 0) NO INHERIT"), "a");
        assert_eq!(names("CHECK (a IS UNKNOWN OR b IS NOT UNKNOWN)"), "a b");
        assert_eq!(names("CHECK (substring(a FROM 1 FOR 3) <> 'x')"), "a");
        assert_eq!(
            names("CHECK (trim(LEADING FROM a) <> '' AND trim(BOTH FROM b) <> '')"),
            "a b"
        );
        assert_eq!(
            names("(a) WITH (fillfactor = 70, deduplicate_items = off) TABLESPACE fast WHERE b"),
            "a b"
        );
        assert_eq!(names("UNIQUE (a) USING INDEX TABLESPACE fast"), "a");
        assert_eq!(names("CHECK (time > 0 AND interval > 0)"), "time interval");
    }

    #[test]
    fn places_each_definition_column_and_skips_what_it_cannot_read() {
        assert_eq!(
            definition_columns("UNIQUE (a, \"B\")"),
            Some(vec![("a".to_owned(), 8), ("B".to_owned(), 11)])
        );
        for text in ["CHECK (a IN %s)", "UNIQUE ({})", "CHECK (a = 'x)", "UNIQUE (a"] {
            assert!(definition_columns(text).is_none(), "{text}");
        }
    }
}
