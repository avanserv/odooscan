use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, Operator, Stmt};
use ruff_python_parser::parse_module;
use ruff_source_file::LineIndex;
use ruff_text_size::{Ranged, TextSize};

use crate::facts::{ClassFact, FieldFact, ModelKind, Pos, PyFacts, RawDiag, Spanned};

const FIELD_TYPES: &[&str] = &[
    "Boolean",
    "Integer",
    "Float",
    "Monetary",
    "Char",
    "Text",
    "Html",
    "Date",
    "Datetime",
    "Binary",
    "Image",
    "Selection",
    "Reference",
    "Many2one",
    "Many2oneReference",
    "One2many",
    "Many2many",
    "Json",
    "Properties",
    "PropertiesDefinition",
    "Id",
];

/// ORM methods whose overrides must chain to the parent implementation.
const SUPER_REQUIRED: &[&str] = &["create", "write", "unlink", "copy", "default_get"];

pub struct Locator<'a> {
    src: &'a str,
    index: LineIndex,
}

impl<'a> Locator<'a> {
    pub fn new(src: &'a str) -> Self {
        Self {
            src,
            index: LineIndex::from_source_text(src),
        }
    }

    pub fn pos(&self, offset: TextSize) -> Pos {
        let lc = self.index.line_column(offset, self.src);
        Pos {
            line: lc.line.get() as u32,
            col: lc.column.get() as u32,
        }
    }

    pub fn at(&self, node: &impl Ranged) -> Pos {
        self.pos(node.start())
    }
}

pub fn str_lit(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::StringLiteral(s) => Some(s.value.to_str()),
        _ => None,
    }
}

/// The last name in a dotted expression: `cr` for `self.env.cr`.
fn tail_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Name(n) => Some(n.id.as_str()),
        Expr::Attribute(a) => Some(a.attr.as_str()),
        _ => None,
    }
}

fn keyword<'a>(call: &'a ast::ExprCall, name: &str) -> Option<&'a Expr> {
    call.arguments
        .keywords
        .iter()
        .find(|k| k.arg.as_ref().is_some_and(|a| a.as_str() == name))
        .map(|k| &k.value)
}

fn is_cursor(expr: &Expr) -> bool {
    matches!(tail_name(expr), Some("cr" | "_cr"))
}

/// Values the model's own code produces, such as `self._table` or `self._select()` in report models.
fn is_code_built(expr: &Expr) -> bool {
    let on_self =
        |e: &Expr| matches!(e, Expr::Attribute(a) if matches!(&*a.value, Expr::Name(n) if n.id.as_str() == "self"));
    match expr {
        Expr::Call(c) => on_self(&c.func) && c.arguments.is_empty(),
        Expr::Tuple(t) => t.elts.iter().all(is_code_built),
        Expr::StringLiteral(_) | Expr::NumberLiteral(_) => true,
        e => on_self(e),
    }
}

/// A string whose content was spliced at runtime by `%`, `+`, `.format()` or an f-string, from
/// values other than the model's own code.
fn is_formatted_string(expr: &Expr) -> bool {
    match expr {
        Expr::FString(f) => f.value.elements().any(|e| match e {
            ast::InterpolatedStringElement::Interpolation(i) => !is_code_built(&i.expression),
            ast::InterpolatedStringElement::Literal(_) => false,
        }),
        Expr::BinOp(b) if b.op == Operator::Mod => {
            (str_lit(&b.left).is_some() || is_formatted_string(&b.left)) && !is_code_built(&b.right)
        }
        Expr::BinOp(b) if b.op == Operator::Add => {
            let literal = |e: &Expr| str_lit(e).is_some();
            (literal(&b.left) || literal(&b.right)) && !(literal(&b.left) && literal(&b.right))
                || is_formatted_string(&b.left)
                || is_formatted_string(&b.right)
        }
        Expr::Call(c) => {
            matches!(&*c.func, Expr::Attribute(a) if a.attr.as_str() == "format" && str_lit(&a.value).is_some())
                && !(c.arguments.args.iter().all(is_code_built)
                    && c.arguments.keywords.iter().all(|k| is_code_built(&k.value)))
        }
        _ => false,
    }
}

