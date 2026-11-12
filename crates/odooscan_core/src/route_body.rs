//! What a route's body does that its declaration has to allow for: the record writes a GET or a
//! user-less route should not make, what it reads of the user, and the checks that stand in for
//! the ones its declaration leaves out. One walk covers the body with the functions it nests,
//! which run as part of the route.

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, BoolOp, CmpOp, Expr, Stmt};
use rustc_hash::FxHashSet;

use crate::python::{SAME_MODEL, tail_name};
use crate::superuser;

/// The methods that change records in the database.
const WRITES: &[&str] = &["write", "create", "unlink"];

/// Recordset methods whose result is records again.
const RECORD_METHODS: &[&str] = &["browse", "search", "filtered", "exists", "mapped", "create"];

/// Calls that compare a secret the request carries with the one the server holds.
const TOKEN_CHECKS: &[&str] = &["validate_csrf", "consteq", "compare_digest", "check_access_token"];

/// Calls whose result runs as the user they name, whatever the environment they start from, unless
/// it is `None`: `with_user(None)` returns its records unchanged (18.0 `odoo/models.py:6341-6342`,
/// 20.0 `odoo/orm/models.py:5472-5473`), and `Environment(cr, None, ...)` has no user.
const USER_ENVS: &[&str] = &["with_user", "Environment"];

/// The methods Odoo checks no CSRF token on (18.0 `odoo/http.py:219`, 20.0
/// `odoo/http/dispatcher.py:50`).
const CSRF_FREE: &[&str] = &["GET", "HEAD", "OPTIONS", "TRACE"];

#[derive(Default)]
pub(crate) struct RouteBody<'a> {
    /// The method of the first record write on records that carry no user of their own.
    pub write: Option<&'a str>,
    /// The method of the first record write no test of the request method keeps from a GET.
    pub unguarded_write: Option<&'a str>,
    pub reads_user: bool,
    pub sets_user: bool,
    pub checks_token: bool,
}

impl<'a> RouteBody<'a> {
    /// `records` are the parameters the route's URL converters fill with records.
    pub fn of(func: &'a ast::StmtFunctionDef, records: &[&'a str]) -> Self {
        let mut walk = Walk {
            names: records.iter().copied().collect(),
            users: FxHashSet::default(),
            envs: FxHashSet::default(),
            guards: 0,
            body: Self::default(),
        };
        walk.visit_body(&func.body);
        walk.body
    }
}

struct Walk<'a> {
    /// The names bound to records at the point of the walk.
    names: FxHashSet<&'a str>,
    /// The names bound to an environment or records that run as a user they were given.
    users: FxHashSet<&'a str>,
    /// The names bound to an environment, besides `env`.
    envs: FxHashSet<&'a str>,
    /// How many tests that the request method is one Odoo checks CSRF on enclose the walk.
    guards: u32,
    body: RouteBody<'a>,
}

