use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, BoolOp, CmpOp, Expr, Operator, Stmt};
use ruff_python_parser::parse_module;
use ruff_source_file::LineIndex;
use ruff_text_size::{Ranged, TextSize};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{
    ClassFact, DefinitionSource, DomainFact, FieldFact, ModelKind, OptionValue, Pos, PyFacts, RawDiag, Receiver,
    Spanned, SqlColumn, SqlView, XmlRef,
};
use crate::series::Series;
use crate::sql::{
    self, Created, CursorUse, QueryVars, Write, odoo_cursor, query_head, query_source, query_text, self_cursor,
    source_offset,
};
use crate::taint::{self, Params};
use crate::{domain, method_body, module_state};

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

pub(crate) fn is_cursor(expr: &Expr) -> bool {
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
        "Float" => &["string", "digits"],
        "Many2one" => &["comodel_name", "string"],
        "One2many" => &["comodel_name", "inverse_name", "string"],
        "Many2many" => &["comodel_name", "relation", "column1", "column2", "string"],
        "Selection" | "Reference" => &["selection", "string"],
        _ => &["string"],
    }
}

fn option_value(expr: &Expr) -> OptionValue {
    match expr {
        Expr::StringLiteral(s) => OptionValue::Str(s.value.to_str().to_owned()),
        Expr::BooleanLiteral(b) => OptionValue::Bool(b.value),
        Expr::NoneLiteral(_) => OptionValue::Falsy,
        Expr::NumberLiteral(n) => number_value(&n.value),
        Expr::List(l) => first_strings(&l.elts).map_or(OptionValue::Other, OptionValue::Strs),
        Expr::Tuple(t) => first_strings(&t.elts).map_or(OptionValue::Other, OptionValue::Tuple),
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

fn number_value(n: &ast::Number) -> OptionValue {
    let zero = match n {
        ast::Number::Int(i) => i.as_u64() == Some(0),
        ast::Number::Float(f) => *f == 0.0,
        ast::Number::Complex { real, imag } => *real == 0.0 && *imag == 0.0,
    };
    if zero { OptionValue::Falsy } else { OptionValue::Other }
}

/// `["a", "b"]`, or the values of `[("a", "A"), ("b",)]`.
fn first_strings(elts: &[Expr]) -> Option<Vec<String>> {
    fn first(e: &Expr) -> Option<&str> {
        match e {
            Expr::Tuple(t) => t.elts.first().and_then(str_lit),
            Expr::List(l) => l.elts.first().and_then(str_lit),
            e => str_lit(e),
        }
    }
    elts.iter().map(|e| first(e).map(str::to_owned)).collect()
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

/// The kind of an ORM model base class by its name. `CachedModel`, from 20.0, is a `Model` that
/// caches some fields of its records.
fn orm_kind(name: &str) -> Option<ModelKind> {
    match name {
        "Model" | "CachedModel" => Some(ModelKind::Model),
        "TransientModel" => Some(ModelKind::Transient),
        "AbstractModel" => Some(ModelKind::Abstract),
        _ => None,
    }
}

/// The kind of the ORM base class, or of a model class `kinds` names earlier in the file.
fn model_kind(class: &ast::StmtClassDef, kinds: &FxHashMap<String, ModelKind>) -> Option<ModelKind> {
    class
        .bases()
        .iter()
        .filter_map(tail_name)
        .find_map(|name| orm_kind(name).or_else(|| kinds.get(name).copied()))
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

/// Whether a class attribute is a call of a name that ends in `Constraint`, `Index` or
/// `UniqueIndex`, as `models.Constraint(...)` and an imported `Index(...)` are.
fn is_table_object(value: &Expr) -> bool {
    value
        .as_call_expr()
        .is_some_and(|c| matches!(tail_name(&c.func), Some("Constraint" | "Index" | "UniqueIndex")))
}

/// The query a `SQL()` or an `execute()` call is given, whatever runs it.
fn run_query(call: &ast::ExprCall) -> Option<&Expr> {
    matches!(tail_name(&call.func), Some("SQL" | "execute"))
        .then(|| call.arguments.args.first())
        .flatten()
}

/// The query `call` is given, read through the name it is bound to when the function binds it
/// once.
fn resolved_query<'a>(call: &'a ast::ExprCall, vars: Option<&QueryVars<'a>>) -> Option<&'a Expr> {
    run_query(call).map(|arg| vars.map_or(arg, |v| v.value(arg)))
}

/// The value that fills the first gap of a query given to `call`, as the name of the view it
/// creates does: the first value of an f-string or a `+` when the text after it starts no name,
/// the first value of `%` or `.format()`, or else the first parameter of the call.
fn first_value<'a>(query: &'a Expr, call: &'a ast::ExprCall) -> Option<&'a Expr> {
    match query {
        Expr::FString(_) => whole_value(query),
        Expr::BinOp(b) if b.op == Operator::Add => whole_value(query),
        Expr::BinOp(b) if b.op == Operator::Mod => Some(first_item(&b.right)),
        Expr::Call(c) => match &*c.func {
            Expr::Attribute(a) if a.attr.as_str() == "format" => c.arguments.args.first(),
            _ => None,
        },
        _ => call.arguments.args.get(1).map(first_item),
    }
}

/// A run of the literal text, or a value, of a query that an f-string or a `+` builds.
enum Piece<'a> {
    Text(&'a str),
    Value(&'a Expr),
}

fn pieces<'a>(query: &'a Expr, out: &mut Vec<Piece<'a>>) {
    match query {
        Expr::BinOp(b) if b.op == Operator::Add => {
            pieces(&b.left, out);
            pieces(&b.right, out);
        }
        Expr::FString(f) => {
            for part in f.value.iter() {
                match part {
                    ast::FStringPartRef::Literal(l) => out.push(Piece::Text(&l.value)),
                    ast::FStringPartRef::FString(f) => out.extend(f.elements.iter().map(|e| match e {
                        ast::InterpolatedStringElement::Literal(l) => Piece::Text(&l.value),
                        ast::InterpolatedStringElement::Interpolation(i) => Piece::Value(&i.expression),
                    })),
                }
            }
        }
        _ => out.push(str_lit(query).map_or(Piece::Value(query), Piece::Text)),
    }
}

/// The first value of an f-string or a `+` of a query, when the text after it starts no name, so
/// that a name it starts is that value whole.
fn whole_value(query: &Expr) -> Option<&Expr> {
    let mut all = vec![];
    pieces(query, &mut all);
    let at = all.iter().position(|p| matches!(p, Piece::Value(_)))?;
    let Piece::Value(value) = all[at] else {
        return None;
    };
    let next = all[at + 1..].iter().find(|p| !matches!(p, Piece::Text("")));
    let ends = match next {
        None => true,
        Some(Piece::Text(text)) => !text.starts_with(|c: char| c.is_alphanumeric() || c == '_' || c == '$'),
        Some(Piece::Value(_)) => false,
    };
    ends.then_some(value)
}

fn first_item(values: &Expr) -> &Expr {
    match values {
        Expr::Tuple(t) => t.elts.first().unwrap_or(values),
        _ => values,
    }
}

/// `self._table`, bare or the one argument of a call such as `SQL.identifier()` or `AsIs()`.
fn is_own_table(value: &Expr) -> bool {
    let bare = |e: &Expr| {
        matches!(e, Expr::Attribute(a) if a.attr.as_str() == "_table"
            && matches!(&*a.value, Expr::Name(n) if n.id.as_str() == "self"))
    };
    match value {
        Expr::Call(c) => c.arguments.keywords.is_empty() && matches!(&c.arguments.args[..], [arg] if bare(arg)),
        _ => bare(value),
    }
}

/// The calls of a function body, those of the functions it nests among them.
#[derive(Default)]
struct Calls<'a> {
    calls: Vec<&'a ast::ExprCall>,
}

impl<'a> Visitor<'a> for Calls<'a> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Call(call) = expr {
            self.calls.push(call);
        }
        visitor::walk_expr(self, expr);
    }
}

fn is_delegate(value: &Expr) -> bool {
    field_call(value)
        .and_then(|(_, call)| keyword(call, "delegate"))
        .is_some_and(|d| matches!(d, Expr::BooleanLiteral(b) if b.value))
}

fn is_api(expr: &Expr, name: &str) -> bool {
    matches!(expr, Expr::Attribute(a) if a.attr.as_str() == name && tail_name(&a.value) == Some("api"))
}

