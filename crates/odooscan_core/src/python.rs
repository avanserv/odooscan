use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, BoolOp, CmpOp, Expr, Operator, Stmt};
use ruff_python_parser::parse_module;
use ruff_source_file::LineIndex;
use ruff_text_size::{Ranged, TextSize};
use rustc_hash::FxHashMap;

use crate::domain;
use crate::facts::{
    ClassFact, DomainFact, FieldFact, ModelKind, OptionValue, Pos, PyFacts, RawDiag, Receiver, Spanned, XmlRef,
};
use crate::taint::{self, Params};

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

/// Recordset methods whose first argument, or `domain`, is a domain on the recordset's model.
const DOMAIN_METHODS: &[&str] = &[
    "search",
    "search_count",
    "search_read",
    "search_fetch",
    "filtered_domain",
    "_search",
    "read_group",
    "_read_group",
];

/// Recordset methods that return the same model.
pub(crate) const SAME_MODEL: &[&str] = &["sudo", "with_context", "with_company", "with_user", "with_env"];

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
pub(crate) fn tail_name(expr: &Expr) -> Option<&str> {
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
        Expr::FString(f) => fstring_spliced(f),
        Expr::BinOp(b) if b.op == Operator::Mod => percent_spliced(b),
        Expr::BinOp(b) if b.op == Operator::Add => concat_spliced(b),
        Expr::Call(c) => format_spliced(c),
        _ => false,
    }
}

fn fstring_spliced(f: &ast::ExprFString) -> bool {
    f.value.elements().any(|e| match e {
        ast::InterpolatedStringElement::Interpolation(i) => !is_code_built(&i.expression),
        ast::InterpolatedStringElement::Literal(_) => false,
    })
}

fn percent_spliced(b: &ast::ExprBinOp) -> bool {
    (str_lit(&b.left).is_some() || is_formatted_string(&b.left)) && !is_code_built(&b.right)
}

fn concat_spliced(b: &ast::ExprBinOp) -> bool {
    str_lit(&b.left).is_some() != str_lit(&b.right).is_some()
        || is_formatted_string(&b.left)
        || is_formatted_string(&b.right)
}

fn format_spliced(c: &ast::ExprCall) -> bool {
    matches!(&*c.func, Expr::Attribute(a) if a.attr.as_str() == "format" && str_lit(&a.value).is_some())
        && !(c.arguments.args.iter().all(is_code_built) && c.arguments.keywords.iter().all(|k| is_code_built(&k.value)))
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

/// The attributes a field type's constructor takes by position, in order.
fn positional_names(ftype: &str) -> &'static [&'static str] {
    match ftype {
        "Monetary" => &["string", "currency_field"],
        "Many2many" => &["comodel_name", "relation", "column1", "column2", "string"],
        "Selection" | "Reference" => &["selection", "string"],
        _ => &[],
    }
}

fn option_value(expr: &Expr) -> OptionValue {
    match expr {
        Expr::StringLiteral(s) => OptionValue::Str(s.value.to_str().to_owned()),
        Expr::BooleanLiteral(b) => OptionValue::Bool(b.value),
        Expr::List(l) => first_strings(&l.elts),
        Expr::Tuple(t) => first_strings(&t.elts),
        Expr::Dict(d) => d
            .items
            .iter()
            .map(|i| {
                Some((
                    str_lit(i.key.as_ref()?)?.to_owned(),
                    str_lit(&i.value).map(str::to_owned),
                ))
            })
            .collect::<Option<Vec<_>>>()
            .map_or(OptionValue::Other, OptionValue::Dict),
        _ => OptionValue::Other,
    }
}

/// `["a", "b"]`, or the values of `[("a", "A"), ("b",)]`.
fn first_strings(elts: &[Expr]) -> OptionValue {
    fn first(e: &Expr) -> Option<&str> {
        match e {
            Expr::Tuple(t) => t.elts.first().and_then(str_lit),
            Expr::List(l) => l.elts.first().and_then(str_lit),
            e => str_lit(e),
        }
    }
    elts.iter()
        .map(|e| first(e).map(str::to_owned))
        .collect::<Option<Vec<_>>>()
        .map_or(OptionValue::Other, OptionValue::Strs)
}