fn field_call(expr: &Expr) -> Option<(&str, &ast::ExprCall)> {
    let Expr::Call(call) = expr else { return None };
    let ftype = match &*call.func {
        Expr::Attribute(a) if matches!(&*a.value, Expr::Name(n) if n.id.as_str() == "fields") => a.attr.as_str(),
        Expr::Name(n) => n.id.as_str(),
        _ => return None,
    };
    FIELD_TYPES.contains(&ftype).then_some((ftype, call))
}

fn strings(expr: &Expr) -> Vec<&ast::ExprStringLiteral> {
    match expr {
        Expr::StringLiteral(s) => vec![s],
        Expr::List(l) => l.elts.iter().filter_map(|e| e.as_string_literal_expr()).collect(),
        Expr::Tuple(t) => t.elts.iter().filter_map(|e| e.as_string_literal_expr()).collect(),
        _ => vec![],
    }
}

#[derive(Default)]
struct Finder<'n> {
    attr: Option<&'n str>,
    found: bool,
}

impl<'a> Visitor<'a> for Finder<'_> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        let hit = match (self.attr, expr) {
            (None, Expr::Call(c)) => matches!(&*c.func, Expr::Name(n) if n.id.as_str() == "super"),
            (Some(attr), Expr::Attribute(a)) => a.attr.as_str() == attr,
            _ => false,
        };
        self.found |= hit;
        if !self.found {
            visitor::walk_expr(self, expr);
        }
    }
}

fn body_contains(body: &[Stmt], attr: Option<&str>) -> bool {
    let mut finder = Finder { attr, found: false };
    finder.visit_body(body);
    finder.found
}

pub fn analyze(src: &str) -> PyFacts {
    let loc = Locator::new(src);
    let mut v = PyVisitor {
        loc: &loc,
        facts: PyFacts::default(),
    };
    match parse_module(src) {
        Ok(parsed) => v.visit_body(&parsed.syntax().body),
        Err(e) => v
            .facts
            .diags
            .push(RawDiag::new("OD000", loc.pos(e.location.start()), e.error.to_string())),
    }
    v.facts
}

struct PyVisitor<'a> {
    loc: &'a Locator<'a>,
    facts: PyFacts,
}

