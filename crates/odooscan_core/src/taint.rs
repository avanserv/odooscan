//! Values a user controls, followed through the locals of one function to the calls that must not
//! receive them.

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, CmpOp, Expr, Operator, Stmt};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{
    ArgSlot, CallSite, FlowEdge, FlowOrigin, FlowSummary, FlowTarget, RawDiag, Receiver, RunsAs, Safe, Spanned, Step,
};
use crate::python::{Locator, SAME_MODEL, exits, str_lit, tail_name};
use crate::series::Series;

/// What the user controls in a tainted value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Value,
    /// The keys of a mapping too, so that it can name any field of a record.
    Keys,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sink {
    Sql,
    Eval,
    Markup,
    SudoWrite,
    Redirect,
    Request,
    Path,
}

/// The arguments of a call that a sink checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Arg {
    /// The first positional argument, or without one a keyword of `SINK_KEYWORDS`.
    First,
    /// The positional argument at the index, or without it one of the keywords.
    At(usize, &'static [&'static str]),
    /// Every positional argument.
    Any,
}

/// A literal a call must pass, by keyword or position, for it to be the sink, as `local=False` for
/// `request.redirect()`. Before `since` the callee has no such parameter and always acts as if the
/// literal were passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Guard {
    keyword: &'static str,
    pos: usize,
    value: bool,
    since: Option<Series>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SinkRow {
    sink: Sink,
    arg: Arg,
    guard: Option<Guard>,
    /// The pattern matches the bare name alone, so that `Image.open` is no `open`.
    whole: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Source(Kind),
    Sink(SinkRow),
    Sanitizer,
    /// Returns what its arguments carry, made harmless for the rule of that code.
    Clears(&'static str),
    /// Returns what the positional arguments at these indexes, or these keywords, carry, and
    /// nothing of the others.
    Keeps(&'static [usize], &'static [&'static str]),
}

const fn sink(sink: Sink) -> Role {
    sink_at(sink, Arg::First)
}

const fn sink_at(sink: Sink, arg: Arg) -> Role {
    Role::Sink(SinkRow {
        sink,
        arg,
        guard: None,
        whole: false,
    })
}

const fn guarded(sink: Sink, arg: Arg, guard: Guard) -> Role {
    Role::Sink(SinkRow {
        sink,
        arg,
        guard: Some(guard),
        whole: false,
    })
}

/// The keywords of the URL of a redirect: `url` in the `request.redirect` of 14.0 and before.
const LOCATION: Arg = Arg::At(0, &["location", "url"]);
const URL: Arg = Arg::At(0, &["url"]);
const FILE: Arg = Arg::At(0, &["file", "name", "path_or_file", "filepath_or_fp"]);

/// From 15.0 `request.redirect()` takes `local`, which drops the scheme and the host of the URL
/// unless it is false (`odoo/http.py:303`); before, it redirects to any URL.
const REDIRECT_LOCAL: Guard = Guard {
    keyword: "local",
    pos: 2,
    value: false,
    since: Some(Series { major: 15, minor: 0 }),
};

/// Where user data enters, where it must not arrive, and what makes it harmless. A pattern matches
/// a dotted callee or attribute that ends with its last name and holds its other names in order, so
/// `cr.execute` matches `self.env.cr.execute` and `sudo().write` matches
/// `env["x"].sudo().browse(ids).write`. The parameters of routes and of public model methods are
/// sources too; `python.rs` tells which functions those are.
const TABLE: &[(&str, Role)] = &[
    ("request.params", Role::Source(Kind::Keys)),
    ("request.get_http_params", Role::Source(Kind::Keys)),
    ("request.get_json_data", Role::Source(Kind::Keys)),
    ("request.jsonrequest", Role::Source(Kind::Keys)),
    ("request.httprequest.args", Role::Source(Kind::Keys)),
    ("request.httprequest.form", Role::Source(Kind::Keys)),
    ("request.httprequest.values", Role::Source(Kind::Keys)),
    ("request.httprequest.files", Role::Source(Kind::Keys)),
    ("request.httprequest.cookies", Role::Source(Kind::Keys)),
    ("request.httprequest.headers", Role::Source(Kind::Keys)),
    ("request.httprequest.get_json", Role::Source(Kind::Keys)),
    ("request.httprequest.data", Role::Source(Kind::Value)),
    ("request.httprequest.get_data", Role::Source(Kind::Value)),
    ("cr.execute", sink(Sink::Sql)),
    ("_cr.execute", sink(Sink::Sql)),
    ("SQL", sink(Sink::Sql)),
    ("safe_eval", sink(Sink::Eval)),
    ("eval", sink(Sink::Eval)),
    ("exec", sink(Sink::Eval)),
    ("Markup", sink(Sink::Markup)),
    ("sudo().create", sink(Sink::SudoWrite)),
    ("sudo().write", sink(Sink::SudoWrite)),
    ("werkzeug.utils.redirect", sink_at(Sink::Redirect, LOCATION)),
    ("werkzeug.redirect", sink_at(Sink::Redirect, LOCATION)),
    ("request.redirect", guarded(Sink::Redirect, LOCATION, REDIRECT_LOCAL)),
    (
        "request.redirect_query",
        guarded(
            Sink::Redirect,
            LOCATION,
            Guard {
                pos: 3,
                since: None,
                ..REDIRECT_LOCAL
            },
        ),
    ),
    ("requests.get", sink_at(Sink::Request, URL)),
    ("requests.post", sink_at(Sink::Request, URL)),
    ("requests.put", sink_at(Sink::Request, URL)),
    ("requests.patch", sink_at(Sink::Request, URL)),
    ("requests.delete", sink_at(Sink::Request, URL)),
    ("requests.head", sink_at(Sink::Request, URL)),
    ("requests.options", sink_at(Sink::Request, URL)),
    ("requests.request", sink_at(Sink::Request, Arg::At(1, &["url"]))),
    ("urlopen", sink_at(Sink::Request, URL)),
    (
        "open",
        Role::Sink(SinkRow {
            sink: Sink::Path,
            arg: FILE,
            guard: None,
            whole: true,
        }),
    ),
    ("send_file", sink_at(Sink::Path, FILE)),
    ("os.path.join", sink_at(Sink::Path, Arg::Any)),
    ("int", Role::Sanitizer),
    ("float", Role::Sanitizer),
    ("bool", Role::Sanitizer),
    ("len", Role::Sanitizer),
    ("SQL.identifier", Role::Sanitizer),
    ("escape", Role::Sanitizer),
    ("html_escape", Role::Sanitizer),
    ("html_sanitize", Role::Sanitizer),
    ("plaintext2html", Role::Sanitizer),
    // QWeb escapes the values it renders.
    ("_render", Role::Sanitizer),
    ("_render_template", Role::Sanitizer),
    ("urlencode", Role::Sanitizer),
    ("url_encode", Role::Sanitizer),
    ("secure_filename", Role::Sanitizer),
    // An attribute's value, or the default, and not its name.
    ("getattr", Role::Keeps(&[0, 2], &[])),
    // The query parameters go URL-encoded into the query of the URL (portal/controllers/portal.py).
    ("_build_url_w_params", Role::Keeps(&[0], &["url_string"])),
    // A name without its directories, or a stream of the data, opens no other file.
    ("basename", Role::Clears("OD707")),
    ("BytesIO", Role::Clears("OD707")),
    ("StringIO", Role::Clears("OD707")),
];

/// Keywords that carry the argument a sink checks when it is not passed by position.
const SINK_KEYWORDS: &[&str] = &["query", "code", "expr", "vals", "vals_list"];

/// The built-in row a dotted path matches, with its pattern.
fn built_in(path: &[String]) -> Option<(&'static str, Role)> {
    TABLE.iter().copied().find(|(pattern, role)| {
        let whole = matches!(role, Role::Sink(row) if row.whole);
        matches_path(pattern, path) && (!whole || path.len() == 1)
    })
}

/// The rows a configuration adds to `TABLE`, matched by the same dotted patterns. They match
/// first, so that a team's `sandbox.safe_eval` sanitizer wins over the built-in `safe_eval` sink.
/// A configured source is read as a mapping, whose keys the user picks too.
#[derive(Clone, Debug, Default)]
pub struct TaintTable {
    rows: Vec<(String, Role)>,
}

impl TaintTable {
    pub fn source(&mut self, pattern: &str) -> Result<(), String> {
        self.add(pattern, Role::Source(Kind::Keys))
    }

    pub fn sanitizer(&mut self, pattern: &str) -> Result<(), String> {
        self.add(pattern, Role::Sanitizer)
    }

    /// A sink reported as `rule`, the code or name of one of OD701 to OD707.
    pub fn sink(&mut self, rule: &str, pattern: &str) -> Result<(), String> {
        let kind = match crate::rules::find(rule).map(|r| r.code) {
            Some("OD701") => Sink::Sql,
            Some("OD702") => Sink::Eval,
            Some("OD703") => Sink::Markup,
            Some("OD704") => Sink::SudoWrite,
            Some("OD705") => Sink::Redirect,
            Some("OD706") => Sink::Request,
            Some("OD707") => Sink::Path,
            _ => {
                return Err(format!(
                    "'{rule}' is not a taint rule: name OD701 to OD707 by code or name"
                ));
            }
        };
        self.add(pattern, sink(kind))
    }

    fn add(&mut self, pattern: &str, role: Role) -> Result<(), String> {
        if !valid_pattern(pattern) {
            return Err(format!(
                "'{pattern}' is not a taint pattern: write dotted names such as cr.execute, where a name but the last may end in () and [] stands for an item"
            ));
        }
        self.rows.push((pattern.to_owned(), role));
        Ok(())
    }

    /// The row a dotted path matches, with its pattern: a configured one before a built-in one.
    fn lookup(&self, path: &[String]) -> Option<(&str, Role)> {
        self.rows
            .iter()
            .find(|(pattern, _)| matches_path(pattern, path))
            .map(|(pattern, role)| (pattern.as_str(), *role))
            .or_else(|| built_in(path))
    }
}

/// Whether each segment of `pattern` is a name, a name and `()`, or `[]`, the last no call, as
/// `Flows::path` writes a path and `matches_path` compares one.
fn valid_pattern(pattern: &str) -> bool {
    let identifier = |s: &str| {
        let mut chars = s.chars();
        chars.next().is_some_and(|c| c == '_' || c.is_alphabetic()) && chars.all(|c| c == '_' || c.is_alphanumeric())
    };
    let segments: Vec<&str> = pattern.split('.').collect();
    let last = segments.len() - 1;
    segments
        .iter()
        .enumerate()
        .all(|(i, segment)| match segment.strip_suffix("()") {
            Some(name) => i != last && identifier(name),
            None => *segment == "[]" || identifier(segment),
        })
}

fn matches_path(pattern: &str, path: &[String]) -> bool {
    let mut names = pattern.split('.').rev();
    let Some((last, rest)) = path.split_last() else {
        return false;
    };
    if names.next() != Some(last.as_str()) {
        return false;
    }
    let mut rest = rest.iter().rev();
    names.all(|name| rest.any(|p| p == name))
}

/// Which parameters of a function a user sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Params {
    None,
    /// A controller method behind `@route`: its URL and form parameters.
    Route,
    /// A public model method, which any user with access to the model can call over RPC.
    Rpc,
}

#[derive(Clone, Debug)]
struct Taint {
    kind: Kind,
    origin: FlowOrigin,
    /// Where the value entered, then each local it was assigned to.
    steps: Vec<Spanned<String>>,
    safe: Safe,
}

impl Taint {
    fn kind(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
    }

    fn safe(mut self, safe: Safe) -> Self {
        self.safe = self.safe.union(safe);
        self
    }
}

/// Of two values, the one the user sets on entry wins over one a caller or callee may taint, so
/// that a finding in the function is never traded for a summary edge. Among those, keys win so
/// that OD704 still sees them. A source joined to an RPC argument stays the source, with the keys
/// of either: a Python caller sets only the argument, so its edge would drop the source. The
/// result is harmless only where both are.
fn join(a: Option<Taint>, b: Option<Taint>) -> Option<Taint> {
    let rank = |t: &Taint| (t.origin.is_user(), t.kind);
    match (a, b) {
        (Some(a), Some(b)) => {
            let safe = a.safe.intersect(b.safe);
            let kind = a.kind.max(b.kind);
            let mut t = match (a.origin, b.origin) {
                (FlowOrigin::Source, FlowOrigin::Rpc(_)) => a.kind(kind),
                (FlowOrigin::Rpc(_), FlowOrigin::Source) => b.kind(kind),
                _ if rank(&b) > rank(&a) => b,
                _ => a,
            };
            t.safe = safe;
            Some(t)
        }
        (a, b) => a.or(b),
    }
}

/// The codes a string's start keeps harmless when the code, not the user, picks it: a redirect or
/// a request then goes to the host the code chose.
const FIXED: &[&str] = &["OD705", "OD706"];

/// Whether a URL that starts with `s` has the host the code chose, whatever follows: `s` puts what
/// follows in the query or the fragment, is a path on the same host, or names its host.
fn fixes_start(s: &str) -> bool {
    let path = s
        .strip_prefix('/')
        .and_then(|rest| rest.chars().next())
        .is_some_and(|c| c != '/' && c != '\\');
    let host = s.split_once("://").is_some_and(|(scheme, rest)| {
        let scheme_ok = scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
        scheme_ok
            && rest
                .split_once('/')
                .is_some_and(|(host, _)| !host.is_empty() && !host.contains('\\'))
    });
    s.contains(['?', '#']) || path || host
}

/// What a string that starts with `expr`, of taint `t`, keeps harmless for its start.
fn start_of(expr: &Expr, t: Option<&Taint>) -> Safe {
    match constant(expr) {
        Some(s) => literal_start(&s),
        None => start_of_value(t),
    }
}

fn literal_start(s: &str) -> Safe {
    if fixes_start(s) {
        Safe::of(FIXED)
    } else {
        Safe::default()
    }
}

/// A value the user does not control, or one that starts as the code picked.
fn start_of_value(t: Option<&Taint>) -> Safe {
    t.map_or(Safe::of(FIXED), |t| t.safe.intersect(Safe::of(FIXED)))
}

/// The text of a string the code writes out whole: a literal, an f-string without fields, or a
/// concatenation of them.
fn constant(expr: &Expr) -> Option<String> {
    match expr {
        Expr::StringLiteral(s) => Some(s.value.to_str().to_owned()),
        Expr::FString(f) => fstring_text(f),
        Expr::BinOp(b) if b.op == Operator::Add => Some(constant(&b.left)? + &constant(&b.right)?),
        _ => None,
    }
}

fn fstring_text(f: &ast::ExprFString) -> Option<String> {
    let mut text = String::new();
    for part in f.value.iter() {
        let elements = match part {
            ast::FStringPartRef::Literal(l) => {
                text.push_str(&l.value);
                continue;
            }
            ast::FStringPartRef::FString(f) => &f.elements,
        };
        for e in elements {
            let ast::InterpolatedStringElement::Literal(l) = e else {
                return None;
            };
            text.push_str(&l.value);
        }
    }
    Some(text)
}

/// Where `left <op> right` starts, `first` being the first value `%` puts in a template.
fn bin_start(b: &ast::ExprBinOp, left: Option<&Taint>, first: Option<(&Expr, Option<Taint>)>) -> Safe {
    match b.op {
        Operator::Add => start_of(&b.left, left),
        Operator::Mod => constant(&b.left).map_or_else(Safe::default, |template| {
            match template_prefix(&template, '%') {
                (prefix, _) if !prefix.is_empty() => literal_start(&prefix),
                // A mapping's values have no first one.
                (_, rest) if rest.starts_with('(') => Safe::default(),
                _ => first.map_or_else(Safe::default, |(e, t)| start_of(e, t.as_ref())),
            }
        }),
        _ => Safe::default(),
    }
}

/// The text of a template before its first field, which `open` starts, and what follows the
/// field's `open`. A doubled `open` is the character itself.
fn template_prefix(template: &str, open: char) -> (String, &str) {
    let mut prefix = String::new();
    let mut chars = template.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != open {
            prefix.push(c);
        } else if chars.peek().is_some_and(|(_, next)| *next == open) {
            prefix.push(c);
            chars.next();
        } else {
            return (prefix, &template[i + c.len_utf8()..]);
        }
    }
    (prefix, "")
}

fn values(t: Option<Taint>) -> Option<Taint> {
    t.map(|t| t.kind(Kind::Value))
}

#[derive(Clone, Default)]
struct State {
    vars: FxHashMap<String, Taint>,
    /// Locals bound to a recordset under `sudo()`.
    sudo: FxHashSet<String>,
    /// Locals bound to a model, as `Orders = env["sale.order"]` does.
    models: FxHashMap<String, Receiver>,
}

impl State {
    fn merge(&mut self, other: State) {
        for (name, t) in other.vars {
            let mine = self.vars.remove(&name);
            if let Some(t) = join(mine, Some(t)) {
                self.vars.insert(name, t);
            }
        }
        self.sudo.extend(other.sudo);
        self.models.extend(other.models);
    }
}

/// The flows of one function: findings for those that start in it, and its summary when data
/// may cross into or out of the methods it calls. `owner` is what `self` is, and `series` that of
/// the module, an unknown one read as the newest.
pub fn function(
    func: &ast::StmtFunctionDef,
    params: Params,
    owner: Option<Receiver>,
    series: Option<Series>,
    loc: &Locator,
    table: &TaintTable,
) -> (Vec<RawDiag>, Option<FlowSummary>) {
    let mut flows = Flows {
        loc,
        table,
        series,
        state: State::default(),
        locals: FxHashSet::default(),
        owner,
        calls: vec![],
        edges: vec![],
        diags: vec![],
    };
    let names = flows.params(&func.parameters, params);
    flows.body(&func.body);
    let mut seen = FxHashSet::default();
    flows.diags.retain(|d| seen.insert((d.code, d.pos)));
    let summary = (!flows.edges.is_empty()).then(|| FlowSummary {
        owner: flows.owner,
        method: func.name.to_string(),
        params: names,
        calls: flows.calls,
        edges: flows.edges,
    });
    (flows.diags, summary)
}

struct Flows<'a> {
    loc: &'a Locator<'a>,
    table: &'a TaintTable,
    series: Option<Series>,
    state: State,
    /// Every name the function binds, so that `x.f()` on another name is a module's function.
    locals: FxHashSet<String>,
    owner: Option<Receiver>,
    calls: Vec<CallSite>,
    edges: Vec<FlowEdge>,
    diags: Vec<RawDiag>,
}