/// The methods a field names by a string: its compute, inverse and search, the method a
/// `selection` names that returns the values, and the one `group_expand` names.
fn methods(options: &[(String, Spanned<OptionValue>)]) -> Vec<Spanned<String>> {
    let names = ["compute", "inverse", "search", "selection", "group_expand"];
    options
        .iter()
        .filter(|(name, _)| names.contains(&name.as_str()))
        .filter_map(|(_, o)| match &o.value {
            OptionValue::Str(method) => Some(Spanned {
                value: method.clone(),
                pos: o.pos,
            }),
            _ => None,
        })
        .collect()
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

/// Whether an expression tests that a model, field or module is installed, as `'x' in self.env`,
/// `'f' in self._fields`, `self.env.get('x')` or `ir.module.module._get('m')` do.
#[derive(Default)]
struct GuardFinder {
    found: bool,
}

impl<'a> Visitor<'a> for GuardFinder {
    fn visit_expr(&mut self, expr: &'a Expr) {
        self.found |= match expr {
            Expr::Compare(c) => c
                .ops
                .iter()
                .zip(c.comparators())
                .any(|(op, right)| matches!(op, CmpOp::In | CmpOp::NotIn) && is_registry(right)),
            Expr::Call(c) => match &*c.func {
                Expr::Attribute(a) if a.attr.as_str() == "get" => is_registry(&a.value),
                func => tail_name(func) == Some("_get"),
            },
            _ => false,
        };
        if !self.found {
            visitor::walk_expr(self, expr);
        }
    }
}

/// A block that leaves the function or the loop when its guard fails.
pub(crate) fn exits(body: &[Stmt]) -> bool {
    matches!(body.last(), Some(Stmt::Return(_) | Stmt::Raise(_) | Stmt::Continue(_)))
}

fn is_registry(expr: &Expr) -> bool {
    matches!(tail_name(expr), Some("env" | "_fields" | "pool"))
}

fn is_guard(expr: &Expr) -> bool {
    let mut finder = GuardFinder::default();
    finder.visit_expr(expr);
    finder.found
}

/// The methods called as `super().name(...)` or `super(Class, self).name(...)`.
#[derive(Default)]
struct SuperCalls<'a> {
    calls: Vec<(&'a str, TextSize)>,
}

impl<'a> Visitor<'a> for SuperCalls<'a> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Call(call) = expr
            && let Expr::Attribute(a) = &*call.func
            && let Expr::Call(inner) = &*a.value
            && matches!(&*inner.func, Expr::Name(n) if n.id.as_str() == "super")
        {
            self.calls.push((a.attr.as_str(), a.attr.start()));
        }
        visitor::walk_expr(self, expr);
    }
}

fn body_contains(body: &[Stmt], attr: Option<&str>) -> bool {
    let mut finder = Finder { attr, found: false };
    finder.visit_body(body);
    finder.found
}

/// The kind of the ORM base class, or of a model class `kinds` names earlier in the file.
fn model_kind(class: &ast::StmtClassDef, kinds: &FxHashMap<String, ModelKind>) -> Option<ModelKind> {
    class.bases().iter().find_map(|b| match tail_name(b) {
        Some("Model") => Some(ModelKind::Model),
        Some("TransientModel") => Some(ModelKind::Transient),
        Some("AbstractModel") => Some(ModelKind::Abstract),
        Some(name) => kinds.get(name).copied(),
        None => None,
    })
}

fn declares_model(class: &ast::StmtClassDef) -> bool {
    class
        .body
        .iter()
        .flat_map(|stmt| match stmt {
            Stmt::Assign(a) => &a.targets[..],
            Stmt::AnnAssign(a) => std::slice::from_ref(&*a.target),
            _ => &[],
        })
        .filter_map(Expr::as_name_expr)
        .any(|t| matches!(t.id.as_str(), "_name" | "_inherit"))
}

fn str_pairs(d: &ast::ExprDict) -> Vec<(String, String)> {
    d.items
        .iter()
        .filter_map(|i| Some((str_lit(i.key.as_ref()?)?.to_owned(), str_lit(&i.value)?.to_owned())))
        .collect()
}