/// What an ORM method returns to its callers, for the OD119 message.
fn orm_result(method: &str) -> &'static str {
    match method {
        "write" => "True",
        _ => "the new records",
    }
}

/// The method of the class that computes the field, unless it calls super(), whose implementation
/// may assign the field.
fn compute_method<'c>(class: &'c ast::StmtClassDef, field: &FieldFact) -> Option<&'c ast::StmtFunctionDef> {
    let Some(OptionValue::Str(name)) = field.option("compute").map(|o| &o.value) else {
        return None;
    };
    let func = class.body.iter().rev().find_map(|stmt| match stmt {
        Stmt::FunctionDef(f) if f.name.as_str() == name => Some(f),
        _ => None,
    })?;
    (!body_contains(&func.body, None)).then_some(func)
}

/// Each method a field of the class names as its `option`, with the field.
fn field_methods<'f>(fact: &'f ClassFact, option: &str) -> Vec<(&'f str, &'f str)> {
    fact.fields
        .iter()
        .filter_map(|field| match field.option(option).map(|o| &o.value) {
            Some(OptionValue::Str(method)) => Some((method.as_str(), field.name.as_str())),
            _ => None,
        })
        .collect()
}

/// The table of the model a class defines or extends, unless `_table` is set to something other
/// than a literal.
fn model_table(fact: &ClassFact) -> Option<String> {
    match &fact.table {
        Some(OptionValue::Str(table)) => Some(table.clone()),
        Some(_) => None,
        None => Some(fact.model.replace('.', "_")),
    }
}

/// Methods that write while no record of the table can be cached: those Odoo runs when it sets up
/// tables and reflects them in `ir.model`, its `@api.autovacuum` jobs, and the `init_storage`
/// methods `storage` names.
fn cache_free(func: &ast::StmtFunctionDef, storage: &[(&str, &str)]) -> bool {
    let name = func.name.as_str();
    matches!(name, "init" | "_auto_init" | "_init_column")
        || name.starts_with("_reflect_")
        || storage.iter().any(|(method, _)| *method == name)
        || func.decorator_list.iter().any(|d| is_api(&d.expression, "autovacuum"))
}

/// Whether a write stores, in the model's own table, only the columns of `fields`, those whose
/// `inverse` the method is: the values the cache already holds.
fn stores_inverse(write: &Write, table: Option<&str>, fields: &[&str]) -> bool {
    !fields.is_empty()
        && table == Some(write.table.as_str())
        && write
            .columns
            .as_ref()
            .is_some_and(|columns| columns.iter().all(|c| fields.contains(&c.as_str())))
}

/// The facts of a Python file of a module of `series`, which names a class without `_name`.
pub fn analyze(src: &str, series: Option<Series>) -> PyFacts {
    let loc = Locator::new(src);
    let parsed = match parse_module(src) {
        Ok(parsed) => parsed,
        Err(e) => {
            let diag = RawDiag::new("OD000", loc.pos(e.location.start()), e.error.to_string());
            return PyFacts {
                diags: vec![diag],
                ..PyFacts::default()
            };
        }
    };
    let body = &parsed.syntax().body;
    let mut v = PyVisitor {
        loc: &loc,
        series,
        facts: PyFacts::default(),
        kinds: FxHashMap::default(),
        guarded: 0,
        class_models: vec![],
        class_facts: vec![],
        owners: vec![],
        query_vars: None,
        read_queries: FxHashSet::default(),
        self_cursors: vec![],
        cursor_uses: vec![],
        in_view_maker: false,
        odoo_names: module_state::odoo_names(&module_state::top_level(body))
            .into_iter()
            .map(|(local, _)| local)
            .collect(),
    };
    v.visit_body(body);
    let (patches, diags) = module_state::check(body, &loc);
    v.facts.import_patches = patches;
    v.facts.diags.extend(diags);
    v.facts
}

/// The first series that names a class without `_name` after the class, dotted, whenever
/// `_inherit` is not one string (`saas~18.1`).
const DOTTED_CLASS_NAME: Series = Series { major: 18, minor: 1 };

/// What a class body says about the name of its model.
#[derive(Default)]
struct Naming {
    /// The class assigns `_name`, a literal or not.
    named: bool,
    /// `_name` when it is a literal.
    name: Option<String>,
    inherit: InheritShape,
}

/// The form of `_inherit`, which decides the name of a class without `_name`.
#[derive(Clone, Copy, Default)]
enum InheritShape {
    #[default]
    Other,
    Str,
    /// A list or tuple, by its element count, literal or not.
    List(usize),
}

impl InheritShape {
    fn of(value: &Expr) -> Self {
        match value {
            Expr::StringLiteral(_) => Self::Str,
            Expr::List(l) => Self::List(l.elts.len()),
            Expr::Tuple(t) => Self::List(t.elts.len()),
            _ => Self::Other,
        }
    }
}

/// The name Odoo gives a class without `_name` from saas~18.1: a dot before each ASCII capital that
/// follows a character other than `_`, then lowercased, so `ResPartner` names `res.partner`.
fn dotted(class_name: &str) -> String {
    let mut out = String::with_capacity(class_name.len() + 4);
    let mut prev = None;
    for c in class_name.chars() {
        if c.is_ascii_uppercase() && prev.is_some_and(|p| p != '_') {
            out.push('.');
        }
        out.push(c);
        prev = Some(c);
    }
    out.to_lowercase()
}

struct PyVisitor<'a, 'ast> {
    loc: &'a Locator<'a>,
    series: Option<Series>,
    facts: PyFacts,
    /// Model classes of the file by Python class name.
    kinds: FxHashMap<String, ModelKind>,
    /// Depth of code that runs only once a guard has passed.
    guarded: u32,
    /// The model of each enclosing class, None for a mixin or a class that is no model.
    class_models: Vec<Option<String>>,
    /// Where the fact of each enclosing class sits in `facts.classes`, None for a class that is no
    /// model.
    class_facts: Vec<Option<usize>>,
    /// What `self` is in a method of each enclosing class, none for an enclosing function.
    owners: Vec<Option<Receiver>>,
    /// The query names of the outermost enclosing function, whose nested functions they count.
    query_vars: Option<QueryVars<'ast>>,
    /// Where each query literal `sql_columns` read starts.
    read_queries: FxHashSet<TextSize>,
    /// Whether each enclosing class holds Odoo's cursor as `self.cr`.
    self_cursors: Vec<bool>,
    /// What each enclosing function does with cursors, innermost last.
    cursor_uses: Vec<CursorUse<'ast>>,
    /// Whether the code is in the `init()` or `_auto_init()` of a class, which creates the view of
    /// the model the class is or is mixed into.
    in_view_maker: bool,
    /// The names the file's imports from `odoo` bind at import.
    odoo_names: Vec<&'ast str>,
}