impl Flows<'_> {
    /// Taints each parameter but `self`: as a source when the user sets it, otherwise by its
    /// position, for the callers' flows. Returns their names.
    fn params(&mut self, params: &ast::Parameters, source: Params) -> Vec<String> {
        let positional = params.posonlyargs.iter().chain(&params.args).chain(&params.kwonlyargs);
        let named = positional.map(|p| (&p.parameter, false));
        let rest = params.vararg.iter().map(|p| (&**p, false));
        let kwarg = params.kwarg.iter().map(|p| (&**p, true));
        let mut names = vec![];
        for (param, mapping) in named.chain(rest).chain(kwarg) {
            let name = param.name.as_str();
            self.locals.insert(name.to_owned());
            if matches!(name, "self" | "cls") {
                continue;
            }
            let (label, kind, origin) = match source {
                Params::None => ("parameter", Kind::Keys, FlowOrigin::Param(names.len())),
                Params::Route if mapping => ("route parameter", Kind::Keys, FlowOrigin::Source),
                Params::Route => ("route parameter", Kind::Value, FlowOrigin::Source),
                Params::Rpc => ("RPC argument", Kind::Keys, FlowOrigin::Rpc(names.len())),
            };
            names.push(name.to_owned());
            let steps = vec![Spanned {
                value: format!("{label} '{name}'"),
                pos: self.loc.at(param),
            }];
            let safe = Safe::default();
            self.state.vars.insert(
                name.to_owned(),
                Taint {
                    kind,
                    origin,
                    steps,
                    safe,
                },
            );
        }
        names
    }

    fn body(&mut self, body: &[Stmt]) {
        for stmt in body {
            self.stmt(stmt);
        }
    }

    /// Runs `f` on a copy of the state and merges what it leaves into the state before it.
    fn branch(&mut self, f: impl FnOnce(&mut Self)) {
        let before = self.state.clone();
        f(self);
        let after = std::mem::replace(&mut self.state, before);
        self.state.merge(after);
    }

    /// A loop body runs twice, so that a value tainted late in it reaches the code above.
    fn repeat(&mut self, body: &[Stmt]) {
        self.branch(|f| {
            f.body(body);
            f.body(body);
        });
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Assign(a) => {
                let t = self.expr(&a.value);
                for target in &a.targets {
                    self.bind(target, t.clone(), Some(&a.value));
                }
            }
            Stmt::AnnAssign(ast::StmtAnnAssign {
                target,
                value: Some(value),
                ..
            }) => {
                let t = self.expr(value);
                self.bind(target, t, Some(value));
            }
            Stmt::AugAssign(a) => {
                let t = join(self.expr(&a.target), self.expr(&a.value));
                self.bind(&a.target, values(t), None);
            }
            Stmt::Expr(e) => {
                self.expr(&e.value);
            }
            Stmt::Return(ast::StmtReturn { value: Some(v), .. }) => {
                if let Some(t) = self.expr(v) {
                    self.edge(t, FlowTarget::Return);
                }
            }
            Stmt::If(i) => self.if_stmt(i),
            _ => self.compound(stmt),
        }
    }

    fn compound(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::For(f) => {
                let t = values(self.expr(&f.iter));
                self.branch(|s| {
                    s.bind(&f.target, t, None);
                    s.body(&f.body);
                    s.body(&f.body);
                });
                self.body(&f.orelse);
            }
            Stmt::While(w) => {
                self.expr(&w.test);
                self.repeat(&w.body);
                self.body(&w.orelse);
            }
            Stmt::With(w) => {
                for item in &w.items {
                    let t = self.expr(&item.context_expr);
                    if let Some(target) = &item.optional_vars {
                        self.bind(target, t, None);
                    }
                }
                self.body(&w.body);
            }
            Stmt::Try(t) => self.try_stmt(t),
            Stmt::Assert(a) => {
                self.expr(&a.test);
                self.guard(&a.test, CmpOp::In);
            }
            _ => {}
        }
    }

    fn if_stmt(&mut self, i: &ast::StmtIf) {
        let before = self.state.clone();
        let mut joined = None;
        self.arm(Some(&i.test), &i.body, &before, &mut joined);
        for clause in &i.elif_else_clauses {
            self.arm(clause.test.as_ref(), &clause.body, &before, &mut joined);
        }
        if i.elif_else_clauses.last().is_none_or(|c| c.test.is_some()) {
            self.state = before.clone();
            if i.elif_else_clauses.is_empty() && exits(&i.body) {
                self.guard(&i.test, CmpOp::NotIn);
            }
            let rest = std::mem::take(&mut self.state);
            merge_into(&mut joined, rest);
        }
        self.state = joined.unwrap_or(before);
    }

    /// Runs one branch of an `if` from the state before it; a branch that leaves the function or
    /// the loop adds nothing to the state after it.
    fn arm(&mut self, test: Option<&Expr>, body: &[Stmt], before: &State, joined: &mut Option<State>) {
        self.state = before.clone();
        if let Some(test) = test {
            self.expr(test);
        }
        self.body(body);
        if !exits(body) {
            let after = std::mem::take(&mut self.state);
            merge_into(joined, after);
        }
    }

    /// Applies a check that holds from here on: `x in allowed` leaves `x` harmless, and a test
    /// that every key of a mapping is allowed takes the choice of fields away from the user. `op`
    /// is the comparison the check holds for: `in` after an `assert`, `not in` after an `if` that
    /// raises.
    fn guard(&mut self, test: &Expr, op: CmpOp) {
        let mut checks = Checks {
            op,
            values: vec![],
            keys: vec![],
            starts: vec![],
        };
        checks.visit_expr(test);
        for name in checks.values {
            self.state.vars.remove(&name);
        }
        for name in checks.keys {
            if let Some(t) = self.state.vars.get_mut(&name) {
                t.kind = Kind::Value;
            }
        }
        for name in checks.starts {
            if let Some(t) = self.state.vars.get_mut(&name) {
                t.safe = t.safe.union(Safe::of(FIXED));
            }
        }
    }

    fn try_stmt(&mut self, t: &ast::StmtTry) {
        self.body(&t.body);
        for handler in &t.handlers {
            let ast::ExceptHandler::ExceptHandler(h) = handler;
            self.branch(|s| s.body(&h.body));
        }
        self.body(&t.orelse);
        self.body(&t.finalbody);
    }

    /// Assigns `t` to a target. A plain name or an attribute of `self` loses what it held; an item
    /// or attribute set on either adds the value to it.
    fn bind(&mut self, target: &Expr, t: Option<Taint>, value: Option<&Expr>) {
        if let Some(key) = self_attr(target) {
            return self.set(key, target, t);
        }
        match target {
            Expr::Name(n) => {
                let name = n.id.as_str();
                self.locals.insert(name.to_owned());
                if value.is_some_and(|v| self.is_sudo(v)) {
                    self.state.sudo.insert(name.to_owned());
                } else {
                    self.state.sudo.remove(name);
                }
                match value.and_then(|v| self.receiver(v)) {
                    Some(r) => self.state.models.insert(name.to_owned(), r),
                    None => self.state.models.remove(name),
                };
                self.set(name.to_owned(), target, t);
            }
            Expr::Tuple(ast::ExprTuple { elts, .. }) | Expr::List(ast::ExprList { elts, .. }) => {
                for elt in elts {
                    self.bind(elt, values(t.clone()), None);
                }
            }
            Expr::Starred(s) => self.bind(&s.value, t, None),
            Expr::Subscript(ast::ExprSubscript { value: base, .. })
            | Expr::Attribute(ast::ExprAttribute { value: base, .. }) => self.add_to(base, values(t)),
            _ => {}
        }
    }

    fn set(&mut self, key: String, target: &Expr, t: Option<Taint>) {
        match t {
            Some(mut t) => {
                t.steps.push(Spanned {
                    value: key.clone(),
                    pos: self.loc.at(target),
                });
                self.state.vars.insert(key, t);
            }
            None => {
                self.state.vars.remove(&key);
            }
        }
    }

    /// Adds a tainted value to the local or attribute of `self` that `base` names, as
    /// `vals["x"] = t` or `vals.update(t)` do. `self` itself stays clean, so that the methods
    /// called on it are still followed.
    fn add_to(&mut self, base: &Expr, t: Option<Taint>) {
        let name = match base {
            Expr::Name(n) if !matches!(n.id.as_str(), "self" | "cls") => n.id.to_string(),
            _ => match self_attr(base) {
                Some(key) => key,
                None => return,
            },
        };
        let Some(t) = t else { return };
        let mine = self.state.vars.remove(&name);
        if let Some(t) = join(mine, Some(t)) {
            self.state.vars.insert(name, t);
        }
    }

    /// The dotted path of a callee or attribute, a call ending a name with `()` and an item
    /// access adding `[]`. A local bound under `sudo()` reads as `sudo()`.
    fn path(&self, expr: &Expr) -> Vec<String> {
        match expr {
            Expr::Name(n) if self.state.sudo.contains(n.id.as_str()) => vec!["sudo()".to_owned()],
            Expr::Name(n) => vec![n.id.to_string()],
            Expr::Attribute(a) => {
                let mut path = self.path(&a.value);
                path.push(a.attr.to_string());
                path
            }
            Expr::Call(c) => {
                let mut path = self.path(&c.func);
                if let Some(last) = path.last_mut() {
                    last.push_str("()");
                }
                path
            }
            Expr::Subscript(s) => {
                let mut path = self.path(&s.value);
                path.push("[]".to_owned());
                path
            }
            _ => vec![],
        }
    }

    fn is_sudo(&self, expr: &Expr) -> bool {
        matches!(expr, Expr::Call(_) | Expr::Name(_)) && self.path(expr).iter().any(|p| p == "sudo()")
    }

    fn source(&self, label: String, expr: &Expr, kind: Kind) -> Option<Taint> {
        let steps = vec![Spanned {
            value: label,
            pos: self.loc.at(expr),
        }];
        Some(Taint {
            kind,
            origin: FlowOrigin::Source,
            steps,
            safe: Safe::default(),
        })
    }

    /// The methods a call on `expr` runs, when a summary may tell what they do: `self`, a model
    /// of `env`, or a local bound to one, through methods that keep the model such as `sudo()`.
    fn receiver(&self, expr: &Expr) -> Option<Receiver> {
        match expr {
            Expr::Name(n) if n.id.as_str() == "self" => self.owner.clone(),
            Expr::Name(n) => self.state.models.get(n.id.as_str()).cloned(),
            Expr::Subscript(sub) if tail_name(&sub.value) == Some("env") => {
                str_lit(&sub.slice).map(|m| Receiver::Model(m.to_owned()))
            }
            Expr::Call(c) => match &*c.func {
                Expr::Attribute(a) if SAME_MODEL.contains(&a.attr.as_str()) => self.receiver(&a.value),
                _ => None,
            },
            _ => None,
        }
    }

    fn edge(&mut self, t: Taint, target: FlowTarget) {
        if let Some(e) = self
            .edges
            .iter_mut()
            .find(|e| e.origin == t.origin && e.target == target)
        {
            // Harmless at the target only if it is so on every path that gets there.
            e.safe = e.safe.intersect(t.safe);
            return;
        }
        self.edges.push(FlowEdge {
            origin: t.origin,
            target,
            keys: t.kind == Kind::Keys,
            safe: t.safe,
            steps: t.steps,
        });
    }

    /// Records a call to a method the index may resolve, with the arguments that carry taint,
    /// and returns what it may return.
    fn method_call(
        &mut self,
        call: &ast::ExprCall,
        expr: &Expr,
        args: Vec<Option<Taint>>,
        keywords: Vec<Option<Taint>>,
    ) -> Option<Taint> {
        let Expr::Attribute(a) = &*call.func else { return None };
        let receiver = self.receiver(&a.value)?;
        let method = a.attr.to_string();
        let pos = self.loc.at(expr);
        let runs_as = runs_as(&self.path(&a.value));
        let site = CallSite {
            receiver,
            method,
            pos,
            runs_as,
        };
        let c = match self.calls.iter().position(|s| *s == site) {
            Some(c) => c,
            None => {
                self.calls.push(site);
                self.calls.len() - 1
            }
        };
        let slots = (0..args.len()).map(ArgSlot::Pos).chain(
            call.arguments
                .keywords
                .iter()
                .map(|k| ArgSlot::Name(k.arg.as_ref().map(|a| a.to_string()).unwrap_or_default())),
        );
        for (slot, t) in slots.zip(args.into_iter().chain(keywords)) {
            if let Some(t) = t {
                self.edge(t, FlowTarget::Arg(c, slot));
            }
        }
        let steps = vec![Spanned {
            value: format!("{}()", a.attr),
            pos,
        }];
        Some(Taint {
            kind: Kind::Keys,
            origin: FlowOrigin::Return(c),
            steps,
            safe: Safe::default(),
        })
    }

    fn expr(&mut self, expr: &Expr) -> Option<Taint> {
        match expr {
            Expr::Name(n) => self.state.vars.get(n.id.as_str()).cloned(),
            Expr::Attribute(a) => match self.table.lookup(&self.path(expr)) {
                Some((pattern, Role::Source(kind))) => self.source(pattern.to_owned(), expr, kind),
                _ => match self_attr(expr).and_then(|key| self.state.vars.get(&key)) {
                    Some(t) => Some(t.clone()),
                    None => values(self.expr(&a.value)),
                },
            },
            Expr::Subscript(s) => {
                self.expr(&s.slice);
                values(self.expr(&s.value))
            }
            Expr::Call(c) => self.call(c, expr),
            Expr::BinOp(b) => self.bin_op(b),
            Expr::FString(f) => self.fstring(f),
            Expr::Named(n) => {
                let t = self.expr(&n.value);
                self.bind(&n.target, t.clone(), Some(&n.value));
                t
            }
            _ => self.collection(expr),
        }
    }

    fn collection(&mut self, expr: &Expr) -> Option<Taint> {
        match expr {
            Expr::List(ast::ExprList { elts, .. })
            | Expr::Tuple(ast::ExprTuple { elts, .. })
            | Expr::Set(ast::ExprSet { elts, .. }) => self.all(elts.iter()),
            Expr::Dict(d) => self.dict(d),
            Expr::DictComp(d) => self.dict_comp(d),
            Expr::ListComp(ast::ExprListComp { elt, generators, .. })
            | Expr::SetComp(ast::ExprSetComp { elt, generators, .. })
            | Expr::Generator(ast::ExprGenerator { elt, generators, .. }) => {
                self.comprehension(generators, |s| values(s.expr(elt)))
            }
            _ => self.operator(expr),
        }
    }

    fn operator(&mut self, expr: &Expr) -> Option<Taint> {
        match expr {
            Expr::BoolOp(b) => self.all(b.values.iter()),
            Expr::If(i) => {
                self.expr(&i.test);
                let body = self.expr(&i.body);
                join(body, self.expr(&i.orelse))
            }
            Expr::Starred(s) => self.expr(&s.value),
            Expr::Await(a) => self.expr(&a.value),
            Expr::Compare(c) => {
                self.all(c.operands.iter());
                None
            }
            Expr::UnaryOp(u) => {
                self.expr(&u.operand);
                None
            }
            _ => None,
        }
    }

    fn all<'e>(&mut self, exprs: impl Iterator<Item = &'e Expr>) -> Option<Taint> {
        exprs.fold(None, |acc, e| {
            let t = self.expr(e);
            join(acc, t)
        })
    }

    fn bin_op(&mut self, b: &ast::ExprBinOp) -> Option<Taint> {
        let left = self.expr(&b.left);
        let (right, first) = self.right_operand(b);
        // `Markup("<b>%s</b>") % value` escapes the value.
        if b.op == Operator::Mod && self.is_markup(&b.left) {
            return None;
        }
        // An intersection with values the code chose holds only those values.
        let chosen = |t: &Option<Taint>| t.as_ref().is_none_or(|t| matches!(t.origin, FlowOrigin::Return(_)));
        if b.op == Operator::BitAnd && (chosen(&left) || chosen(&right)) {
            return None;
        }
        let start = bin_start(b, left.as_ref(), first);
        let t = join(left, right);
        let t = if b.op == Operator::BitOr { t } else { values(t) };
        t.map(|t| t.safe(start))
    }

    /// The taint of the right operand, and the first value it puts in a `%` template with its own.
    fn right_operand<'e>(&mut self, b: &'e ast::ExprBinOp) -> (Option<Taint>, Option<(&'e Expr, Option<Taint>)>) {
        match &*b.right {
            Expr::Tuple(tuple) if b.op == Operator::Mod => {
                let ts: Vec<Option<Taint>> = tuple.elts.iter().map(|e| self.expr(e)).collect();
                let first = tuple.elts.first().zip(ts.first().cloned());
                (ts.into_iter().fold(None, join), first)
            }
            right => {
                let t = self.expr(right);
                (t.clone(), Some((right, t)))
            }
        }
    }

    /// Only the built-in row: a configured OD703 sink, such as one that marks a value safe, does
    /// not escape what `%` puts in it.
    fn is_markup(&self, expr: &Expr) -> bool {
        matches!(expr, Expr::Call(c) if built_in(&self.path(&c.func)).is_some_and(|(_, r)| matches!(r, Role::Sink(row) if row.sink == Sink::Markup)))
    }

    /// An f-string starts with the text before its first field, or without any with that field.
    fn fstring(&mut self, f: &ast::ExprFString) -> Option<Taint> {
        let mut prefix = String::new();
        let mut first = None;
        let mut t = None;
        for part in f.value.iter() {
            let elements = match part {
                ast::FStringPartRef::Literal(l) => {
                    if first.is_none() {
                        prefix.push_str(&l.value);
                    }
                    continue;
                }
                ast::FStringPartRef::FString(f) => &f.elements,
            };
            for e in elements {
                match e {
                    ast::InterpolatedStringElement::Literal(l) if first.is_none() => prefix.push_str(&l.value),
                    ast::InterpolatedStringElement::Literal(_) => {}
                    ast::InterpolatedStringElement::Interpolation(i) => {
                        let field = self.expr(&i.expression);
                        if first.is_none() {
                            first = Some(start_of(&i.expression, field.as_ref()));
                        }
                        t = join(t, field);
                    }
                }
            }
        }
        let start = if prefix.is_empty() {
            first.unwrap_or_default()
        } else {
            literal_start(&prefix)
        };
        values(t).map(|t| t.safe(start))
    }

    /// `{**t}` keeps the keys of `t`; a literal key set holds the user to the values.
    fn dict(&mut self, d: &ast::ExprDict) -> Option<Taint> {
        d.items.iter().fold(None, |acc, item| {
            let key = item.key.as_ref().map(|k| self.expr(k));
            let value = self.expr(&item.value);
            let t = match key {
                None => value,
                Some(key) => join(key.map(|t| t.kind(Kind::Keys)), values(value)),
            };
            join(acc, t)
        })
    }

    /// A comprehension whose keys come from the user names any field, unless an `if` filters it.
    fn dict_comp(&mut self, d: &ast::ExprDictComp) -> Option<Taint> {
        let filtered = d.generators.iter().any(|g| !g.ifs.is_empty());
        self.comprehension(&d.generators, |s| {
            let key = d.key.as_ref().and_then(|k| s.expr(k));
            let value = values(s.expr(&d.value));
            let key = key.map(|t| t.kind(if filtered { Kind::Value } else { Kind::Keys }));
            join(key, value)
        })
    }

    fn comprehension(
        &mut self,
        generators: &[ast::Comprehension],
        elt: impl FnOnce(&mut Self) -> Option<Taint>,
    ) -> Option<Taint> {
        let before = self.state.clone();
        for g in generators {
            let t = values(self.expr(&g.iter));
            self.bind(&g.target, t, None);
            self.all(g.ifs.iter());
        }
        let t = elt(self);
        self.state = before;
        t
    }

    fn call(&mut self, call: &ast::ExprCall, expr: &Expr) -> Option<Taint> {
        let receiver = match &*call.func {
            Expr::Attribute(a) => self.expr(&a.value),
            _ => None,
        };
        let args: Vec<Option<Taint>> = call.arguments.args.iter().map(|a| self.expr(a)).collect();
        let keywords: Vec<Option<Taint>> = call.arguments.keywords.iter().map(|k| self.expr(&k.value)).collect();
        let path = self.path(&call.func);
        match self.table.lookup(&path) {
            Some((pattern, Role::Source(kind))) => self.source(format!("{pattern}()"), expr, kind),
            Some((_, Role::Sanitizer)) => None,
            Some((_, Role::Clears(code))) => {
                let t = args.into_iter().chain(keywords).fold(None, join);
                values(t).map(|t| t.safe(Safe::of(&[code])))
            }
            Some((_, Role::Keeps(positions, names))) => {
                let kept = args.into_iter().enumerate().filter(|(i, _)| positions.contains(i));
                let named = call
                    .arguments
                    .keywords
                    .iter()
                    .zip(keywords)
                    .filter(|(k, _)| k.arg.as_ref().is_some_and(|a| names.contains(&a.as_str())));
                let t = kept.map(|(_, t)| t).chain(named.map(|(_, t)| t)).fold(None, join);
                values(t)
            }
            Some((pattern, Role::Sink(row))) => {
                if row.guard.is_none_or(|g| self.met(g, call)) {
                    let arg = sink_arg(row.arg, code(row.sink), call, &args, &keywords);
                    self.sink(row.sink, pattern, arg, expr);
                }
                None
            }
            None => {
                self.self_write(call, &args, &keywords);
                if receiver.is_none() && self.method_target(call) {
                    return self.method_call(call, expr, args, keywords);
                }
                let start = format_start(call, &args, &keywords);
                let args = args.into_iter().chain(keywords).fold(None, join);
                let t = self.result(call, &path, receiver, args);
                t.map(|t| t.safe(start))
            }
        }
    }

    /// Whether a call passes the literal the guard asks for, or the module's series predates the
    /// parameter.
    fn met(&self, guard: Guard, call: &ast::ExprCall) -> bool {
        if guard.since.is_some_and(|since| self.series.is_some_and(|s| s < since)) {
            return true;
        }
        let arg = match call.arguments.find_keyword(guard.keyword) {
            Some(k) => Some(&k.value),
            None => call.arguments.args.get(guard.pos),
        };
        matches!(arg, Some(Expr::BooleanLiteral(b)) if b.value == guard.value)
    }

    /// Keeps a parameter whose keys the user may pick that `write()` or `create()` receives on the
    /// records of a model method, for the callers that call the method under `sudo()`.
    fn self_write(&mut self, call: &ast::ExprCall, args: &[Option<Taint>], keywords: &[Option<Taint>]) {
        let Expr::Attribute(a) = &*call.func else { return };
        if !matches!(self.owner, Some(Receiver::Model(_))) || !matches!(a.attr.as_str(), "write" | "create") {
            return;
        }
        let records = self.path(&a.value);
        if runs_as(&records) != RunsAs::AsCaller {
            return;
        }
        let Some(mut t) = sink_arg(Arg::First, "OD704", call, args, keywords) else {
            return;
        };
        if t.kind == Kind::Keys && matches!(t.origin, FlowOrigin::Param(_)) {
            t.steps.push(Spanned {
                value: format!("{}.{}()", records.join("."), a.attr),
                pos: self.loc.at(call),
            });
            self.edge(t, FlowTarget::SelfWrite);
        }
    }

    /// What a call no row names returns: string methods keep the taint of their receiver and
    /// arguments, and a module's function or a pattern's `match()` that of its arguments. Methods
    /// of other objects, such as recordsets, return values the user does not control.
    fn result(
        &mut self,
        call: &ast::ExprCall,
        path: &[String],
        receiver: Option<Taint>,
        args: Option<Taint>,
    ) -> Option<Taint> {
        let method = path.last().map(String::as_str).unwrap_or_default();
        let Expr::Attribute(a) = &*call.func else {
            let keeps = method == "dict";
            return if keeps { args } else { values(args) };
        };
        if method == "update" {
            self.add_to(&a.value, args);
            return None;
        }
        // What a method returned is records as often as values: a call on it keeps what the
        // summary of that method may tell, and takes nothing from its own arguments.
        if receiver
            .as_ref()
            .is_some_and(|t| matches!(t.origin, FlowOrigin::Return(_)))
        {
            return receiver;
        }
        if let Some(t) = receiver {
            let kind = if method == "copy" { t.kind } else { Kind::Value };
            return join(Some(t.kind(kind)), values(args));
        }
        // A compiled pattern's match holds the groups of the string it matched. `search` is
        // left out, as on records it returns records rather than parts of the domain.
        let matched = matches!(method, "match" | "fullmatch");
        let plain = matches!(&*a.value, Expr::StringLiteral(_)) || self.is_module(&a.value);
        if plain || matched { values(args) } else { None }
    }

    /// A call of a model's method that a summary may describe; `sudo()` and the like return the
    /// same records.
    fn method_target(&self, call: &ast::ExprCall) -> bool {
        matches!(&*call.func, Expr::Attribute(a)
            if !SAME_MODEL.contains(&a.attr.as_str()) && self.receiver(&a.value).is_some())
    }

    /// A dotted name rooted in a name the function never binds, such as `json` or `tools.misc`.
    fn is_module(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Name(n) => !matches!(n.id.as_str(), "self" | "cls" | "env") && !self.locals.contains(n.id.as_str()),
            // `request.env` and its models return records, not values derived from the arguments.
            Expr::Attribute(a) => a.attr.as_str() != "env" && self.is_module(&a.value),
            _ => false,
        }
    }

    /// Reports a value from a source at a sink, and keeps one from a parameter or a call for the
    /// callers' and callees' flows.
    fn sink(&mut self, sink: Sink, pattern: &str, t: Option<Taint>, expr: &Expr) {
        let code = code(sink);
        let Some(mut t) = t.filter(|t| !t.safe.has(code)) else {
            return;
        };
        if sink == Sink::SudoWrite && t.kind != Kind::Keys {
            return;
        }
        let callee = format!("{pattern}()");
        let pos = self.loc.at(expr);
        if !t.origin.is_user() {
            t.steps.push(Spanned { value: callee, pos });
            return self.edge(t, FlowTarget::Sink(code));
        }
        let origin = &t.steps[0].value;
        let names: Vec<&str> = t.steps[1..].iter().map(|s| s.value.as_str()).collect();
        let message = message(code, &callee, origin, &names);
        let mut flow: Vec<Step> = t.steps.iter().map(step).collect();
        flow.push(Step {
            path: None,
            pos,
            label: callee,
        });
        self.diags.push(RawDiag::new(code, pos, message).with_flow(flow));
    }
}