/// The OD114 message for a dependency with an `id` segment: the path up to that segment is the
/// relational field whose change the method means to follow.
fn depends_on_id(path: &str) -> String {
    let field: Vec<&str> = path.split('.').take_while(|segment| *segment != "id").collect();
    let fix = if field.is_empty() {
        "remove it".to_owned()
    } else {
        format!("depend on '{}'", field.join("."))
    };
    format!("'{path}': a compute method cannot depend on 'id', so importing this file raises; {fix}")
}

/// The OD115 message: the field the path starts from is the one whose change Odoo can follow.
fn dotted_name(kind: &str, path: &str) -> String {
    let field = path.split('.').next().unwrap_or_default();
    format!(
        "'{path}': @api.{kind} takes field names of the model, not paths, so Odoo ignores it; name '{field}' instead"
    )
}

fn is_delegate(value: &Expr) -> bool {
    field_call(value)
        .and_then(|(_, call)| keyword(call, "delegate"))
        .is_some_and(|d| matches!(d, Expr::BooleanLiteral(b) if b.value))
}

pub fn analyze(src: &str) -> PyFacts {
    let loc = Locator::new(src);
    let mut v = PyVisitor {
        loc: &loc,
        facts: PyFacts::default(),
        kinds: FxHashMap::default(),
        guarded: 0,
        class_models: vec![],
        owners: vec![],
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
    /// Model classes of the file by Python class name.
    kinds: FxHashMap<String, ModelKind>,
    /// Depth of code that runs only once a guard has passed.
    guarded: u32,
    /// The model of each enclosing class, None for a mixin or a class that is no model.
    class_models: Vec<Option<String>>,
    /// What `self` is in a method of each enclosing class, none for an enclosing function.
    owners: Vec<Option<Receiver>>,
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
        let kind = model_kind(class, &self.kinds);
        if kind.is_none() && !declares_model(class) {
            return None;
        }
        if let Some(kind) = kind {
            self.kinds.insert(class.name.to_string(), kind);
        }
        let mut name: Option<String> = None;
        let mut fact = ClassFact {
            pos: self.loc.at(&class.name),
            model: String::new(),
            new_model: false,
            kind: kind.unwrap_or(ModelKind::Model),
            inherit: vec![],
            inherits: vec![],
            delegations: vec![],
            fields: vec![],
            methods: vec![],
            paths: vec![],
            order_fields: vec![],
            rec_names_search: vec![],
            super_calls: vec![],
            table: None,
            attrs: vec![],
            plain_bases: class
                .bases()
                .iter()
                .any(|b| !matches!(tail_name(b), Some("Model" | "TransientModel" | "AbstractModel"))),
        };
        for stmt in &class.body {
            match stmt {
                Stmt::Assign(assign) => self.class_attrs(&assign.targets, &assign.value, &mut fact, &mut name),
                Stmt::AnnAssign(ast::StmtAnnAssign {
                    target,
                    value: Some(value),
                    ..
                }) => self.class_attrs(std::slice::from_ref(&**target), value, &mut fact, &mut name),
                Stmt::FunctionDef(func) => {
                    fact.methods.push(func.name.to_string());
                    self.method_checks(func, &mut fact);
                    let mut calls = SuperCalls::default();
                    calls.visit_body(&func.body);
                    fact.super_calls
                        .extend(calls.calls.into_iter().map(|(name, at)| Spanned {
                            value: name.to_owned(),
                            pos: self.loc.pos(at),
                        }));
                }
                _ => {}
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

    fn class_attrs(&mut self, targets: &[Expr], value: &Expr, fact: &mut ClassFact, name: &mut Option<String>) {
        for target in targets.iter().filter_map(Expr::as_name_expr) {
            match target.id.as_str() {
                "_name" => {
                    *name = str_lit(value).map(str::to_owned);
                    self.facts.dynamic_models |= name.is_none();
                }
                "_inherit" => self.inherit(value, fact, name.as_deref()),
                "_order" | "_rec_name" | "_parent_name" | "_fold_name" | "_active_name" => {
                    self.order_fields(value, fact);
                }
                "_rec_names_search" => {
                    fact.rec_names_search = strings(value).into_iter().map(|s| self.spanned(s)).collect();
                }
                "_inherits" => self.inherits(value, fact),
                "_table" => fact.table = Some(option_value(value)),
                "_parent_store" | "_auto" => {
                    let attr = Spanned {
                        value: option_value(value),
                        pos: self.loc.at(target),
                    };
                    fact.attrs.push((target.id.to_string(), attr));
                }
                "_sql_constraints" => self.diag(
                    "OD110",
                    target,
                    "_sql_constraints is ignored; declare models.Constraint, models.Index or models.UniqueIndex attributes instead",
                ),
                field_name => self.class_field(field_name, value, fact),
            }
        }
    }

    /// `_inherit = _name` extends the model the class names.
    fn inherit(&mut self, value: &Expr, fact: &mut ClassFact, name: Option<&str>) {
        fact.inherit = match (value, name) {
            (Expr::Name(n), Some(name)) if n.id.as_str() == "_name" => vec![Spanned {
                value: name.to_owned(),
                pos: self.loc.at(value),
            }],
            _ => strings(value).into_iter().map(|s| self.spanned(s)).collect(),
        };
    }

    /// A `delegate=True` field may come before the dict, which adds to the pairs it gave.
    fn inherits(&mut self, value: &Expr, fact: &mut ClassFact) {
        let Expr::Dict(d) = value else { return };
        for pair in str_pairs(d) {
            if !fact.inherits.contains(&pair) {
                fact.inherits.push(pair);
            }
        }
        let fields = d
            .items
            .iter()
            .filter(|i| i.key.as_ref().is_some_and(|k| str_lit(k).is_some()));
        fact.delegations = fields
            .filter_map(|i| i.value.as_string_literal_expr())
            .map(|s| self.spanned(s))
            .collect();
    }

    /// The field of each `field [asc|desc]` term; a term that is an SQL expression is not one.
    fn order_fields(&mut self, value: &Expr, fact: &mut ClassFact) {
        let Some(s) = value.as_string_literal_expr() else {
            return;
        };
        let pos = self.loc.at(s);
        for term in s.value.to_str().split(',') {
            let field = term.split_whitespace().next().unwrap_or_default();
            if !field.is_empty() && field.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') {
                fact.order_fields.push(Spanned {
                    value: field.to_owned(),
                    pos,
                });
            }
        }
    }

    fn class_field(&mut self, name: &str, value: &Expr, fact: &mut ClassFact) {
        let Some(field) = self.field_fact(name, value) else {
            return;
        };
        if is_delegate(value)
            && let Some(comodel) = &field.comodel
        {
            let pair = (comodel.value.clone(), field.name.clone());
            if !fact.inherits.contains(&pair) {
                fact.inherits.push(pair);
            }
        }
        fact.fields.push(field);
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
        let options = self.options(ftype, call);
        Some(FieldFact {
            name: name.to_owned(),
            ftype: ftype.to_owned(),
            pos: self.loc.at(value),
            comodel: comodel.map(|s| self.spanned(s)),
            inverse_name: inverse_name.map(|s| self.spanned(s)),
            related: kw_str("related").map(|s| self.spanned(s)),
            methods: methods(&options),
            domain: keyword(call, "domain").map(|d| self.domain(d)).unwrap_or_default(),
            options,
            splat: call.arguments.args.iter().any(Expr::is_starred_expr)
                || call.arguments.keywords.iter().any(|k| k.arg.is_none()),
        })
    }

    fn options(&self, ftype: &str, call: &ast::ExprCall) -> Vec<(String, Spanned<OptionValue>)> {
        let spanned = |e: &Expr| Spanned {
            value: option_value(e),
            pos: self.loc.at(e),
        };
        let positional = positional_names(ftype)
            .iter()
            .zip(call.arguments.args.iter().take_while(|a| !a.is_starred_expr()))
            .map(|(name, e)| ((*name).to_owned(), spanned(e)));
        let keywords = call
            .arguments
            .keywords
            .iter()
            .filter_map(|k| Some((k.arg.as_ref()?.to_string(), spanned(&k.value))));
        positional.chain(keywords).collect()
    }

    /// The fields a literal domain names, given as a list or as the text of one.
    fn domain(&self, expr: &Expr) -> Vec<Spanned<String>> {
        if let Expr::StringLiteral(s) = expr {
            let pos = self.loc.at(s);
            let fields = domain::parse(s.value.to_str()).unwrap_or_default();
            return fields.into_iter().map(|value| Spanned { value, pos }).collect();
        }
        domain::fields(expr)
            .unwrap_or_default()
            .into_iter()
            .map(|s| self.spanned(s))
            .collect()
    }

    /// The model of `self` in a model class other than a mixin, or of `env['x']`, through methods
    /// that keep the model.
    fn receiver_model(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Name(n) if n.id.as_str() == "self" => self.class_models.last().cloned().flatten(),
            Expr::Subscript(sub) if tail_name(&sub.value) == Some("env") => str_lit(&sub.slice).map(str::to_owned),
            Expr::Call(c) => match &*c.func {
                Expr::Attribute(a) if SAME_MODEL.contains(&a.attr.as_str()) => self.receiver_model(&a.value),
                _ => None,
            },
            _ => None,
        }
    }

    fn domain_call(&mut self, call: &ast::ExprCall) {
        let Expr::Attribute(a) = &*call.func else { return };
        if !DOMAIN_METHODS.contains(&a.attr.as_str()) || self.guarded > 0 {
            return;
        }
        let Some(model) = self.receiver_model(&a.value) else {
            return;
        };
        let Some(domain) = keyword(call, "domain").or_else(|| call.arguments.args.first()) else {
            return;
        };
        let fields = self.domain(domain);
        if !fields.is_empty() {
            self.facts.domains.push(DomainFact { model, fields });
        }
    }

    fn method_checks(&mut self, func: &ast::StmtFunctionDef, fact: &mut ClassFact) {
        self.decorator_checks(func, fact);
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

    fn decorator_checks(&mut self, func: &ast::StmtFunctionDef, fact: &mut ClassFact) {
        for decorator in &func.decorator_list {
            match &decorator.expression {
                Expr::Call(call) => self.decorator_paths(call, fact),
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
    }

    /// The names given to `@api.depends`, `@api.onchange` and `@api.constrains`. The last two match
    /// field names only, so a dotted one is no path to resolve.
    fn decorator_paths(&mut self, call: &ast::ExprCall, fact: &mut ClassFact) {
        let Some(kind @ ("depends" | "onchange" | "constrains")) = tail_name(&call.func) else {
            return;
        };
        for s in call.arguments.args.iter().filter_map(Expr::as_string_literal_expr) {
            let path = s.value.to_str();
            if kind == "depends" && path.split('.').any(|segment| segment == "id") {
                self.diag("OD114", s, depends_on_id(path));
            }
            if kind != "depends" && path.contains('.') {
                self.diag("OD115", s, dotted_name(kind, path));
            } else {
                fact.paths.push(self.spanned(s));
            }
        }
    }

    /// Routes take their parameters from the request, and public model methods from RPC callers,
    /// unless `@api.private` keeps them from RPC.
    fn flows(&mut self, func: &ast::StmtFunctionDef) {
        let decorated = |name: &str| {
            func.decorator_list.iter().any(|d| match &d.expression {
                Expr::Call(call) => tail_name(&call.func) == Some(name),
                e => tail_name(e) == Some(name),
            })
        };
        let params = if decorated("route") {
            Params::Route
        } else if matches!(self.owners.last(), Some(Some(Receiver::Model(_))))
            && !func.name.starts_with('_')
            && !decorated("private")
        {
            Params::Rpc
        } else {
            Params::None
        };
        let owner = self.owners.last().cloned().flatten();
        let (diags, summary) = taint::function(func, params, owner, self.loc);
        self.facts.diags.extend(diags);
        self.facts.flows.extend(summary);
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

    fn file_mention(&mut self, s: &ast::ExprStringLiteral) {
        let text = s.value.to_str();
        if text.ends_with(".xml") || text.ends_with(".csv") {
            let base = text.rsplit(['/', '\\']).next().unwrap_or(text);
            self.facts.file_mentions.push(base.to_owned());
        }
    }

    fn call_checks(&mut self, call: &ast::ExprCall, expr: &Expr) {
        let formatted = call.arguments.args.first().is_some_and(is_formatted_string);
        match &*call.func {
            Expr::Attribute(a) if is_cursor(&a.value) && matches!(a.attr.as_str(), "execute" | "commit") => {
                self.cursor_call(a.attr.as_str(), expr, formatted);
            }
            func if matches!(tail_name(func), Some("_" | "_lt")) && formatted => {
                self.diag(
                    "OD102",
                    expr,
                    "format after translating: _(\"Hello %s\", name), not _(\"Hello %s\" % name)",
                );
            }
            _ => {}
        }
        self.domain_call(call);
        self.id_refs(call, expr);
    }

    fn id_refs(&mut self, call: &ast::ExprCall, expr: &Expr) {
        let first = call.arguments.args.first().and_then(Expr::as_string_literal_expr);
        match &*call.func {
            Expr::Attribute(a) if a.attr.as_str() == "ref" && tail_name(&a.value) == Some("env") => self.env_ref(call),
            Expr::Attribute(a) if a.attr.as_str() == "has_group" => first.into_iter().for_each(|s| self.group_refs(s)),
            _ if field_call(expr).is_some() => {
                if let Some(groups) = keyword(call, "groups").and_then(Expr::as_string_literal_expr) {
                    self.group_refs(groups);
                }
            }
            _ => {}
        }
    }

    /// `env.ref()` raises on an unknown id unless told not to, and then the caller handles it.
    fn env_ref(&mut self, call: &ast::ExprCall) {
        let raise = keyword(call, "raise_if_not_found").or_else(|| call.arguments.args.get(1));
        if raise.is_none_or(|e| matches!(e, Expr::BooleanLiteral(b) if b.value))
            && let Some(id) = call.arguments.args.first().and_then(Expr::as_string_literal_expr)
            && id.value.to_str().contains('.')
        {
            let pos = self.loc.at(id);
            self.facts.xml_refs.push(XmlRef {
                id: id.value.to_str().to_owned(),
                pos,
            });
        }
    }

    /// The ids in a comma-separated list of groups.
    fn group_refs(&mut self, s: &ast::ExprStringLiteral) {
        let pos = self.loc.at(s);
        let ids = s
            .value
            .to_str()
            .split(',')
            .map(|g| g.trim().trim_start_matches(['!', '-']));
        for id in ids.filter(|id| id.contains('.')) {
            self.facts.group_refs.push(XmlRef { id: id.to_owned(), pos });
        }
    }

    fn cursor_call(&mut self, method: &str, expr: &Expr, formatted: bool) {
        match method {
            "execute" if formatted => self.diag(
                "OD101",
                expr,
                "query built by string formatting; pass parameters separately or build it with odoo.tools.SQL",
            ),
            "commit" => self.diag(
                "OD103",
                expr,
                "do not commit the cursor; Odoo commits when the request ends and rolls back on error",
            ),
            _ => {}
        }
    }

    fn env_model(&mut self, sub: &ast::ExprSubscript) {
        if tail_name(&sub.value) == Some("env")
            && self.guarded == 0
            && let Some(s) = sub.slice.as_string_literal_expr()
        {
            let spanned = self.spanned(s);
            self.facts.env_models.push(spanned);
        }
    }

    fn guarded(&mut self, guard: &Expr, then: impl FnOnce(&mut Self)) {
        self.visit_expr(guard);
        let depth = u32::from(is_guard(guard));
        self.guarded += depth;
        then(self);
        self.guarded -= depth;
    }

    /// Operands of an `and` after a guard run only once it passed.
    fn and_chain(&mut self, values: &[Expr]) {
        let mut depth = 0;
        for value in values {
            self.visit_expr(value);
            if depth == 0 && is_guard(value) {
                depth = 1;
                self.guarded += 1;
            }
        }
        self.guarded -= depth;
    }

    fn private_attr(&mut self, a: &ast::ExprAttribute, expr: &Expr) {
        if matches!(a.attr.as_str(), "_cr" | "_uid" | "_context")
            && matches!(&*a.value, Expr::Name(n) if n.id.as_str() == "self")
        {
            self.diag(
                "OD109",
                expr,
                format!("self.{} is deprecated; use self.env.{}", a.attr, &a.attr.as_str()[1..]),
            );
        }
    }
}

impl<'a> Visitor<'a> for PyVisitor<'_> {
    /// Statements after `if 'x' not in self.env: return` run only once the guard passed.
    fn visit_body(&mut self, body: &'a [Stmt]) {
        let mut depth = 0;
        for stmt in body {
            self.visit_stmt(stmt);
            if depth == 0
                && let Stmt::If(i) = stmt
                && is_guard(&i.test)
                && exits(&i.body)
            {
                depth = 1;
                self.guarded += 1;
            }
        }
        self.guarded -= depth;
    }

    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::ClassDef(class) => {
                let fact = self.class_fact(class);
                let model = fact
                    .as_ref()
                    .filter(|f| f.kind != ModelKind::Abstract)
                    .map(|f| f.model.clone());
                let owner = match &fact {
                    Some(f) => Receiver::Model(f.model.clone()),
                    None => Receiver::Class(class.name.to_string()),
                };
                self.owners.push(Some(owner));
                self.facts.classes.extend(fact);
                self.class_models.push(model);
                visitor::walk_stmt(self, stmt);
                self.class_models.pop();
                self.owners.pop();
                return;
            }
            Stmt::FunctionDef(func) => {
                self.route_checks(func);
                self.flows(func);
                self.owners.push(None);
                visitor::walk_stmt(self, stmt);
                self.owners.pop();
                return;
            }
            Stmt::If(i) => {
                self.guarded(&i.test, |v| v.visit_body(&i.body));
                for clause in &i.elif_else_clauses {
                    self.visit_elif_else_clause(clause);
                }
                return;
            }
            _ => {}
        }
        visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::StringLiteral(s) => self.file_mention(s),
            Expr::Call(call) => self.call_checks(call, expr),
            Expr::BoolOp(b) if b.op == BoolOp::And => return self.and_chain(&b.values),
            Expr::If(i) => {
                self.guarded(&i.test, |v| v.visit_expr(&i.body));
                return self.visit_expr(&i.orelse);
            }
            Expr::Subscript(sub) => self.env_model(sub),
            Expr::Attribute(a) => self.private_attr(a, expr),
            _ => {}
        }
        visitor::walk_expr(self, expr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(src: &str) -> FieldFact {
        let src = format!("class X(models.Model):\n    _name = 'x'\n    f = {src}\n");
        let mut facts = analyze(&src);
        facts.classes.remove(0).fields.remove(0)
    }

    fn option(f: &FieldFact, name: &str) -> Option<OptionValue> {
        f.option(name).map(|o| o.value.clone())
    }

    #[test]
    fn names_positional_arguments_by_field_type() {
        let f = field("fields.Many2many('res.partner', 'x_partner_rel', 'x_id', string='Partners')");
        assert_eq!(option(&f, "comodel_name"), Some(OptionValue::Str("res.partner".into())));
        assert_eq!(option(&f, "relation"), Some(OptionValue::Str("x_partner_rel".into())));
        assert_eq!(option(&f, "column1"), Some(OptionValue::Str("x_id".into())));
        assert_eq!(option(&f, "column2"), None);
        assert_eq!(option(&f, "string"), Some(OptionValue::Str("Partners".into())));
        let f = field("fields.Monetary('Amount', 'company_currency_id')");
        assert_eq!(
            option(&f, "currency_field"),
            Some(OptionValue::Str("company_currency_id".into()))
        );
        assert!(field("fields.Char('Name')").options.is_empty());
    }

    #[test]
    fn reads_literal_values() {
        let f = field(
            "fields.Selection([('a', 'A'), ('b',)], required=True, ondelete={'a': 'cascade', 'b': drop}, \
             selection_add=compute_values())",
        );
        assert_eq!(
            option(&f, "selection"),
            Some(OptionValue::Strs(vec!["a".into(), "b".into()]))
        );
        assert_eq!(option(&f, "required"), Some(OptionValue::Bool(true)));
        assert_eq!(
            option(&f, "ondelete"),
            Some(OptionValue::Dict(vec![
                ("a".into(), Some("cascade".into())),
                ("b".into(), None)
            ]))
        );
        assert_eq!(option(&f, "selection_add"), Some(OptionValue::Other));
        assert!(!f.splat);
    }

    #[test]
    fn notes_unpacked_arguments() {
        assert!(field("fields.Monetary(**kwargs)").splat);
        let f = field("fields.Many2many(*args, relation='r')");
        assert!(f.splat);
        assert_eq!(option(&f, "comodel_name"), None);
        assert_eq!(option(&f, "relation"), Some(OptionValue::Str("r".into())));
    }

    #[test]
    fn reads_the_methods_a_field_names() {
        let names = |f: FieldFact| f.methods.into_iter().map(|m| m.value).collect::<Vec<_>>();
        assert_eq!(
            names(field(
                "fields.Selection('_sizes', group_expand='_expand', compute='_compute')"
            )),
            ["_sizes", "_expand", "_compute"]
        );
        assert_eq!(names(field("fields.Reference('_targets')")), ["_targets"]);
        assert!(names(field("fields.Selection([('a', 'A')], group_expand=True)")).is_empty());
        assert!(names(field("fields.Char('_label')")).is_empty());
    }

    #[test]
    fn reads_the_fields_model_attributes_name() {
        let facts = analyze(
            "class X(models.Model):\n    _name = 'x'\n    _order = 'sequence desc, id'\n    _parent_name = 'up_id'\n    \
             _fold_name = 'folded'\n    _active_name = 'x_active'\n    _rec_names_search = ['name', 'partner_id.ref']\n",
        );
        let values = |v: &[Spanned<String>]| v.iter().map(|s| s.value.clone()).collect::<Vec<_>>();
        let class = &facts.classes[0];
        assert_eq!(
            values(&class.order_fields),
            ["sequence", "id", "up_id", "folded", "x_active"]
        );
        assert_eq!(values(&class.rec_names_search), ["name", "partner_id.ref"]);
    }

    #[test]
    fn reads_parent_store_auto_and_delegations() {
        let facts = analyze(
            "class X(models.Model):\n    _name = 'x'\n    company_id = fields.Many2one('res.company', delegate=True)\n    \
             _inherits = {'res.partner': 'partner_id', 'res.company': 'company_id'}\n    _parent_store = True\n    \
             _auto = AUTO\n",
        );
        let class = &facts.classes[0];
        assert_eq!(
            class.attr("_parent_store").map(|a| (&a.value, a.pos.line)),
            Some((&OptionValue::Bool(true), 5))
        );
        assert_eq!(class.attr("_auto").map(|a| &a.value), Some(&OptionValue::Other));
        let delegations: Vec<_> = class
            .delegations
            .iter()
            .map(|d| (d.value.as_str(), d.pos.col))
            .collect();
        assert_eq!(delegations, [("partner_id", 33), ("company_id", 62)]);
        assert_eq!(
            class.inherits,
            [
                ("res.company".to_owned(), "company_id".to_owned()),
                ("res.partner".to_owned(), "partner_id".to_owned())
            ]
        );
    }

    #[test]
    fn reads_the_table() {
        let facts = analyze("class X(models.Model):\n    _name = 'x'\n    _table = 'x_data'\n");
        assert_eq!(facts.classes[0].table, Some(OptionValue::Str("x_data".into())));
    }

    #[test]
    fn reports_a_compute_depending_on_id() {
        let facts = analyze(
            "class X(models.Model):\n    _name = 'x'\n\n    @api.depends('id', 'partner_id.id', 'identity')\n    \
             def _compute_y(self):\n        pass\n\n    @api.onchange('id')\n    def _onchange_id(self):\n        pass\n",
        );
        let codes: Vec<_> = facts.diags.iter().map(|d| (d.code, d.pos.col)).collect();
        assert_eq!(codes, [("OD114", 18), ("OD114", 24)]);
    }

    #[test]
    fn reports_dotted_onchange_and_constrains_names_and_keeps_them_out_of_paths() {
        let facts = analyze(
            "class X(models.Model):\n    _name = 'x'\n\n    @api.onchange('partner_id.name', 'partner_id')\n    \
             def _onchange_partner(self):\n        pass\n\n    @api.constrains('line_ids.qty')\n    \
             def _check_qty(self):\n        pass\n\n    @api.depends('partner_id.name')\n    \
             def _compute_label(self):\n        pass\n",
        );
        let codes: Vec<_> = facts.diags.iter().map(|d| (d.code, d.pos.line)).collect();
        assert_eq!(codes, [("OD115", 4), ("OD115", 8)]);
        let paths: Vec<_> = facts.classes[0].paths.iter().map(|p| p.value.as_str()).collect();
        assert_eq!(paths, ["partner_id", "partner_id.name"]);
    }
}