impl PyVisitor<'_> {
    fn diag(&mut self, code: &'static str, node: &impl Ranged, message: impl Into<String>) {
        let pos = self.loc.at(node);
        self.facts.diags.push(RawDiag::new(code, pos, message));
    }

    fn spanned(&self, s: &ast::ExprStringLiteral) -> Spanned<String> {
        Spanned {
            value: s.value.to_str().to_owned(),
            pos: self.loc.at(s),
        }
    }

    fn class_fact(&mut self, class: &ast::StmtClassDef) -> Option<ClassFact> {
        let kind = class.bases().iter().find_map(|b| match tail_name(b) {
            Some("Model") => Some(ModelKind::Model),
            Some("TransientModel") => Some(ModelKind::Transient),
            Some("AbstractModel") => Some(ModelKind::Abstract),
            _ => None,
        });
        let declares_model = class.body.iter().any(|stmt| {
            let targets: &[Expr] = match stmt {
                Stmt::Assign(a) => &a.targets,
                Stmt::AnnAssign(a) => std::slice::from_ref(&*a.target),
                _ => return false,
            };
            targets
                .iter()
                .filter_map(Expr::as_name_expr)
                .any(|t| matches!(t.id.as_str(), "_name" | "_inherit"))
        });
        if kind.is_none() && !declares_model {
            return None;
        }
        let mut name: Option<String> = None;
        let mut fact = ClassFact {
            pos: self.loc.at(&class.name),
            model: String::new(),
            new_model: false,
            kind: kind.unwrap_or(ModelKind::Model),
            inherit: vec![],
            inherits: vec![],
            fields: vec![],
            methods: vec![],
            paths: vec![],
        };
        for stmt in &class.body {
            let (targets, value): (&[Expr], &Expr) = match stmt {
                Stmt::Assign(assign) => (&assign.targets, &assign.value),
                Stmt::AnnAssign(ann) => match &ann.value {
                    Some(value) => (std::slice::from_ref(&*ann.target), value),
                    None => continue,
                },
                Stmt::FunctionDef(func) => {
                    fact.methods.push(func.name.to_string());
                    self.method_checks(func, &mut fact);
                    continue;
                }
                _ => continue,
            };
            for target in targets.iter().filter_map(Expr::as_name_expr) {
                match target.id.as_str() {
                    "_name" => name = str_lit(value).map(str::to_owned),
                    "_inherit" => fact.inherit = strings(value).into_iter().map(|s| self.spanned(s)).collect(),
                    "_inherits" => {
                        if let Expr::Dict(d) = value {
                            fact.inherits = d
                                .items
                                .iter()
                                .filter_map(|i| Some((str_lit(i.key.as_ref()?)?.to_owned(), str_lit(&i.value)?.to_owned())))
                                .collect();
                        }
                    }
                    "_sql_constraints" => self.diag(
                        "OD110",
                        target,
                        "_sql_constraints is ignored; declare models.Constraint, models.Index or models.UniqueIndex attributes instead",
                    ),
                    field_name => {
                        if let Some(field) = self.field_fact(field_name, value) {
                            let delegate = field_call(value)
                                .and_then(|(_, call)| keyword(call, "delegate"))
                                .is_some_and(|d| matches!(d, Expr::BooleanLiteral(b) if b.value));
                            if let (true, Some(comodel)) = (delegate, &field.comodel) {
                                fact.inherits.push((comodel.value.clone(), field.name.clone()));
                            }
                            fact.fields.push(field);
                        }
                    }
                }
            }
        }
        fact.model = match (&name, fact.inherit.first()) {
            (Some(n), _) => n.clone(),
            (None, Some(first)) => first.value.clone(),
            (None, None) => return None,
        };
        fact.new_model = name.is_some_and(|n| !fact.inherit.iter().any(|i| i.value == n));
        Some(fact)
    }

    fn field_fact(&mut self, name: &str, value: &Expr) -> Option<FieldFact> {
        let (ftype, call) = field_call(value)?;
        let relational = matches!(ftype, "Many2one" | "One2many" | "Many2many");
        let positional = |i: usize| call.arguments.args.get(i).and_then(|a| a.as_string_literal_expr());
        let kw_str = |k: &str| keyword(call, k).and_then(|e| e.as_string_literal_expr());
        let comodel = kw_str("comodel_name").or_else(|| relational.then(|| positional(0)).flatten());
        let inverse_name = kw_str("inverse_name").or_else(|| (ftype == "One2many").then(|| positional(1)).flatten());
        if let Some(Expr::Call(default)) = keyword(call, "default")
            && matches!(tail_name(&default.func), Some("today" | "now" | "context_today"))
        {
            self.diag(
                "OD104",
                default,
                "this default is computed once at import; pass the function itself, e.g. default=fields.Date.today",
            );
        }
        if let Some(k) = call
            .arguments
            .keywords
            .iter()
            .find(|k| k.arg.as_ref().is_some_and(|a| a.as_str() == "group_operator"))
        {
            self.diag("OD108", k, "group_operator is renamed aggregator");
        }
        Some(FieldFact {
            name: name.to_owned(),
            ftype: ftype.to_owned(),
            pos: self.loc.at(value),
            comodel: comodel.map(|s| self.spanned(s)),
            inverse_name: inverse_name.map(|s| self.spanned(s)),
            related: kw_str("related").map(|s| self.spanned(s)),
            methods: ["compute", "inverse", "search"]
                .iter()
                .filter_map(|k| kw_str(k))
                .map(|s| self.spanned(s))
                .collect(),
        })
    }

    fn method_checks(&mut self, func: &ast::StmtFunctionDef, fact: &mut ClassFact) {
        for decorator in &func.decorator_list {
            match &decorator.expression {
                Expr::Call(call) if matches!(tail_name(&call.func), Some("depends" | "onchange" | "constrains")) => {
                    for arg in &call.arguments.args {
                        if let Some(s) = arg.as_string_literal_expr() {
                            fact.paths.push(self.spanned(s));
                        }
                    }
                }
                Expr::Attribute(a)
                    if matches!(a.attr.as_str(), "multi" | "one") && tail_name(&a.value) == Some("api") =>
                {
                    self.diag(
                        "OD107",
                        decorator,
                        format!("@api.{} no longer exists; methods always receive a recordset", a.attr),
                    );
                }
                _ => {}
            }
        }
        let name = func.name.as_str();
        if name == "fields_view_get" {
            self.diag(
                "OD113",
                &func.name,
                "fields_view_get() is removed; override _get_view() or get_views() instead",
            );
        }
        if name == "name_get" {
            self.diag(
                "OD106",
                &func.name,
                "name_get() is no longer called; override _compute_display_name() instead",
            );
        }
        if SUPER_REQUIRED.contains(&name) && !body_contains(&func.body, None) {
            self.diag(
                "OD105",
                &func.name,
                format!(
                    "{name}() override never calls super(), so other modules' overrides and the ORM itself are skipped"
                ),
            );
        }
    }

    fn route_checks(&mut self, func: &ast::StmtFunctionDef) {
        for decorator in &func.decorator_list {
            let Expr::Call(call) = &decorator.expression else {
                continue;
            };
            if tail_name(&call.func) != Some("route") {
                continue;
            }
            let auth = keyword(call, "auth").and_then(str_lit).unwrap_or("user");
            let kind = keyword(call, "type").and_then(str_lit).unwrap_or("http");
            if matches!(auth, "public" | "none") && body_contains(&func.body, Some("sudo")) {
                self.diag("OD111", &func.name, format!("route with auth='{auth}' calls sudo(); check that every record it reads or writes is filtered by an access check"));
            }
            if kind == "http"
                && let Some(k) = call
                    .arguments
                    .keywords
                    .iter()
                    .find(|k| k.arg.as_ref().is_some_and(|a| a.as_str() == "csrf"))
                && matches!(&k.value, Expr::BooleanLiteral(b) if !b.value)
            {
                self.diag(
                    "OD112",
                    k,
                    "csrf=False lets other sites submit this form on the user's behalf",
                );
            }
        }
    }
}