impl<'a> Walk<'a> {
    fn is_records(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Name(n) => self.names.contains(n.id.as_str()),
            Expr::Subscript(s) => self.is_env(&s.value) || self.is_records(&s.value),
            Expr::Attribute(a) => (a.attr.as_str() == "user" && self.is_env(&a.value)) || self.is_records(&a.value),
            Expr::Call(c) => self.records_call(c),
            _ => false,
        }
    }

    /// Only recordsets have the `SAME_MODEL` methods, whatever they are called on.
    fn records_call(&self, call: &ast::ExprCall) -> bool {
        let Expr::Attribute(a) = &*call.func else { return false };
        let method = a.attr.as_str();
        match method {
            "ref" => self.is_env(&a.value),
            _ if SAME_MODEL.contains(&method) => true,
            _ => RECORD_METHODS.contains(&method) && self.is_records(&a.value),
        }
    }

    /// `env`, a name bound to an environment, `request.env` and the like, or a call of `Environment`
    /// or of an environment, which builds another.
    fn is_env(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Name(n) => n.id.as_str() == "env" || self.envs.contains(n.id.as_str()),
            Expr::Attribute(a) => a.attr.as_str() == "env",
            Expr::Call(c) => tail_name(&c.func) == Some("Environment") || self.is_env(&c.func),
            _ => false,
        }
    }

    /// Whether `expr` is an environment, or records, built for a user it names (`with_user()`,
    /// `api.Environment(cr, uid, ...)`, `env(user=...)`) or called on an environment that carries
    /// one, so that a write on it has one.
    fn carries_user(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Name(n) => self.users.contains(n.id.as_str()),
            Expr::Subscript(s) => self.carries_user(&s.value),
            Expr::Attribute(a) => self.carries_user(&a.value),
            Expr::Call(c) if self.is_env(&c.func) => names_user(env_user(c)) || self.carries_user(&c.func),
            Expr::Call(c) => user_call(c) || matches!(&*c.func, Expr::Attribute(a) if self.carries_user(&a.value)),
            _ => false,
        }
    }

    fn bind(&mut self, target: &'a Expr, value: &Expr) {
        let Expr::Name(n) = target else { return };
        let (records, user, env) = (self.is_records(value), self.carries_user(value), self.is_env(value));
        toggle(&mut self.names, n.id.as_str(), records);
        toggle(&mut self.users, n.id.as_str(), user);
        toggle(&mut self.envs, n.id.as_str(), env);
    }

    fn call(&mut self, call: &'a ast::ExprCall) {
        let name = tail_name(&call.func).unwrap_or_default();
        self.body.checks_token |= TOKEN_CHECKS.contains(&name);
        self.body.sets_user |= sets_user(call, name) || is_login(call);
        if let Expr::Attribute(a) = &*call.func
            && WRITES.contains(&name)
            && self.is_records(&a.value)
        {
            if !self.carries_user(&a.value) {
                self.body.write.get_or_insert(a.attr.as_str());
            }
            if self.guards == 0 {
                self.body.unguarded_write.get_or_insert(a.attr.as_str());
            }
        }
    }

    fn guarded(&mut self, guard: bool, f: impl FnOnce(&mut Self)) {
        let guard = u32::from(guard);
        self.guards += guard;
        f(self);
        self.guards -= guard;
    }

    fn visit_if(&mut self, i: &'a ast::StmtIf) {
        self.visit_expr(&i.test);
        self.guarded(is_method_guard(&i.test), |w| w.visit_body(&i.body));
        for clause in &i.elif_else_clauses {
            if let Some(test) = &clause.test {
                self.visit_expr(test);
            }
            let guard = clause.test.as_ref().is_some_and(is_method_guard);
            self.guarded(guard, |w| w.visit_body(&clause.body));
        }
    }

    /// `request.uid = ...`, or `request.env = ...` with an environment built for a user.
    fn sets_request_user(&self, target: &Expr, value: &Expr) -> bool {
        let Expr::Attribute(a) = target else { return false };
        is_request(&a.value) && (a.attr.as_str() == "uid" || (a.attr.as_str() == "env" && self.carries_user(value)))
    }

    /// The operands of `and` after a method test run only when it passed.
    fn visit_and(&mut self, values: &'a [Expr]) {
        let mut guard = false;
        for value in values {
            self.guarded(guard, |w| w.visit_expr(value));
            guard |= is_method_guard(value);
        }
    }
}

impl<'a> Visitor<'a> for Walk<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::If(i) => self.visit_if(i),
            Stmt::Assign(a) => {
                self.visit_expr(&a.value);
                for target in &a.targets {
                    self.body.sets_user |= self.sets_request_user(target, &a.value);
                    self.bind(target, &a.value);
                    self.visit_expr(target);
                }
            }
            Stmt::For(f) => {
                self.visit_expr(&f.iter);
                self.bind(&f.target, &f.iter);
                self.visit_body(&f.body);
                self.visit_body(&f.orelse);
            }
            _ => visitor::walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::BoolOp(b) if b.op == BoolOp::And => {
                self.visit_and(&b.values);
                return;
            }
            Expr::Call(c) => self.call(c),
            Expr::Attribute(a) => self.body.reads_user |= a.attr.as_str() == "user" && is_request_env(&a.value),
            _ => {}
        }
        visitor::walk_expr(self, expr);
    }
}

