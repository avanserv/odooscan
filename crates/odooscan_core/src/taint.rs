//! Values a user controls, followed through the locals of one function to the calls that must not
//! receive them.

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, CmpOp, Expr, Operator, Stmt};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{RawDiag, Spanned, Step};
use crate::python::{Locator, exits};

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Source(Kind),
    Sink(Sink),
    Sanitizer,
}

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
    ("cr.execute", Role::Sink(Sink::Sql)),
    ("_cr.execute", Role::Sink(Sink::Sql)),
    ("SQL", Role::Sink(Sink::Sql)),
    ("safe_eval", Role::Sink(Sink::Eval)),
    ("eval", Role::Sink(Sink::Eval)),
    ("exec", Role::Sink(Sink::Eval)),
    ("Markup", Role::Sink(Sink::Markup)),
    ("sudo().create", Role::Sink(Sink::SudoWrite)),
    ("sudo().write", Role::Sink(Sink::SudoWrite)),
    ("int", Role::Sanitizer),
    ("float", Role::Sanitizer),
    ("bool", Role::Sanitizer),
    ("len", Role::Sanitizer),
    ("SQL.identifier", Role::Sanitizer),
    ("escape", Role::Sanitizer),
    ("html_escape", Role::Sanitizer),
    ("html_sanitize", Role::Sanitizer),
    ("plaintext2html", Role::Sanitizer),
];

/// Keywords that carry the argument a sink checks when it is not passed by position.
const SINK_KEYWORDS: &[&str] = &["query", "code", "expr", "vals", "vals_list"];

/// The row a dotted path matches, with its pattern.
fn lookup(path: &[String]) -> Option<(&'static str, Role)> {
    TABLE
        .iter()
        .find(|(pattern, _)| matches_path(pattern, path))
        .map(|&(pattern, role)| (pattern, role))
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
    /// Where the value entered, then each local it was assigned to.
    steps: Vec<Spanned<String>>,
}

impl Taint {
    fn kind(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
    }
}

fn join(a: Option<Taint>, b: Option<Taint>) -> Option<Taint> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.kind > a.kind { b } else { a }),
        (a, b) => a.or(b),
    }
}

fn values(t: Option<Taint>) -> Option<Taint> {
    t.map(|t| t.kind(Kind::Value))
}

#[derive(Clone, Default)]
struct State {
    vars: FxHashMap<String, Taint>,
    /// Locals bound to a recordset under `sudo()`.
    sudo: FxHashSet<String>,
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
    }
}

pub fn function(func: &ast::StmtFunctionDef, params: Params, loc: &Locator) -> Vec<RawDiag> {
    let mut flows = Flows {
        loc,
        state: State::default(),
        locals: FxHashSet::default(),
        diags: vec![],
    };
    flows.params(&func.parameters, params);
    flows.body(&func.body);
    let mut seen = FxHashSet::default();
    flows.diags.retain(|d| seen.insert((d.code, d.pos)));
    flows.diags
}

struct Flows<'a> {
    loc: &'a Locator<'a>,
    state: State,
    /// Every name the function binds, so that `x.f()` on another name is a module's function.
    locals: FxHashSet<String>,
    diags: Vec<RawDiag>,
}

