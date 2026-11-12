//! The calls that make code run as the superuser: `sudo()`, and the environments built for uid 1,
//! which Odoo runs in superuser mode whatever `su` they pass (18.0 `odoo/api.py:572`, 20.0
//! `odoo/orm/environments.py:66`).

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, Stmt};

use crate::python::tail_name;
use crate::route_body;
use crate::series::Series;

/// The first series whose `request.uid` raises on assignment (16.0 `odoo/http.py:1493`); before it,
/// assigning it rebuilds `request.env` for that user (15.0 `odoo/http.py:223`).
pub(crate) const REQUEST_UID_END: Series = Series { major: 16, minor: 0 };

/// How a body first runs as the superuser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Escalation {
    Sudo,
    /// A call `switch` labels.
    Switch(&'static str),
}

/// `sudo()` anywhere in `body`, with the functions it nests, else the first call `switch` labels
/// or, in a series before `REQUEST_UID_END`, the first `request.uid = SUPERUSER_ID`.
pub(crate) fn escalation(body: &[Stmt], series: Option<Series>) -> Option<Escalation> {
    let request_uid = series.is_none_or(|s| s < REQUEST_UID_END);
    let mut walk = Walk {
        request_uid,
        ..Walk::default()
    };
    walk.visit_body(body);
    if walk.sudo {
        Some(Escalation::Sudo)
    } else {
        walk.switch.map(Escalation::Switch)
    }
}

/// The label of a call that builds an environment, or records, for the superuser: its user is
/// `SUPERUSER_ID` or `1`, or it passes `su=True`. `with_user()` drops superuser mode for any other
/// user (18.0 `odoo/models.py:6341`), so only its user counts.
pub(crate) fn switch(call: &ast::ExprCall) -> Option<&'static str> {
    let (label, _, su) = signature(call)?;
    let su = su
        .and_then(|i| call.arguments.find_argument_value("su", i))
        .is_some_and(|v| matches!(v, Expr::BooleanLiteral(b) if b.value));
    (root(call) || su).then_some(label)
}

/// Whether a call `switch` reads builds the environment of uid 1, which stays in superuser mode
/// after a later `sudo(False)`, unlike one built with `su=True`.
pub(crate) fn root(call: &ast::ExprCall) -> bool {
    user_arg(call).is_some_and(is_superuser)
}

/// The user argument, by keyword or position, of a call that `signature` names.
pub(crate) fn user_arg(call: &ast::ExprCall) -> Option<&Expr> {
    let (_, (keyword, position), _) = signature(call)?;
    call.arguments.find_argument_value(keyword, position)
}

/// The label of a call that may build an environment for another user, the keyword and position
/// of its user, and the position of its `su`.
fn signature(call: &ast::ExprCall) -> Option<(&'static str, (&'static str, usize), Option<usize>)> {
    Some(match tail_name(&call.func)? {
        "with_user" => ("with_user()", ("user", 0), None),
        "env" => ("env()", ("user", 1), Some(3)),
        "update_env" => ("update_env()", ("user", 0), Some(2)),
        "Environment" => ("Environment()", ("uid", 1), Some(3)),
        _ => return None,
    })
}

/// `SUPERUSER_ID` or `1`.
pub(crate) fn is_superuser(user: &Expr) -> bool {
    match user {
        Expr::NumberLiteral(n) => matches!(&n.value, ast::Number::Int(i) if i.as_u8() == Some(1)),
        _ => tail_name(user) == Some("SUPERUSER_ID"),
    }
}

/// `request.uid`.
pub(crate) fn is_request_uid(target: &Expr) -> bool {
    matches!(target, Expr::Attribute(a) if a.attr.as_str() == "uid" && route_body::is_request(&a.value))
}

/// `sudo(False)`, which leaves superuser mode.
fn leaves_sudo(call: &ast::ExprCall) -> bool {
    tail_name(&call.func) == Some("sudo")
        && call
            .arguments
            .find_argument_value("flag", 0)
            .is_some_and(|v| matches!(v, Expr::BooleanLiteral(b) if !b.value))
}

#[derive(Default)]
struct Walk {
    sudo: bool,
    switch: Option<&'static str>,
    /// Whether `request.uid` can be assigned.
    request_uid: bool,
}

impl<'a> Visitor<'a> for Walk {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if let Stmt::Assign(a) = stmt
            && self.request_uid
            && self.switch.is_none()
            && is_superuser(&a.value)
            && a.targets.iter().any(is_request_uid)
        {
            self.switch = Some("request.uid");
        }
        visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Call(c) if leaves_sudo(c) => {
                if let Expr::Attribute(a) = &*c.func {
                    self.visit_expr(&a.value);
                }
                return self.visit_arguments(&c.arguments);
            }
            Expr::Call(c) if self.switch.is_none() => self.switch = switch(c),
            Expr::Attribute(a) => self.sudo |= a.attr.as_str() == "sudo",
            _ => {}
        }
        visitor::walk_expr(self, expr);
    }
}