fn toggle<'a>(names: &mut FxHashSet<&'a str>, name: &'a str, on: bool) {
    if on {
        names.insert(name);
    } else {
        names.remove(name);
    }
}

/// `request` or `http.request`, the request a route serves.
pub(crate) fn is_request(expr: &Expr) -> bool {
    match expr {
        Expr::Name(n) => n.id.as_str() == "request",
        Expr::Attribute(a) => {
            a.attr.as_str() == "request" && matches!(&*a.value, Expr::Name(n) if n.id.as_str() == "http")
        }
        _ => false,
    }
}

fn user_call(call: &ast::ExprCall) -> bool {
    tail_name(&call.func).is_some_and(|name| USER_ENVS.contains(&name))
        && !superuser::user_arg(call).is_some_and(Expr::is_none_literal_expr)
}

/// The user an environment is called with, which keeps the user of the environment it is called on
/// when left out or `None` (18.0 `odoo/api.py:646`, 20.0 `odoo/orm/environments.py:144`). The
/// callee may be any name bound to an environment, so `superuser::user_arg` cannot tell its kind.
pub(crate) fn env_user(call: &ast::ExprCall) -> Option<&Expr> {
    call.arguments.find_argument_value("user", 1)
}

/// `update_env()` passes its user to `env()` (18.0 `odoo/http.py:1779-1788`, 20.0
/// `odoo/http/requestlib.py:142-151`), and `_auth_method_none()` clears the user (18.0
/// `odoo/addons/base/models/ir_http.py:251-252`, 20.0 `:369-370`).
fn sets_user(call: &ast::ExprCall, name: &str) -> bool {
    match name {
        "update_env" => names_user(superuser::user_arg(call)),
        "_auth_method_none" => false,
        _ => name.starts_with("_auth_method_"),
    }
}

fn names_user(user: Option<&Expr>) -> bool {
    user.is_some_and(|u| !u.is_none_literal_expr())
}

/// `request.session.authenticate()` (18.0 `odoo/http.py:1109`) or `authenticate(request.session,
/// ...)` (20.0 `odoo/http/session.py:225`), which sign the user in, unlike the `authenticate()` of
/// `res.users`, which only checks the credentials.
fn is_login(call: &ast::ExprCall) -> bool {
    let session = match &*call.func {
        Expr::Attribute(a) if a.attr.as_str() == "authenticate" => &*a.value,
        Expr::Name(n) if n.id.as_str() == "authenticate" => match call.arguments.args.first() {
            Some(first) => first,
            None => return false,
        },
        _ => return false,
    };
    matches!(session, Expr::Attribute(s) if s.attr.as_str() == "session" && is_request(&s.value))
}

fn is_request_env(expr: &Expr) -> bool {
    matches!(expr, Expr::Attribute(a) if a.attr.as_str() == "env" && is_request(&a.value))
}

/// A test that passes only for methods Odoo checks a CSRF token on: `request.httprequest.method`,
/// upper- or lower-cased or not, compared with `==` to, or `in`, such literals, alone or as an
/// operand of `and`.
fn is_method_guard(test: &Expr) -> bool {
    match test {
        Expr::BoolOp(b) if b.op == BoolOp::And => b.values.iter().any(is_method_guard),
        Expr::Compare(c) => match (&*c.ops, &*c.operands) {
            ([CmpOp::Eq], [left, right]) => {
                (is_http_method(left) && checked_method(right)) || (is_http_method(right) && checked_method(left))
            }
            ([CmpOp::In], [left, values]) => is_http_method(left) && checked_methods(values),
            _ => false,
        },
        _ => false,
    }
}

fn is_http_method(expr: &Expr) -> bool {
    match expr {
        Expr::Attribute(a) => a.attr.as_str() == "method" && tail_name(&a.value) == Some("httprequest"),
        Expr::Call(c) if c.arguments.is_empty() => {
            matches!(&*c.func, Expr::Attribute(a) if matches!(a.attr.as_str(), "upper" | "lower") && is_http_method(&a.value))
        }
        _ => false,
    }
}

fn checked_method(expr: &Expr) -> bool {
    let Expr::StringLiteral(s) = expr else { return false };
    let method = s.value.to_str();
    !CSRF_FREE.iter().any(|free| free.eq_ignore_ascii_case(method))
}