/// Whose rights a method called on the records of the dotted path `records` runs with.
fn runs_as(records: &[String]) -> RunsAs {
    let last = |name: &str| records.iter().rposition(|p| p == name);
    let on_self = records.first().is_some_and(|p| p == "self") && records[1..].iter().all(|p| p.ends_with("()"));
    match (last("sudo()"), last("with_user()")) {
        (Some(sudo), user) if user.is_none_or(|user| sudo > user) => RunsAs::Superuser,
        (_, None) if on_self => RunsAs::AsCaller,
        _ => RunsAs::User,
    }
}

/// The key `self.<attr>` keeps a value under, as a local's name does.
fn self_attr(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Attribute(a) if matches!(&*a.value, Expr::Name(n) if n.id.as_str() == "self") => {
            Some(format!("self.{}", a.attr))
        }
        _ => None,
    }
}

/// Whether every prefix `startswith()` takes, one or a tuple, keeps the host.
fn fixed_prefixes(arg: &Expr) -> bool {
    let prefixes = match arg {
        Expr::Tuple(t) => t.elts.iter().collect(),
        p => vec![p],
    };
    !prefixes.is_empty() && prefixes.iter().all(|p| str_lit(p).is_some_and(fixes_start))
}

fn merge_into(joined: &mut Option<State>, state: State) {
    match joined {
        Some(j) => j.merge(state),
        None => *joined = Some(state),
    }
}

