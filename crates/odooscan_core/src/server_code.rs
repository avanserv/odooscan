//! The `code` of server actions and crons, read as Odoo compiles it when it loads a record.

use std::mem::replace;

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, ExceptHandler, Expr, ExprContext, Pattern, Stmt};
use ruff_python_parser::parse_module;
use ruff_text_size::{Ranged, TextRange};
use rustc_hash::FxHashSet;

/// The names Odoo sets from the record's `model_id` before it runs the code, in every series.
const FROM_MODEL_ID: [&str; 3] = ["model", "records", "record"];

/// What `read` finds in code, at byte offsets into it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ServerCode {
    /// Why `compile()` refuses the code, and where.
    pub error: Option<(String, usize)>,
    /// The method of each call whose receiver is known to hold records of `model_id`.
    pub calls: Vec<(String, usize)>,
}

pub fn read(code: &str) -> ServerCode {
    let refused = |message, at: TextRange| ServerCode {
        error: Some((message, at.start().to_usize())),
        calls: vec![],
    };
    let parsed = match parse_module(code) {
        Ok(parsed) => parsed,
        Err(e) => return refused(e.error.to_string(), e.location),
    };
    let mut walk = Walk::default();
    walk.visit_body(&parsed.syntax().body);
    if let Some((message, at)) = walk.error {
        return refused(message.to_owned(), at);
    }
    let bound = walk.bound;
    let trusted = walk.calls.into_iter().filter(|c| !bound.contains(c.receiver));
    ServerCode {
        error: None,
        calls: trusted.map(|c| (c.method.to_owned(), c.at)).collect(),
    }
}

struct Call<'a> {
    receiver: &'a str,
    method: &'a str,
    at: usize,
}

#[derive(Clone, Copy, Default)]
struct Place {
    in_function: bool,
    in_loop: bool,
}

/// What the parser leaves to `compile()`, and the names the code binds. Python binds a name for its
/// whole scope, so a binding silences a receiver wherever it sits, before the call or after it.
#[derive(Default)]
struct Walk<'a> {
    place: Place,
    /// The depth of `try` bodies, whose handlers may expect the call to fail.
    guarded: usize,
    error: Option<(&'static str, TextRange)>,
    bound: FxHashSet<&'a str>,
    calls: Vec<Call<'a>>,
}

impl<'a> Walk<'a> {
    fn refuse(&mut self, message: &'static str, at: TextRange) {
        self.error.get_or_insert((message, at));
    }

    fn body_in(&mut self, place: Place, body: &'a [Stmt]) {
        let outer = replace(&mut self.place, place);
        self.visit_body(body);
        self.place = outer;
    }

    fn check_placement(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Return(_) if !self.place.in_function => self.refuse("'return' outside function", stmt.range()),
            Stmt::Break(_) if !self.place.in_loop => self.refuse("'break' outside loop", stmt.range()),
            Stmt::Continue(_) if !self.place.in_loop => self.refuse("'continue' not properly in loop", stmt.range()),
            _ => {}
        }
    }

    fn check_suspension(&mut self, expr: &Expr) {
        let message = match expr {
            Expr::Yield(_) => "'yield' outside function",
            Expr::YieldFrom(_) => "'yield from' outside function",
            Expr::Await(_) => "'await' outside function",
            _ => return,
        };
        if !self.place.in_function {
            self.refuse(message, expr.range());
        }
    }

    fn bind_statement(&mut self, stmt: &'a Stmt) {
        let names: &'a [ast::Identifier] = match stmt {
            Stmt::FunctionDef(s) => std::slice::from_ref(&s.name),
            Stmt::ClassDef(s) => std::slice::from_ref(&s.name),
            Stmt::Global(s) => &s.names,
            Stmt::Nonlocal(s) => &s.names,
            _ => &[],
        };
        self.bound.extend(names.iter().map(ast::Identifier::as_str));
    }

    /// Decorators, defaults and annotations run in the enclosing scope, so only the body moves.
    fn function(&mut self, def: &'a ast::StmtFunctionDef) {
        for decorator in &def.decorator_list {
            self.visit_decorator(decorator);
        }
        if let Some(type_params) = &def.type_params {
            self.visit_type_params(type_params);
        }
        self.visit_parameters(&def.parameters);
        if let Some(returns) = &def.returns {
            self.visit_annotation(returns);
        }
        let place = Place {
            in_function: true,
            in_loop: false,
        };
        self.body_in(place, &def.body);
    }

    fn class(&mut self, def: &'a ast::StmtClassDef) {
        for decorator in &def.decorator_list {
            self.visit_decorator(decorator);
        }
        if let Some(type_params) = &def.type_params {
            self.visit_type_params(type_params);
        }
        if let Some(arguments) = &def.arguments {
            self.visit_arguments(arguments);
        }
        self.body_in(Place::default(), &def.body);
    }

    fn loop_body(&mut self, body: &'a [Stmt]) {
        let place = Place {
            in_loop: true,
            ..self.place
        };
        self.body_in(place, body);
    }

    fn try_statement(&mut self, stmt: &'a ast::StmtTry) {
        self.guarded += 1;
        self.visit_body(&stmt.body);
        self.guarded -= 1;
        for handler in &stmt.handlers {
            self.visit_except_handler(handler);
        }
        self.visit_body(&stmt.orelse);
        self.visit_body(&stmt.finalbody);
    }

    fn lambda(&mut self, lambda: &'a ast::ExprLambda) {
        if let Some(parameters) = &lambda.parameters {
            self.visit_parameters(parameters);
        }
        let place = Place {
            in_function: true,
            ..self.place
        };
        let outer = replace(&mut self.place, place);
        self.visit_expr(&lambda.body);
        self.place = outer;
    }

    fn call(&mut self, call: &'a ast::ExprCall) {
        let Expr::Attribute(attribute) = &*call.func else {
            return;
        };
        let Expr::Name(receiver) = &*attribute.value else {
            return;
        };
        if self.guarded == 0 && FROM_MODEL_ID.contains(&receiver.id.as_str()) {
            self.calls.push(Call {
                receiver: receiver.id.as_str(),
                method: attribute.attr.as_str(),
                at: attribute.attr.start().to_usize(),
            });
        }
    }
}