fn checked_methods(expr: &Expr) -> bool {
    let elts = match expr {
        Expr::List(l) => &l.elts,
        Expr::Tuple(t) => &t.elts,
        Expr::Set(s) => &s.elts,
        _ => return false,
    };
    !elts.is_empty() && elts.iter().all(checked_method)
}

#[cfg(test)]
mod tests {
    use ruff_python_parser::parse_module;

    use super::*;

    fn body_of(src: &str, records: &[&str], check: impl FnOnce(&RouteBody)) {
        let module = parse_module(src).unwrap().into_syntax();
        let Some(Stmt::FunctionDef(func)) = module.body.first() else {
            panic!("{src}")
        };
        check(&RouteBody::of(func, records));
    }

    fn writes(line: &str) -> (Option<String>, Option<String>) {
        let src = format!("def f(order, buffer):\n    {line}\n");
        let mut out = (None, None);
        body_of(&src, &["order"], |b| {
            out = (b.write.map(str::to_owned), b.unguarded_write.map(str::to_owned));
        });
        out
    }

    #[test]
    fn a_write_on_records_is_one_and_on_anything_else_is_not() {
        for line in [
            "request.env['x'].browse(1).write({})",
            "request.env['x'].sudo().search([]).filtered('a').unlink()",
            "request.env.user.partner_id.write({})",
            "request.env.ref('x.y').write({})",
            "request.env['x'].create({}).mapped('line_ids').unlink()",
            "partner.sudo().write({})",
            "order.write({})",
            "order[0].line_ids.write({})",
            "lines = request.env['x'].browse(1).exists(); lines.unlink()",
            "for line in order.line_ids: line.unlink()",
        ] {
            assert!(writes(line).1.is_some(), "{line}");
        }
        for line in [
            "buffer.write(b'x')",
            "Command.create({})",
            "os.unlink(path)",
            "order = 1; order.write({})",
            "request.env.cr.commit()",
            "partner.write({})",
        ] {
            assert_eq!(writes(line), (None, None), "{line}");
        }
    }

    #[test]
    fn a_write_behind_a_test_of_a_checked_method_is_guarded() {
        for test in [
            "request.httprequest.method == 'POST'",
            "request.httprequest.method in ('POST', 'put')",
            "kw and request.httprequest.method == 'POST'",
            "'POST' == request.httprequest.method",
            "request.httprequest.method.upper() == 'POST'",
            "request.httprequest.method.lower() in ('post',)",
        ] {
            let line = format!("if {test}: order.write({{}})");
            assert_eq!(writes(&line), (Some("write".into()), None), "{line}");
        }
        let and = "request.httprequest.method == 'POST' and order.write({})";
        assert_eq!(writes(and), (Some("write".into()), None));
        for test in [
            "request.httprequest.method == 'GET'",
            "request.httprequest.method in ('GET', 'POST')",
            "request.httprequest.method != 'POST'",
            "kw or request.httprequest.method == 'POST'",
            "request.httprequest.method in methods",
            "'GET' == request.httprequest.method",
            "request.httprequest.method.upper() in methods",
        ] {
            let line = format!("if {test}: order.write({{}})");
            assert!(writes(&line).1.is_some(), "{line}");
        }
        let elif = "if request.httprequest.method == 'POST': pass\n    else: order.write({})";
        assert!(writes(elif).1.is_some());
    }

    #[test]
    fn a_write_on_records_built_for_a_user_is_still_one_a_get_must_not_make() {
        for line in [
            "request.env['x'].with_user(1).create({})",
            "env = api.Environment(cr, SUPERUSER_ID, {}); env['x'].create({})",
            "env = request.env(user=SUPERUSER_ID); env['x'].browse(1).write({})",
            "env = request.env(user=1); env.user.write({})",
            "env = request.env(cr, uid); env['x'].create({})",
        ] {
            assert_eq!(writes(line).0, None, "{line}");
            assert!(writes(line).1.is_some(), "{line}");
        }
        let rebound = "env = request.env(user=1); env = request.env; env['x'].create({})";
        assert_eq!(writes(rebound).0, Some("create".into()));
        assert_eq!(writes("request.env(user=uid)['x'].create({})").0, None);
    }