/// The names a membership test checks: `x <op> allowed` checks `x`, and
/// `any(k <op> allowed for k in vals)` checks the keys of `vals`. `starts` holds the names whose
/// start `x.startswith(p)` checks, `not` before it for `not in`.
struct Checks {
    op: CmpOp,
    values: Vec<String>,
    keys: Vec<String>,
    starts: Vec<String>,
}

impl Checks {
    fn checked(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Compare(c) if c.ops[..] == [self.op] => c.operands[0].as_name_expr().map(|n| n.id.to_string()),
            _ => None,
        }
    }

    /// The name whose start a `startswith()` test holds to prefixes that keep it harmless.
    fn started(&self, expr: &Expr) -> Option<String> {
        let call = match (self.op, expr) {
            (CmpOp::In, Expr::Call(call)) => call,
            (CmpOp::NotIn, Expr::UnaryOp(u)) if u.op == ast::UnaryOp::Not => u.operand.as_call_expr()?,
            _ => return None,
        };
        let Expr::Attribute(a) = &*call.func else { return None };
        let name = a.value.as_name_expr().filter(|_| a.attr.as_str() == "startswith")?;
        fixed_prefixes(call.arguments.args.first()?).then(|| name.id.to_string())
    }

    fn comprehension(&mut self, elt: &Expr, generators: &[ast::Comprehension]) {
        let mut inner = Checks {
            op: self.op,
            values: vec![],
            keys: vec![],
            starts: vec![],
        };
        inner.visit_expr(elt);
        for g in generators {
            let Some(target) = g.target.as_name_expr() else {
                continue;
            };
            let mapping = match &g.iter {
                Expr::Call(c) => match &*c.func {
                    Expr::Attribute(a) if a.attr.as_str() == "keys" => &*a.value,
                    _ => continue,
                },
                e => e,
            };
            if let Some(m) = mapping.as_name_expr()
                && inner.values.iter().any(|v| v == target.id.as_str())
            {
                self.keys.push(m.id.to_string());
            }
        }
    }
}