#[cfg(test)]
mod tests {
    use ruff_python_parser::parse_module;

    use super::*;

    fn of(line: &str) -> Option<Escalation> {
        let module = parse_module(line).unwrap().into_syntax();
        escalation(&module.body, None)
    }

    #[test]
    fn each_superuser_environment_is_a_switch_and_its_silent_twin_is_not() {
        for (line, label) in [
            ("x.with_user(SUPERUSER_ID)", "with_user()"),
            ("x.with_user(odoo.SUPERUSER_ID)", "with_user()"),
            ("x.with_user(user=1)", "with_user()"),
            ("request.env(user=SUPERUSER_ID)", "env()"),
            ("request.env(None, 1)", "env()"),
            ("request.env(user=1, su=False)", "env()"),
            ("request.env(su=True)", "env()"),
            ("request.update_env(user=SUPERUSER_ID)", "update_env()"),
            ("request.update_env(su=True)", "update_env()"),
            ("api.Environment(cr, SUPERUSER_ID, {})", "Environment()"),
            ("api.Environment(cr, uid=SUPERUSER_ID, context={})", "Environment()"),
            ("request.uid = SUPERUSER_ID", "request.uid"),
            ("http.request.uid = openerp.SUPERUSER_ID", "request.uid"),
            ("api.Environment(cr, uid, {}, su=True)", "Environment()"),
            ("Environment(cr, uid, {}, True)", "Environment()"),
        ] {
            assert_eq!(of(line), Some(Escalation::Switch(label)), "{line}");
        }
        for line in [
            "x.with_user(request.env.user)",
            "x.with_user(2)",
            "request.env(su=False)",
            "request.env(user=uid)",
            "request.env(context={})",
            "request.update_env(context={})",
            "request.update_env(user=uid, su=False)",
            "api.Environment(cr, uid, {})",
            "api.Environment(cr, user=SUPERUSER_ID, context={})",
            "request.uid = request.session.uid",
            "self.uid = SUPERUSER_ID",
            "api.Environment(cr, uid, {}, su=False)",
            "x.sudo(False)",
            "x.sudo(flag=False)",
        ] {
            assert_eq!(of(line), None, "{line}");
        }
    }

    #[test]
    fn only_a_switch_to_uid_1_builds_the_environment_of_the_superuser() {
        let root = |line: &str| {
            let module = parse_module(line).unwrap().into_syntax();
            let Stmt::Expr(e) = &module.body[0] else {
                panic!("{line}")
            };
            root(e.value.as_call_expr().unwrap())
        };
        for line in [
            "x.with_user(1)",
            "request.env(user=SUPERUSER_ID, su=False)",
            "api.Environment(cr, SUPERUSER_ID, {})",
        ] {
            assert!(root(line), "{line}");
        }
        for line in [
            "request.env(su=True)",
            "x.with_user(user)",
            "api.Environment(cr, uid, {}, True)",
            "x.sudo()",
        ] {
            assert!(!root(line), "{line}");
        }
    }

    #[test]
    fn request_uid_is_a_switch_only_before_it_raises_on_assignment() {
        let module = parse_module("request.uid = SUPERUSER_ID").unwrap().into_syntax();
        let at = |series| escalation(&module.body, Series::parse(series));
        assert_eq!(at("15.0"), Some(Escalation::Switch("request.uid")));
        assert_eq!(at("16.0"), None);
    }

    #[test]
    fn sudo_comes_before_any_switch() {
        assert_eq!(of("x.with_user(1)\nx.sudo()"), Some(Escalation::Sudo));
        assert_eq!(of("x.sudo(False).sudo()"), Some(Escalation::Sudo));
        assert_eq!(of("x.sudo"), Some(Escalation::Sudo));
        // Taint reads a computed flag as leaving superuser mode; a hotspot reports it all the same.
        assert_eq!(of("x.sudo(flag)"), Some(Escalation::Sudo));
        assert_eq!(of("x.sudo(flag=self.flag)"), Some(Escalation::Sudo));
        assert_eq!(of("x.sudo(False).search([], limit=y.sudo().n)"), Some(Escalation::Sudo));
        let first = of("request.env(su=True)\nx.with_user(1)");
        assert_eq!(first, Some(Escalation::Switch("env()")));
    }
}