impl Flows<'_> {
    fn params(&mut self, params: &ast::Parameters, source: Params) {
        let positional = params.posonlyargs.iter().chain(&params.args).chain(&params.kwonlyargs);
        let named = positional.map(|p| (&p.parameter, false));
        let rest = params.vararg.iter().map(|p| (&**p, false));
        let kwarg = params.kwarg.iter().map(|p| (&**p, true));
        for (param, mapping) in named.chain(rest).chain(kwarg) {
            let name = param.name.as_str();
            self.locals.insert(name.to_owned());
            if matches!(name, "self" | "cls") {
                continue;
            }
            let (label, kind) = match source {
                Params::None => continue,
                Params::Route if mapping => ("route parameter", Kind::Keys),
                Params::Route => ("route parameter", Kind::Value),
                Params::Rpc => ("RPC argument", Kind::Keys),
            };
            let steps = vec![Spanned {
                value: format!("{label} '{name}'"),
                pos: self.loc.at(param),
            }];
            self.state.vars.insert(name.to_owned(), Taint { kind, steps });
        }
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
                self.expr(v);
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

    /// Assigns `t` to a target. A plain name loses what it held; an item or attribute set on a
    /// local adds the value to it.
    fn bind(&mut self, target: &Expr, t: Option<Taint>, value: Option<&Expr>) {
        match target {
            Expr::Name(n) => {
                let name = n.id.as_str();
                self.locals.insert(name.to_owned());
                if value.is_some_and(|v| self.is_sudo(v)) {
                    self.state.sudo.insert(name.to_owned());
                } else {
                    self.state.sudo.remove(name);
                }
                match t {
                    Some(mut t) => {
                        t.steps.push(Spanned {
                            value: name.to_owned(),
                            pos: self.loc.at(target),
                        });
                        self.state.vars.insert(name.to_owned(), t);
                    }
                    None => {
                        self.state.vars.remove(name);
                    }
                }
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

    /// Adds a tainted value to the local `base` names, as `vals["x"] = t` or `vals.update(t)` do.
    fn add_to(&mut self, base: &Expr, t: Option<Taint>) {
        let (Expr::Name(n), Some(t)) = (base, t) else {
            return;
        };
        let name = n.id.to_string();
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
        Some(Taint { kind, steps })
    }

    fn expr(&mut self, expr: &Expr) -> Option<Taint> {
        match expr {
            Expr::Name(n) => self.state.vars.get(n.id.as_str()).cloned(),
            Expr::Attribute(a) => match lookup(&self.path(expr)) {
                Some((pattern, Role::Source(kind))) => self.source(pattern.to_owned(), expr, kind),
                _ => values(self.expr(&a.value)),
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
        let right = self.expr(&b.right);
        // `Markup("<b>%s</b>") % value` escapes the value.
        if b.op == Operator::Mod && self.is_markup(&b.left) {
            return None;
        }
        // An intersection with values the user does not control holds only those values.
        if b.op == Operator::BitAnd && (left.is_none() || right.is_none()) {
            return None;
        }
        let t = join(left, right);
        if b.op == Operator::BitOr { t } else { values(t) }
    }

    fn is_markup(&self, expr: &Expr) -> bool {
        matches!(expr, Expr::Call(c) if lookup(&self.path(&c.func)).is_some_and(|(_, r)| r == Role::Sink(Sink::Markup)))
    }

    fn fstring(&mut self, f: &ast::ExprFString) -> Option<Taint> {
        let parts: Vec<&Expr> = f
            .value
            .elements()
            .filter_map(|e| match e {
                ast::InterpolatedStringElement::Interpolation(i) => Some(&*i.expression),
                ast::InterpolatedStringElement::Literal(_) => None,
            })
            .collect();
        values(self.all(parts.into_iter()))
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
        match lookup(&path) {
            Some((pattern, Role::Source(kind))) => self.source(format!("{pattern}()"), expr, kind),
            Some((_, Role::Sanitizer)) => None,
            Some((pattern, Role::Sink(sink))) => {
                let arg = sink_arg(call, &args, &keywords);
                self.sink(sink, pattern, arg, expr);
                None
            }
            None => {
                let args = args.into_iter().chain(keywords).fold(None, join);
                self.result(call, &path, receiver, args)
            }
        }
    }

    /// What a call no row names returns: string methods keep the taint of their receiver and
    /// arguments, and a module's function that of its arguments. Methods of other objects, such
    /// as recordsets, return values the user does not control.
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
        if let Some(t) = receiver {
            let kind = if method == "copy" { t.kind } else { Kind::Value };
            return join(Some(t.kind(kind)), values(args));
        }
        let plain = matches!(&*a.value, Expr::StringLiteral(_)) || self.is_module(&a.value);
        if plain { values(args) } else { None }
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

    fn sink(&mut self, sink: Sink, pattern: &str, t: Option<Taint>, expr: &Expr) {
        let Some(t) = t else { return };
        if sink == Sink::SudoWrite && t.kind != Kind::Keys {
            return;
        }
        let callee = format!("{pattern}()");
        let pos = self.loc.at(expr);
        let (code, message) = message(sink, &callee, &t);
        let mut flow: Vec<Step> = t.steps.iter().map(step).collect();
        flow.push(Step {
            path: None,
            pos,
            label: callee,
        });
        self.diags.push(RawDiag::new(code, pos, message).with_flow(flow));
    }
}

fn merge_into(joined: &mut Option<State>, state: State) {
    match joined {
        Some(j) => j.merge(state),
        None => *joined = Some(state),
    }
}

/// The names a membership test checks: `x <op> allowed` checks `x`, and
/// `any(k <op> allowed for k in vals)` checks the keys of `vals`.
struct Checks {
    op: CmpOp,
    values: Vec<String>,
    keys: Vec<String>,
}

impl Checks {
    fn checked(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Compare(c) if c.ops[..] == [self.op] => c.operands[0].as_name_expr().map(|n| n.id.to_string()),
            _ => None,
        }
    }

    fn comprehension(&mut self, elt: &Expr, generators: &[ast::Comprehension]) {
        let mut inner = Checks {
            op: self.op,
            values: vec![],
            keys: vec![],
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
            _ => self.values.extend(self.checked(expr)),
        }
        visitor::walk_expr(self, expr);
    }
}

fn sink_arg(call: &ast::ExprCall, args: &[Option<Taint>], keywords: &[Option<Taint>]) -> Option<Taint> {
    if !call.arguments.args.is_empty() {
        return args.first().cloned().flatten();
    }
    call.arguments
        .keywords
        .iter()
        .zip(keywords)
        .find(|(k, _)| k.arg.as_ref().is_some_and(|a| SINK_KEYWORDS.contains(&a.as_str())))
        .and_then(|(_, t)| t.clone())
}

fn step(s: &Spanned<String>) -> Step {
    Step {
        path: None,
        pos: s.pos,
        label: s.value.clone(),
    }
}

fn message(sink: Sink, callee: &str, t: &Taint) -> (&'static str, String) {
    let origin = t.steps.first().map(|s| s.value.as_str()).unwrap_or_default();
    let mut through: Vec<&str> = vec![];
    for s in &t.steps[1..] {
        if through.last() != Some(&s.value.as_str()) {
            through.push(&s.value);
        }
    }
    let through = if through.is_empty() {
        String::new()
    } else {
        format!(" through '{}'", through.join("', '"))
    };
    match sink {
        Sink::Sql => (
            "OD701",
            format!(
                "{origin} reaches the query of {callee}{through}; pass it as a query parameter or build the query with odoo.tools.SQL"
            ),
        ),
        Sink::Eval => (
            "OD702",
            format!("{origin} reaches {callee}{through}, which runs it as code"),
        ),
        Sink::Markup => (
            "OD703",
            format!(
                "{origin} reaches {callee}{through} unescaped; escape it, or pass it as an argument of Markup's % or format()"
            ),
        ),
        Sink::SudoWrite => (
            "OD704",
            format!(
                "{origin} reaches {callee}{through}, so the user picks the fields it sets as superuser; copy only the fields the user may set"
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use ruff_python_parser::parse_module;

    use super::*;

    /// The findings of the first function in `src`, as `code message`.
    fn scan(params: Params, src: &str) -> Vec<String> {
        let parsed = parse_module(src).expect("valid Python");
        let Some(Stmt::FunctionDef(func)) = parsed.syntax().body.first() else {
            panic!("no function");
        };
        let loc = Locator::new(src);
        function(func, params, &loc)
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
        let diags = function(func, Params::None, &Locator::new(src));
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

    #[test]
    fn loops_carry_taint_around() {
        let src = "def f(self, items):\n    q = 'a'\n    for item in items:\n        self.env.cr.execute(q)\n        q = item\n";
        assert_eq!(scan(Params::Rpc, src).len(), 1);
    }
}