impl<'a> Visitor<'a> for Checks {
    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::ListComp(ast::ExprListComp { elt, generators, .. })
            | Expr::SetComp(ast::ExprSetComp { elt, generators, .. })
            | Expr::Generator(ast::ExprGenerator { elt, generators, .. }) => {
                return self.comprehension(elt, generators);
            }
            _ => {
                self.values.extend(self.checked(expr));
                self.starts.extend(self.started(expr));
            }
        }
        visitor::walk_expr(self, expr);
    }
}

/// The value of a call that a sink of `code` checks. Of every positional argument, the first not
/// harmless for the code.
fn sink_arg(
    arg: Arg,
    code: &str,
    call: &ast::ExprCall,
    args: &[Option<Taint>],
    keywords: &[Option<Taint>],
) -> Option<Taint> {
    let (pos, names) = match arg {
        Arg::First => (0, SINK_KEYWORDS),
        Arg::At(pos, names) => (pos, names),
        Arg::Any => return args.iter().flatten().find(|t| !t.safe.has(code)).cloned(),
    };
    if call.arguments.args.len() > pos {
        return args.get(pos).cloned().flatten();
    }
    call.arguments
        .keywords
        .iter()
        .zip(keywords)
        .find(|(k, _)| k.arg.as_ref().is_some_and(|a| names.contains(&a.as_str())))
        .and_then(|(_, t)| t.clone())
}