impl<'a> Visitor<'a> for Walk<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        self.check_placement(stmt);
        self.bind_statement(stmt);
        match stmt {
            Stmt::FunctionDef(def) => self.function(def),
            Stmt::ClassDef(def) => self.class(def),
            Stmt::For(s) => {
                self.visit_expr(&s.iter);
                self.visit_expr(&s.target);
                self.loop_body(&s.body);
                self.visit_body(&s.orelse);
            }
            Stmt::While(s) => {
                self.visit_expr(&s.test);
                self.loop_body(&s.body);
                self.visit_body(&s.orelse);
            }
            Stmt::Try(s) => self.try_statement(s),
            _ => visitor::walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        self.check_suspension(expr);
        match expr {
            Expr::Name(name) if name.ctx != ExprContext::Load => {
                self.bound.insert(name.id.as_str());
            }
            Expr::Call(call) => self.call(call),
            Expr::Lambda(lambda) => return self.lambda(lambda),
            _ => {}
        }
        visitor::walk_expr(self, expr);
    }

    fn visit_parameter(&mut self, parameter: &'a ast::Parameter) {
        self.bound.insert(parameter.name.as_str());
        visitor::walk_parameter(self, parameter);
    }

    fn visit_except_handler(&mut self, handler: &'a ExceptHandler) {
        let ExceptHandler::ExceptHandler(h) = handler;
        self.bound.extend(h.name.as_ref().map(ast::Identifier::as_str));
        visitor::walk_except_handler(self, handler);
    }

    fn visit_alias(&mut self, alias: &'a ast::Alias) {
        let name = alias.asname.as_ref().unwrap_or(&alias.name).as_str();
        self.bound.extend(name.split('.').next());
    }

    fn visit_pattern(&mut self, pattern: &'a Pattern) {
        let name = match pattern {
            Pattern::MatchAs(p) => p.name.as_ref(),
            Pattern::MatchStar(p) => p.name.as_ref(),
            Pattern::MatchMapping(p) => p.rest.as_ref(),
            _ => None,
        };
        self.bound.extend(name.map(ast::Identifier::as_str));
        visitor::walk_pattern(self, pattern);
    }

    fn visit_type_param(&mut self, type_param: &'a ast::TypeParam) {
        self.bound.insert(type_param.name().as_str());
        visitor::walk_type_param(self, type_param);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn methods(code: &str) -> Vec<String> {
        let read = read(code);
        assert_eq!(read.error, None, "{code}");
        read.calls.into_iter().map(|(method, _)| method).collect()
    }

    fn error(code: &str) -> (String, usize) {
        read(code).error.unwrap_or_else(|| panic!("{code}"))
    }

    #[test]
    fn reads_the_calls_of_each_trusted_name_at_the_method() {
        let code = "model.a()\nrecords.b(1)\nif record:\n    record.c(x=2)\n";
        assert_eq!(
            read(code).calls,
            [("a".to_owned(), 6), ("b".to_owned(), 18), ("c".to_owned(), 45)]
        );
    }

    #[test]
    fn a_nested_def_sees_the_names_of_the_code() {
        assert_eq!(methods("def run():\n    model.a()\nrun()"), ["a"]);
        assert_eq!(methods("f = lambda: model.a()"), ["a"]);
        assert_eq!(methods("x = f'{model.a()}'"), ["a"]);
    }

    #[test]
    fn a_binding_silences_its_name_in_any_order() {
        for code in [
            "model = env['a']\nmodel.b()",
            "model.b()\nmodel = env['a']",
            "def run():\n    model = env['a']\n    model.b()\nmodel.c()",
            "for model in x:\n    pass\nmodel.b()",
            "[model.b() for model in x]",
            "model |= x\nmodel.b()",
            "model += x\nmodel.b()",
            "del model\nmodel.b()",
            "(model := x).b()\nmodel.b()",
            "with x as model:\n    model.b()",
            "a, *model = x\nmodel.b()",
            "def run(model):\n    pass\nmodel.b()",
            "def run(*model):\n    pass\nmodel.b()",
            "def run(**model):\n    pass\nmodel.b()",
            "def run(a, /, model=1):\n    pass\nmodel.b()",
            "(lambda model: 1)\nmodel.b()",
            "def model():\n    pass\nmodel.b()",
            "class model:\n    pass\nmodel.b()",
            "import model\nmodel.b()",
            "import model.a\nmodel.b()",
            "import a as model\nmodel.b()",
            "from a import model\nmodel.b()",
            "from a import b as model\nmodel.b()",
            "try:\n    pass\nexcept E as model:\n    pass\nmodel.b()",
            "global model\nmodel.b()",
            "def run():\n    nonlocal model\nmodel.b()",
            "match x:\n    case model:\n        pass\nmodel.b()",
            "match x:\n    case [*model]:\n        pass\nmodel.b()",
            "match x:\n    case {**model}:\n        pass\nmodel.b()",
            "match x:\n    case [a] as model:\n        pass\nmodel.b()",
            "def run[model]():\n    pass\nmodel.b()",
            "type model = int\nmodel.b()",
        ] {
            assert_eq!(methods(code), [""; 0], "{code}");
        }
    }

    #[test]
    fn a_binding_leaves_the_other_names_alone() {
        assert_eq!(methods("model = x\nmodel.a()\nrecords.b()"), ["b"]);
        assert_eq!(methods("for record in records:\n    record.a()\nrecords.b()"), ["b"]);
    }

    #[test]
    fn only_a_bare_trusted_name_is_a_receiver() {
        for code in [
            "env['a'].b()",
            "model.sudo().b()",
            "model.env.b()",
            "self.model.b()",
            "other.b()",
            "b()",
            "model.b",
            "x = model",
        ] {
            let found = methods(code);
            assert!(found.is_empty() || found == ["sudo"], "{code}");
        }
    }

    #[test]
    fn a_try_body_is_silent_but_its_other_blocks_are_not() {
        assert_eq!(methods("try:\n    model.a()\nexcept E:\n    pass"), [""; 0]);
        assert_eq!(
            methods("try:\n    if x:\n        model.a()\nfinally:\n    pass"),
            [""; 0]
        );
        assert_eq!(
            methods("try:\n    pass\nexcept E:\n    model.a()\nelse:\n    model.b()\nfinally:\n    model.c()"),
            ["a", "b", "c"]
        );
    }

    #[test]
    fn a_try_body_still_binds_and_compiles() {
        assert_eq!(methods("try:\n    model = x\nexcept E:\n    pass\nmodel.a()"), [""; 0]);
        assert_eq!(
            error("try:\n    return\nexcept E:\n    pass").0,
            "'return' outside function"
        );
    }

    #[test]
    fn refuses_what_the_parser_refuses() {
        assert_eq!(error("model.a(").1, 8);
        assert!(read("a\n    b").error.is_some());
        assert!(read("print 'a'").error.is_some());
    }

    #[test]
    fn refuses_what_only_compile_refuses() {
        for (code, message, at) in [
            ("return", "'return' outside function", 0),
            ("if a:\n    return 1", "'return' outside function", 10),
            ("class A:\n    return", "'return' outside function", 13),
            (
                "def f():\n    class A:\n        return",
                "'return' outside function",
                30,
            ),
            ("yield", "'yield' outside function", 0),
            ("x = yield 1", "'yield' outside function", 4),
            ("yield from a", "'yield from' outside function", 0),
            ("await a", "'await' outside function", 0),
            ("def f(a=(yield)):\n    pass", "'yield' outside function", 9),
            ("break", "'break' outside loop", 0),
            ("continue", "'continue' not properly in loop", 0),
            ("for a in b:\n    pass\nelse:\n    break", "'break' outside loop", 31),
            (
                "while a:\n    pass\nelse:\n    continue",
                "'continue' not properly in loop",
                28,
            ),
            ("for a in b:\n    def f():\n        break", "'break' outside loop", 33),
            ("for a in b:\n    class A:\n        break", "'break' outside loop", 33),
        ] {
            assert_eq!(error(code), (message.to_owned(), at), "{code}");
        }
    }

    #[test]
    fn accepts_what_compile_accepts() {
        for code in [
            "",
            "# a comment",
            "def f():\n    return 1",
            "def f():\n    yield 1",
            "async def f():\n    await a",
            "f = lambda: (yield)",
            "for a in b:\n    break\nelse:\n    pass",
            "while a:\n    if b:\n        continue",
            "for a in b:\n    try:\n        pass\n    finally:\n        continue",
            "def f():\n    for a in b:\n        break\n    return a",
            "action = {'type': 'ir.actions.act_window'}",
        ] {
            assert_eq!(read(code), ServerCode::default(), "{code}");
        }
    }

    #[test]
    fn a_refused_code_makes_no_calls() {
        assert!(read("model.a()\nreturn").calls.is_empty());
        assert!(read("model.a()\n(").calls.is_empty());
    }
}