impl<'ast> PyVisitor<'_, 'ast> {
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
        let mut naming = Naming::default();
        let mut fact = ClassFact {
            pos: self.loc.at(&class.name),
            class_name: class.name.to_string(),
            model: String::new(),
            new_model: false,
            kind: kind.unwrap_or(ModelKind::Model),
            inherit: vec![],
            inherit_unknown: false,
            inherits: vec![],
            delegations: vec![],
            inherits_unknown: false,
            fields: vec![],
            methods: vec![],
            private_methods: vec![],
            paths: vec![],
            depends: vec![],
            order_fields: vec![],
            rec_names_search: vec![],
            super_calls: vec![],
            extended: vec![],
            table: None,
            attrs: vec![],
            unassigned_computes: vec![],
            views: vec![],
            own_view: false,
            definitions: vec![],
            plain_bases: class.bases().iter().any(|b| tail_name(b).and_then(orm_kind).is_none()),
        };
        for stmt in &class.body {
            match stmt {
                Stmt::Assign(assign) => self.class_attrs(&assign.targets, &assign.value, &mut fact, &mut naming),
                Stmt::AnnAssign(ast::StmtAnnAssign {
                    target,
                    value: Some(value),
                    ..
                }) => self.class_attrs(std::slice::from_ref(&**target), value, &mut fact, &mut naming),
                Stmt::FunctionDef(func) => self.method_fact(func, &mut fact),
                _ => {}
            }
        }
        fact.unassigned_computes = self.unassigned_computes(class, &fact);
        fact.model = self.model_name(class.name.as_str(), &naming, &fact)?;
        fact.new_model = !fact.inherit.iter().any(|i| i.value == fact.model);
        Some(fact)
    }

    /// The model a class names: its `_name`, else the one Odoo derives in the module's series from
    /// `_inherit` and the class name. An unknown series reads it as before saas~18.1, and a class
    /// that the two readings name apart makes the file's models dynamic. A list with a non-literal
    /// entry hides the name before saas~18.1 and whether the model is new from it, so the class
    /// extends its first literal entry.
    fn model_name(&mut self, class_name: &str, naming: &Naming, fact: &ClassFact) -> Option<String> {
        let first = || fact.inherit.first().map(|i| i.value.clone());
        let count = match naming.inherit {
            _ if naming.named => return naming.name.clone().or_else(first),
            InheritShape::List(count) if count > 0 && !fact.inherit_unknown => count,
            _ => return first(),
        };
        let earlier = if count == 1 {
            first()
        } else {
            Some(class_name.to_owned())
        };
        match self.series {
            Some(series) if series >= DOTTED_CLASS_NAME => Some(dotted(class_name)),
            Some(_) => earlier,
            None => {
                self.facts.dynamic_models |= earlier.as_deref() != Some(dotted(class_name).as_str());
                earlier
            }
        }
    }

    /// What a method adds to its class: its name, whether it is `@api.private`, the `super()` calls
    /// it makes, whether it calls the method it overrides through `super()`, and for `init()` the
    /// views it creates.
    fn method_fact(&mut self, func: &ast::StmtFunctionDef, fact: &mut ClassFact) {
        fact.methods.push(func.name.to_string());
        if func.decorator_list.iter().any(|d| is_api(&d.expression, "private")) {
            fact.private_methods.push(func.name.to_string());
        }
        if func.name.as_str() == "init" {
            fact.views = self.init_views(func);
        }
        let mut calls = SuperCalls::default();
        calls.visit_body(&func.body);
        self.method_checks(func, fact);
        self.override_checks(func, &calls.calls);
        if calls.calls.iter().any(|(name, _)| *name == func.name.as_str()) {
            fact.extended.push(func.name.to_string());
        }
        fact.super_calls
            .extend(calls.calls.into_iter().map(|(name, at)| Spanned {
                value: name.to_owned(),
                pos: self.loc.pos(at),
            }));
    }

    fn class_attrs(&mut self, targets: &[Expr], value: &Expr, fact: &mut ClassFact, naming: &mut Naming) {
        for target in targets.iter().filter_map(Expr::as_name_expr) {
            match target.id.as_str() {
                "_name" => {
                    naming.named = true;
                    naming.name = str_lit(value).map(str::to_owned);
                    self.facts.dynamic_models |= naming.name.is_none();
                }
                "_inherit" => naming.inherit = self.inherit(value, fact, naming.name.as_deref()),
                "_order" | "_rec_name" | "_parent_name" | "_fold_name" | "_active_name" => {
                    self.order_fields(value, fact);
                }
                "_rec_names_search" => {
                    fact.rec_names_search = strings(value).into_iter().map(|s| self.spanned(s)).collect();
                }
                "_inherits" => self.inherits(value, fact),
                _ => self.attr_or_field(target, value, fact),
            }
        }
    }

    /// Any other class attribute: one that shapes the table, a company domain or a table query
    /// that is not a field, `_sql_constraints`, a constraint or an index, an alias of a method, or
    /// else a field.
    fn attr_or_field(&mut self, target: &ast::ExprName, value: &Expr, fact: &mut ClassFact) {
        match target.id.as_str() {
            "_table" => fact.table = Some(option_value(value)),
            "_parent_store" | "_auto" | "_log_access" | "_register" | "_date_name" => {
                let attr = Spanned {
                    value: option_value(value),
                    pos: self.loc.at(target),
                };
                fact.attrs.push((target.id.to_string(), attr));
            }
            // Odoo registers a field under any attribute name, so a field so named replaces the
            // attribute or method and gives the model no table: `_auto_init` reads `_auto` alone.
            "_check_company_domain" | "_table_query" | "_table_sql" if field_call(value).is_none() => {
                fact.methods.push(target.id.to_string());
            }
            "_sql_constraints" => {
                self.diag(
                    "OD110",
                    target,
                    "_sql_constraints is ignored; declare models.Constraint, models.Index or models.UniqueIndex attributes instead",
                );
                self.sql_constraints(value, fact);
            }
            _ => self.unnamed_attr(target, value, fact),
        }
    }

    /// A class attribute whose name means nothing to Odoo: its value tells a constraint or an index
    /// from an alias of a method and from a field.
    fn unnamed_attr(&mut self, target: &ast::ExprName, value: &Expr, fact: &mut ClassFact) {
        if is_table_object(value) {
            if let Some(definition) = value.as_call_expr().and_then(|c| c.arguments.args.first()) {
                self.definition(DefinitionSource::TableObject, definition, fact);
            }
        } else if matches!(value, Expr::Name(_) | Expr::Attribute(_) | Expr::Lambda(_)) {
            fact.methods.push(target.id.to_string());
        } else {
            self.class_field(target, value, fact);
        }
    }

    /// The definition of each `(name, definition, message)` entry of a literal `_sql_constraints`.
    fn sql_constraints(&self, value: &Expr, fact: &mut ClassFact) {
        let entries = match value {
            Expr::List(l) => &l.elts,
            Expr::Tuple(t) => &t.elts,
            _ => return,
        };
        for entry in entries {
            if let Expr::Tuple(t) = entry
                && let Some(definition) = t.elts.get(1)
            {
                self.definition(DefinitionSource::SqlConstraints, definition, fact);
            }
        }
    }

    /// The columns a literal constraint or index definition names, each where the source spells it.
    fn definition(&self, source: DefinitionSource, definition: &Expr, fact: &mut ClassFact) {
        let Some(columns) = str_lit(definition).and_then(sql::definition_columns) else {
            return;
        };
        for (name, at) in columns {
            let pos = source_offset(definition, at).map_or_else(|| self.loc.at(definition), |o| self.loc.pos(o));
            fact.definitions.push((source, Spanned { value: name, pos }));
        }
    }

    /// The views `init()` creates with a literal `CREATE VIEW` it gives `SQL()` or `execute()`,
    /// directly or through a name it binds once. A literal read twice counts once.
    fn init_views(&self, func: &ast::StmtFunctionDef) -> Vec<SqlView> {
        let vars = QueryVars::of(func);
        let mut calls = Calls::default();
        calls.visit_body(&func.body);
        let mut seen = FxHashSet::default();
        let mut views = vec![];
        for call in calls.calls {
            let Some(source) = resolved_query(call, Some(&vars)).and_then(query_source) else {
                continue;
            };
            if !seen.insert(source.literal.start()) {
                continue;
            }
            if let Some(view) = sql::view(&source.text) {
                views.push(SqlView {
                    table: view.table,
                    names: view.names,
                    pos: self.loc.at(source.literal),
                });
            }
        }
        views
    }

    /// `_inherit = _name` extends the model the class names.
    fn inherit(&mut self, value: &Expr, fact: &mut ClassFact, name: Option<&str>) -> InheritShape {
        let literal = |elts: &[Expr]| elts.iter().all(Expr::is_string_literal_expr);
        fact.inherit_unknown = match value {
            Expr::Name(n) => n.id.as_str() != "_name" || name.is_none(),
            Expr::StringLiteral(_) => false,
            Expr::List(l) => !literal(&l.elts),
            Expr::Tuple(t) => !literal(&t.elts),
            _ => true,
        };
        fact.inherit = match (value, name) {
            (Expr::Name(n), Some(name)) if n.id.as_str() == "_name" => vec![Spanned {
                value: name.to_owned(),
                pos: self.loc.at(value),
            }],
            _ => strings(value).into_iter().map(|s| self.spanned(s)).collect(),
        };
        InheritShape::of(value)
    }

    /// A `delegate=True` field may come before the dict, which adds to the pairs it gave.
    fn inherits(&mut self, value: &Expr, fact: &mut ClassFact) {
        let Expr::Dict(d) = value else {
            fact.inherits_unknown = true;
            return;
        };
        for pair in str_pairs(d) {
            if !fact.inherits.contains(&pair) {
                fact.inherits.push(pair);
            }
        }
        let entries: Vec<_> = d
            .items
            .iter()
            .map(|i| Some((str_lit(i.key.as_ref()?)?.to_owned(), i.value.as_string_literal_expr()?)))
            .collect();
        fact.inherits_unknown = entries.iter().any(Option::is_none);
        fact.delegations = entries
            .into_iter()
            .flatten()
            .map(|(parent, field)| (parent, self.spanned(field)))
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

    fn class_field(&mut self, target: &ast::ExprName, value: &Expr, fact: &mut ClassFact) {
        let Some(field) = self.field_fact(target, value) else {
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

    fn field_fact(&mut self, target: &ast::ExprName, value: &Expr) -> Option<FieldFact> {
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
            name: target.id.to_string(),
            name_pos: self.loc.at(target),
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

    /// OD119 on an override whose result never reaches its callers, and OD117 and OD118 on a
    /// `create` that takes one dict at a time.
    fn override_checks(&mut self, func: &ast::StmtFunctionDef, supers: &[(&str, TextSize)]) {
        let name = func.name.as_str();
        if matches!(name, "create" | "write" | "copy")
            && supers.iter().any(|(called, _)| *called == name)
            && !method_body::returns_value(&func.body)
        {
            self.diag(
                "OD119",
                &func.name,
                format!(
                    "{name}() calls super() but returns nothing, so callers get None instead of {}; return what super().{name}() returns",
                    orm_result(name)
                ),
            );
        }
        if name == "create" {
            self.create_decorator_checks(func);
        }
    }

    fn create_decorator_checks(&mut self, func: &ast::StmtFunctionDef) {
        let Some(decorator) = func.decorator_list.iter().find(|d| is_api(&d.expression, "model")) else {
            return;
        };
        self.diag(
            "OD117",
            decorator,
            "@api.model on create() makes Odoo call it once per record; decorate it with @api.model_create_multi and take a list of dicts",
        );
        let params = &func.parameters;
        if let Some(vals) = params.posonlyargs.iter().chain(&params.args).nth(1)
            && method_body::reads_as_dict(&func.body, vals.parameter.name.as_str())
        {
            self.diag(
                "OD118",
                decorator,
                format!(
                    "@api.model on create() passes a list of dicts, so reading {} as one dict fails; decorate it with @api.model_create_multi and loop over the list",
                    vals.parameter.name
                ),
            );
        }
    }

    /// OD120: Odoo evaluates each key in a lambda that takes the method's parameters, unless the
    /// legacy `skiparg` keys on the arguments instead. The globals of the lambda depend on the
    /// series, so `checks.rs` decides.
    fn cache_key_checks(&mut self, func: &ast::StmtFunctionDef, call: &ast::ExprCall) {
        if !matches!(tail_name(&call.func), Some("ormcache" | "ormcache_context")) || keyword(call, "skiparg").is_some()
        {
            return;
        }
        let params: Vec<&str> = func.parameters.iter().map(|p| p.name().as_str()).collect();
        for key in call.arguments.args.iter().filter_map(Expr::as_string_literal_expr) {
            for name in method_body::unknown_key_names(key.value.to_str(), &params).unwrap_or_default() {
                let message = format!(
                    "'{name}' in an ormcache key is not a parameter of {}(), so every call raises NameError; key on a parameter or on self",
                    func.name
                );
                let diag = RawDiag::new("OD120", self.loc.at(key), message);
                self.facts.cache_key_names.push((name, diag));
            }
        }
    }

    /// The compute methods of the class that can leave a field they compute unassigned on a record,
    /// with those fields; `checks.rs` keeps the fields that raise once every definition is merged.
    fn unassigned_computes(&self, class: &ast::StmtClassDef, fact: &ClassFact) -> Vec<(Spanned<String>, Vec<String>)> {
        let mut unassigned: Vec<(&ast::StmtFunctionDef, Vec<String>)> = vec![];
        for field in &fact.fields {
            let Some(func) = compute_method(class, field) else {
                continue;
            };
            if !method_body::leaves_unassigned(&func.body, &field.name, &field.ftype) {
                continue;
            }
            match unassigned.iter_mut().find(|(f, _)| std::ptr::eq(*f, func)) {
                Some((_, fields)) if !fields.contains(&field.name) => fields.push(field.name.clone()),
                Some(_) => {}
                None => unassigned.push((func, vec![field.name.clone()])),
            }
        }
        let at = |func: &ast::StmtFunctionDef| Spanned {
            value: func.name.to_string(),
            pos: self.loc.at(&func.name),
        };
        unassigned
            .into_iter()
            .map(|(func, fields)| (at(func), fields))
            .collect()
    }

    fn decorator_checks(&mut self, func: &ast::StmtFunctionDef, fact: &mut ClassFact) {
        for decorator in &func.decorator_list {
            match &decorator.expression {
                Expr::Call(call) => {
                    self.decorator_paths(func.name.as_str(), call, fact);
                    self.cache_key_checks(func, call);
                }
                Expr::Attribute(a)
                    if is_api(&decorator.expression, "multi") || is_api(&decorator.expression, "one") =>
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
    fn decorator_paths(&mut self, method: &str, call: &ast::ExprCall, fact: &mut ClassFact) {
        let Some(kind @ ("depends" | "onchange" | "constrains")) = tail_name(&call.func) else {
            return;
        };
        let mut depends = vec![];
        for s in call.arguments.args.iter().filter_map(Expr::as_string_literal_expr) {
            let path = s.value.to_str();
            if kind == "depends" {
                if path.split('.').any(|segment| segment == "id") {
                    self.diag("OD114", s, depends_on_id(path));
                }
                depends.push(self.spanned(s));
            } else if path.contains('.') {
                self.diag("OD115", s, dotted_name(kind, path));
            } else {
                fact.paths.push(self.spanned(s));
            }
        }
        if !depends.is_empty() {
            fact.depends.push((method.to_owned(), depends));
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

    fn call_checks(&mut self, call: &'ast ast::ExprCall, expr: &Expr) {
        let formatted = call.arguments.args.first().is_some_and(is_formatted_string);
        match &*call.func {
            Expr::Attribute(a) if is_cursor(&a.value) && matches!(a.attr.as_str(), "execute" | "commit") => {
                self.cursor_call(a.attr.as_str(), call, expr, formatted);
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
        self.sql_columns(call);
        self.added_columns(call);
        self.created_views(call);
    }

    /// The names of the views a `CREATE VIEW` creates, where `SQL()` spells it out or an
    /// `execute()` runs it, whichever cursor that is, when the literal head of the query spells
    /// them. A name that is `self._table` whole in a model class is the view of the class's model;
    /// another name it does not spell outside the `init()` or `_auto_init()` of a class may be any.
    fn created_views(&mut self, call: &'ast ast::ExprCall) {
        let Some(query) = resolved_query(call, self.query_vars.as_ref()) else {
            return;
        };
        let created = query_head(query).map(|head| sql::created_views(&head));
        for created in created.unwrap_or_default() {
            match created {
                Created::Named(table) if !self.facts.created_views.contains(&table) => {
                    self.facts.created_views.push(table);
                }
                Created::Named(_) => {}
                Created::FirstValue if first_value(query, call).is_some_and(is_own_table) => self.own_view(),
                Created::FirstValue | Created::Computed => self.facts.creates_any_view |= !self.in_view_maker,
            }
        }
    }

    /// A view `self._table` names: that of the model of the innermost class, or any view outside a
    /// view maker when the class is no model, whose `self` may be any model.
    fn own_view(&mut self) {
        match self.class_facts.last() {
            Some(&Some(class)) => self.facts.classes[class].own_view = true,
            _ => self.facts.creates_any_view |= !self.in_view_maker,
        }
    }

    /// The columns a `create_column()` call names with literals, or a literal `ALTER TABLE` adds
    /// where `SQL()` spells it out or a cursor runs it, whichever cursor that is.
    fn added_columns(&mut self, call: &'ast ast::ExprCall) {
        let literal = |i: usize| call.arguments.args.get(i).and_then(str_lit);
        let added = match &*call.func {
            func if tail_name(func) == Some("create_column") => match (literal(1), literal(2)) {
                (Some(table), Some(column)) => vec![(table.to_owned(), column.to_owned())],
                _ => vec![],
            },
            _ => resolved_query(call, self.query_vars.as_ref())
                .and_then(query_text)
                .map(|text| sql::added_columns(&text))
                .unwrap_or_default(),
        };
        for column in added {
            if !self.facts.added_columns.contains(&column) {
                self.facts.added_columns.push(column);
            }
        }
    }

    /// The query a call spells out or runs, unless it runs an Odoo `SQL()` value, which is read
    /// where it is built. Another `SQL()`, such as psycopg2's, is read only where an Odoo cursor
    /// runs it, since it may build a query for another database.
    fn sql_query(&self, call: &'ast ast::ExprCall) -> Option<&'ast Expr> {
        let arg = call.arguments.args.first()?;
        match &*call.func {
            func if self.odoo_sql(func) => Some(arg),
            Expr::Attribute(a) if self.runs_query(a) => {
                let value = self.query_vars.as_ref().map_or(arg, |v| v.value(arg));
                let built = matches!(value, Expr::Call(c) if self.odoo_sql(&c.func));
                (!built).then_some(value)
            }
            _ => None,
        }
    }

    /// Whether a call builds Odoo's `SQL`: its name starts from one an import of `odoo` binds, as
    /// `SQL`, `tools.SQL` or `odoo.tools.SQL` do and psycopg2's `sql.SQL` does not.
    fn odoo_sql(&self, func: &Expr) -> bool {
        tail_name(func) == Some("SQL") && module_state::root(func).is_some_and(|name| self.odoo_names.contains(&name))
    }

    /// `<cursor>.execute` on Odoo's database, or `<env>.execute_query`, which run the query they
    /// are given.
    fn runs_query(&self, method: &'ast ast::ExprAttribute) -> bool {
        match method.attr.as_str() {
            "execute" => {
                let self_cr = self.self_cursors.last().copied().unwrap_or_default();
                odoo_cursor(&method.value, self.query_vars.as_ref(), self_cr)
            }
            "execute_query" => tail_name(&method.value) == Some("env"),
            _ => false,
        }
    }

    /// The qualified columns of the query `SQL()` spells out, or that an Odoo cursor's `execute()` or
    /// `env.execute_query()` runs from a literal or from a name the function binds once to one.
    /// A literal run at several calls is read once.
    fn sql_columns(&mut self, call: &'ast ast::ExprCall) {
        let Some(source) = self.sql_query(call).and_then(query_source) else {
            return;
        };
        if !self.read_queries.insert(source.literal.start()) {
            return;
        }
        for column in sql::parse(&source.text).map(|q| q.columns).unwrap_or_default() {
            let pos = match source_offset(source.literal, column.at) {
                Some(offset) => self.loc.pos(offset),
                None => self.loc.at(source.literal),
            };
            self.facts.sql_columns.push(SqlColumn {
                table: column.table,
                column: column.column,
                text: column.text,
                pos,
            });
        }
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

    /// A commit on a cursor the function opens with a `.cursor()` call ends a transaction of its
    /// own, not the request's.
    fn cursor_call(&mut self, method: &str, call: &ast::ExprCall, expr: &Expr, formatted: bool) {
        let own = self.cursor_uses.last().is_some_and(|uses| uses.opened(call));
        match method {
            "execute" if formatted => self.diag(
                "OD101",
                expr,
                "query built by string formatting; pass parameters separately or build it with odoo.tools.SQL",
            ),
            "commit" if !own => self.diag(
                "OD103",
                expr,
                "do not commit the cursor; Odoo commits when the request ends and rolls back on error",
            ),
            _ => {}
        }
    }

    /// OD121 on each rollback of a cursor the function did not open, unless it also makes a commit
    /// OD103 reports, or Odoo calls it once the request has failed: `ir.http._handle_error()`, and
    /// `_handle_exception()` before 16.0, whose findings wait for the series of the module.
    fn rollback_checks(&mut self, func: &ast::StmtFunctionDef, uses: &CursorUse) {
        let ir_http = matches!(self.owners.last(), Some(Some(Receiver::Model(m))) if m == "ir.http");
        let name = func.name.as_str();
        if ir_http && name == "_handle_error" {
            return;
        }
        if uses.commits {
            return;
        }
        let diags = uses.rollbacks.iter().map(|call| {
            RawDiag::new(
                "OD121",
                self.loc.at(*call),
                "rolling back discards the whole transaction, the caller's work included, while the code carries on; raise an exception so that Odoo rolls back, or run the step that may fail under self.env.cr.savepoint()",
            )
        });
        if ir_http && name == "_handle_exception" {
            self.facts.legacy_hook_rollbacks.extend(diags);
        } else {
            self.facts.diags.extend(diags);
        }
    }

    /// OD122 on each literal write statement a method of the class runs without invalidating the
    /// cache itself or through another method of the class.
    fn sql_write_checks(&mut self, class: &ast::StmtClassDef, fact: Option<&ClassFact>) {
        let methods: Vec<(&ast::StmtFunctionDef, CursorUse)> = class
            .body
            .iter()
            .filter_map(Stmt::as_function_def_stmt)
            .map(|f| (f, CursorUse::scan_nested(f)))
            .collect();
        let invalidating: Vec<&str> = methods
            .iter()
            .filter(|(_, uses)| uses.invalidates)
            .map(|(f, _)| f.name.as_str())
            .collect();
        let storage = fact.map(|f| field_methods(f, "init_storage")).unwrap_or_default();
        let inverses = fact.map(|f| field_methods(f, "inverse")).unwrap_or_default();
        let table = fact.and_then(model_table);
        for (func, uses) in &methods {
            if cache_free(func, &storage)
                || uses.invalidates
                || uses.own_methods.iter().any(|m| invalidating.contains(m))
            {
                continue;
            }
            let fields: Vec<&str> = inverses
                .iter()
                .filter(|(m, _)| *m == func.name.as_str())
                .map(|(_, f)| *f)
                .collect();
            let stale = uses
                .writes
                .iter()
                .filter(|w| !uses.adds_columns_of(w) && !stores_inverse(w, table.as_deref(), &fields));
            for write in stale {
                self.diag(
                    "OD122",
                    write.call,
                    format!(
                        "{} on {} bypasses the ORM, so records this transaction already read keep stale values; call flush_model() before the query and invalidate_model() after it (flush() and invalidate_cache() before 16.0)",
                        write.verb, write.table
                    ),
                );
            }
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

    fn guarded(&mut self, guard: &'ast Expr, then: impl FnOnce(&mut Self)) {
        self.visit_expr(guard);
        let depth = u32::from(is_guard(guard));
        self.guarded += depth;
        then(self);
        self.guarded -= depth;
    }

    /// Operands of an `and` after a guard run only once it passed.
    fn and_chain(&mut self, values: &'ast [Expr]) {
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

    fn visit_function(&mut self, func: &'ast ast::StmtFunctionDef, stmt: &'ast Stmt) {
        self.route_checks(func);
        let uses = CursorUse::scan(func, self.cursor_uses.last());
        self.rollback_checks(func, &uses);
        self.flows(func);
        let outermost = self.query_vars.is_none();
        if outermost {
            self.query_vars = Some(QueryVars::of(func));
        }
        let in_class = matches!(self.owners.last(), Some(Some(_)));
        let maker = in_class && matches!(func.name.as_str(), "init" | "_auto_init");
        let in_view_maker = self.in_view_maker;
        self.in_view_maker |= maker;
        self.owners.push(None);
        self.cursor_uses.push(uses);
        visitor::walk_stmt(self, stmt);
        self.cursor_uses.pop();
        self.owners.pop();
        self.in_view_maker = in_view_maker;
        if outermost {
            self.query_vars = None;
        }
    }
}

impl<'ast> Visitor<'ast> for PyVisitor<'_, 'ast> {
    /// Statements after `if 'x' not in self.env: return` run only once the guard passed.
    fn visit_body(&mut self, body: &'ast [Stmt]) {
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

    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        match stmt {
            Stmt::ClassDef(class) => {
                let fact = self.class_fact(class);
                self.sql_write_checks(class, fact.as_ref());
                let model = fact
                    .as_ref()
                    .filter(|f| f.kind != ModelKind::Abstract)
                    .map(|f| f.model.clone());
                let owner = match &fact {
                    Some(f) => Receiver::Model(f.model.clone()),
                    None => Receiver::Class(class.name.to_string()),
                };
                self.owners.push(Some(owner));
                self.class_facts
                    .push(fact.is_some().then_some(self.facts.classes.len()));
                self.facts.classes.extend(fact);
                self.class_models.push(model);
                self.self_cursors.push(self_cursor(class));
                visitor::walk_stmt(self, stmt);
                self.self_cursors.pop();
                self.class_models.pop();
                self.class_facts.pop();
                self.owners.pop();
                return;
            }
            Stmt::FunctionDef(func) => return self.visit_function(func, stmt),
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

    fn visit_expr(&mut self, expr: &'ast Expr) {
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
        let mut facts = analyze(&src, None);
        facts.classes.remove(0).fields.remove(0)
    }

    fn option(f: &FieldFact, name: &str) -> Option<OptionValue> {
        f.option(name).map(|o| o.value.clone())
    }

    #[test]
    fn reads_none_and_zero_as_falsy() {
        let f = field("fields.Many2one('res.partner', delegate=None, required=0, index=1)");
        assert_eq!(option(&f, "delegate"), Some(OptionValue::Falsy));
        assert_eq!(option(&f, "required"), Some(OptionValue::Falsy));
        assert_eq!(option(&f, "index"), Some(OptionValue::Other));
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
        assert_eq!(
            option(&field("fields.Char('Name')"), "string"),
            Some(OptionValue::Str("Name".into()))
        );
        assert_eq!(
            option(&field("fields.Float('Rate', (3, 2))"), "digits"),
            Some(OptionValue::Other)
        );
        let f = field("fields.Many2one('res.partner', 'Owner')");
        assert_eq!(option(&f, "comodel_name"), Some(OptionValue::Str("res.partner".into())));
        assert_eq!(option(&f, "string"), Some(OptionValue::Str("Owner".into())));
        let f = field("fields.One2many('res.partner', 'parent_id', 'Contacts')");
        assert_eq!(option(&f, "inverse_name"), Some(OptionValue::Str("parent_id".into())));
        assert_eq!(option(&f, "string"), Some(OptionValue::Str("Contacts".into())));
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
        assert_eq!(
            option(&field("fields.Selection((('a', 'A'),))"), "selection"),
            Some(OptionValue::Tuple(vec!["a".into()]))
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
            None,
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
    fn reads_date_name_as_an_attribute() {
        let date_name = |value: &str| {
            let facts = analyze(
                &format!("class X(models.Model):\n    _name = 'x'\n    _date_name = {value}\n"),
                None,
            );
            let class = &facts.classes[0];
            assert!(class.methods.is_empty() && class.fields.is_empty());
            class.attr("_date_name").map(|a| a.value.clone())
        };
        assert_eq!(
            date_name("'planned_on'"),
            Some(OptionValue::Str("planned_on".to_owned()))
        );
        assert_eq!(date_name("DATE_FIELD"), Some(OptionValue::Other));
    }

    #[test]
    fn reads_parent_store_auto_and_delegations() {
        let facts = analyze(
            "class X(models.Model):\n    _name = 'x'\n    company_id = fields.Many2one('res.company', delegate=True)\n    \
             _inherits = {'res.partner': 'partner_id', 'res.company': 'company_id'}\n    _parent_store = True\n    \
             _auto = AUTO\n",
            None,
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
            .map(|(parent, d)| (parent.as_str(), d.value.as_str(), d.pos.col))
            .collect();
        assert_eq!(
            delegations,
            [("res.partner", "partner_id", 33), ("res.company", "company_id", 62)]
        );
        assert!(!class.inherits_unknown);
        assert_eq!(
            class.inherits,
            [
                ("res.company".to_owned(), "company_id".to_owned()),
                ("res.partner".to_owned(), "partner_id".to_owned())
            ]
        );
    }

    #[test]
    fn reads_register_from_the_own_body() {
        let registered = |body: &str| {
            let facts = analyze(
                &format!("class X(models.Model):\n    _name = 'x'\n{body}\n\nclass Y(X):\n    _name = 'y'\n"),
                None,
            );
            let [x, y] = &facts.classes[..] else {
                panic!("expected two classes, got {:?}", facts.classes);
            };
            (x.registered(), y.registered())
        };
        assert_eq!(registered("    _register = False"), (false, true));
        assert_eq!(registered("    _register = None"), (false, true));
        assert_eq!(registered("    _register = 0"), (false, true));
        assert_eq!(registered("    _register = True"), (true, true));
        assert_eq!(registered("    _register = REGISTER"), (true, true));
        assert_eq!(registered(""), (true, true));
    }

    #[test]
    fn counts_cached_model_as_a_model_base() {
        for base in ["models.CachedModel", "CachedModel"] {
            let facts = analyze(&format!("class X({base}):\n    _name = 'x'\n"), None);
            let class = &facts.classes[0];
            assert_eq!(class.kind, ModelKind::Model, "{base}");
            assert!(!class.plain_bases, "{base}");
        }
        let facts = analyze("class X(models.CachedModel, Layout):\n    _name = 'x'\n", None);
        assert!(facts.classes[0].plain_bases);
    }

    #[test]
    fn marks_an_inherits_with_a_non_literal_entry_unknown() {
        for value in ["PARENTS", "{'res.partner': FIELD}", "{**PARENTS}"] {
            let facts = analyze(
                &format!("class X(models.Model):\n    _name = 'x'\n    _inherits = {value}\n"),
                None,
            );
            assert!(facts.classes[0].inherits_unknown, "{value}");
        }
    }

    #[test]
    fn marks_an_inherit_with_a_non_literal_entry_unknown() {
        let unknown = |value: &str| {
            let facts = analyze(
                &format!("class X(models.Model):\n    _name = 'x'\n    _inherit = {value}\n"),
                None,
            );
            facts.classes[0].inherit_unknown
        };
        for value in [
            "PARENTS",
            "['a', PARENT]",
            "('a', PARENT)",
            "['a'] + PARENTS",
            "[*PARENTS]",
        ] {
            assert!(unknown(value), "{value}");
        }
        for value in ["_name", "'a'", "['a', 'b']", "('a', 'b')", "[]"] {
            assert!(!unknown(value), "{value}");
        }
    }

    #[test]
    fn names_a_class_without_name_as_its_series_does() {
        let named = |body: &str, series: Option<&str>| {
            let src = format!("class ResPartnerX(models.Model):\n{body}\n");
            let facts = analyze(&src, series.and_then(Series::parse));
            let class = facts.classes.first();
            class.map(|c| (c.model.clone(), c.new_model, facts.dynamic_models))
        };
        let model = |name: &str, new: bool, dynamic: bool| Some((name.to_owned(), new, dynamic));
        let extended = model("a", false, false);
        let derived = model("res.partner.x", true, false);
        let verbatim = model("ResPartnerX", true, false);
        let named_x = model("x", true, false);
        // Both readings name it after the class, so an unknown series needs no runtime models.
        let own = model("res.partner.x", false, false);
        let cases = [
            ("    _inherit = 'a'", [&extended, &extended, &extended, &extended]),
            (
                "    _inherit = ['a']",
                [&extended, &derived, &derived, &model("a", false, true)],
            ),
            (
                "    _inherit = ('a',)",
                [&extended, &derived, &derived, &model("a", false, true)],
            ),
            (
                "    _inherit = ['a', 'b']",
                [&verbatim, &derived, &derived, &model("ResPartnerX", true, true)],
            ),
            ("    _inherit = ('a', B)", [&extended, &extended, &extended, &extended]),
            (
                "    _inherit = ['a', *MIXINS]",
                [&extended, &extended, &extended, &extended],
            ),
            ("    _inherit = [PARENT]", [&None, &None, &None, &None]),
            ("    _inherit = []", [&None, &None, &None, &None]),
            ("    _inherit = PARENTS", [&None, &None, &None, &None]),
            (
                "    _name = 'x'\n    _inherit = ['a', 'b']",
                [&named_x, &named_x, &named_x, &named_x],
            ),
            (
                "    _inherit = ['res.partner.x', 'b']",
                [
                    &verbatim,
                    &model("res.partner.x", false, false),
                    &model("res.partner.x", false, false),
                    &model("ResPartnerX", true, true),
                ],
            ),
            ("    _inherit = ['res.partner.x']", [&own; 4]),
        ];
        for (body, expected) in cases {
            for (series, want) in [Some("18.0"), Some("saas~18.1"), Some("20.0"), None]
                .into_iter()
                .zip(expected)
            {
                assert_eq!(&named(body, series), want, "{body} at {series:?}");
            }
        }
    }

    #[test]
    fn derives_a_model_name_from_a_class_name() {
        for (class, model) in [
            ("ResPartner", "res.partner"),
            ("IrConfig_parameter", "ir.config_parameter"),
            ("EG", "e.g"),
            ("_Foo", "_foo"),
        ] {
            assert_eq!(dotted(class), model, "{class}");
        }
    }

    #[test]
    fn records_where_a_field_is_named() {
        let f = field("fields.Boolean()");
        assert_eq!((f.name_pos.line, f.name_pos.col), (3, 5));
        assert_eq!((f.pos.line, f.pos.col), (3, 9));
    }

    #[test]
    fn reads_the_table() {
        let facts = analyze("class X(models.Model):\n    _name = 'x'\n    _table = 'x_data'\n", None);
        assert_eq!(facts.classes[0].table, Some(OptionValue::Str("x_data".into())));
    }

    #[test]
    fn reports_a_compute_depending_on_id() {
        let facts = analyze(
            "class X(models.Model):\n    _name = 'x'\n\n    @api.depends('id', 'partner_id.id', 'identity')\n    \
             def _compute_y(self):\n        pass\n\n    @api.onchange('id')\n    def _onchange_id(self):\n        pass\n",
            None,
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
            None,
        );
        let codes: Vec<_> = facts.diags.iter().map(|d| (d.code, d.pos.line)).collect();
        assert_eq!(codes, [("OD115", 4), ("OD115", 8)]);
        let paths: Vec<_> = facts.classes[0].paths.iter().map(|p| p.value.as_str()).collect();
        assert_eq!(paths, ["partner_id"]);
    }

    #[test]
    fn keeps_depends_paths_by_method() {
        let facts = analyze(
            "class X(models.Model):\n    _name = 'x'\n\n    @api.depends('child_ids.total', 'qty')\n    \
             def _compute_total(self):\n        pass\n\n    @api.onchange('qty')\n    \
             def _onchange_qty(self):\n        pass\n",
            None,
        );
        let class = &facts.classes[0];
        let depends: Vec<_> = class
            .depends
            .iter()
            .map(|(method, paths)| {
                (
                    method.as_str(),
                    paths.iter().map(|p| p.value.as_str()).collect::<Vec<_>>(),
                )
            })
            .collect();
        assert_eq!(depends, [("_compute_total", vec!["child_ids.total", "qty"])]);
        assert_eq!(
            class.paths.iter().map(|p| p.value.as_str()).collect::<Vec<_>>(),
            ["qty"]
        );
    }

    #[test]
    fn reads_log_access_and_an_assigned_company_domain() {
        let facts = analyze(
            "class X(models.Model):\n    _name = 'x'\n    _log_access = False\n    \
             _check_company_domain = models.check_company_domain_parent_of\n\n\
             class Y(models.AbstractModel):\n    _name = 'y'\n    _table_query = fields.Char()\n",
            None,
        );
        let class = &facts.classes[0];
        assert_eq!(
            class.attr("_log_access").map(|a| &a.value),
            Some(&OptionValue::Bool(false))
        );
        assert_eq!(class.methods, ["_check_company_domain"]);
        let ledger = &facts.classes[1];
        let names: Vec<&str> = ledger.fields.iter().map(|f| f.name.as_str()).collect();
        assert!(names == ["_table_query"] && ledger.methods.is_empty());
    }

    #[test]
    fn reads_private_methods_and_the_names_a_class_binds_to_callables() {
        let facts = analyze(
            "class X(models.Model):\n    _name = 'x'\n    limit = 3\n    state = fields.Char()\n\n    \
             @api.private\n    def a(self):\n        pass\n\n    \
             @api.model\n    def b(self):\n        pass\n\n    \
             c = a\n    d = helpers.run\n    e = lambda self: None\n",
            None,
        );
        let class = &facts.classes[0];
        assert_eq!(class.private_methods, ["a"]);
        assert_eq!(class.methods, ["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn finds_the_columns_of_literal_queries_on_odoo_cursors_once_at_the_column_the_source_spells() {
        let src = "class X(models.Model):\n    _name = 'x'\n    _table_query = 'SELECT 1'\n\n    def f(self):\n        \
                   query = 'SELECT p.name FROM res_partner p'\n        self.env.cr.execute(query)\n        \
                   self.env.cr.execute(SQL(\"\"\"\n            SELECT q.ref\n            \
                   FROM res_partner q\"\"\"))\n        \
                   self.env.cr.execute('SELECT r.a\\n FROM t r')\n        \
                   self.env.cr.execute('SELECT s.b ' 'FROM t s')\n        \
                   self.env.cr.execute(f'SELECT u.c FROM t u WHERE {x}')\n        \
                   tail = SQL('SELECT v.d FROM t v')\n        self.env.execute_query(tail)\n        \
                   self._cr.execute(query)\n\n\
                   def g(cr, registry):\n    cr.execute('SELECT w.e FROM t w')\n    \
                   other = psycopg2.connect(dsn).cursor()\n    other.execute('SELECT y.f FROM t y')\n    \
                   with registry.cursor() as new_cr:\n        new_cr.execute('SELECT z.g FROM t z WHERE %s' % w)\n";
        let facts = analyze(src, None);
        let found: Vec<String> = facts
            .sql_columns
            .iter()
            .map(|c| format!("{}.{} {} {}:{}", c.table, c.column, c.text, c.pos.line, c.pos.col))
            .collect();
        assert_eq!(
            found,
            [
                "res_partner.name p.name 6:27",
                "res_partner.ref q.ref 9:22",
                "t.a r.a 11:29",
                "t.b s.b 12:29",
                "t.d v.d 14:30",
                "t.g z.g 23:34",
            ]
        );
        assert_eq!(facts.classes[0].methods, ["_table_query", "f"]);
    }

    #[test]
    fn reads_odoo_sql_where_it_is_built_and_psycopg2_sql_where_an_odoo_cursor_runs_it() {
        let odoo = "from odoo import tools\nfrom odoo.tools import sql\nimport odoo\n\n\
                    def f():\n    \
                    tools.SQL('SELECT a.x FROM t a')\n    \
                    sql.SQL('SELECT b.x FROM t b')\n    \
                    odoo.tools.SQL('SELECT c.x FROM t c')\n";
        assert_eq!(read_columns(odoo), ["t.x", "t.x", "t.x"]);
        let psycopg2 = "from psycopg2 import sql\n\n\
                        def f(env, remote):\n    \
                        sql.SQL('SELECT d.x FROM t d')\n    \
                        remote.execute(sql.SQL('SELECT e.x FROM t e'))\n    \
                        env.cr.execute(sql.SQL('SELECT f.y FROM t f'))\n";
        assert_eq!(read_columns(psycopg2), ["t.y"]);
    }

    /// The table and column of each query column a source reads, in order.
    fn read_columns(src: &str) -> Vec<String> {
        analyze(src, None)
            .sql_columns
            .iter()
            .map(|c| format!("{}.{}", c.table, c.column))
            .collect()
    }

    #[test]
    fn reads_queries_on_the_cursors_of_records_environments_requests_and_registries() {
        let src = "class X(models.Model):\n    _name = 'x'\n\n    \
                   def f(self, records, partner, other_env, db):\n        \
                   records.env.cr.execute('SELECT a.records_env FROM t a')\n        \
                   request.env.cr.execute('SELECT a.request_env FROM t a')\n        \
                   http.request.cr.execute('SELECT a.http_request FROM t a')\n        \
                   partner._cr.execute('SELECT a.partner_cr FROM t a')\n        \
                   self.env(su=True).cr.execute('SELECT a.env_call FROM t a')\n        \
                   new_env = api.Environment(cr, SUPERUSER_ID, {})\n        \
                   new_env.cr.execute('SELECT a.new_env FROM t a')\n        \
                   env2 = self.env\n        env2.cr.execute('SELECT a.env2 FROM t a')\n        \
                   with Registry(db).cursor() as reg_cr:\n            \
                   reg_cr.execute('SELECT a.registry FROM t a')\n        \
                   other_env.cr.execute('SELECT a.param_env FROM t a')\n        \
                   self.cr.execute('SELECT a.plain_self_cr FROM t a')\n        \
                   odoo.sql_db.db_connect(db).cursor().execute('SELECT a.db_connect FROM t a')\n\n    \
                   @classmethod\n    def g(cls):\n        cls._cr.execute('SELECT a.cls_cr FROM t a')\n        \
                   with contextlib.closing(cls.pool.cursor()) as cr:\n            \
                   cr.execute('SELECT a.closing FROM t a')\n";
        assert_eq!(
            read_columns(src),
            [
                "t.records_env",
                "t.request_env",
                "t.http_request",
                "t.partner_cr",
                "t.env_call",
                "t.new_env",
                "t.env2",
                "t.registry",
                "t.cls_cr",
                "t.closing",
            ]
        );
    }

    #[test]
    fn reads_self_cr_only_in_test_cases_and_classes_that_bind_it_to_odoo_s_cursor() {
        let src = "class TestX(common.TransactionCase):\n    def test_x(self):\n        \
                   self.cr.execute('SELECT a.test_case FROM t a')\n\n\
                   class Job:\n    def __init__(self, env):\n        self.cr = env.cr\n\n    \
                   def run(self):\n        self.cr.execute('SELECT a.bound FROM t a')\n\n\
                   class Legacy:\n    def __init__(self, dsn):\n        self.cr = psycopg2.connect(dsn).cursor()\n\n    \
                   def run(self):\n        self.cr.execute('SELECT a.connector FROM t a')\n\n\
                   class Plain(unittest.TestCase):\n    def test_y(self):\n        \
                   self.cr.execute('SELECT a.unit_test FROM t a')\n";
        assert_eq!(read_columns(src), ["t.test_case", "t.bound"]);
    }

    #[test]
    fn finds_the_columns_a_module_adds_with_literals_on_any_cursor() {
        let src = "def pre_init_hook(cr):\n    \
                   cr.execute('ALTER TABLE res_users ADD COLUMN IF NOT EXISTS totp_secret varchar')\n    \
                   query = 'ALTER TABLE x_box ADD COLUMN code varchar'\n    cr.execute(query)\n    \
                   env.cr.execute(SQL('ALTER TABLE x_box ADD COLUMN ref varchar, ADD COLUMN code varchar'))\n    \
                   create_column(cr, 'x_box', 'label', 'varchar')\n    \
                   tools.create_column(cr, table, 'skipped', 'varchar')\n    \
                   cr.execute('CREATE INDEX x_idx ON x_box (code)')\n";
        let pair = |t: &str, c: &str| (t.to_owned(), c.to_owned());
        assert_eq!(
            analyze(src, None).added_columns,
            [
                pair("res_users", "totp_secret"),
                pair("x_box", "code"),
                pair("x_box", "ref"),
                pair("x_box", "label")
            ]
        );
    }

    #[test]
    fn reads_the_views_init_creates_once_each() {
        let src = "class X(models.Model):\n    _name = 'x'\n    _auto = False\n\n    def init(self):\n        \
                   tools.drop_view_if_exists(self.env.cr, self._table)\n        \
                   query = 'CREATE VIEW x AS (SELECT 1 AS id, 2 AS total)'\n        self.env.cr.execute(query)\n        \
                   self.env.cr.execute(SQL('CREATE OR REPLACE VIEW %s AS SELECT * FROM y', SQL.identifier(self._table)))\n        \
                   self.env.cr.execute('SELECT 1')\n\n    \
                   def refresh(self):\n        self.env.cr.execute('CREATE VIEW z AS SELECT 1 AS id')\n";
        let facts = analyze(src, None);
        let views: Vec<String> = facts.classes[0]
            .views
            .iter()
            .map(|v| format!("{:?} {:?} {}", v.table, v.names, v.pos.line))
            .collect();
        assert_eq!(views, ["Some(\"x\") Some([\"id\", \"total\"]) 7", "None None 9"]);
        assert_eq!(facts.created_views, ["x", "z"]);
    }

    #[test]
    fn reads_the_columns_of_literal_definitions_by_source() {
        let src = "class X(models.Model):\n    _name = 'x'\n    \
                   _sql_constraints = [\n        ('a_uniq', 'unique(a)', 'A is unique'),\n        \
                   ('b_check', 'CHECK (b > %s)' % LIMIT, 'B is large'),\n        ('c_check', \"CHECK(c\\n > 0)\", ''),\n    ]\n    \
                   _d_uniq = models.UniqueIndex('(d) WHERE e IS NOT NULL')\n    \
                   _f_check = models.Constraint('CHECK (f > 0)', 'F is positive')\n    \
                   _g_index = models.Index(lambda registry: '(g)')\n    _h_index = Index('(h)')\n";
        let facts = analyze(src, None);
        let found: Vec<String> = facts.classes[0]
            .definitions
            .iter()
            .map(|(source, c)| format!("{source:?} {} {}:{}", c.value, c.pos.line, c.pos.col))
            .collect();
        assert_eq!(
            found,
            [
                "SqlConstraints a 4:28",
                "SqlConstraints c 6:21",
                "TableObject d 8:36",
                "TableObject e 8:45",
                "TableObject f 9:42",
                "TableObject h 11:24",
            ]
        );
        assert!(facts.classes[0].fields.is_empty());
    }

    #[test]
    fn reads_a_view_self_table_names_in_any_method_as_the_class_own() {
        let model =
            |call: &str| format!("class X(models.Model):\n    _name = 'x'\n\n    def refresh(self):\n        {call}\n");
        for call in [
            "self.env.cr.execute(SQL('CREATE VIEW %s AS SELECT 1', SQL.identifier(self._table)))",
            "self.env.cr.execute(f'CREATE OR REPLACE VIEW {self._table} AS SELECT 1')",
            "self.env.cr.execute(f'CREATE OR REPLACE VIEW public.{self._table}')",
            "self.env.cr.execute('CREATE VIEW %s AS SELECT 1' % (self._table,))",
            "self.env.cr.execute('CREATE VIEW %s AS SELECT 1', (AsIs(self._table),))",
            "self.env.cr.execute('CREATE VIEW ' + self._table + ' AS SELECT 1')",
        ] {
            let facts = analyze(&model(call), None);
            assert!(facts.classes[0].own_view && !facts.creates_any_view, "{call}");
        }
        for src in [
            "def hook(self):\n    self.env.cr.execute(SQL('CREATE VIEW %s AS SELECT 1', SQL.identifier(self._table)))\n"
                .to_owned(),
            model("self.env.cr.execute(SQL('CREATE VIEW %s AS SELECT 1', SQL.identifier(self._table, 'v')))"),
            model("self.env.cr.execute('CREATE VIEW %(t)s AS SELECT 1' % {'t': self._table})"),
            model("self.env.cr.execute(f'CREATE VIEW x_{self._table} AS SELECT 1')"),
            model("self.env.cr.execute(f'CREATE VIEW {self._table}_report AS SELECT 1')"),
            model("self.env.cr.execute('CREATE VIEW %s_report AS SELECT 1' % self._table)"),
            model("self.env.cr.execute('CREATE VIEW ' + self._table + '_report AS SELECT 1')"),
            "class X:\n    def refresh(self):\n        \
             self.env.cr.execute(SQL('CREATE VIEW %s', SQL.identifier(self._table)))\n"
                .to_owned(),
        ] {
            let facts = analyze(&src, None);
            assert!(facts.creates_any_view && facts.classes.iter().all(|c| !c.own_view), "{src}");
        }
        let builder = "class X:\n    def init(self):\n        \
                       self.env.cr.execute(SQL('CREATE VIEW %s', SQL.identifier(self._x_name())))\n";
        assert!(!analyze(builder, None).creates_any_view);
    }

    #[test]
    fn reads_the_views_a_query_head_names_and_whether_one_may_be_any() {
        let src = "def hook(env):\n    \
                   env.cr.execute(f\"CREATE OR REPLACE VIEW a AS ({query})\")\n    \
                   env.cr.execute(\"CREATE VIEW public.b AS \" + query)\n    \
                   env.cr.execute(SQL(\"CREATE VIEW c AS \" + query))\n    \
                   env.cr.execute(\"CREATE VIEW d AS {}\".format(query))\n    \
                   env.cr.execute(\"CREATE OR REPLACE VIEW e\" + \" AS (SELECT 1)\")\n\n\
                   class X(models.Model):\n    _name = 'x'\n\n    def init(self):\n        \
                   self.env.cr.execute(SQL('CREATE VIEW %s AS SELECT 1', SQL.identifier(self._table)))\n";
        let facts = analyze(src, None);
        assert_eq!(facts.created_views, ["a", "b", "c", "d", "e"]);
        assert!(!facts.creates_any_view);
        assert!(
            analyze(
                "def hook(env, name):\n    env.cr.execute(f'CREATE VIEW {name} AS SELECT 1')\n",
                None
            )
            .creates_any_view
        );
    }
}