/// Where a literal template's `.format()` starts: the text before its first field, or the value
/// of that field.
fn format_start(call: &ast::ExprCall, args: &[Option<Taint>], keywords: &[Option<Taint>]) -> Safe {
    let Expr::Attribute(a) = &*call.func else {
        return Safe::default();
    };
    let Some(template) = constant(&a.value).filter(|_| a.attr.as_str() == "format") else {
        return Safe::default();
    };
    let (prefix, rest) = template_prefix(&template, '{');
    if !prefix.is_empty() {
        return literal_start(&prefix);
    }
    let field = rest.split(['}', ':', '!', '.', '[']).next().unwrap_or_default();
    let value = match field {
        "" | "0" => call.arguments.args.first().zip(args.first()),
        name => call
            .arguments
            .keywords
            .iter()
            .zip(keywords)
            .find(|(k, _)| k.arg.as_ref().is_some_and(|a| a.as_str() == name))
            .map(|(k, t)| (&k.value, t)),
    };
    value.map_or_else(Safe::default, |(e, t)| start_of(e, t.as_ref()))
}

fn step(s: &Spanned<String>) -> Step {
    Step {
        path: None,
        pos: s.pos,
        label: s.value.clone(),
    }
}

fn code(sink: Sink) -> &'static str {
    match sink {
        Sink::Sql => "OD701",
        Sink::Eval => "OD702",
        Sink::Markup => "OD703",
        Sink::SudoWrite => "OD704",
        Sink::Redirect => "OD705",
        Sink::Request => "OD706",
        Sink::Path => "OD707",
    }
}

/// The message of a flow from `origin` to the call `callee`, naming the locals and methods it
/// went through, without lines, so that a baseline keeps it when lines move.
pub(crate) fn message(code: &str, callee: &str, origin: &str, names: &[&str]) -> String {
    let mut through: Vec<&str> = vec![];
    for name in names {
        if through.last() != Some(name) {
            through.push(name);
        }
    }
    let through = if through.is_empty() {
        String::new()
    } else {
        format!(" through '{}'", through.join("', '"))
    };
    match code {
        "OD701" => format!(
            "{origin} reaches the query of {callee}{through}; pass it as a query parameter or build the query with odoo.tools.SQL"
        ),
        "OD702" => format!("{origin} reaches {callee}{through}, which runs it as code"),
        "OD703" => format!(
            "{origin} reaches {callee}{through} unescaped; escape it, or pass it as an argument of Markup's % or format()"
        ),
        "OD705" => format!(
            "{origin} reaches the URL of {callee}{through}, so the user picks the site the browser goes to; start the URL with a path the code picks"
        ),
        "OD706" => format!(
            "{origin} reaches the URL of {callee}{through}, so the user picks the host the server connects to; start the URL with a host the code picks"
        ),
        "OD707" => format!(
            "{origin} reaches the path of {callee}{through}, so the user picks the file the server opens; pass the name through secure_filename() or os.path.basename()"
        ),
        _ => format!(
            "{origin} reaches {callee}{through}, so the user picks the fields it sets as superuser; copy only the fields the user may set"
        ),
    }
}

#[cfg(test)]
mod tests {
    use ruff_python_parser::parse_module;

    use super::*;

    /// The findings of the first function in `src`, as `code message`.
    fn scan(params: Params, src: &str) -> Vec<String> {
        scan_with(&TaintTable::default(), params, src)
    }

    fn scan_with(table: &TaintTable, params: Params, src: &str) -> Vec<String> {
        scan_in(table, None, params, src)
    }

    /// The findings of the first function in `src`, in a module of `series`.
    fn scan_in(table: &TaintTable, series: Option<&str>, params: Params, src: &str) -> Vec<String> {
        let series = series.and_then(Series::parse);
        let parsed = parse_module(src).expect("valid Python");
        let Some(Stmt::FunctionDef(func)) = parsed.syntax().body.first() else {
            panic!("no function");
        };
        let loc = Locator::new(src);
        function(func, params, None, series, &loc, table)
            .0
            .into_iter()
            .map(|d| format!("{} {}", d.code, d.message.split(';').next().unwrap_or_default()))
            .collect()
    }

    #[test]
    fn follows_assignments_to_the_sink() {
        let src = "def f(self, term):\n    where = \"name = '%s'\" % term\n    q = 'SELECT 1 WHERE ' + where\n    self.env.cr.execute(q)\n";
        assert_eq!(
            scan(Params::Route, src),
            ["OD701 route parameter 'term' reaches the query of cr.execute() through 'where', 'q'"]
        );
        assert!(scan(Params::None, src).is_empty());
    }

    #[test]
    fn records_the_path_with_positions() {
        let src = "def f(self):\n    data = request.get_json_data()\n    safe_eval(data['x'])\n";
        let parsed = parse_module(src).unwrap();
        let Some(Stmt::FunctionDef(func)) = parsed.syntax().body.first() else {
            panic!()
        };
        let (diags, _) = function(
            func,
            Params::None,
            None,
            None,
            &Locator::new(src),
            &TaintTable::default(),
        );
        let flow: Vec<_> = diags[0].flow.iter().map(|s| (s.pos.line, s.label.as_str())).collect();
        assert_eq!(flow, [(2, "request.get_json_data()"), (2, "data"), (3, "safe_eval()")]);
    }

    #[test]
    fn joins_branches_and_skips_those_that_exit() {
        let joined = "def f(self, x):\n    q = 'a'\n    if self:\n        q = x\n    self.env.cr.execute(q)\n";
        assert_eq!(scan(Params::Route, joined).len(), 1);
        let exits =
            "def f(self, x):\n    q = 'a'\n    if self:\n        q = x\n        return\n    self.env.cr.execute(q)\n";
        assert!(scan(Params::Route, exits).is_empty());
        let both =
            "def f(self, x):\n    if self:\n        q = x\n    else:\n        q = 'a'\n    self.env.cr.execute(q)\n";
        assert_eq!(scan(Params::Route, both).len(), 1);
    }

    #[test]
    fn sudo_writes_need_user_chosen_keys() {
        let write = |body: &str| {
            scan(
                Params::Route,
                &format!("def f(self, name, **kw):\n    M = request.env['x'].sudo()\n{body}\n"),
            )
        };
        assert_eq!(write("    M.create(dict(kw))").len(), 1);
        assert_eq!(write("    M.create({**kw})").len(), 1);
        assert_eq!(write("    vals = {}\n    vals.update(kw)\n    M.create(vals)").len(), 1);
        assert_eq!(write("    M.browse(1).write(kw)").len(), 1);
        assert!(write("    M.create({'name': kw.get('name')})").is_empty());
        assert!(write("    M.create({'name': name})").is_empty());
        assert!(write("    M.create({k: kw[k] for k in kw if k in ('a',)})").is_empty());
        assert!(write("    request.env['x'].create(kw)").is_empty());
    }