    #[test]
    fn records_built_for_no_user_or_a_none_one_carry_none() {
        for line in [
            "lines = request.env['x'].with_user(None); lines.create({})",
            "env = api.Environment(cr, None, {}); env['x'].create({})",
            "new_env = request.env(user=None); new_env['x'].create({})",
            "new_env = request.env(None, None); new_env['x'].create({})",
            "request.env(context={})['x'].create({})",
        ] {
            assert_eq!(writes(line), (Some("create".into()), Some("create".into())), "{line}");
        }
    }

    #[test]
    fn an_environment_called_without_a_user_keeps_the_one_it_is_called_on() {
        for line in [
            "env = request.env(user=uid); env(context={'lang': 'fr'})['x'].create({})",
            "base = request.env; base(user=uid)['x'].create({})",
            "env = api.Environment(cr, uid, {}); env(user=None)['x'].create({})",
            "new_env = request.env(user=uid); new_env(su=False).ref('x.y').write({})",
        ] {
            assert_eq!(writes(line).0, None, "{line}");
            assert!(writes(line).1.is_some(), "{line}");
        }
        let unset = "base = request.env; base(user=None)['x'].create({})";
        assert_eq!(writes(unset).0, Some("create".into()));
    }

    #[test]
    fn calls_of_an_environment_and_names_bound_to_one_give_records() {
        for line in [
            "request.env(user=1)['x'].create({})",
            "request.env(context={})['x'].create({})",
            "api.Environment(cr, uid, {})['x'].create({})",
            "new_env = request.env(user=1); new_env['x'].create({})",
            "admin_env = request.env; admin_env.ref('x.y').write({})",
            "new_env = request.env(user=1); new_env.user.write({})",
        ] {
            assert!(writes(line).1.is_some(), "{line}");
        }
        let rebound = "new_env = request.env(user=1); new_env = buffer; new_env['x'].create({})";
        assert_eq!(writes(rebound), (None, None));
    }

    #[test]
    fn a_body_that_reads_the_user_sets_one_or_checks_a_token_says_so() {
        let src = "def f():\n    def inner():\n        request.env['x'].create({})\n    \
                   return request.env.user.name\n";
        body_of(src, &[], |b| {
            assert_eq!(b.write, Some("create"));
            assert!(b.reads_user && !b.sets_user && !b.checks_token);
        });
        for line in [
            "request.update_env(user=1)",
            "request.session.authenticate(db, creds)",
            "request.env['ir.http']._auth_method_public()",
            "request.uid = 1",
            "http.request.uid = 1",
            "request.env = request.env(user=uid)",
            "request.env = api.Environment(request.cr, uid, {})",
            "authenticate(request.session, request.env, credential)",
            "request.update_env(uid)",
            "request.update_env(user=request.session.uid)",
        ] {
            body_of(&format!("def f():\n    {line}\n"), &[], |b| {
                assert!(b.sets_user, "{line}")
            });
        }
        for line in [
            "request.env['res.users'].authenticate(db, credential, env)",
            "authenticate(session, request.env, credential)",
            "request.env = request.env(context={})",
            "self.request.uid = 1",
            "request.update_env(user=None)",
            "request.update_env(None)",
            "request.update_env()",
            "request.update_env(context={})",
            "request.update_env(su=True)",
            "request.env['ir.http']._auth_method_none()",
            "request.env = request.env(user=None)",
            "request.env = api.Environment(request.cr, None, {})",
        ] {
            body_of(&format!("def f():\n    {line}\n"), &[], |b| {
                assert!(!b.sets_user, "{line}")
            });
        }
        body_of("def f():\n    return http.request.env.user\n", &[], |b| {
            assert!(b.reads_user)
        });
        for line in ["request.validate_csrf(t)", "consteq(a, b)", "hmac.compare_digest(a, b)"] {
            body_of(&format!("def f():\n    {line}\n"), &[], |b| {
                assert!(b.checks_token, "{line}")
            });
        }
        body_of("def f(self):\n    return self.env.user\n", &[], |b| {
            assert!(!b.reads_user)
        });
    }
}