impl<'a> Visitor<'a> for PyVisitor<'_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::ClassDef(class) => {
                if let Some(fact) = self.class_fact(class) {
                    self.facts.classes.push(fact);
                }
            }
            Stmt::FunctionDef(func) => self.route_checks(func),
            _ => {}
        }
        visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::StringLiteral(s) => {
                let text = s.value.to_str();
                if text.ends_with(".xml") || text.ends_with(".csv") {
                    let base = text.rsplit(['/', '\\']).next().unwrap_or(text);
                    self.facts.file_mentions.push(base.to_owned());
                }
            }
            Expr::Call(call) => match &*call.func {
                Expr::Attribute(a) if a.attr.as_str() == "execute" && is_cursor(&a.value) => {
                    if call.arguments.args.first().is_some_and(is_formatted_string) {
                        self.diag("OD101", expr, "query built by string formatting; pass parameters separately or build it with odoo.tools.SQL");
                    }
                }
                Expr::Attribute(a) if a.attr.as_str() == "commit" && is_cursor(&a.value) => {
                    self.diag(
                        "OD103",
                        expr,
                        "do not commit the cursor; Odoo commits when the request ends and rolls back on error",
                    );
                }
                func if matches!(tail_name(func), Some("_" | "_lt"))
                    && call.arguments.args.first().is_some_and(is_formatted_string) =>
                {
                    self.diag(
                        "OD102",
                        expr,
                        "format after translating: _(\"Hello %s\", name), not _(\"Hello %s\" % name)",
                    );
                }
                _ => {}
            },
            Expr::Subscript(sub) if tail_name(&sub.value) == Some("env") => {
                if let Some(s) = sub.slice.as_string_literal_expr() {
                    let spanned = self.spanned(s);
                    self.facts.env_models.push(spanned);
                }
            }
            Expr::Attribute(a)
                if matches!(a.attr.as_str(), "_cr" | "_uid" | "_context")
                    && matches!(&*a.value, Expr::Name(n) if n.id.as_str() == "self") =>
            {
                self.diag(
                    "OD109",
                    expr,
                    format!("self.{} is deprecated; use self.env.{}", a.attr, &a.attr.as_str()[1..]),
                );
            }
            _ => {}
        }
        visitor::walk_expr(self, expr);
    }
}