    #[test]
    fn a_source_value_keeps_the_keys_of_an_rpc_mapping() {
        for body in [
            "    vals['note'] = request.params.get('note')\n    self.sudo().write(vals)",
            "    vals.update(note=request.params.get('note'))\n    self.sudo().write(vals)",
            "    self.sudo().write(dict(vals, note=request.params.get('note')))",
        ] {
            let src = format!("def save(self, vals):\n{body}\n");
            assert_eq!(scan(Params::Rpc, &src).len(), 1, "{body}");
        }
    }

    #[test]
    fn sanitizers_and_checks_end_the_flow() {
        let sql = |body: &str| scan(Params::Rpc, &format!("def f(self, x, vals):\n{body}\n"));
        assert!(sql("    self.env.cr.execute('SELECT %s' % int(x))").is_empty());
        assert!(sql("    self.env.cr.execute('SELECT %s', [x])").is_empty());
        assert!(sql("    self.env.cr.execute(SQL('SELECT %s', x))").is_empty());
        assert!(sql("    self.env.cr.execute(SQL('SELECT %s', SQL.identifier(x)))").is_empty());
        assert_eq!(sql("    self.env.cr.execute(SQL('SELECT ' + x))").len(), 1);
        assert!(sql("    assert x in ('a', 'b')\n    self.env.cr.execute('SELECT %s' % x)").is_empty());
        assert!(
            sql("    if x not in ('a', 'b'):\n        raise ValueError()\n    self.env.cr.execute('SELECT %s' % x)")
                .is_empty()
        );
        assert!(
            sql("    if any(k not in F for k in vals):\n        raise ValueError()\n    self.sudo().write(vals)")
                .is_empty()
        );
        assert!(sql("    self.sudo().write({k: 1 for k in set(vals) & set(F)})").is_empty());
        assert!(sql("    return Markup('<b>%s</b>') % x").is_empty());
        assert!(sql("    return escape(x)").is_empty());
    }

    #[test]
    fn records_and_recordsets_carry_no_taint() {
        let src = "def f(self, model, domain):\n    M = request.env.get(model)\n    q = M.sudo()._search(domain)\n    sql, params = q.select()\n    request.env.cr.execute(f'EXPLAIN {sql}', params)\n";
        assert!(scan(Params::Route, src).is_empty());
        let module = "def f(self, x):\n    self.env.cr.execute(json.dumps(x))\n";
        assert_eq!(scan(Params::Route, module).len(), 1);
    }

    #[test]
    fn markup_and_eval_sinks() {
        let src = "def f(self, **post):\n    a = Markup(post.get('a'))\n    b = Markup(f\"<p>{post['b']}</p>\")\n    return safe_eval(post['c'])\n";
        assert_eq!(
            scan(Params::Route, src),
            [
                "OD703 route parameter 'post' reaches Markup() unescaped",
                "OD703 route parameter 'post' reaches Markup() unescaped",
                "OD702 route parameter 'post' reaches safe_eval(), which runs it as code",
            ]
        );
    }

    fn summary(params: Params, src: &str) -> FlowSummary {
        let parsed = parse_module(src).expect("valid Python");
        let Some(Stmt::FunctionDef(func)) = parsed.syntax().body.first() else {
            panic!("no function");
        };
        let owner = Some(Receiver::Model("x.y".to_owned()));
        function(func, params, owner, None, &Locator::new(src), &TaintTable::default())
            .1
            .expect("a summary")
    }

    #[test]
    fn summaries_keep_flows_from_parameters_and_calls() {
        let helper = summary(
            Params::None,
            "def _h(self, a, b):\n    q = 'SELECT %s' % b\n    self.env.cr.execute(q)\n    return a\n",
        );
        let targets: Vec<_> = helper.edges.iter().map(|e| (e.origin, e.target.clone())).collect();
        assert_eq!(
            targets,
            [
                (FlowOrigin::Param(1), FlowTarget::Sink("OD701")),
                (FlowOrigin::Param(0), FlowTarget::Return),
            ]
        );
        assert_eq!(helper.params, ["a", "b"]);
        let route = summary(
            Params::Route,
            "def f(self, term):\n    M = request.env['x.y'].sudo()\n    rows = M._h(1, b=term)\n    return Markup(self._g())\n",
        );
        let calls: Vec<_> = route
            .calls
            .iter()
            .map(|c| (c.receiver.clone(), c.method.as_str()))
            .collect();
        assert_eq!(
            calls,
            [
                (Receiver::Model("x.y".to_owned()), "_h"),
                (Receiver::Model("x.y".to_owned()), "_g"),
            ]
        );
        let targets: Vec<_> = route.edges.iter().map(|e| (e.origin, e.target.clone())).collect();
        assert_eq!(
            targets,
            [
                (FlowOrigin::Source, FlowTarget::Arg(0, ArgSlot::Name("b".to_owned()))),
                (FlowOrigin::Return(1), FlowTarget::Sink("OD703")),
            ]
        );
    }

    #[test]
    fn a_configured_source_reaches_a_built_in_sink() {
        let mut table = TaintTable::default();
        table.source("lib.read_form").unwrap();
        let src = "def f(self):\n    term = lib.read_form()\n    self.env.cr.execute('SELECT %s' % term)\n";
        assert_eq!(
            scan_with(&table, Params::None, src),
            ["OD701 lib.read_form() reaches the query of cr.execute() through 'term'"]
        );
    }

    #[test]
    fn a_configured_sink_receives_a_route_parameter() {
        let mut table = TaintTable::default();
        table.sink("OD702", "tools.run_code").unwrap();
        let src = "def f(self, code):\n    tools.run_code(code)\n";
        assert_eq!(
            scan_with(&table, Params::Route, src),
            ["OD702 route parameter 'code' reaches tools.run_code(), which runs it as code"]
        );
    }

    #[test]
    fn a_configured_sanitizer_ends_the_flow() {
        let mut table = TaintTable::default();
        table.sanitizer("lib.clean_term").unwrap();
        let src = "def f(self, x):\n    self.env.cr.execute('SELECT %s' % lib.clean_term(x))\n";
        assert_eq!(scan(Params::Rpc, src).len(), 1);
        assert!(scan_with(&table, Params::Rpc, src).is_empty());
    }

    #[test]
    fn a_configured_row_wins_over_a_built_in_one() {
        let mut table = TaintTable::default();
        table.sanitizer("sandbox.safe_eval").unwrap();
        let eval = |call: &str| scan_with(&table, Params::Rpc, &format!("def f(self, x):\n    {call}(x)\n"));
        assert!(eval("sandbox.safe_eval").is_empty());
        assert_eq!(
            eval("safe_eval"),
            ["OD702 RPC argument 'x' reaches safe_eval(), which runs it as code"]
        );
    }

    #[test]
    fn a_configured_markup_sink_escapes_nothing() {
        let mut table = TaintTable::default();
        table.sink("markup-injection-flow", "raw").unwrap();
        let src = "def f(self, x):\n    return Markup(raw('%s') % x)\n";
        assert_eq!(
            scan_with(&table, Params::Rpc, src),
            ["OD703 RPC argument 'x' reaches Markup() unescaped"]
        );
    }

    #[test]
    fn configured_rows_take_patterns_and_taint_rules_only() {
        let mut table = TaintTable::default();
        for pattern in ["", "a..b", "a.b()", "1a", "a.", "a.[]()", "a-b"] {
            assert!(table.source(pattern).is_err(), "{pattern}");
        }
        for pattern in ["cr.execute", "sudo().write", "env.[].create", "_x9"] {
            assert!(table.source(pattern).is_ok(), "{pattern}");
        }
        let error = table.sink("OD101", "lib.run").unwrap_err();
        assert!(error.contains("'OD101'"), "{error}");
        assert!(table.sink("no-such-rule", "lib.run").is_err());
        assert!(table.sink("sql-injection-flow", "lib.run").is_ok());
        assert!(table.sink("od704", "lib.run").is_ok());
    }

    #[test]
    fn attributes_of_self_hold_values_like_locals() {
        let query = |attr: &str| {
            scan(
                Params::Route,
                &format!(
                    "def f(self, term):\n    self.term = term\n    self.env.cr.execute('SELECT %s' % self.{attr})\n"
                ),
            )
        };
        assert_eq!(
            query("term"),
            ["OD701 route parameter 'term' reaches the query of cr.execute() through 'self.term'"]
        );
        assert!(query("other").is_empty());
        let src =
            "def f(self, **kw):\n    self.vals = {}\n    self.vals.update(kw)\n    self.sudo().write(self.vals)\n";
        assert_eq!(
            scan(Params::Route, src),
            ["OD704 route parameter 'kw' reaches sudo().write(), so the user picks the fields it sets as superuser"]
        );
    }

    #[test]
    fn a_call_on_self_after_a_store_is_followed() {
        let route = summary(
            Params::Route,
            "def f(self, term):\n    self.term = term\n    self._h(term)\n",
        );
        let calls: Vec<_> = route.calls.iter().map(|c| c.method.as_str()).collect();
        assert_eq!(calls, ["_h"]);
    }

    /// Whether the first function of `src`, a method of `owner`, writes a parameter on its records.
    fn self_writes(owner: Receiver, src: &str) -> bool {
        let parsed = parse_module(src).expect("valid Python");
        let Some(Stmt::FunctionDef(func)) = parsed.syntax().body.first() else {
            panic!("no function");
        };
        let summary = function(
            func,
            Params::None,
            Some(owner),
            None,
            &Locator::new(src),
            &TaintTable::default(),
        )
        .1;
        summary.is_some_and(|s| s.edges.iter().any(|e| e.target == FlowTarget::SelfWrite))
    }

    #[test]
    fn a_model_method_writing_a_parameter_on_its_records_is_a_self_write() {
        let model = Receiver::Model("x.y".to_owned());
        let write =
            |owner: &Receiver, call: &str| self_writes(owner.clone(), &format!("def _f(self, vals):\n    {call}\n"));
        assert!(write(&model, "self.write(vals)"));
        assert!(write(&model, "self.browse(1).create(vals_list=vals)"));
        assert!(write(&model, "self.with_context(a=1).write(vals)"));
        assert!(!write(&model, "self.write({'name': vals['name']})"));
        assert!(!write(&model, "self.partner_id.write(vals)"));
        assert!(!write(&model, "self.env['y'].create(vals)"));
        assert!(!write(&model, "self.with_user(1).write(vals)"));
        assert!(!write(&Receiver::Class("C".to_owned()), "self.write(vals)"));
    }

    #[test]
    fn a_call_runs_as_its_receiver_shows() {
        let route = summary(
            Params::Route,
            "def f(self, **kw):\n    M = request.env['x.y'].sudo()\n    M._a(kw)\n    request.env['x.y']._a(kw)\n    request.env['x.y'].sudo()._a(kw)\n    request.env['x.y'].with_user(1).sudo()._a(kw)\n    request.env['x.y'].sudo().with_user(1)._a(kw)\n    self._a(kw)\n    self.with_context(a=1)._a(kw)\n    self.with_user(1)._a(kw)\n",
        );
        let runs_as: Vec<RunsAs> = route.calls.iter().map(|c| c.runs_as).collect();
        assert_eq!(
            runs_as,
            [
                RunsAs::Superuser,
                RunsAs::User,
                RunsAs::Superuser,
                RunsAs::Superuser,
                RunsAs::User,
                RunsAs::AsCaller,
                RunsAs::AsCaller,
                RunsAs::User,
            ]
        );
    }

    /// The codes `call` reports in a module of `series`, where `t` is a route parameter and `base`
    /// a value the user does not control.
    fn codes_in(series: Option<&str>, call: &str) -> Vec<String> {
        let src = format!("def f(self, t):\n    base = self.get_base_url()\n    {call}\n");
        let found = scan_in(&TaintTable::default(), series, Params::Route, &src);
        found.iter().map(|f| f[..5].to_owned()).collect()
    }

    fn codes(call: &str) -> Vec<String> {
        codes_in(None, call)
    }

    #[test]
    fn redirects_requests_and_paths_are_sinks() {
        for (call, code) in [
            ("werkzeug.utils.redirect(t)", "OD705"),
            ("werkzeug.redirect(location=t)", "OD705"),
            ("request.redirect(t, local=False)", "OD705"),
            ("request.redirect(t, 303, False)", "OD705"),
            ("request.redirect_query(t, {}, 303, False)", "OD705"),
            ("request.redirect_query(t, local=False)", "OD705"),
            ("requests.get(t, timeout=5)", "OD706"),
            ("requests.post(url=t)", "OD706"),
            ("requests.Session().put(t)", "OD706"),
            ("requests.request('GET', t)", "OD706"),
            ("requests.request(method='GET', url=t)", "OD706"),
            ("urllib.request.urlopen(t)", "OD706"),
            ("open(t)", "OD707"),
            ("open(file=t)", "OD707"),
            ("http.send_file(filepath_or_fp=t)", "OD707"),
            ("os.path.join(base, t)", "OD707"),
        ] {
            assert_eq!(codes(call), [code], "{call}");
        }
        for call in [
            "request.redirect(t)",
            "request.redirect(t, local=True)",
            "request.redirect_query(t, {})",
            "requests.request(t, base)",
            "Image.open(t)",
            "io.open(t)",
            "os.path.join(base, 'x')",
            "tools.file_open(t)",
        ] {
            assert!(codes(call).is_empty(), "{call}");
        }
    }

    #[test]
    fn a_redirect_is_local_from_15_0_on() {
        for series in ["12.0", "14.0"] {
            assert_eq!(codes_in(Some(series), "request.redirect(t)"), ["OD705"], "{series}");
        }
        for series in [Some("15.0"), Some("18.0"), None] {
            assert!(codes_in(series, "request.redirect(t)").is_empty(), "{series:?}");
        }
        assert_eq!(codes_in(Some("18.0"), "request.redirect(t, local=False)"), ["OD705"]);
    }

    #[test]
    fn a_name_without_its_directories_opens_no_other_file() {
        for call in [
            "open(os.path.basename(t))",
            "os.path.join(base, os.path.basename(t))",
            "http.send_file(io.BytesIO(t))",
            "open(werkzeug.utils.secure_filename(t))",
            "open('/tmp/' + basename(t))",
        ] {
            assert!(codes(call).is_empty(), "{call}");
        }
        assert_eq!(codes("open(basename(t) + t)"), ["OD707"]);
        assert_eq!(codes("self.env.cr.execute('SELECT %s' % basename(t))"), ["OD701"]);
        assert!(codes("self.env.cr.execute('SELECT %s' % secure_filename(t))").is_empty());
    }

    #[test]
    fn a_start_the_code_picks_keeps_the_host() {
        for call in [
            "werkzeug.utils.redirect(f'/shop/{t}')",
            "werkzeug.utils.redirect(f'https://example.com/{t}')",
            "werkzeug.utils.redirect(f'{base}{t}')",
            "werkzeug.utils.redirect('/shop?next=' + t)",
            "werkzeug.utils.redirect('#' + t + t)",
            "werkzeug.utils.redirect(base + t)",
            "werkzeug.utils.redirect('/shop/%s' % t)",
            "werkzeug.utils.redirect('%s/%s' % (base, t))",
            "werkzeug.utils.redirect('/shop/{}'.format(t))",
            "werkzeug.utils.redirect('{}/{}'.format(base, t))",
            "werkzeug.utils.redirect('{root}{t}'.format(root=base, t=t))",
            "requests.get('https://api.example.com/v1/%s' % t)",
            "werkzeug.utils.redirect('/shop?' + urllib.parse.urlencode(t))",
            "werkzeug.utils.redirect(werkzeug.urls.url_encode(t))",
        ] {
            assert!(codes(call).is_empty(), "{call}");
        }
        for call in [
            "werkzeug.utils.redirect(f'/{t}')",
            "werkzeug.utils.redirect(f'//{t}')",
            "werkzeug.utils.redirect('/\\\\' + t)",
            "werkzeug.utils.redirect(f'https://{t}')",
            "werkzeug.utils.redirect('https://example.com' + t)",
            "werkzeug.utils.redirect(t + '/shop')",
            "werkzeug.utils.redirect('%s/shop' % t)",
            "werkzeug.utils.redirect('%(a)s/shop' % {'a': t})",
            "werkzeug.utils.redirect('{}/shop'.format(t))",
            "werkzeug.utils.redirect(f'{t}/shop')",
            "werkzeug.utils.redirect(urllib.parse.urljoin(base, t))",
            "open('/tmp/' + t)",
        ] {
            assert_eq!(codes(call).len(), 1, "{call}");
        }
    }

    #[test]
    fn calls_that_keep_some_arguments() {
        for call in [
            "request.redirect(getattr(request.website, 'social_%s' % t, False), local=False)",
            "request.redirect(_build_url_w_params(base, {'next': t}), local=False)",
        ] {
            assert!(codes(call).is_empty(), "{call}");
        }
        for call in [
            "request.redirect(getattr(t, 'url'), local=False)",
            "request.redirect(getattr(base, 'url', t), local=False)",
            "request.redirect(_build_url_w_params(t, {}), local=False)",
            "request.redirect(_build_url_w_params(url_string=t, query_params={}), local=False)",
            "open(self.base_re.match(t).group('rest'))",
            "pat = self.base_re\n    open(pat.fullmatch(t)[1])",
        ] {
            assert_eq!(codes(call).len(), 1, "{call}");
        }
    }

    #[test]
    fn a_checked_start_keeps_the_host() {
        let redirect = |check: &str| {
            scan(
                Params::Route,
                &format!("def f(self, t):\n{check}\n    return werkzeug.utils.redirect(t)\n"),
            )
        };
        assert!(redirect("    if not t.startswith('/shop'):\n        raise ValueError()").is_empty());
        assert!(redirect("    assert t.startswith(('/web', 'https://example.com/'))").is_empty());
        assert_eq!(
            redirect("    if not t.startswith('/'):\n        raise ValueError()").len(),
            1
        );
        assert_eq!(
            redirect("    if t.startswith('/shop'):\n        raise ValueError()").len(),
            1
        );
        assert_eq!(redirect("    assert t.startswith(('/web', '//'))").len(), 1);
    }

    #[test]
    fn a_value_harmless_on_one_branch_only_reaches_the_sink() {
        let route = summary(
            Params::None,
            "def _h(self, t):\n    if self:\n        return '/shop/' + t\n    return t\n",
        );
        assert_eq!(route.edges.len(), 1);
        assert!(!route.edges[0].safe.has("OD705"));
    }

    #[test]
    fn loops_carry_taint_around() {
        let src = "def f(self, items):\n    q = 'a'\n    for item in items:\n        self.env.cr.execute(q)\n        q = item\n";
        assert_eq!(scan(Params::Rpc, src).len(), 1);
    }
}
