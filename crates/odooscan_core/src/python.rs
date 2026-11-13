use ruff_python_ast::comparable::ComparableArguments;
use ruff_python_ast::token::{Token, TokenKind};
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, BoolOp, CmpOp, Expr, Operator, Stmt, UnaryOp};
use ruff_python_parser::{Mode, parse_expression_range, parse_module, parse_unchecked};
use ruff_source_file::LineIndex;
use ruff_text_size::{Ranged, TextRange, TextSize};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::api_table::{self, Kind};
use crate::facts::{
    ClassFact, DefinitionSource, DomainFact, FieldClass, FieldFact, GuardedRef, MethodDef, ModelKind, OptionValue,
    Param, Pos, PyFacts, RawDiag, Receiver, Signature, Spanned, SqlColumn, SqlView, Subject, UNREAD_TYPE, XmlRef,
};
use crate::fix::{self, Edit, Fix};
use crate::route_body::{self, RouteBody};
use crate::routing::{self, Routing, Scope};
use crate::series::Series;
use crate::sql::{
    self, Created, CursorUse, QueryVars, Write, is_self, odoo_cursor, query_head, query_source, query_text,
    self_cursor, source_offset,
};
use crate::superuser::{self, Escalation};
use crate::taint::{self, Params, TaintTable};
use crate::{async_names, domain, method_body, module_state, python2};

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
    "Serialized",
];

/// The packages whose `fields` module holds the ORM's field classes, by series.
const ORM_PACKAGES: &[&str] = &["odoo", "openerp"];

/// The field classes a whole scan defines, by class name, each with its field type.
pub type FieldClasses = FxHashMap<String, String>;

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

    /// The fix that writes `replacement` over the text of `range`.
    fn replace(&self, range: TextRange, replacement: &str) -> Fix {
        Fix::single(Edit {
            start: range.start().to_usize(),
            end: range.end().to_usize(),
            expected: self.src[range].to_owned(),
            replacement: replacement.to_owned(),
        })
    }

    /// The fix that removes `node` with its line, when it is alone there.
    fn remove_line(&self, node: &impl Ranged) -> Option<Fix> {
        fix::remove_line(self.src, node.start().to_usize(), node.end().to_usize()).map(Fix::single)
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

pub(crate) fn keyword<'a>(call: &'a ast::ExprCall, name: &str) -> Option<&'a Expr> {
    keyword_arg(call, name).map(|k| &k.value)
}

pub(crate) fn keyword_arg<'a>(call: &'a ast::ExprCall, name: &str) -> Option<&'a ast::Keyword> {
    call.arguments
        .keywords
        .iter()
        .find(|k| k.arg.as_ref().is_some_and(|a| a.as_str() == name))
}

/// The range of the name of keyword `k` of `call`, which a rename to `to` replaces, unless the call
/// already passes `to` or may through `**`, which would pass it twice.
fn renamed_keyword(call: &ast::ExprCall, k: &ast::Keyword, to: &str) -> Option<TextRange> {
    let unpacked = call.arguments.keywords.iter().any(|k| k.arg.is_none());
    (!unpacked && keyword_arg(call, to).is_none())
        .then(|| k.arg.as_ref().map(Ranged::range))
        .flatten()
}

/// Whether `track_visibility=` can become `tracking=True`: 13.0 tracks a field whose `tracking` or
/// `track_visibility` is truthy (`addons/mail/models/mail_thread.py:549`), which a non-empty string
/// is, and the call sets neither `tracking=` nor `track_sequence=`, nor may through `**`.
fn tracked(call: &ast::ExprCall, k: &ast::Keyword) -> bool {
    let truthy = str_lit(&k.value).is_some_and(|v| !v.is_empty());
    let unpacked = call.arguments.keywords.iter().any(|k| k.arg.is_none());
    truthy
        && !unpacked
        && ["tracking", "track_sequence"]
            .iter()
            .all(|o| keyword_arg(call, o).is_none())
}

/// Whether the argument of `sudo()` is a user rather than the flag: an id, a record a call looks
/// up, or a name whose last word is a user, such as `user`, `self.env.uid`, `admin_user.id`,
/// `env.ref("base.user_admin").id` or `SUPERUSER_ID`. A flag such as `is_superuser`,
/// `use_superuser` or `self.user_can_edit` names something else.
fn names_user(arg: &Expr) -> bool {
    let (named, id) = match arg {
        Expr::NumberLiteral(n) => return matches!(n.value, ast::Number::Int(_)),
        Expr::Call(c) => return matches!(tail_name(&c.func), Some("ref" | "browse")),
        Expr::Attribute(a) if a.attr.as_str() == "id" => match &*a.value {
            Expr::Call(_) => return names_user(&a.value),
            value => (value, true),
        },
        _ => (arg, false),
    };
    tail_name(named).is_some_and(|name| {
        let name = name.to_ascii_lowercase();
        let mut words: Vec<&str> = name.split('_').filter(|w| !w.is_empty()).collect();
        let id = id || words.last() == Some(&"id");
        if words.last() == Some(&"id") {
            words.pop();
        }
        let flag = matches!(words.first(), Some(&("is" | "has" | "can")));
        // Flags name the superuser mode too, so only its id is a user.
        !flag
            && matches!(
                (words.last(), id),
                (Some(&("user" | "uid")), _) | (Some(&"superuser"), true)
            )
    })
}

/// `request.session.uid` or `request.session['uid']`.
fn is_session_uid(target: &Expr) -> bool {
    let (session, uid) = match target {
        Expr::Attribute(a) => (&*a.value, a.attr.as_str() == "uid"),
        Expr::Subscript(s) => (&*s.value, str_lit(&s.slice) == Some("uid")),
        _ => return false,
    };
    uid && matches!(session, Expr::Attribute(s) if s.attr.as_str() == "session" && route_body::is_request(&s.value))
}

/// Whether a user given to `update_env()` or assigned as the uid switches to another one: not `None`,
/// which keeps the user, nor `request.session.uid`, the user Odoo checked the session of before any
/// `_auth_method_*()` (18.0 `odoo/addons/base/models/ir_http.py:268-272`).
fn switches_user(user: &Expr) -> bool {
    !user.is_none_literal_expr() && !is_session_uid(user)
}

/// A method that compares a field with the value it is set to.
struct ChangeCheck<'a> {
    /// The values to set: the parameter named `vals` or `values`, else the second positional one,
    /// which an old-API `write(self, cr, uid, ids, vals)` names. Filled for an onchange too: Odoo
    /// passes one no values, but a helper named `_onchange_*` may take them.
    vals: Option<&'a str>,
    onchange: bool,
    /// The target of each enclosing `for` loop, and whether it loops over `self`.
    loops: Vec<(&'a str, bool)>,
}

/// `write()`, or an onchange by its decorator or its `_onchange_` name.
fn checks_change(func: &ast::StmtFunctionDef) -> Option<ChangeCheck<'_>> {
    let decorated = |d: &ast::Decorator| matches!(&d.expression, Expr::Call(c) if is_api(&c.func, "onchange"));
    let onchange = func.name.starts_with("_onchange_") || func.decorator_list.iter().any(decorated);
    if !onchange && func.name.as_str() != "write" {
        return None;
    }
    let params = &func.parameters;
    let mut positional = params
        .posonlyargs
        .iter()
        .chain(&params.args)
        .map(|p| p.parameter.name.as_str());
    let named = params
        .iter_non_variadic_params()
        .map(|p| p.parameter.name.as_str())
        .find(|n| matches!(*n, "vals" | "values"));
    Some(ChangeCheck {
        vals: named.or_else(|| positional.nth(1)),
        onchange,
        loops: vec![],
    })
}

impl ChangeCheck<'_> {
    /// Whether `expr` is `self`, or a loop target the innermost loop binding it takes from `self`.
    fn holds_form(&self, expr: &Expr) -> bool {
        is_self(expr)
            || expr.as_name_expr().is_some_and(|n| {
                let target = self.loops.iter().rev().find(|(t, _)| *t == n.id.as_str());
                target.is_some_and(|&(_, over_self)| over_self)
            })
    }
}

/// The word that makes `expr` a token, a secret or a signature: the last word of its name,
/// attribute, literal key, `.get()` key or called method, as `access_token`, `kw['token']`,
/// `kw.get('signature')` or `self._get_secret()`.
fn secret_word(expr: &Expr) -> Option<&'static str> {
    let name = match expr {
        Expr::Subscript(s) => str_lit(&s.slice),
        Expr::Call(c) => match &*c.func {
            Expr::Attribute(a) if a.attr.as_str() == "get" => c.arguments.args.first().and_then(str_lit),
            func => tail_name(func),
        },
        _ => tail_name(expr),
    };
    let word = name.and_then(last_word)?;
    ["token", "secret", "signature"]
        .into_iter()
        .find(|s| word.eq_ignore_ascii_case(s))
}

/// Whether `a` and `b` read one field of two records, call one function on two of them, or call
/// one method other than `get()` with the same arguments on two, as `a.signature == b.signature`,
/// `get_signature(a) == get_signature(b)` or `a._get_signature() == b._get_signature()`.
fn same_source(a: &Expr, b: &Expr) -> bool {
    match (a, b) {
        (Expr::Attribute(x), Expr::Attribute(y)) => x.attr.as_str() == y.attr.as_str(),
        (Expr::Call(x), Expr::Call(y)) => match (&*x.func, &*y.func) {
            (Expr::Name(f), Expr::Name(g)) => f.id.as_str() == g.id.as_str(),
            // A method gets its record as receiver, so other arguments differing, as in
            // `_get_signature(kw)`, may hand it what the caller sent.
            (Expr::Attribute(f), Expr::Attribute(g)) => {
                f.attr.as_str() == g.attr.as_str()
                    && f.attr.as_str() != "get"
                    && ComparableArguments::from(&x.arguments) == ComparableArguments::from(&y.arguments)
            }
            _ => false,
        },
        _ => false,
    }
}

/// `vals[key]` or `vals.get(key)`.
fn vals_key<'a>(expr: &'a Expr, vals: &str) -> Option<&'a str> {
    let is_vals = |e: &Expr| matches!(e, Expr::Name(n) if n.id.as_str() == vals);
    match expr {
        Expr::Subscript(s) if is_vals(&s.value) => str_lit(&s.slice),
        Expr::Call(c) => match &*c.func {
            Expr::Attribute(a) if a.attr.as_str() == "get" && is_vals(&a.value) => {
                c.arguments.args.first().and_then(str_lit)
            }
            _ => None,
        },
        _ => None,
    }
}

/// The last word of a snake-case name, as `token` of `access_token_`.
pub(crate) fn last_word(name: &str) -> Option<&str> {
    name.rsplit('_').find(|w| !w.is_empty())
}

/// `self`, through methods that keep the model, such as `self.sudo()`.
fn self_model(expr: &Expr) -> bool {
    match expr {
        Expr::Name(n) => n.id.as_str() == "self",
        Expr::Call(c) => {
            matches!(&*c.func, Expr::Attribute(a) if SAME_MODEL.contains(&a.attr.as_str()) && self_model(&a.value))
        }
        _ => false,
    }
}

/// The keyword of `with_context()` that is `key`, or the key of the literal dict it is given.
fn context_key(call: &ast::ExprCall, key: &str) -> Option<TextRange> {
    let keyed = keyword_arg(call, key).map(Ranged::range);
    let first = call.arguments.args.first();
    keyed.or_else(|| method_body::dict_key(first?, key).map(Ranged::range))
}

/// Whether the call passes a literal `False` as the argument `row.if_false` names.
fn passes_false(row: &api_table::Row, call: &ast::ExprCall) -> bool {
    row.if_false.as_ref().is_some_and(|arg| {
        keyword(call, arg.keyword)
            .or_else(|| call.arguments.args.get(arg.position))
            .is_some_and(|value| matches!(value, Expr::BooleanLiteral(b) if !b.value))
    })
}

/// The packages an import statement imports, `odoo.osv` for `from odoo import osv` too.
fn imported_modules(stmt: &Stmt) -> Vec<String> {
    match stmt {
        Stmt::Import(i) => i.names.iter().map(|a| a.name.to_string()).collect(),
        Stmt::ImportFrom(ast::StmtImportFrom {
            module: Some(module),
            names,
            level: 0,
            ..
        }) => std::iter::once(module.to_string())
            .chain(names.iter().map(|a| format!("{module}.{}", a.name)))
            .collect(),
        _ => vec![],
    }
}

fn under_package(module: &str, package: &str) -> bool {
    module
        .strip_prefix(package)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
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

/// The type of the field a call declares. Only the last name counts, since a module may reach the
/// ORM's `fields` under any alias.
fn field_call<'e>(expr: &'e Expr, known: &'e FieldClasses) -> Option<(&'e str, &'e ast::ExprCall)> {
    let Expr::Call(call) = expr else { return None };
    let name = tail_name(&call.func)?;
    let ftype = if FIELD_TYPES.contains(&name) {
        name
    } else {
        known.get(name)?.as_str()
    };
    Some((ftype, call))
}

/// A base class without its type arguments: from 18.0 `Field` is generic, as in `Field[list]`.
fn generic_origin(base: &Expr) -> &Expr {
    match base {
        Expr::Subscript(s) => &s.value,
        _ => base,
    }
}

/// The field type a base class proves through the names the file imports from the ORM's packages:
/// `Field` makes the subclass a type of its own. A bare `Char` proves nothing unless so imported,
/// as other libraries name classes after field types.
fn proven_type(base: &Expr, class: &str, imported: &[(&str, String)]) -> Option<String> {
    let path = module_state::imported_path(base, imported)?;
    match path.split('.').collect::<Vec<_>>().as_slice() {
        [_, "fields", "Field"] => Some(class.to_owned()),
        [_, "fields", ftype] if FIELD_TYPES.contains(ftype) => Some((*ftype).to_owned()),
        _ => None,
    }
}

/// The field classes of `facts`, those that reach the ORM through other classes of the scan among
/// them, which may live in another file or module. A call names its class by name alone, so a name
/// with two field types tells neither: it gets `UNREAD_TYPE`. A type only ever moves towards that,
/// so the result does not depend on the order files are read in.
pub fn field_classes<'f>(facts: impl Iterator<Item = &'f PyFacts>) -> FieldClasses {
    let classes: Vec<&FieldClass> = facts.flat_map(|f| &f.field_classes).collect();
    let mut known = FieldClasses::default();
    let mut grew = true;
    while grew {
        grew = false;
        for class in &classes {
            let inherited = class.bases.iter().filter_map(|base| known.get(base));
            let types: Vec<String> = class.ftype.iter().chain(inherited).cloned().collect();
            for ftype in types {
                grew |= add_type(&mut known, &class.name, ftype);
            }
        }
    }
    known
}

/// Gives `name` the type `ftype`, and tells whether that changed what `known` holds.
fn add_type(known: &mut FieldClasses, name: &str, ftype: String) -> bool {
    let merged = match known.get(name) {
        None => ftype,
        Some(held) if *held == ftype || held == UNREAD_TYPE => return false,
        Some(_) => UNREAD_TYPE.to_owned(),
    };
    known.insert(name.to_owned(), merged);
    true
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
        UNREAD_TYPE => &[],
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

/// The labels of the lists a Selection gives, by position or as `selection` and `selection_add`.
fn selection_labels(ftype: &str, call: &ast::ExprCall) -> Vec<Option<String>> {
    if ftype != "Selection" {
        return vec![];
    }
    let items = |list: &Expr| match list {
        Expr::List(l) => l.elts.clone(),
        Expr::Tuple(t) => t.elts.clone(),
        _ => vec![],
    };
    let label_of = |item: Expr| match item {
        Expr::Tuple(t) => t.elts.get(1).map(label),
        Expr::List(l) => l.elts.get(1).map(label),
        _ => None,
    };
    let lists = [
        call.arguments.args.first(),
        keyword(call, "selection"),
        keyword(call, "selection_add"),
    ];
    lists
        .into_iter()
        .flatten()
        .flat_map(items)
        .filter_map(label_of)
        .collect()
}

/// A literal label, translated or not.
fn label(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Call(c) if matches!(tail_name(&c.func), Some("_" | "_lt")) => {
            str_lit(c.arguments.args.first()?).map(str::to_owned)
        }
        literal => str_lit(literal).map(str::to_owned),
    }
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
            (None, Expr::Call(c)) => is_super(&c.func),
            (Some(attr), Expr::Attribute(a)) => a.attr.as_str() == attr,
            _ => false,
        };
        self.found |= hit;
        if !self.found {
            visitor::walk_expr(self, expr);
        }
    }
}

/// Reads a runtime test of a model, field or module, with what holds such a result. `names` follow
/// the walk of an outermost function, so a test reads them as they stand at its line. `attrs` hold
/// across a class, whatever else its methods assign them, as methods run in no set order.
#[derive(Default)]
struct GuardHolders<'a> {
    names: FxHashMap<&'a str, Held>,
    attrs: FxHashMap<&'a str, Held>,
}

/// What a name or an attribute holds that a runtime test can read.
#[derive(Clone, Copy, PartialEq)]
enum Held {
    /// A result that shows the thing there when it has this truth value.
    Proof(bool),
    /// `ir.module.module` or records of it, which only their state speaks for: a record exists for
    /// every module the database has listed, installed or not.
    Modules,
    /// What `_installed()` returns.
    Installed,
}

/// What a passed guard shows installed: the modules and models its test names, or `None` when it
/// names nothing the scan reads, so that it may prove the module of any id.
type Proven = Option<Vec<Subject>>;

/// What two guards that both passed prove.
fn join(a: Proven, b: Proven) -> Proven {
    let (mut a, b) = (a?, b?);
    a.extend(b);
    Some(a)
}

/// The model a registry is tested for under `key`: a field's `_fields` and a model's own fields
/// name none.
fn registry_subject(registry: &Expr, key: &Expr) -> Proven {
    let model = str_lit(key).filter(|_| is_registry(registry) && tail_name(registry) != Some("_fields"))?;
    Some(vec![Subject::Model(model.to_owned())])
}

/// The model `env.get('x')` asks for. `hasattr` names none.
fn presence_subject(call: &ast::ExprCall) -> Proven {
    let Expr::Attribute(a) = &*call.func else { return None };
    registry_subject(&a.value, call.arguments.args.first()?)
}

/// The modules a search of `ir.module.module` asks for by name.
fn searched_modules(call: &ast::ExprCall) -> Proven {
    let domain = keyword(call, "domain").or_else(|| call.arguments.args.first())?;
    let names = domain::values(domain, "name")?;
    Some(names.into_iter().map(|m| Subject::Module(m.to_owned())).collect())
}

/// The modules a chain on `ir.module.module` such as `.sudo()._get('m')` picks.
fn named_modules(expr: &Expr) -> Proven {
    match expr {
        Expr::Attribute(a) => named_modules(&a.value),
        Expr::Call(c) => {
            let Expr::Attribute(a) = &*c.func else { return None };
            match a.attr.as_str() {
                "_get" => Some(vec![Subject::Module(str_lit(c.arguments.args.first()?)?.to_owned())]),
                "search" | "search_count" => searched_modules(c),
                _ => named_modules(&a.value),
            }
        }
        _ => None,
    }
}

impl<'a> GuardHolders<'a> {
    fn attrs_of(class: &'a ast::StmtClassDef) -> FxHashMap<&'a str, Held> {
        let mut holders = Self::default();
        for stmt in &class.body {
            holders.names.clear();
            holders.visit_stmt(stmt);
        }
        holders.attrs
    }

    /// What `test` having the value `truth` shows there, if it shows the thing there at all.
    fn cover(&self, test: &Expr, truth: bool) -> Option<Proven> {
        match test {
            Expr::UnaryOp(u) if u.op == UnaryOp::Not => self.cover(&u.operand, !truth),
            Expr::Named(n) => self.cover(&n.value, truth),
            Expr::BoolOp(b) => self.chain_cover(b, truth),
            Expr::Compare(c) => self
                .compare_sense(c)
                .and_then(|(sense, proven)| (sense == truth).then_some(proven)),
            Expr::Call(c) => self.call_cover(c).filter(|_| truth),
            _ => (self.held(test) == Some(Held::Proof(truth))).then_some(None),
        }
    }

    /// An `and` that holds and an `or` that fails fix every operand, so one proof is enough, and
    /// each operand that proves adds what it names.
    fn chain_cover(&self, chain: &ast::ExprBoolOp, truth: bool) -> Option<Proven> {
        let covers = chain.values.iter().map(|v| self.cover(v, truth));
        if (chain.op == BoolOp::And) == truth {
            covers.flatten().reduce(join)
        } else {
            covers.collect::<Option<Vec<_>>>()?.into_iter().reduce(join)
        }
    }

    fn call_cover(&self, call: &ast::ExprCall) -> Option<Proven> {
        if is_presence_call(call) {
            Some(presence_subject(call))
        } else {
            self.searches_installed(call).then(|| searched_modules(call))
        }
    }

    /// The truth value of a value that proves the thing there, with what it proves, if it proves
    /// it at all.
    fn sense(&self, value: &Expr) -> Option<(bool, Proven)> {
        [true, false]
            .into_iter()
            .find_map(|truth| self.cover(value, truth).map(|proven| (truth, proven)))
    }

    fn compare_sense(&self, c: &ast::ExprCompare) -> Option<(bool, Proven)> {
        let ([op], [left, right]) = (&*c.ops, &*c.operands) else {
            return None;
        };
        let (sense, proven) = match op {
            CmpOp::In | CmpOp::NotIn => self.member_sense(left, right),
            CmpOp::Eq | CmpOp::NotEq => self.state_sense(left, &[str_lit(right)?]),
            CmpOp::Is | CmpOp::IsNot if right.is_none_literal_expr() => self.sense(left),
            _ => None,
        }?;
        Some((sense == matches!(op, CmpOp::In | CmpOp::Eq | CmpOp::IsNot), proven))
    }

    /// The truth value of `left in right` that shows the thing there.
    fn member_sense(&self, left: &Expr, right: &Expr) -> Option<(bool, Proven)> {
        if let Some(name) = str_lit(left) {
            if self.reads_installed(right) {
                return Some((true, Some(vec![Subject::Module(name.to_owned())])));
            }
            if is_name_set(right) {
                return Some((true, registry_subject(right, left)));
            }
        }
        self.state_sense(left, &str_items(right)?)
    }

    /// The truth value of `left in states` that shows a module there, for `left` the state of one.
    /// A set that leaves `uninstalled` out says nothing when it fails, as the module may be on its
    /// way in, but one that holds it leaves only states of a module that is there, or nearly.
    fn state_sense(&self, left: &Expr, states: &[&str]) -> Option<(bool, Proven)> {
        let Expr::Attribute(a) = left else { return None };
        if a.attr.as_str() != "state" || !self.reads_modules(&a.value) {
            return None;
        }
        let proven = named_modules(&a.value);
        if states.contains(&"uninstalled") {
            return Some((false, proven));
        }
        let loaded = |state: &&str| matches!(*state, "installed" | "to upgrade" | "to remove");
        (states.contains(&"installed") && states.iter().all(loaded)).then_some((true, proven))
    }

    fn held(&self, expr: &Expr) -> Option<Held> {
        match expr {
            Expr::Name(n) => self.names.get(n.id.as_str()).copied(),
            Expr::Attribute(a) if is_self(&a.value) => self.attrs.get(a.attr.as_str()).copied(),
            _ => None,
        }
    }

    /// What a value gives the name or the attribute assigned it.
    fn holding(&self, value: &Expr) -> Option<Held> {
        if self.reads_installed(value) {
            return Some(Held::Installed);
        }
        let proof = self.sense(value).map(|(truth, _)| Held::Proof(truth));
        proof.or_else(|| self.reads_modules(value).then_some(Held::Modules))
    }

    /// Whether `expr` reads `ir.module.module` or a holder of it, in a chain such as
    /// `.sudo()._get('m')`.
    fn reads_modules(&self, expr: &Expr) -> bool {
        self.held(expr) == Some(Held::Modules)
            || match expr {
                Expr::Attribute(a) => self.reads_modules(&a.value),
                Expr::Call(c) => self.reads_modules(&c.func),
                _ => env_literal(expr) == Some("ir.module.module"),
            }
    }

    fn reads_installed(&self, expr: &Expr) -> bool {
        self.held(expr) == Some(Held::Installed)
            || matches!(expr, Expr::Call(c) if matches!(
                &*c.func,
                Expr::Attribute(a) if a.attr.as_str() == "_installed" && self.reads_modules(&a.value)
            ))
    }

    /// Whether `call` searches `ir.module.module` under a domain that asks for installed modules.
    fn searches_installed(&self, call: &ast::ExprCall) -> bool {
        let Expr::Attribute(a) = &*call.func else { return false };
        matches!(a.attr.as_str(), "search" | "search_count")
            && self.reads_modules(&a.value)
            && keyword(call, "domain")
                .or_else(|| call.arguments.args.first())
                .is_some_and(|domain| domain::requires(domain, "state", "installed"))
    }

    fn unbind(&mut self, target: &Expr) {
        match target {
            Expr::Name(n) => drop(self.names.remove(n.id.as_str())),
            Expr::Tuple(t) => t.elts.iter().for_each(|e| self.unbind(e)),
            _ => {}
        }
    }

    /// Follows the names through `stmt`, once its value has been read.
    fn rebind(&mut self, stmt: &'a Stmt) {
        if let Stmt::AugAssign(a) = stmt {
            return self.unbind(&a.target);
        }
        let Some((targets, value)) = assigned(stmt) else {
            return;
        };
        let held = self.holding(value);
        for target in targets {
            match (target, held) {
                (Expr::Name(n), Some(held)) => drop(self.names.insert(n.id.as_str(), held)),
                _ => self.unbind(target),
            }
        }
    }

    fn hold_attrs(&mut self, stmt: &'a Stmt) {
        let Some((targets, value)) = assigned(stmt) else {
            return;
        };
        let Some(held) = self.holding(value) else {
            return;
        };
        for target in targets {
            if let Expr::Attribute(a) = target
                && is_self(&a.value)
            {
                self.attrs.insert(a.attr.as_str(), held);
            }
        }
    }
}

impl<'a> Visitor<'a> for GuardHolders<'a> {
    /// A nested class holds its own attributes.
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if matches!(stmt, Stmt::ClassDef(_)) {
            return;
        }
        self.hold_attrs(stmt);
        if let Stmt::For(f) = stmt {
            self.unbind(&f.target);
        }
        visitor::walk_stmt(self, stmt);
        self.rebind(stmt);
    }
}

/// A block that leaves the function or the loop when its guard fails.
pub(crate) fn exits(body: &[Stmt]) -> bool {
    matches!(body.last(), Some(Stmt::Return(_) | Stmt::Raise(_) | Stmt::Continue(_)))
}

/// A block that ends with `skipTest`, which raises, so the code after it needs what it tested.
fn skips(body: &[Stmt]) -> bool {
    let Some(Stmt::Expr(last)) = body.last() else {
        return false;
    };
    matches!(&*last.value, Expr::Call(c) if tail_name(&c.func) == Some("skipTest"))
}

/// The module of `self.ensure_installed('m')`, a test helper that raises `SkipTest` unless the
/// module is installed. It returns nothing to test, so the call itself is the guard. As with
/// `env.get`, a variable says nothing of a literal use further on.
fn ensures_installed(expr: &Expr) -> Option<&str> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Expr::Attribute(a) = &*call.func else {
        return None;
    };
    match &*call.arguments.args {
        [module] if a.attr.as_str() == "ensure_installed" && is_self(&a.value) => str_lit(module),
        _ => None,
    }
}

fn is_registry(expr: &Expr) -> bool {
    matches!(tail_name(expr), Some("env" | "_fields" | "pool" | "registry"))
}

/// A value that `in` tests for the name of a model or a field.
fn is_name_set(expr: &Expr) -> bool {
    is_registry(expr) || env_literal(expr).is_some()
}

/// The model `env['x']` names.
fn env_literal(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Subscript(sub) if tail_name(&sub.value) == Some("env") => str_lit(&sub.slice),
        _ => None,
    }
}

/// The strings a literal collection holds, unless it holds anything else.
fn str_items(expr: &Expr) -> Option<Vec<&str>> {
    let items = match expr {
        Expr::Set(s) => &*s.elts,
        _ => domain::items(expr)?,
    };
    items.iter().map(str_lit).collect()
}

/// `env.get('x')`, or `hasattr` with what can name a field: a variable key says nothing of a literal
/// use further on.
fn is_presence_call(call: &ast::ExprCall) -> bool {
    match &*call.func {
        Expr::Attribute(a) if a.attr.as_str() == "get" => {
            is_registry(&a.value) && call.arguments.args.first().is_some_and(|key| str_lit(key).is_some())
        }
        Expr::Name(n) if n.id.as_str() == "hasattr" => hasattr_guard(call),
        _ => false,
    }
}

/// `hasattr(x, 'name')`, but for `x` a `super()`, which tests what a method can extend, for `x` the
/// `request`, which holds no model, and for a private name, which is more often a method or an
/// internal attribute than a field.
fn hasattr_guard(call: &ast::ExprCall) -> bool {
    let [target, name] = &*call.arguments.args else {
        return false;
    };
    let on_super = matches!(target, Expr::Call(c) if is_super(&c.func));
    !on_super && tail_name(target) != Some("request") && str_lit(name).is_some_and(|n| !n.starts_with('_'))
}

/// Finds each `env.ref` and `env[...]`, each of which raises when what it names is missing, with
/// the module of the id or the model, none when the scan cannot read it. When `settled`, it keeps
/// to the lookups that run whenever the code around them does.
#[derive(Default)]
struct LookupFinder {
    settled: bool,
    records: Vec<Option<Subject>>,
    models: Vec<Option<Subject>>,
}

impl<'a> Visitor<'a> for LookupFinder {
    /// A `with` is passed over too, as its context manager may swallow the error.
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        let branches = matches!(
            stmt,
            Stmt::If(_)
                | Stmt::For(_)
                | Stmt::While(_)
                | Stmt::Try(_)
                | Stmt::With(_)
                | Stmt::Match(_)
                | Stmt::FunctionDef(_)
                | Stmt::ClassDef(_)
        );
        if !(self.settled && branches) {
            visitor::walk_stmt(self, stmt);
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        self.lookup(expr);
        match expr {
            _ if !self.settled => visitor::walk_expr(self, expr),
            Expr::BoolOp(b) => self.visit_expr(&b.values[0]),
            Expr::If(e) => self.visit_expr(&e.test),
            Expr::Lambda(_) | Expr::ListComp(_) | Expr::SetComp(_) | Expr::DictComp(_) | Expr::Generator(_) => {}
            _ => visitor::walk_expr(self, expr),
        }
    }
}

impl LookupFinder {
    fn lookup(&mut self, expr: &Expr) {
        match expr {
            Expr::Call(c) if is_env_ref(c) && raises_if_not_found(c) => {
                let id = c.arguments.args.first().and_then(str_lit);
                let module = id.and_then(|id| id.split_once('.')).map(|(m, _)| m);
                self.records.push(module.map(|m| Subject::Module(m.to_owned())));
            }
            Expr::Subscript(sub) if tail_name(&sub.value) == Some("env") => {
                self.models
                    .push(env_literal(expr).map(|m| Subject::Model(m.to_owned())));
            }
            _ => {}
        }
    }
}

fn is_env_ref(call: &ast::ExprCall) -> bool {
    matches!(&*call.func, Expr::Attribute(a) if a.attr.as_str() == "ref" && tail_name(&a.value) == Some("env"))
}

/// `env.ref()` raises on an unknown id unless told not to, and then the caller handles it.
fn raises_if_not_found(call: &ast::ExprCall) -> bool {
    let raise = keyword(call, "raise_if_not_found").or_else(|| call.arguments.args.get(1));
    raise.is_none_or(|e| matches!(e, Expr::BooleanLiteral(b) if b.value))
}

/// Whether a handler catches `error`: by name, as a bare `except`, or through a base of every error.
fn catches(handler: &ast::ExceptHandlerExceptHandler, error: &str) -> bool {
    let names =
        |name: &Expr| matches!(tail_name(name), Some(n) if n == error || n == "Exception" || n == "BaseException");
    match handler.type_.as_deref() {
        None => true,
        Some(Expr::Tuple(types)) => types.iter().any(names),
        Some(name) => names(name),
    }
}

/// What a `try` proves when it looks something up, catches what the lookup raises and skips from
/// every handler, so a run that gets past it found what it looked up: `env.ref` raises
/// `ValueError` and `env[...]` raises `KeyError`. Its body runs behind every lookup it catches, as
/// each one skips when it fails, but what runs after it has passed only those that always run.
fn try_guards(t: &ast::StmtTry) -> Option<(Proven, Proven)> {
    let within = caught_lookups(t, false);
    let skipping = t
        .handlers
        .iter()
        .all(|ast::ExceptHandler::ExceptHandler(h)| skips(&h.body));
    (!within.is_empty() && skipping).then(|| {
        let after = caught_lookups(t, true);
        (within.into_iter().collect(), after.into_iter().collect())
    })
}

/// The lookups in the body of `t` that raise what one of its handlers catches.
fn caught_lookups(t: &ast::StmtTry, settled: bool) -> Vec<Option<Subject>> {
    let mut lookup = LookupFinder {
        settled,
        ..LookupFinder::default()
    };
    lookup.visit_body(&t.body);
    let caught = |error| {
        t.handlers
            .iter()
            .any(|ast::ExceptHandler::ExceptHandler(h)| catches(h, error))
    };
    let mut found = vec![];
    if caught("ValueError") {
        found.extend(lookup.records);
    }
    if caught("KeyError") {
        found.extend(lookup.models);
    }
    found
}

/// The methods called as `super().name(...)` or `super(Class, self).name(...)`, each with whether
/// a `hasattr` test on the same `super()` and name encloses the call.
#[derive(Default)]
struct SuperCalls<'a> {
    calls: Vec<(&'a str, TextSize, bool)>,
    guards: Vec<(&'a Expr, &'a str)>,
}

impl<'a> SuperCalls<'a> {
    fn under(&mut self, test: &'a Expr, visit: impl FnOnce(&mut Self)) {
        let depth = self.guards.len();
        hasattr_tests(test, &mut self.guards);
        visit(self);
        self.guards.truncate(depth);
    }
}

/// The `hasattr(super(...), 'name')` tests that must hold for `test` to hold.
fn hasattr_tests<'a>(test: &'a Expr, tests: &mut Vec<(&'a Expr, &'a str)>) {
    match test {
        Expr::BoolOp(all) if all.op == BoolOp::And => {
            for value in &all.values {
                hasattr_tests(value, tests);
            }
        }
        Expr::Call(call) if matches!(&*call.func, Expr::Name(n) if n.id.as_str() == "hasattr") => {
            if let [target @ Expr::Call(inner), name] = &*call.arguments.args
                && is_super(&inner.func)
                && let Some(name) = str_lit(name)
            {
                tests.push((target, name));
            }
        }
        _ => {}
    }
}

fn is_super(func: &Expr) -> bool {
    matches!(func, Expr::Name(n) if n.id.as_str() == "super")
}

impl<'a> Visitor<'a> for SuperCalls<'a> {
    /// A class body's `super()` calls are not the method's. Its decorators and bases run in the
    /// method.
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::ClassDef(class) => {
                for decorator in &class.decorator_list {
                    self.visit_decorator(decorator);
                }
                if let Some(arguments) = &class.arguments {
                    self.visit_arguments(arguments);
                }
            }
            Stmt::If(branch) => {
                self.visit_expr(&branch.test);
                self.under(&branch.test, |calls| calls.visit_body(&branch.body));
                for clause in &branch.elif_else_clauses {
                    match &clause.test {
                        Some(test) => {
                            self.visit_expr(test);
                            self.under(test, |calls| calls.visit_body(&clause.body));
                        }
                        None => self.visit_body(&clause.body),
                    }
                }
            }
            _ => visitor::walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::If(branch) = expr {
            self.visit_expr(&branch.test);
            self.under(&branch.test, |calls| calls.visit_expr(&branch.body));
            self.visit_expr(&branch.orelse);
            return;
        }
        if let Expr::Call(call) = expr
            && let Expr::Attribute(a) = &*call.func
            && let Expr::Call(inner) = &*a.value
            && is_super(&inner.func)
        {
            let name = a.attr.as_str();
            let guarded = self
                .guards
                .iter()
                .any(|(target, test)| *test == name && method_body::same(target, &a.value));
            self.calls.push((name, a.attr.start(), guarded));
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
/// caches some fields of its records. The `osv` names alias the first three up to 17.0: a later
/// class that names one does not import, so they need no gate.
fn orm_kind(name: &str) -> Option<ModelKind> {
    match name {
        "Model" | "CachedModel" | "osv" => Some(ModelKind::Model),
        "TransientModel" | "osv_memory" => Some(ModelKind::Transient),
        "AbstractModel" | "osv_abstract" => Some(ModelKind::Abstract),
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

pub(crate) fn declares_model(class: &ast::StmtClassDef) -> bool {
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

/// Whether a class attribute may hold a method: anything but a field, a table object or plain data.
/// Counting too much costs little, since a typo matches no name the class binds.
fn may_be_method(value: &Expr, known: &FieldClasses) -> bool {
    let data = value.is_literal_expr()
        || matches!(
            value,
            Expr::FString(_)
                | Expr::TString(_)
                | Expr::List(_)
                | Expr::Tuple(_)
                | Expr::Dict(_)
                | Expr::Set(_)
                | Expr::ListComp(_)
                | Expr::SetComp(_)
                | Expr::DictComp(_)
                | Expr::Generator(_)
        );
    !data && field_call(value, known).is_none() && !is_table_object(value)
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

/// Whether a field call may set attributes it does not name.
fn hides_options(ftype: &str, call: &ast::ExprCall) -> bool {
    let args = &call.arguments.args;
    args.iter().any(Expr::is_starred_expr)
        || call.arguments.keywords.iter().any(|k| k.arg.is_none())
        || (ftype == UNREAD_TYPE && !args.is_empty())
}

fn is_delegate(value: &Expr, known: &FieldClasses) -> bool {
    field_call(value, known)
        .and_then(|(_, call)| keyword(call, "delegate"))
        .is_some_and(|d| matches!(d, Expr::BooleanLiteral(b) if b.value))
}

/// Whether the Many2one may delegate in a way `ClassFact::inherits` cannot hold: Odoo reads the truth
/// of `delegate`, which a non-literal value or a `**` unpacking may give, and the comodel of a field
/// of unread type is unknown.
fn may_delegate(value: &Expr, known: &FieldClasses) -> bool {
    let Some(("Many2one" | UNREAD_TYPE, call)) = field_call(value, known) else {
        return false;
    };
    let falsy = |d: &Expr| matches!(option_value(d), OptionValue::Bool(false) | OptionValue::Falsy);
    call.arguments.keywords.iter().any(|k| k.arg.is_none()) || keyword(call, "delegate").is_some_and(|d| !falsy(d))
}

fn is_api(expr: &Expr, name: &str) -> bool {
    matches!(expr, Expr::Attribute(a) if a.attr.as_str() == name && tail_name(&a.value) == Some("api"))
}

/// Whether a decorator, dotted, hands the method's calls on as they come. Odoo's own keep the
/// arity on 18.0: `api.model` (`odoo/api.py:421`), `api.returns` (`:393`) and `ormcache`
/// (`odoo/tools/cache.py:64`).
fn keeps_arity(decorator: &str) -> bool {
    let (head, last) = decorator.rsplit_once('.').unwrap_or(("", decorator));
    head.rsplit('.').next() == Some("api")
        || matches!(last, "ormcache" | "ormcache_context" | "override")
        || matches!(decorator, "staticmethod" | "classmethod")
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

/// A class of a file that does not parse, as far as its tokens name its model.
struct UnreadClass {
    fact: ClassFact,
    naming: Naming,
    /// How many blocks enclose the `class` statement.
    depth: usize,
    /// What `built_name_start` reads of an `_inherit` the class hides, which leaves a class without
    /// `_name` as unnamed as a built `_name` does.
    inherit_start: Option<String>,
    may_reach_orm: bool,
}

/// A file that does not parse, walked over its tokens. Each assignment to a `NAMING_ATTRS` name
/// parses alone, so it reads as the class body of a file that parses does. It parses in place in the file,
/// as `PyVisitor` hands every offset to a `Locator` of the file, which panics inside a character.
struct UnreadFile<'s, 'a, 'ast> {
    src: &'s str,
    visitor: PyVisitor<'a, 'ast>,
    depth: usize,
    /// The classes whose block is open, by their place in `classes`.
    open: Vec<usize>,
    classes: Vec<UnreadClass>,
}

impl UnreadFile<'_, '_, '_> {
    fn read(&mut self) {
        let parsed = parse_unchecked(self.src, Mode::Module.into());
        let tokens = parsed.tokens();
        let mut starts = true;
        for (i, token) in tokens.iter().enumerate() {
            match token.kind() {
                TokenKind::NonLogicalNewline | TokenKind::Comment => continue,
                // An indent that mixes tabs and spaces, which Python 2 accepts, lexes as this token.
                TokenKind::Unknown if starts => continue,
                TokenKind::Indent => self.depth += 1,
                TokenKind::Dedent => self.dedent(),
                _ if starts => self.statement(&tokens[i..]),
                _ => {}
            }
            starts = opens_statement(token.kind());
        }
    }

    fn dedent(&mut self) {
        self.depth = self.depth.saturating_sub(1);
        let classes = &self.classes;
        let depth = self.depth;
        self.open.retain(|&at| classes[at].depth < depth);
    }

    fn class(&mut self, tokens: &[Token]) {
        let [_, name, ..] = tokens else {
            return;
        };
        self.open.push(self.classes.len());
        self.classes.push(UnreadClass {
            fact: ClassFact::new(self.visitor.loc.pos(name.start()), self.src[name.range()].to_owned()),
            naming: Naming::default(),
            depth: self.depth,
            inherit_start: None,
            may_reach_orm: header_reaches_orm(self.src, tokens),
        });
    }

    fn statement(&mut self, tokens: &[Token]) {
        if tokens[0].kind() == TokenKind::Class {
            self.class(tokens);
        } else {
            self.attribute(tokens);
        }
    }

    /// Reads an assignment to a `NAMING_ATTRS` name of the class whose body the statement sits in.
    fn attribute(&mut self, tokens: &[Token]) {
        let body = self.open.last().copied();
        let Some(at) = body.filter(|&at| self.classes[at].depth + 1 == self.depth) else {
            return;
        };
        let pairs = tokens.as_chunks::<2>().0.iter().take_while(|pair| is_target(pair));
        let names = |pair: &&[Token; 2]| NAMING_ATTRS.contains(&&self.src[pair[0].range()]);
        let targets: Vec<_> = pairs.clone().filter(names).map(|pair| pair[0].range()).collect();
        let (Some(value), false) = (tokens.get(pairs.count() * 2), targets.is_empty()) else {
            return;
        };
        let ends = |t: &&Token| matches!(t.kind(), TokenKind::Newline | TokenKind::Semi | TokenKind::EndOfFile);
        let Some(end) = tokens.iter().find(ends) else {
            return;
        };
        let parsed = parse_expression_range(self.src, TextRange::new(value.start(), end.start()));
        let unparsed = hidden_value(value.range());
        let value = parsed.as_ref().map_or(&unparsed, |parsed| parsed.expr());
        for target in targets {
            if let Ok(target) = parse_expression_range(self.src, target) {
                self.assignment(at, target.expr(), value);
            }
        }
    }

    fn assignment(&mut self, at: usize, target: &Expr, value: &Expr) {
        let class = &mut self.classes[at];
        let visitor = &mut self.visitor;
        visitor.class_attrs(std::slice::from_ref(target), value, &mut class.fact, &mut class.naming);
        if target.as_name_expr().is_some_and(|n| n.id.as_str() == "_inherit") && class.fact.inherit_unknown {
            class.inherit_start = built_name_start(value);
        }
    }

    /// The facts of the file: only what a class names survives, so nothing that reads a class's body
    /// takes the class for whole.
    fn into_facts(self) -> PyFacts {
        let mut visitor = self.visitor;
        for mut class in self.classes {
            // `_name` settles the model whatever `_inherit` hides, in either order in the class.
            if class.fact.inherit_unknown && !class.naming.named {
                visitor.facts.dynamic_models = true;
                class.naming.built_start = class.inherit_start;
            }
            if let Some(fact) = visitor.named_model(class.fact, class.naming, class.may_reach_orm) {
                visitor.facts.classes.push(ClassFact {
                    inherit: vec![],
                    inherit_unknown: true,
                    unread: true,
                    attrs: vec![],
                    ..fact
                });
            }
        }
        visitor.facts
    }
}

/// The class attributes a file that does not parse is read for: the two that name a model, the one
/// that keeps a class out of the registry, and the one that spells the ids of its fields before
/// saas~11.2.
const NAMING_ATTRS: [&str; 4] = ["_name", "_inherit", "_register", "_table"];

/// A name in place of a value that does not parse, as both hide what they hold.
fn hidden_value(range: TextRange) -> Expr {
    Expr::Name(ast::ExprName {
        node_index: ast::AtomicNodeIndex::NONE,
        range,
        id: ast::name::Name::new_static("unparsed"),
        ctx: ast::ExprContext::Load,
    })
}

/// Whether a pair of tokens opens an assignment: a name and `=`.
fn is_target([name, equal]: &[Token; 2]) -> bool {
    name.kind() == TokenKind::Identifier && equal.kind() == TokenKind::Equal
}

/// Whether the token after one of `kind` starts a statement.
pub(crate) fn opens_statement(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Indent | TokenKind::Dedent | TokenKind::Newline | TokenKind::Semi
    )
}

/// Whether a `class` statement of a file that does not parse, from its first token, may reach the
/// ORM. A header that does not parse either may.
fn header_reaches_orm(src: &str, tokens: &[Token]) -> bool {
    let colon = tokens.iter().find(|t| t.kind() == TokenKind::Colon);
    let header = colon.map(|colon| format!("{}: pass", &src[TextRange::new(tokens[0].start(), colon.start())]));
    let parsed = header.and_then(|header| parse_module(&header).ok());
    match parsed.as_ref().and_then(|parsed| parsed.suite().first()) {
        Some(Stmt::ClassDef(class)) => may_reach_orm(class),
        _ => true,
    }
}

/// What the tokens of a file that does not parse say of its models, so that rules stay silent on
/// those and only those. A name they hide may be any, as for a parsed class that builds it.
fn unread_facts(src: &str, visitor: PyVisitor<'_, '_>) -> PyFacts {
    let mut file = UnreadFile {
        src,
        visitor,
        depth: 0,
        open: vec![],
        classes: vec![],
    };
    file.read();
    file.into_facts()
}

/// The facts of a Python file of a module of `series`, which names a class without `_name`.
#[cfg(test)]
pub fn analyze(src: &str, series: Option<Series>) -> PyFacts {
    analyze_with(
        src,
        series,
        &FieldClasses::default(),
        &TaintTable::default(),
        Scope::default(),
    )
}

/// The facts of a Python file of a module of `series`, reading a call of a class of `known` as a
/// field, following data by the rows of `taint` as well as the built-in ones, and reading a route
/// with what it inherits from the controllers of `scope`.
pub fn analyze_with(
    src: &str,
    series: Option<Series>,
    known: &FieldClasses,
    taint: &TaintTable,
    scope: Scope,
) -> PyFacts {
    let loc = Locator::new(src);
    let module = match async_names::parse(src, series) {
        Ok(module) => module,
        Err(e) => {
            let mut visitor = PyVisitor::new(&loc, series, known, taint, scope, &[]);
            let error = if python2::runs_on(series) {
                python2::remaining_error(src, series)
            } else {
                Some((loc.pos(e.location.start()), e.error.to_string()))
            };
            match error {
                Some((pos, message)) => visitor.facts.diags.push(RawDiag::new("OD000", pos, message)),
                None => visitor.facts.python2 = Some(loc.pos(e.location.start())),
            }
            return unread_facts(src, visitor);
        }
    };
    let body = &module.body;
    let mut v = PyVisitor::new(&loc, series, known, taint, scope, body);
    v.facts.controllers = routing::controllers(body);
    v.facts.inherits_routes = routing::inherits_routes(&v.facts.controllers);
    v.visit_body(body);
    let (patches, diags) = module_state::check(body, &loc);
    v.facts.import_patches = patches;
    v.facts.diags.extend(diags);
    v.facts
}

/// The targets and the value of an assignment that gives one.
fn assigned(stmt: &Stmt) -> Option<(&[Expr], &Expr)> {
    match stmt {
        Stmt::Assign(a) => Some((&a.targets, &a.value)),
        Stmt::AnnAssign(ast::StmtAnnAssign {
            target,
            value: Some(value),
            ..
        }) => Some((std::slice::from_ref(&**target), value)),
        _ => None,
    }
}

/// Records that a class has a method, and whether `@api.private` marks it.
fn method_name(func: &ast::StmtFunctionDef, fact: &mut ClassFact) {
    fact.methods.push(func.name.to_string());
    if func.decorator_list.iter().any(|d| is_api(&d.expression, "private")) {
        fact.private_methods.push(func.name.to_string());
    }
}

/// The method names an `if`, `try` or `with` block of a class body binds. Such a branch often
/// serves another series, so its body stays out of the checks: `name_get` there must not report.
fn conditional_methods(stmt: &Stmt, fact: &mut ClassFact) {
    let unread = FieldClasses::default();
    for stmt in module_state::top_level(std::slice::from_ref(stmt)) {
        if let Stmt::FunctionDef(func) = stmt {
            method_name(func, fact);
        } else if let Some((targets, _)) = assigned(stmt).filter(|(_, value)| may_be_method(value, &unread)) {
            let names = targets.iter().filter_map(Expr::as_name_expr);
            fact.methods.extend(names.map(|name| name.id.to_string()));
        }
    }
}

/// The first series that names a class without `_name` after the class, dotted, whenever
/// `_inherit` is not one string (`saas~18.1`).
const DOTTED_CLASS_NAME: Series = Series { major: 18, minor: 1 };

/// The first series whose metaclass makes no fields of `_columns` (9.0 `openerp/models.py:255`).
const COLUMNS_END: Series = Series { major: 10, minor: 0 };

/// What a class body says about the name of its model.
#[derive(Default)]
struct Naming {
    /// The class assigns `_name`, a literal or not.
    named: bool,
    /// `_name` when it is a literal.
    name: Option<String>,
    /// The literal start of the names `_name` may build.
    built_start: Option<String>,
    /// Odoo may register the class, so a name it reads as another model may be built.
    registers: bool,
    inherit: InheritShape,
    /// `_columns` hides a key. Waits here for the name: `_inherit` assigns `inherit_unknown`, in any
    /// order in the class, and the name reads that flag for what `_inherit` lists.
    columns_hidden: bool,
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

/// The literal start of the names a `_name` may build, up to the first value it splices in. A
/// literal builds nothing, and a false `_name` names no model.
fn built_name_start(value: &Expr) -> Option<String> {
    if str_lit(value).is_some() || matches!(option_value(value), OptionValue::Bool(false) | OptionValue::Falsy) {
        return None;
    }
    let head = query_head(value).unwrap_or_default();
    Some(head.split(['%', '{']).next().unwrap_or_default().to_owned())
}

/// Whether a base of the class may reach `models.Model`, so that Odoo registers it. With no base or
/// only `object` it never can, while any other plain base may be a mixin that does.
fn may_reach_orm(class: &ast::StmtClassDef) -> bool {
    class
        .bases()
        .iter()
        .any(|base| !matches!(base, Expr::Name(n) if n.id.as_str() == "object"))
}

/// The names a statement of the module binds. It stops at a function or a class, whose body binds
/// names of its own.
#[derive(Default)]
pub(crate) struct BoundNames<'a>(pub(crate) Vec<&'a str>);

impl<'a> Visitor<'a> for BoundNames<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::ClassDef(class) => self.0.push(class.name.as_str()),
            Stmt::FunctionDef(func) => self.0.push(func.name.as_str()),
            Stmt::Import(import) => self.aliases(&import.names),
            Stmt::ImportFrom(import) => self.aliases(&import.names),
            _ => visitor::walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Name(name) = expr
            && name.ctx.is_store()
        {
            self.0.push(name.id.as_str());
        }
        visitor::walk_expr(self, expr);
    }
}

impl<'a> BoundNames<'a> {
    pub(crate) fn aliases(&mut self, aliases: &'a [ast::Alias]) {
        for alias in aliases {
            let name = alias.asname.as_ref().unwrap_or(&alias.name);
            self.0.push(name.as_str().split('.').next().unwrap_or_default());
        }
    }
}

/// A class of the file that can give a subclass no field.
struct Fieldless<'a> {
    end: TextSize,
    /// The names its body binds with a `def` or an assignment, but those starting with `__`: Python
    /// mangles a private one with this class's name, and a dunder is the object protocol's.
    binds: Vec<&'a str>,
}

/// Each class that can give a subclass no field, by name. Only a class at the top of the file
/// counts, and only until something rebinds its name: a fallback beside an import names a class
/// the file cannot tell.
fn fieldless_classes(body: &[Stmt]) -> FxHashMap<&str, Fieldless<'_>> {
    let mut classes = FxHashMap::default();
    for stmt in body {
        let mut bound = BoundNames::default();
        bound.visit_stmt(stmt);
        for name in bound.0 {
            classes.remove(name);
        }
        if let Stmt::ClassDef(class) = stmt
            && holds_no_field(class)
        {
            let binds = class_binds(class)
                .into_iter()
                .filter(|n| !n.starts_with("__"))
                .collect();
            classes.insert(
                class.name.as_str(),
                Fieldless {
                    end: class.end(),
                    binds,
                },
            );
        }
    }
    classes
}

/// The names the body of `class` binds with a `def` or an assignment.
fn class_binds(class: &ast::StmtClassDef) -> Vec<&str> {
    let mut bound = BoundNames::default();
    for stmt in &class.body {
        match stmt {
            Stmt::FunctionDef(func) => bound.0.push(func.name.as_str()),
            _ => assigned(stmt)
                .into_iter()
                .flat_map(|(targets, _)| targets)
                .for_each(|t| bound.visit_expr(t)),
        }
    }
    bound.0
}

/// Whether a class can give a subclass no field: no base but `object` to get one from, nothing that
/// runs on creating it, and a body of methods and literals. Any other value may be a field, of a
/// class the index does not know, and a `_name` or an `_inherit` passes on to the subclass.
fn holds_no_field(class: &ast::StmtClassDef) -> bool {
    let plain = !may_reach_orm(class) && class.decorator_list.is_empty() && !declares_model(class);
    plain && class.arguments.as_ref().is_none_or(|a| a.keywords.is_empty()) && class.body.iter().all(no_field_stmt)
}

fn no_field_stmt(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::FunctionDef(_) | Stmt::Pass(_) | Stmt::AnnAssign(ast::StmtAnnAssign { value: None, .. }) => true,
        Stmt::Expr(e) => matches!(*e.value, Expr::StringLiteral(_) | Expr::EllipsisLiteral(_)),
        _ => assigned(stmt).is_some_and(|(_, value)| literal(value)),
    }
}

fn literal(value: &Expr) -> bool {
    match value {
        Expr::Tuple(t) => t.elts.iter().all(literal),
        Expr::List(l) => l.elts.iter().all(literal),
        _ => value.is_literal_expr(),
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

/// The parameters a `model` or `models` converter fills with records in every URL (18.0
/// `odoo/addons/base/models/ir_http.py:58-88`): the user picks their ids, not their values. A URL
/// that does not name one leaves it to the query string (18.0 `odoo/http.py:2279`, 20.0
/// `odoo/http/dispatcher.py:244`), which may give any value.
fn record_params<'u>(urls: &[&'u str]) -> Vec<&'u str> {
    let filled = |url: &str, name: &str| url_converters(url).any(|(c, n)| n == name && matches!(c, "model" | "models"));
    let mut records = vec![];
    for (converter, name) in urls.iter().flat_map(|url| url_converters(url)) {
        if matches!(converter, "model" | "models")
            && !records.contains(&name)
            && urls.iter().all(|url| filled(url, name))
        {
            records.push(name);
        }
    }
    records
}

/// The parameters a `model` or `models` converter fills with records in some URL and no other
/// converter fills in any: what a route's body may write as records whichever URL served it.
fn converted_records<'u>(urls: &[&'u str]) -> Vec<&'u str> {
    let mut records = vec![];
    let mut others = vec![];
    for (converter, name) in urls.iter().flat_map(|url| url_converters(url)) {
        if matches!(converter, "model" | "models") {
            records.push(name);
        } else {
            others.push(name);
        }
    }
    records.retain(|name| !others.contains(name));
    records
}

/// The URL literals of a function's `@route` decorators, one or a list each.
fn route_urls(func: &ast::StmtFunctionDef) -> Vec<&str> {
    let mut urls = vec![];
    for decorator in &func.decorator_list {
        let Expr::Call(call) = &decorator.expression else {
            continue;
        };
        if tail_name(&call.func) != Some("route") {
            continue;
        }
        let Some(arg) = call.arguments.args.first().or_else(|| keyword(call, "route")) else {
            continue;
        };
        match arg {
            Expr::List(ast::ExprList { elts, .. }) | Expr::Tuple(ast::ExprTuple { elts, .. }) => {
                urls.extend(elts.iter().filter_map(str_lit));
            }
            e => urls.extend(str_lit(e)),
        }
    }
    urls
}

/// The converter and the name of each `<converter(arguments):name>` part of a URL, `<name>` taking
/// werkzeug's `default`.
fn url_converters(url: &str) -> impl Iterator<Item = (&str, &str)> {
    url.split('<').skip(1).filter_map(|part| {
        let (inner, _) = part.split_once('>')?;
        let (converter, name) = inner.rsplit_once(':').unwrap_or(("default", inner));
        let converter = converter.split('(').next().unwrap_or_default();
        Some((converter.trim(), name.trim()))
    })
}

/// The `csrf=False` of a route, which turns Odoo's token check off for every method.
fn csrf_off(call: &ast::ExprCall) -> Option<&ast::Keyword> {
    keyword_arg(call, "csrf").filter(|k| matches!(&k.value, Expr::BooleanLiteral(b) if !b.value))
}

/// Whether a route's literal `methods` lets a GET through: werkzeug upper-cases them, and with
/// none every method passes.
fn accepts_get(call: &ast::ExprCall) -> bool {
    let elts = match keyword(call, "methods") {
        None | Some(Expr::NoneLiteral(_)) => return true,
        Some(Expr::List(l)) => &l.elts,
        Some(Expr::Tuple(t)) => &t.elts,
        Some(_) => return false,
    };
    elts.iter().filter_map(str_lit).any(|m| m.eq_ignore_ascii_case("GET"))
}

/// The URLs of a route, those it inherits included.
fn merged_urls<'r>(func: &'r ast::StmtFunctionDef, routing: Option<&'r Routing>) -> Vec<&'r str> {
    match routing.and_then(|r| r.urls.as_ref()) {
        Some(urls) => urls.iter().map(String::as_str).collect(),
        None => route_urls(func),
    }
}

/// The `auth` of a route, the one it inherits when it gives none, `user` by default. Each `merged_`
/// key reads the route's own decorator when a definition the scan does not read may give the key.
fn merged_auth<'r>(call: &'r ast::ExprCall, routing: Option<&'r Routing>) -> &'r str {
    match routing.and_then(|r| r.auth.as_deref()) {
        Some(auth) => auth,
        None => keyword(call, "auth").and_then(str_lit).unwrap_or("user"),
    }
}

/// The type of a route, the one of the route it overrides whatever it says, none when the type
/// is no literal.
fn merged_type<'r>(call: &'r ast::ExprCall, routing: Option<&'r Routing>) -> Option<&'r str> {
    match routing.and_then(|r| r.kind.as_deref()) {
        Some(kind) => Some(kind),
        None => keyword(call, "type").map_or(Some("http"), str_lit),
    }
}

fn merged_has_url(call: &ast::ExprCall, routing: Option<&Routing>) -> bool {
    match routing.and_then(|r| r.urls.as_ref()) {
        Some(urls) => !urls.is_empty(),
        None => call.arguments.args.first().or_else(|| keyword(call, "route")).is_some(),
    }
}

fn merged_accepts_get(call: &ast::ExprCall, routing: Option<&Routing>) -> bool {
    match routing.and_then(|r| r.methods.as_ref()) {
        Some(Some(methods)) => methods.iter().any(|m| m.eq_ignore_ascii_case("GET")),
        Some(None) => true,
        None => accepts_get(call),
    }
}

fn merged_csrf_off(call: &ast::ExprCall, routing: Option<&Routing>) -> bool {
    match routing.and_then(|r| r.csrf) {
        Some(csrf) => !csrf,
        None => csrf_off(call).is_some(),
    }
}

struct PyVisitor<'a, 'ast> {
    loc: &'a Locator<'a>,
    series: Option<Series>,
    known: &'a FieldClasses,
    taint: &'a TaintTable,
    scope: Scope<'a>,
    facts: PyFacts,
    /// Model classes of the file by Python class name.
    kinds: FxHashMap<String, ModelKind>,
    /// What `fieldless_classes` reads of the file.
    fieldless: FxHashMap<&'ast str, Fieldless<'ast>>,
    /// What each guard that code runs behind proves, outermost first.
    guarded: Vec<Proven>,
    /// The holders of a guard in reach: the outermost enclosing function's, the innermost class's.
    holders: GuardHolders<'ast>,
    /// The model of each enclosing class, None for a mixin or a class that is no model.
    class_models: Vec<Option<String>>,
    /// Where the fact of each enclosing class sits in `facts.classes`, None for a class that is no
    /// model.
    class_facts: Vec<Option<usize>>,
    /// What `self` is in a method of each enclosing class, none for an enclosing function.
    owners: Vec<Option<Receiver>>,
    /// Where the outermost enclosing class starts, which the routing table knows it by.
    class_start: Option<TextSize>,
    /// The query names of the outermost enclosing function, whose nested functions they count.
    query_vars: Option<QueryVars<'ast>>,
    /// Where each query literal `sql_columns` read starts.
    read_queries: FxHashSet<TextSize>,
    /// Whether each enclosing class holds Odoo's cursor as `self.cr`.
    self_cursors: Vec<bool>,
    /// What each enclosing function does with cursors, innermost last.
    cursor_uses: Vec<CursorUse<'ast>>,
    /// Whether the code is in the `init()` or `_auto_init()` of a class, which creates the view of
    /// the model the class is or is mixed into, and which Odoo runs ahead of the module's data.
    in_view_maker: bool,
    /// Whether the code is in an `_auth_method_*()`, which sets the user of a request by design, or
    /// in an override of `update_env()`, which passes on the user its caller gives.
    in_user_setter: bool,
    /// The `write()` or onchange of a model the code is directly in.
    change_check: Option<ChangeCheck<'ast>>,
    /// How many enclosing `try` bodies have a handler that catches a failed import.
    import_shims: usize,
    /// The names the file's imports from `odoo` bind at import.
    odoo_names: Vec<&'ast str>,
    /// The names the file's imports bind to `odoo.osv.expression.AND` or `OR`.
    domain_joins: Vec<&'ast str>,
    /// The names the file binds to a collection of literals at import.
    collections: Vec<&'ast str>,
    /// The names the file's imports from `ORM_PACKAGES` bind at import, each with its dotted path.
    orm_names: Vec<(&'ast str, String)>,
    /// Where the method name starts in each `super()` call of the overridden method made by an
    /// override `api_table` reports. That finding covers the call of the method's name.
    covered_supers: FxHashSet<TextSize>,
}

impl<'a, 'ast> PyVisitor<'a, 'ast> {
    fn new(
        loc: &'a Locator<'a>,
        series: Option<Series>,
        known: &'a FieldClasses,
        taint: &'a TaintTable,
        scope: Scope<'a>,
        body: &'ast [Stmt],
    ) -> Self {
        let top = module_state::top_level(body);
        let odoo_imports = module_state::odoo_names(&top);
        Self {
            loc,
            series,
            known,
            taint,
            scope,
            facts: PyFacts::default(),
            kinds: FxHashMap::default(),
            fieldless: fieldless_classes(body),
            guarded: vec![],
            holders: GuardHolders::default(),
            class_models: vec![],
            class_facts: vec![],
            owners: vec![],
            class_start: None,
            query_vars: None,
            read_queries: FxHashSet::default(),
            self_cursors: vec![],
            cursor_uses: vec![],
            in_view_maker: false,
            in_user_setter: false,
            change_check: None,
            import_shims: 0,
            odoo_names: odoo_imports.iter().map(|(local, _)| *local).collect(),
            domain_joins: taint::domain_joins(&odoo_imports),
            collections: module_state::collections(&top),
            orm_names: ORM_PACKAGES
                .iter()
                .flat_map(|package| module_state::imported_names(&top, package))
                .collect(),
            covered_supers: FxHashSet::default(),
        }
    }

    fn diag(&mut self, code: &'static str, node: &impl Ranged, message: impl Into<String>) {
        self.fixed_diag(code, node, message, None);
    }

    fn fixed_diag(&mut self, code: &'static str, node: &impl Ranged, message: impl Into<String>, fix: Option<Fix>) {
        let mut diag = RawDiag::new(code, self.loc.at(node), message);
        diag.fix = fix;
        self.facts.diags.push(diag);
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
            self.field_class(class);
            return None;
        }
        if let Some(kind) = kind {
            self.kinds.insert(class.name.to_string(), kind);
        }
        let mut naming = Naming::default();
        let plain_bases: Vec<String> = class
            .bases()
            .iter()
            .filter(|b| tail_name(b).and_then(orm_kind).is_none())
            .map(|b| b.as_name_expr().map_or("", |n| n.id.as_str()).to_owned())
            .collect();
        let (opaque_bases, base_methods) = self.read_bases(class, &plain_bases);
        let mut fact = ClassFact {
            kind: kind.unwrap_or(ModelKind::Model),
            plain_bases,
            opaque_bases,
            base_methods,
            ..ClassFact::new(self.loc.at(&class.name), class.name.to_string())
        };
        for stmt in &class.body {
            if let Some((targets, value)) = assigned(stmt) {
                self.class_attrs(targets, value, &mut fact, &mut naming);
            } else if let Stmt::FunctionDef(func) = stmt {
                self.method_fact(func, &mut fact);
            } else {
                conditional_methods(stmt, &mut fact);
            }
        }
        fact.unassigned_computes = self.unassigned_computes(class, &fact);
        self.named_model(fact, naming, may_reach_orm(class))
    }

    /// The `plain_bases` of `class` the scan does not read, and the names those it reads bind: the
    /// classes its file defines above it that can hold no field.
    fn read_bases(&self, class: &ast::StmtClassDef, plain_bases: &[String]) -> (Vec<String>, Vec<String>) {
        let mut opaque = vec![];
        let mut binds = vec![];
        for base in plain_bases {
            match self.fieldless.get(base.as_str()).filter(|f| f.end <= class.start()) {
                Some(fieldless) => binds.extend(fieldless.binds.iter().map(|n| (*n).to_owned())),
                None => opaque.push(base.clone()),
            }
        }
        (opaque, binds)
    }

    /// `fact` with the model what a class says about it gives, when that gives one.
    fn named_model(&mut self, mut fact: ClassFact, mut naming: Naming, may_reach_orm: bool) -> Option<ClassFact> {
        naming.registers = fact.registered() && may_reach_orm;
        if naming.registers {
            self.facts.built_model_starts.extend(naming.built_start.take());
        }
        fact.model = self.model_name(&fact.class_name, &naming, &fact)?;
        fact.new_model = !fact.inherit.iter().any(|i| i.value == fact.model);
        fact.inherit_unknown |= naming.columns_hidden;
        Some(fact)
    }

    /// Records a class that is no model and has a base, which may be a field class.
    fn field_class(&mut self, class: &ast::StmtClassDef) {
        if class.bases().is_empty() {
            return;
        }
        let name = class.name.as_str();
        let mut fact = FieldClass {
            name: name.to_owned(),
            ftype: None,
            bases: vec![],
        };
        for base in class.bases().iter().map(generic_origin) {
            match proven_type(base, name, &self.orm_names) {
                Some(ftype) => fact.ftype = fact.ftype.take().or(Some(ftype)),
                None => fact.bases.extend(tail_name(base).map(str::to_owned)),
            }
        }
        self.facts.field_classes.push(fact);
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
                self.unknown_series_reading(dotted(class_name), earlier.as_deref(), naming, fact);
                earlier
            }
        }
    }

    /// Records the reading of a class of unknown series that `earlier` does not take, the only name
    /// in doubt. One of the class's own parents is no such name: both readings need it from
    /// `depends`.
    fn unknown_series_reading(&mut self, later: String, earlier: Option<&str>, naming: &Naming, fact: &ClassFact) {
        if earlier == Some(later.as_str()) {
            return;
        }
        self.facts.dynamic_models = true;
        if naming.registers && !fact.inherit.iter().any(|parent| parent.value == later) {
            self.facts.built_model_names.push(later);
        }
    }

    /// What a method adds to its class: its name, whether it is `@api.private`, the `super()` calls
    /// it makes, whether it calls the method it overrides through `super()`, and for `init()` the
    /// views it creates.
    fn method_fact(&mut self, func: &ast::StmtFunctionDef, fact: &mut ClassFact) {
        method_name(func, fact);
        fact.defs.push(self.method_def(func));
        if func.name.as_str() == "init" {
            fact.views = self.init_views(func);
        }
        let mut calls = SuperCalls::default();
        calls.visit_body(&func.body);
        self.method_checks(func, fact, &calls.calls);
        self.override_checks(func, &calls.calls);
        if calls.calls.iter().any(|(name, ..)| *name == func.name.as_str()) {
            fact.extended.push(func.name.to_string());
        }
        // A guarded call cannot fail, so only OD211 skips it: OD119 and OD223 still see an override.
        // No class defines `__getattr__` to end the chain: the AttributeError the last call raises
        // is what `__getattr__` owes its caller.
        let fails = |name: &str| name != "__getattr__" || func.name.as_str() != name;
        fact.super_calls.extend(
            calls
                .calls
                .into_iter()
                .filter(|(name, _, guarded)| !guarded && fails(name))
                .map(|(name, at, _)| Spanned {
                    value: name.to_owned(),
                    pos: self.loc.pos(at),
                }),
        );
    }

    fn method_def(&self, func: &ast::StmtFunctionDef) -> MethodDef {
        let decorators = func.decorator_list.iter().map(|d| match &d.expression {
            Expr::Call(call) => module_state::dotted(&call.func),
            e => module_state::dotted(e),
        });
        let mut def = MethodDef {
            name: Spanned {
                value: func.name.to_string(),
                pos: self.loc.at(&func.name),
            },
            decorators: decorators.collect(),
            signature: None,
        };
        if def.decorators.iter().all(|d| keeps_arity(d)) {
            let unbound = def.decorators.iter().any(|d| d == "staticmethod");
            def.signature = Some(self.signature(&func.parameters, unbound));
        }
        def
    }

    fn signature(&self, params: &ast::Parameters, unbound: bool) -> Signature {
        let param = |p: &ast::ParameterWithDefault| Param {
            name: p.parameter.name.to_string(),
            default: p.default.as_ref().map(|d| self.loc.src[d.range()].to_owned()),
        };
        let mut positional: Vec<Param> = params.posonlyargs.iter().chain(&params.args).map(param).collect();
        let mut positional_only = params.posonlyargs.len();
        let bound = (!unbound && !positional.is_empty()).then(|| {
            positional_only = positional_only.saturating_sub(1);
            positional.remove(0).name
        });
        Signature {
            bound,
            positional,
            positional_only,
            var_positional: params.vararg.as_ref().map(|p| p.name.to_string()),
            keyword_only: params.kwonlyargs.iter().map(param).collect(),
            var_keyword: params.kwarg.as_ref().map(|p| p.name.to_string()),
        }
    }

    fn class_attrs(&mut self, targets: &[Expr], value: &Expr, fact: &mut ClassFact, naming: &mut Naming) {
        for target in targets.iter().filter_map(Expr::as_name_expr) {
            match target.id.as_str() {
                "_name" => {
                    naming.named = true;
                    naming.name = str_lit(value).map(str::to_owned);
                    naming.built_start = built_name_start(value);
                    self.facts.dynamic_models |= naming.name.is_none();
                }
                "_inherit" => naming.inherit = self.inherit(value, fact, naming.name.as_deref()),
                "_order" | "_parent_name" | "_fold_name" => fact.order_fields.extend(self.term_fields(value)),
                "_rec_name" | "_active_name" => fact.setup_fields.extend(self.term_fields(value)),
                "_rec_names_search" => {
                    fact.rec_names_search = strings(value).into_iter().map(|s| self.spanned(s)).collect();
                }
                "_inherits" => self.inherits(value, fact),
                "_columns" if self.series.is_none_or(|s| s < COLUMNS_END) => self.columns(value, fact, naming),
                _ => self.attr_or_field(target, value, fact),
            }
        }
    }

    /// The string keys of `_columns`, each a field of unread type: the entry may be any column,
    /// computed or relational, so only its name is certain.
    fn columns(&self, value: &Expr, fact: &mut ClassFact, naming: &mut Naming) {
        let Expr::Dict(dict) = value else {
            naming.columns_hidden = true;
            return;
        };
        for item in &dict.items {
            match item.key.as_ref().and_then(Expr::as_string_literal_expr) {
                Some(key) => fact.fields.push(self.column(key)),
                None => naming.columns_hidden = true,
            }
        }
    }

    fn column(&self, key: &ast::ExprStringLiteral) -> FieldFact {
        let pos = self.loc.at(key);
        FieldFact {
            name: key.value.to_str().to_owned(),
            name_pos: pos,
            ftype: UNREAD_TYPE.to_owned(),
            pos,
            comodel: None,
            inverse_name: None,
            related: None,
            methods: vec![],
            labels: vec![],
            domain: vec![],
            domain_string: false,
            options: vec![],
            splat: true,
        }
    }

    /// Any other class attribute: one that shapes the table, a company domain or a table query
    /// that is not a field, `_sql_constraints`, a constraint or an index, an alias of a method, or
    /// else a field.
    fn attr_or_field(&mut self, target: &ast::ExprName, value: &Expr, fact: &mut ClassFact) {
        match target.id.as_str() {
            "_table" => fact.table = Some(option_value(value)),
            "_parent_store" | "_auto" | "_log_access" | "_register" | "_date_name" | "_defaults" => {
                let attr = Spanned {
                    value: option_value(value),
                    pos: self.loc.at(target),
                };
                fact.attrs.push((target.id.to_string(), attr));
            }
            // Odoo registers a field under any attribute name, so a field so named replaces the
            // attribute or method and gives the model no table: `_auto_init` reads `_auto` alone.
            "_check_company_domain" | "_table_query" | "_table_sql" if field_call(value, self.known).is_none() => {
                self.method_attr(target, value, fact);
            }
            "_sql_constraints" => {
                self.diag(
                    "OD110",
                    target,
                    "_sql_constraints is ignored; declare models.Constraint, models.Index or models.UniqueIndex attributes instead",
                );
                self.sql_constraints(value, fact);
            }
            _ => self.value_attr(target, value, fact),
        }
    }

    /// A class attribute whose name means nothing to Odoo: its value tells a constraint or an index
    /// from an alias of a method and from a field.
    fn value_attr(&mut self, target: &ast::ExprName, value: &Expr, fact: &mut ClassFact) {
        if is_table_object(value) {
            if let Some(definition) = value.as_call_expr().and_then(|c| c.arguments.args.first()) {
                self.definition(DefinitionSource::TableObject, definition, fact);
            }
        } else if may_be_method(value, self.known) {
            self.method_attr(target, value, fact);
        } else {
            self.class_field(target, value, fact);
        }
    }

    /// Records a class attribute as a method, with what it calls: `attr_calls` tells which files a
    /// scan must read again once it knows its field classes.
    fn method_attr(&mut self, target: &ast::ExprName, value: &Expr, fact: &mut ClassFact) {
        fact.methods.push(target.id.to_string());
        let called = value.as_call_expr().and_then(|c| tail_name(&c.func));
        self.facts.attr_calls.extend(called.map(str::to_owned));
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
    fn term_fields(&self, value: &Expr) -> Vec<Spanned<String>> {
        let Some(s) = value.as_string_literal_expr() else {
            return vec![];
        };
        let pos = self.loc.at(s);
        let fields = s.value.to_str().split(',').filter_map(|term| {
            let field = term.split_whitespace().next().unwrap_or_default();
            let named = !field.is_empty() && field.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.');
            named.then(|| Spanned {
                value: field.to_owned(),
                pos,
            })
        });
        fields.collect()
    }

    fn class_field(&mut self, target: &ast::ExprName, value: &Expr, fact: &mut ClassFact) {
        let Some(field) = self.field_fact(target, value) else {
            return;
        };
        if is_delegate(value, self.known)
            && let Some(comodel) = &field.comodel
        {
            let pair = (comodel.value.clone(), field.name.clone());
            if !fact.inherits.contains(&pair) {
                fact.inherits.push(pair);
            }
        } else if may_delegate(value, self.known) {
            fact.delegate_unknown = true;
        }
        fact.fields.push(field);
    }

    fn field_fact(&mut self, target: &ast::ExprName, value: &Expr) -> Option<FieldFact> {
        let (ftype, call) = field_call(value, self.known)?;
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
        if let Some(k) = keyword_arg(call, "group_operator") {
            let fix = renamed_keyword(call, k, "aggregator").map(|arg| self.loc.replace(arg, "aggregator"));
            self.fixed_diag("OD108", k, "group_operator is renamed aggregator", fix);
        }
        if let Some(k) = keyword_arg(call, "oldname") {
            self.api_use(Kind::FieldOption("oldname"), k);
        }
        if let Some(k) = keyword_arg(call, "track_visibility") {
            let fix = tracked(call, k).then(|| self.loc.replace(k.range(), "tracking=True"));
            self.fixed_api_use(Kind::FieldOption("track_visibility"), k, fix);
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
            labels: selection_labels(ftype, call),
            domain: keyword(call, "domain").map(|d| self.domain(d)).unwrap_or_default(),
            domain_string: keyword(call, "domain").is_some_and(|d| matches!(d, Expr::StringLiteral(_))),
            options,
            splat: hides_options(ftype, call),
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
            Expr::Call(c) => match &*c.func {
                Expr::Attribute(a) if SAME_MODEL.contains(&a.attr.as_str()) => self.receiver_model(&a.value),
                _ => None,
            },
            _ => env_literal(expr).map(str::to_owned),
        }
    }

    fn domain_call(&mut self, call: &ast::ExprCall) {
        let Expr::Attribute(a) = &*call.func else { return };
        if !DOMAIN_METHODS.contains(&a.attr.as_str()) || !self.guarded.is_empty() {
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

    fn method_checks(&mut self, func: &ast::StmtFunctionDef, fact: &mut ClassFact, supers: &[(&str, TextSize, bool)]) {
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
        self.api_definitions(func, supers);
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

    /// The method definitions `api_table` has a row for: an override of its name, or an onchange
    /// returning its key in a literal dict. A reported override covers its `super()` calls of the
    /// method it overrides, which would report the same form again.
    fn api_definitions(&mut self, func: &ast::StmtFunctionDef, supers: &[(&str, TextSize, bool)]) {
        let name = func.name.as_str();
        let overridden = api_table::ROWS
            .iter()
            .find(|row| matches!(row.kind, Kind::Override(m) if m == name));
        if let Some((code, message)) = overridden.and_then(|row| api_table::stage(row, self.series)) {
            self.diag(code, &func.name, message);
            let same = supers.iter().filter(|(called, ..)| *called == name);
            self.covered_supers.extend(same.map(|(_, at, _)| *at));
        }
        let onchange = |d: &ast::Decorator| matches!(&d.expression, Expr::Call(c) if is_api(&c.func, "onchange"));
        if func.decorator_list.iter().any(onchange)
            && let Some(key) = method_body::returned_key(&func.body, "domain")
        {
            self.api_use(Kind::OnchangeKey("domain"), key);
        }
    }

    /// OD119 on an override whose result never reaches its callers, and OD117 and OD118 on a
    /// `create` that takes one dict at a time.
    fn override_checks(&mut self, func: &ast::StmtFunctionDef, supers: &[(&str, TextSize, bool)]) {
        let name = func.name.as_str();
        if matches!(name, "create" | "write" | "copy")
            && supers.iter().any(|(called, ..)| *called == name)
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
                    // `@api.one` returned a list of the results, so dropping it changes what callers get.
                    let fix = (a.attr.as_str() == "multi")
                        .then(|| self.loc.remove_line(decorator))
                        .flatten();
                    self.fixed_diag(
                        "OD107",
                        decorator,
                        format!("@api.{} no longer exists; methods always receive a recordset", a.attr),
                        fix,
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
    fn flows(&mut self, func: &ast::StmtFunctionDef, routing: Option<&Routing>) {
        let decorated = |name: &str| {
            func.decorator_list.iter().any(|d| match &d.expression {
                Expr::Call(call) => tail_name(&call.func) == Some(name),
                e => tail_name(e) == Some(name),
            })
        };
        let records = record_params(&merged_urls(func, routing));
        let params = if decorated("route") {
            Params::Route(&records)
        } else if matches!(self.owners.last(), Some(Some(Receiver::Model(_))))
            && !func.name.starts_with('_')
            && !decorated("private")
        {
            Params::Rpc
        } else {
            Params::None
        };
        let owner = self.owners.last().cloned().flatten();
        let names = taint::FileNames {
            odoo: &self.odoo_names,
            collections: &self.collections,
            domain_joins: &self.domain_joins,
        };
        let found = taint::function(func, params, owner, self.series, &names, self.loc, self.taint);
        self.facts.diags.extend(found.diags);
        self.facts.granted_attachments.extend(found.granted);
        self.facts.flows.extend(found.summary);
    }

    /// The routing Odoo merges for a method of a class at the top of the file.
    fn routing(&self, func: &ast::StmtFunctionDef) -> Option<Routing> {
        let ([Some(Receiver::Class(_))], Some(start)) = (self.owners.as_slice(), self.class_start) else {
            return None;
        };
        self.scope.routing(start, &func.name)
    }

    fn route_checks(&mut self, func: &ast::StmtFunctionDef, routing: Option<&Routing>) {
        let urls = merged_urls(func, routing);
        let records = converted_records(&urls);
        let mut body = None;
        for decorator in &func.decorator_list {
            if let Expr::Call(call) = &decorator.expression
                && tail_name(&call.func) == Some("route")
            {
                let body = body.get_or_insert_with(|| RouteBody::of(func, &records));
                self.route_check(func, call, routing, body);
            }
        }
    }

    fn route_check(
        &mut self,
        func: &ast::StmtFunctionDef,
        call: &ast::ExprCall,
        routing: Option<&Routing>,
        body: &RouteBody,
    ) {
        let auth = merged_auth(call, routing);
        self.route_sudo(func, auth);
        self.route_type(call, routing);
        self.route_get_writes(func, call, routing, body);
        self.route_cors(call);
        self.route_no_user(func, auth, body);
    }

    fn route_sudo(&mut self, func: &ast::StmtFunctionDef, auth: &str) {
        if !matches!(auth, "public" | "none") {
            return;
        }
        let what = match superuser::escalation(&func.body, self.series) {
            Some(Escalation::Sudo) => "calls sudo()".to_owned(),
            Some(Escalation::Switch(form)) => format!("runs as superuser through {form}"),
            None => return,
        };
        self.diag("OD111", &func.name, format!("route with auth='{auth}' {what}; check that every record it reads or writes is filtered by an access check"));
    }

    /// OD125 reads the route's own `type`, while Odoo keeps the type the route it overrides has,
    /// which decides whether it checks a CSRF token.
    fn route_type(&mut self, call: &ast::ExprCall, routing: Option<&Routing>) {
        if let Some(k) = keyword_arg(call, "type").filter(|k| str_lit(&k.value) == Some("json")) {
            self.api_use(Kind::RouteType("json"), k);
        }
        if merged_type(call, routing).is_none_or(|kind| kind == "http") {
            self.route_csrf(call);
        }
    }

    fn route_csrf(&mut self, call: &ast::ExprCall) {
        if let Some(k) = csrf_off(call) {
            self.diag(
                "OD112",
                k,
                "csrf=False lets other sites submit this form on the user's behalf",
            );
        }
    }

    /// A route with no URL the scan reads is an override of a route it does not, whose methods it
    /// does not know either.
    fn route_get_writes(
        &mut self,
        func: &ast::StmtFunctionDef,
        call: &ast::ExprCall,
        routing: Option<&Routing>,
        body: &RouteBody,
    ) {
        let http = merged_type(call, routing) == Some("http");
        let open = http && merged_has_url(call, routing) && merged_accepts_get(call, routing);
        if !open || merged_csrf_off(call, routing) || body.checks_token {
            return;
        }
        if let Some(write) = body.unguarded_write {
            self.diag(
                "OD127",
                &func.name,
                format!("route accepts GET, on which Odoo checks no CSRF token, and calls {write}(); restrict it with methods=['POST'] or check a token"),
            );
        }
    }

    fn route_cors(&mut self, call: &ast::ExprCall) {
        if let Some(k) = keyword_arg(call, "cors")
            && str_lit(&k.value) == Some("*")
        {
            self.diag(
                "OD128",
                k,
                "cors='*' lets a script on any site call this route and read its response; name the origin that needs it",
            );
        }
    }

    fn route_no_user(&mut self, func: &ast::StmtFunctionDef, auth: &str, body: &RouteBody) {
        if auth != "none" || body.sets_user {
            return;
        }
        let what = if body.reads_user {
            "reads request.env.user, which is empty as the route runs with no user".to_owned()
        } else if let Some(write) = body.write {
            format!("calls {write}(), but runs with no user, so the write belongs to no one")
        } else {
            return;
        };
        self.diag(
            "OD129",
            &func.name,
            format!("route with auth='none' {what}; use auth='public' or set the user with request.update_env()"),
        );
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
        self.api_calls(call);
        self.domain_call(call);
        self.id_refs(call, expr);
        self.sql_columns(call);
        self.added_columns(call);
        self.created_views(call);
        self.update_env_user(call);
    }

    /// OD130 on `update_env()` with a user, which runs the rest of the request as that user (18.0
    /// `odoo/http.py:1702-1712`). In a user setter it is the override or the caller that is reported.
    fn update_env_user(&mut self, call: &ast::ExprCall) {
        if self.in_user_setter || tail_name(&call.func) != Some("update_env") {
            return;
        }
        let user = call.arguments.find_argument_value("user", 0);
        if user.is_some_and(switches_user) {
            self.diag(
                "OD130",
                call,
                "update_env() switches the request to another user; check that the caller has proven to be that user",
            );
        }
    }

    /// OD130 on `request.session.uid = ...`, the `uid` key of the session (18.0
    /// `odoo/http.py:1058-1062`), which later requests of the browser run as, and on `request.uid =
    /// ...`, which runs the rest of the request as that user before `REQUEST_UID_END` (15.0
    /// `odoo/http.py:223-226,241-244`). Putting back a `request.uid` the body saved switches to no
    /// other user, as the web login does after a failed one (15.0
    /// `addons/web/controllers/main.py:870,876`).
    fn session_uid(&mut self, assign: &'ast ast::StmtAssign) {
        if self.in_user_setter || !switches_user(&assign.value) {
            return;
        }
        let restores = self
            .query_vars
            .as_ref()
            .is_some_and(|vars| superuser::is_request_uid(vars.value(&assign.value)));
        let request_uid = !restores && self.series.is_none_or(|s| s < superuser::REQUEST_UID_END);
        for target in &assign.targets {
            let message = if is_session_uid(target) {
                "assigning the session's uid switches the user the browser's requests run as; check that the caller has proven to be that user"
            } else if request_uid && superuser::is_request_uid(target) {
                "assigning request.uid switches the request to another user; check that the caller has proven to be that user"
            } else {
                continue;
            };
            self.diag("OD130", target, message);
        }
    }

    /// OD130 on an override of `_auth_method_*()` of `ir.http`, which Odoo calls to set the user of
    /// every route of that auth (18.0 `odoo/addons/base/models/ir_http.py:272`).
    fn auth_method_override(&mut self, func: &ast::StmtFunctionDef) {
        let Some(auth) = func.name.strip_prefix("_auth_method_") else {
            return;
        };
        let ir_http = matches!(self.owners.last(), Some(Some(Receiver::Model(m))) if m == "ir.http");
        let inherited = matches!(self.class_facts.last(), Some(Some(i)) if !self.facts.classes[*i].new_model);
        if ir_http && inherited {
            self.diag(
                "OD130",
                &func.name,
                format!("_auth_method_{auth}() decides which user every route with auth='{auth}' runs as; check that it sets only a user the request has proven to be"),
            );
        }
    }

    /// OD131 on `==` or `!=` between two secrets, which takes longer the more of the secret a guess
    /// gets right.
    fn secret_compare(&mut self, compare: &'ast ast::ExprCompare) {
        let mut pairs = compare.ops.iter().zip(compare.operands.windows(2));
        let found =
            pairs.find(|(op, pair)| matches!(op, CmpOp::Eq | CmpOp::NotEq) && self.leaks_secret(&pair[0], &pair[1]));
        if let Some((op, _)) = found {
            self.diag(
                "OD131",
                compare,
                format!("{op} between secrets takes longer the more of the secret a guess gets right, which lets a caller find it a character at a time; compare them with consteq() or hmac.compare_digest()"),
            );
        }
    }

    /// Whether `a` and `b` are two secrets a caller may time the compare of. A field against the
    /// value a change sets it to tells only whether it changes, which a user who may set it knows.
    /// The signatures of two records hold nothing a caller sends.
    fn leaks_secret(&self, a: &'ast Expr, b: &'ast Expr) -> bool {
        let (Some(x), Some(y)) = (secret_word(a), secret_word(b)) else {
            return false;
        };
        let changed = self
            .change_check
            .as_ref()
            .is_some_and(|check| self.changes_field(check, a, b));
        !changed && !(x == "signature" && y == "signature" && same_source(a, b))
    }

    /// Whether `a` and `b`, directly or through a local bound once, tell only whether a field `f`
    /// changes: `f` read twice, as before and after `super().write()` or on a record and on its
    /// `_origin`, the stored record behind an onchange's new one of the form's values (18.0
    /// `addons/web/models/models.py:982-989`); `f` against `vals['f']` or `vals.get('f')`; or in an
    /// onchange, `f` of `self` or of a record of it a loop takes against a local named `f`.
    fn changes_field(&self, check: &ChangeCheck, a: &'ast Expr, b: &'ast Expr) -> bool {
        let value = |e: &'ast Expr| self.query_vars.as_ref().map_or(e, |vars| vars.value(e));
        let field = |e: &'ast Expr| value(e).as_attribute_expr();
        if let (Some(x), Some(y)) = (field(a), field(b))
            && x.attr.as_str() == y.attr.as_str()
        {
            return true;
        }
        let sets = |stored: &'ast Expr, set: &'ast Expr| {
            let Some(stored) = field(stored) else {
                return false;
            };
            let f = stored.attr.as_str();
            check.vals.is_some_and(|vals| vals_key(value(set), vals) == Some(f))
                || check.onchange
                    && check.holds_form(&stored.value)
                    && matches!(set, Expr::Name(n) if n.id.as_str() == f)
        };
        sets(a, b) || sets(b, a)
    }

    /// The calls `api_table` has a row for, but those of a method the class defines itself, and
    /// the method name of a `super()` call a reported override covers.
    fn api_calls(&mut self, call: &ast::ExprCall) {
        let Expr::Attribute(a) = &*call.func else { return };
        if a.attr.as_str() == "sudo" && call.arguments.args.first().is_some_and(names_user) {
            let alone = call.arguments.args.len() == 1 && call.arguments.keywords.is_empty();
            let fix = alone.then(|| self.loc.replace(a.attr.range(), "with_user"));
            self.fixed_api_use(Kind::SudoUser, &a.attr, fix);
        }
        if self.own_method(&a.value, a.attr.as_str()) {
            return;
        }
        for row in api_table::ROWS {
            if self.covered_super(row.kind, a) {
                continue;
            }
            if let Some(range) = self.api_call_range(row.kind, call, a)
                && let Some((code, message)) = self.row_stage(row, call, a)
            {
                self.diag(code, &range, message);
            }
        }
    }

    /// The stage of a call matching the row, with the advice for the argument it passes.
    fn row_stage(
        &self,
        row: &api_table::Row,
        call: &ast::ExprCall,
        a: &ast::ExprAttribute,
    ) -> Option<(&'static str, String)> {
        if passes_false(row, call) {
            api_table::stage_if_false(row, self.series)
        } else if row.if_negative.is_some() && self.writes_negative(row.kind, call, a) {
            api_table::stage_if_negative(row, self.series)
        } else {
            api_table::stage(row, self.series)
        }
    }

    /// Whether the call writes a literal negative number as the field of a written-field row.
    fn writes_negative(&self, kind: Kind, call: &ast::ExprCall, a: &ast::ExprAttribute) -> bool {
        let Kind::WrittenField(model, field) = kind else {
            return false;
        };
        self.written_field(call, &a.value, model, field).is_some_and(|item| {
            matches!(&item.value, Expr::UnaryOp(u) if u.op == UnaryOp::USub && u.operand.is_number_literal_expr())
        })
    }

    /// The range to report when the call matches the row's form: the method name, the first
    /// keyword the row lists, or the key of the value it passes.
    fn api_call_range(&self, kind: Kind, call: &ast::ExprCall, a: &ast::ExprAttribute) -> Option<TextRange> {
        let method = a.attr.as_str();
        match kind {
            Kind::Method(m) if m == method => Some(a.attr.range()),
            Kind::RecordsetMethod(m) if m == method && self.on_recordset(&a.value) => Some(a.attr.range()),
            Kind::Keywords(m, names) if m == method => {
                names.iter().find_map(|name| keyword_arg(call, name)).map(Ranged::range)
            }
            _ => self.passed_key_range(kind, call, a),
        }
    }

    /// The range of the keyword or the dict key that the call passes for a row of a passed key.
    fn passed_key_range(&self, kind: Kind, call: &ast::ExprCall, a: &ast::ExprAttribute) -> Option<TextRange> {
        match (kind, a.attr.as_str()) {
            (Kind::ContextKey(key), "with_context") => context_key(call, key),
            (Kind::WrittenField(model, field), "create" | "write") => {
                let item = self.written_field(call, &a.value, model, field)?;
                item.key.as_ref().map(Ranged::range)
            }
            _ => None,
        }
    }

    /// The item that names `field` in the literal values a `create()` or `write()` on `model`
    /// writes: a dict, or the first dict of a list or tuple that names it.
    fn written_field<'c>(
        &self,
        call: &'c ast::ExprCall,
        receiver: &Expr,
        model: &str,
        field: &str,
    ) -> Option<&'c ast::DictItem> {
        if self.receiver_model(receiver).as_deref() != Some(model) {
            return None;
        }
        let values = call.arguments.args.first()?;
        let mut dicts = domain::items(values)
            .map_or(std::slice::from_ref(values), |items| items)
            .iter();
        dicts.find_map(|d| method_body::dict_item(d, field))
    }

    /// Whether the row is the call of the method a reported override overrides, at a `super()` call
    /// of that override. The keywords such a call passes stay reported, as they fail on their own.
    fn covered_super(&self, kind: Kind, a: &ast::ExprAttribute) -> bool {
        matches!(kind, Kind::Method(m) if m == a.attr.as_str()) && self.covered_supers.contains(&a.attr.start())
    }

    /// Whether `expr` is `self` in a method of a model class, mixins included, or a model the code
    /// names.
    fn on_recordset(&self, expr: &Expr) -> bool {
        (self_model(expr) && matches!(self.class_facts.last(), Some(Some(_)))) || self.receiver_model(expr).is_some()
    }

    /// Whether the call of `method` on `expr` runs the enclosing model class's own definition:
    /// `expr` is `self`, or the class's model, and the class defines `method` without calling the
    /// definition it overrides through `super()`, which may be the one Odoo removed.
    fn own_method(&self, expr: &Expr, method: &str) -> bool {
        let Some(&Some(class)) = self.class_facts.last() else {
            return false;
        };
        let class = &self.facts.classes[class];
        let named = |names: &[String]| names.iter().any(|m| m == method);
        named(&class.methods)
            && !named(&class.extended)
            && (self_model(expr) || self.receiver_model(expr).is_some_and(|model| model == class.model))
    }

    fn api_use(&mut self, kind: Kind, node: &impl Ranged) {
        self.fixed_api_use(kind, node, None);
    }

    fn fixed_api_use(&mut self, kind: Kind, node: &impl Ranged, fix: Option<Fix>) {
        if let Some((code, message)) = api_table::stage(api_table::row_of(kind), self.series) {
            self.fixed_diag(code, node, message, fix);
        }
    }

    /// One finding per import statement, for the first row whose package the statement imports and
    /// whose stage the series has reached. Behind a `try` that catches the `ImportError`, the import
    /// of a removed package is a fallback, while one that still loads warns all the same.
    fn api_imports(&mut self, stmt: &Stmt) {
        let modules = imported_modules(stmt);
        let found = api_table::ROWS.iter().find_map(|row| match row.kind {
            Kind::Import(package) if modules.iter().any(|m| under_package(m, package)) => {
                api_table::stage(row, self.series)
            }
            _ => None,
        });
        if let Some((code, message)) = found
            && !(code == "OD126" && self.import_shims > 0)
        {
            self.diag(code, stmt, message);
        }
    }

    fn request_attr(&mut self, a: &ast::ExprAttribute, expr: &Expr) {
        if a.attr.as_str() == "jsonrequest" && tail_name(&a.value) == Some("request") {
            self.api_use(Kind::RequestAttr("jsonrequest"), expr);
        }
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
                before_data: self.in_view_maker,
            });
        }
    }

    fn id_refs(&mut self, call: &ast::ExprCall, expr: &Expr) {
        let first = call.arguments.args.first().and_then(Expr::as_string_literal_expr);
        match &*call.func {
            _ if is_env_ref(call) => self.env_ref(call),
            Expr::Attribute(a) if a.attr.as_str() == "has_group" => first.into_iter().for_each(|s| self.group_refs(s)),
            _ if field_call(expr, self.known).is_some() => {
                if let Some(groups) = keyword(call, "groups").and_then(Expr::as_string_literal_expr) {
                    self.group_refs(groups);
                }
            }
            _ => {}
        }
    }

    fn env_ref(&mut self, call: &ast::ExprCall) {
        if raises_if_not_found(call)
            && let Some(id) = call.arguments.args.first().and_then(Expr::as_string_literal_expr)
            && id.value.to_str().contains('.')
        {
            let xml_ref = XmlRef {
                id: id.value.to_str().to_owned(),
                pos: self.loc.at(id),
            };
            match self.guarded.iter().cloned().reduce(join) {
                Some(proven) => self.facts.guarded_refs.push(GuardedRef { xml_ref, proven }),
                None => self.facts.xml_refs.push(xml_ref),
            }
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
            && self.guarded.is_empty()
            && let Some(s) = sub.slice.as_string_literal_expr()
        {
            let spanned = self.spanned(s);
            self.facts.env_models.push(spanned);
        }
    }

    /// Visits `test`, then what runs when it holds and what runs when it does not.
    fn branches(&mut self, test: &'ast Expr, then: impl FnOnce(&mut Self), otherwise: impl FnOnce(&mut Self)) {
        self.visit_expr(test);
        let (opened, closed) = (self.holders.cover(test, true), self.holders.cover(test, false));
        self.guarded_by(opened, then);
        self.guarded_by(closed, otherwise);
    }

    fn guarded_by(&mut self, guard: Option<Proven>, visit: impl FnOnce(&mut Self)) {
        let depth = self.guarded.len();
        self.guarded.extend(guard);
        visit(self);
        self.guarded.truncate(depth);
    }

    /// Operands of an `and` after a guard run only once it passed.
    fn and_chain(&mut self, values: &'ast [Expr]) {
        let depth = self.guarded.len();
        for value in values {
            self.visit_expr(value);
            let guard = self.holders.cover(value, true);
            self.guarded.extend(guard);
        }
        self.guarded.truncate(depth);
    }

    fn private_attr(&mut self, a: &ast::ExprAttribute, expr: &Expr) {
        if matches!(a.attr.as_str(), "_cr" | "_uid" | "_context")
            && matches!(&*a.value, Expr::Name(n) if n.id.as_str() == "self")
        {
            let public = &a.attr.as_str()[1..];
            // Outside a model `self` may be anything, and a write would set another attribute.
            let read_on_model = a.ctx.is_load() && matches!(self.class_facts.last(), Some(Some(_)));
            let fix = read_on_model.then(|| self.loc.replace(a.attr.range(), &format!("env.{public}")));
            self.fixed_diag(
                "OD109",
                expr,
                format!("self.{} is deprecated; use self.env.{public}", a.attr),
                fix,
            );
        }
    }

    fn visit_function(&mut self, func: &'ast ast::StmtFunctionDef, stmt: &'ast Stmt) {
        let routing = self.routing(func);
        self.route_checks(func, routing.as_ref());
        let uses = CursorUse::scan(func, self.cursor_uses.last());
        self.rollback_checks(func, &uses);
        self.flows(func, routing.as_ref());
        let outermost = self.query_vars.is_none();
        if outermost {
            self.query_vars = Some(QueryVars::of(func));
        }
        let names = self.holders.names.clone();
        if outermost {
            self.holders.names.clear();
        }
        let in_class = matches!(self.owners.last(), Some(Some(_)));
        let maker = in_class && matches!(func.name.as_str(), "init" | "_auto_init");
        let in_view_maker = self.in_view_maker;
        self.in_view_maker |= maker;
        self.auth_method_override(func);
        let in_user_setter = self.in_user_setter;
        self.in_user_setter |= func.name.starts_with("_auth_method_") || func.name.as_str() == "update_env";
        let model = matches!(self.owners.last(), Some(Some(Receiver::Model(_))));
        let change_check = std::mem::replace(&mut self.change_check, checks_change(func).filter(|_| model));
        self.owners.push(None);
        self.cursor_uses.push(uses);
        visitor::walk_stmt(self, stmt);
        self.cursor_uses.pop();
        self.owners.pop();
        self.in_view_maker = in_view_maker;
        self.in_user_setter = in_user_setter;
        self.change_check = change_check;
        self.holders.names = names;
        if outermost {
            self.query_vars = None;
        }
    }

    /// What the statements after `stmt` in its block run behind, if they run only once a guard has
    /// passed: an `if` that leaves or skips unless the thing is there, a `try` that looks up what
    /// the rest needs and skips from every handler, or a call that skips unless a module is
    /// installed.
    fn guards_rest(&self, stmt: &Stmt) -> Option<Proven> {
        match stmt {
            Stmt::If(i) if exits(&i.body) || skips(&i.body) => self.holders.cover(&i.test, false),
            Stmt::Try(t) => try_guards(t).map(|(_, after)| after),
            Stmt::Expr(e) => ensures_installed(&e.value).map(|m| Some(vec![Subject::Module(m.to_owned())])),
            _ => None,
        }
    }

    /// A change check knows the records of `self` a loop takes, which hold the form's values in an
    /// onchange as `self` does.
    fn visit_for(&mut self, f: &'ast ast::StmtFor, stmt: &'ast Stmt) {
        self.holders.unbind(&f.target);
        let target = f.target.as_name_expr().map(|n| (n.id.as_str(), is_self(&f.iter)));
        let looped = match (&mut self.change_check, target) {
            (Some(check), Some(target)) => {
                check.loops.push(target);
                true
            }
            _ => false,
        };
        visitor::walk_stmt(self, stmt);
        if looped && let Some(check) = &mut self.change_check {
            check.loops.pop();
        }
        self.holders.rebind(stmt);
    }

    /// The handlers run once the lookup has failed, and `finally` runs either way.
    fn visit_try(&mut self, t: &'ast ast::StmtTry) {
        let (within, after) = try_guards(t).unzip();
        let shim = t
            .handlers
            .iter()
            .any(|ast::ExceptHandler::ExceptHandler(h)| catches(h, "ImportError") || catches(h, "ModuleNotFoundError"));
        self.import_shims += usize::from(shim);
        self.guarded_by(within, |v| v.visit_body(&t.body));
        self.import_shims -= usize::from(shim);
        t.handlers.iter().for_each(|h| self.visit_except_handler(h));
        self.guarded_by(after, |v| v.visit_body(&t.orelse));
        self.visit_body(&t.finalbody);
    }
}

impl<'ast> Visitor<'ast> for PyVisitor<'_, 'ast> {
    /// Statements after `if 'x' not in self.env: return` run only once the thing is there.
    fn visit_body(&mut self, body: &'ast [Stmt]) {
        let depth = self.guarded.len();
        for stmt in body {
            let guard = self.guards_rest(stmt);
            self.visit_stmt(stmt);
            self.guarded.extend(guard);
        }
        self.guarded.truncate(depth);
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
                let class_start = self.class_start;
                self.class_start = class_start.or(Some(class.start()));
                self.owners.push(Some(owner));
                self.class_facts
                    .push(fact.is_some().then_some(self.facts.classes.len()));
                self.facts.classes.extend(fact);
                self.class_models.push(model);
                self.self_cursors.push(self_cursor(class));
                let attrs = std::mem::replace(&mut self.holders.attrs, GuardHolders::attrs_of(class));
                visitor::walk_stmt(self, stmt);
                self.holders.attrs = attrs;
                self.self_cursors.pop();
                self.class_models.pop();
                self.class_facts.pop();
                self.owners.pop();
                self.class_start = class_start;
                return;
            }
            Stmt::FunctionDef(func) => return self.visit_function(func, stmt),
            Stmt::If(i) => {
                return self.branches(
                    &i.test,
                    |v| v.visit_body(&i.body),
                    |v| i.elif_else_clauses.iter().for_each(|c| v.visit_elif_else_clause(c)),
                );
            }
            Stmt::Try(t) => return self.visit_try(t),
            Stmt::For(f) => return self.visit_for(f, stmt),
            Stmt::Assign(a) => self.session_uid(a),
            Stmt::Import(_) | Stmt::ImportFrom(_) => self.api_imports(stmt),
            _ => {}
        }
        visitor::walk_stmt(self, stmt);
        self.holders.rebind(stmt);
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        match expr {
            Expr::StringLiteral(s) => self.file_mention(s),
            Expr::Call(call) => self.call_checks(call, expr),
            Expr::BoolOp(b) if b.op == BoolOp::And => return self.and_chain(&b.values),
            Expr::If(i) => {
                return self.branches(&i.test, |v| v.visit_expr(&i.body), |v| v.visit_expr(&i.orelse));
            }
            Expr::Subscript(sub) => self.env_model(sub),
            Expr::Compare(c) => self.secret_compare(c),
            Expr::Attribute(a) => {
                self.private_attr(a, expr);
                self.request_attr(a, expr);
            }
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
    fn reads_the_labels_of_literal_selection_lists() {
        let literal = |l: &str| Some(l.to_owned());
        let f = field("fields.Selection([('a', 'Ay'), ('b', _('Bee')), ('c', LABEL)], string='S')");
        assert_eq!(f.labels, [literal("Ay"), literal("Bee"), None]);
        let f = field("fields.Selection(selection_add=[('a', _lt('Ay')), ('b',)])");
        assert_eq!(f.labels, [literal("Ay")]);
        assert_eq!(field("fields.Selection('_list')").labels, []);
        assert_eq!(field("fields.Char('[]')").labels, []);
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
             _fold_name = 'folded'\n    _rec_name = 'code'\n    _active_name = 'x_active'\n    \
             _rec_names_search = ['name', 'partner_id.ref']\n",
            None,
        );
        let values = |v: &[Spanned<String>]| v.iter().map(|s| s.value.clone()).collect::<Vec<_>>();
        let class = &facts.classes[0];
        assert_eq!(values(&class.order_fields), ["sequence", "id", "up_id", "folded"]);
        assert_eq!(values(&class.setup_fields), ["code", "x_active"]);
        assert_eq!(values(&class.rec_names_search), ["name", "partner_id.ref"]);
    }

    /// The method names of a model class with the body `body`.
    fn method_names(body: &str) -> Vec<String> {
        let src = format!("class X(models.Model):\n    _name = 'x'\n{body}");
        analyze(&src, None).classes.remove(0).methods
    }

    #[test]
    fn counts_every_class_value_but_a_field_a_table_object_and_data_as_a_method() {
        for value in [
            "_set",
            "tools.a",
            "lambda self: 1",
            "partialmethod(_set, 'a')",
            "functools.partial(_set)",
            "api.model(_set)",
            "api.depends('a')(_set)",
            "helper()",
            "Vector()",
            "a if b else c",
            "H['x']",
        ] {
            assert_eq!(method_names(&format!("    alias = {value}\n")), ["alias"], "{value}");
        }
        for value in [
            "fields.Char()",
            "Char()",
            "odoo.fields.Char()",
            "f.Char()",
            "fields.Serialized()",
            "models.Constraint('CHECK (a)')",
            "models.Index('(a)')",
            "models.UniqueIndex('(a)')",
            "Index('(a)')",
            "3",
            "'a'",
            "b'a'",
            "None",
            "True",
            "...",
            "f'{a}'",
            "t'{a}'",
            "[]",
            "()",
            "{}",
            "{1}",
            "[a for a in b]",
            "{a for a in b}",
            "{a: 1 for a in b}",
            "(a for a in b)",
        ] {
            assert!(method_names(&format!("    alias = {value}\n")).is_empty(), "{value}");
        }
    }

    #[test]
    fn reads_the_method_names_of_conditional_blocks() {
        let body = "    if a:\n        def in_if(self):\n            pass\n    \
                    elif b:\n        def in_elif(self):\n            pass\n    \
                    else:\n        def in_else(self):\n            pass\n    \
                    try:\n        def in_try(self):\n            pass\n    \
                    except E:\n        def in_except(self):\n            pass\n    \
                    else:\n        def in_try_else(self):\n            pass\n    \
                    finally:\n        def in_finally(self):\n            pass\n    \
                    with c:\n        if d:\n            def in_with(self):\n                pass\n    \
                    if e:\n        alias: int = partialmethod(_set)\n        data = 3\n";
        assert_eq!(
            method_names(body),
            [
                "in_if",
                "in_elif",
                "in_else",
                "in_try",
                "in_try_else",
                "in_finally",
                "in_except",
                "in_with",
                "alias"
            ]
        );
    }

    #[test]
    fn reads_only_the_name_of_what_a_conditional_block_binds() {
        let facts = analyze(
            "class X(models.Model):\n    _name = 'x'\n    if a:\n        state = fields.Char()\n        \
             def name_get(self):\n            return super().unlink()\n        \
             @api.private\n        def _hidden(self):\n            pass\n",
            None,
        );
        let class = &facts.classes[0];
        assert!(class.fields.is_empty());
        assert_eq!(class.methods, ["name_get", "_hidden"]);
        assert_eq!(class.private_methods, ["_hidden"]);
        assert!(class.super_calls.is_empty());
        assert!(!facts.diags.iter().any(|d| d.code == "OD106"));
    }

    /// The field classes of one file per source, sorted.
    fn classes(sources: &[&str]) -> Vec<(String, String)> {
        let facts: Vec<PyFacts> = sources.iter().map(|src| analyze(src, None)).collect();
        let mut known: Vec<_> = field_classes(facts.iter()).into_iter().collect();
        known.sort();
        known
    }

    fn pair(name: &str, ftype: &str) -> (String, String) {
        (name.to_owned(), ftype.to_owned())
    }

    #[test]
    fn resolves_field_classes_through_bases_across_files() {
        let base = "from odoo import fields\nclass A(fields.Field):\n    pass\nclass T(fields.Char):\n    pass\n";
        let derived = "class B(A):\n    pass\n";
        let further = "class C(Other, pkg.B):\n    pass\nclass D(T):\n    pass\nclass Lone(Other):\n    pass\n";
        let expected = [
            pair("A", "A"),
            pair("B", "A"),
            pair("C", "A"),
            pair("D", "Char"),
            pair("T", "Char"),
        ];
        assert_eq!(classes(&[further, derived, base]), expected);
        assert_eq!(classes(&[base, derived, further]), expected);
    }

    #[test]
    fn a_cycle_of_bases_proves_nothing() {
        assert!(classes(&["class A(B):\n    pass\nclass B(A):\n    pass\n"]).is_empty());
    }

    #[test]
    fn a_name_with_two_field_types_is_unread_whatever_the_file_order() {
        let text = "from odoo import fields\nclass V(fields.Text):\n    pass\n";
        let char = "from odoo import fields\nclass V(fields.Char):\n    pass\n";
        let derived = "from odoo import fields\nclass A(fields.Char):\n    pass\nclass V(A):\n    pass\n";
        let sub = "class W(V):\n    pass\n";
        let expected = [pair("V", UNREAD_TYPE), pair("W", UNREAD_TYPE)];
        assert_eq!(classes(&[text, char, sub]), expected);
        assert_eq!(classes(&[sub, char, text]), expected);
        let by_base = [pair("A", "Char"), pair("V", UNREAD_TYPE), pair("W", UNREAD_TYPE)];
        assert_eq!(classes(&[sub, derived, text]), by_base);
        assert_eq!(classes(&[text, sub, derived]), by_base);
    }

    #[test]
    fn two_classes_of_a_name_and_type_keep_it() {
        let char = "from odoo import fields\nclass V(fields.Char):\n    pass\n";
        assert_eq!(classes(&[char, char]), [pair("V", "Char")]);
        let field = "from odoo import fields\nclass V(fields.Field):\n    pass\n";
        assert_eq!(classes(&[field, field]), [pair("V", "V")]);
    }

    #[test]
    fn a_field_of_unread_type_gives_nothing_by_position() {
        let known = FieldClasses::from_iter([pair("Mark", UNREAD_TYPE)]);
        let src = "class X(models.Model):\n    _name = 'x'\n    a = Mark('p.q')\n    b = Mark(string='B')\n    \
                   c = Mark('p.q', delegate=True)\n    d = Mark()\n";
        let facts = analyze_with(src, None, &known, &TaintTable::default(), Scope::default());
        let class = &facts.classes[0];
        let [a, b, c, d] = class.fields.as_slice() else {
            panic!("{:?}", class.fields);
        };
        assert!(a.option("string").is_none() && a.comodel.is_none() && a.splat);
        assert!(b.option("string").is_some() && !b.splat);
        assert!(c.comodel.is_none() && !d.splat);
        assert!(class.inherits.is_empty());
        assert!(class.delegate_unknown);
    }

    #[test]
    fn a_subscripted_base_counts_as_the_class_it_subscripts() {
        let base =
            "from odoo import fields\nclass V(fields.Field[list]):\n    pass\nclass T(fields.Char[str]):\n    pass\n";
        let derived = "class Sub(V[int]):\n    pass\nclass Deep(pkg.Sub[int]):\n    pass\n";
        let expected = [pair("Deep", "V"), pair("Sub", "V"), pair("T", "Char"), pair("V", "V")];
        assert_eq!(classes(&[derived, base]), expected);
    }

    #[test]
    fn a_base_proves_only_through_an_import_of_the_orm() {
        for (head, base, ftype) in [
            ("from odoo import fields", "fields.Field", "V"),
            ("from odoo import fields", "fields.Serialized", "Serialized"),
            ("from odoo import fields as f", "f.Char", "Char"),
            ("from openerp import fields as f", "f.Field", "V"),
            ("from odoo.fields import Char", "Char", "Char"),
            ("from odoo.fields import Char as C", "C", "Char"),
            ("from odoo.fields import Field", "Field", "V"),
            ("import odoo", "odoo.fields.Many2one", "Many2one"),
            ("import openerp.fields", "openerp.fields.Text", "Text"),
            ("import odoo.fields as f", "f.Field", "V"),
        ] {
            let src = format!("{head}\nclass V({base}):\n    pass\n");
            assert_eq!(classes(&[src.as_str()]), [pair("V", ftype)], "{head} {base}");
        }
        for (head, base) in [
            ("from generators import Char", "Char"),
            ("", "Char"),
            ("", "fields.Field"),
            ("from wtforms import fields", "fields.Field"),
            ("from odoo.addons.web import fields", "fields.Char"),
            ("from odoo import fields", "fields.Unknown"),
            ("from odoo import fields", "fields.Char.sub"),
            ("from odoo import models", "models.Field"),
            ("from odooish import fields", "fields.Field"),
        ] {
            let src = format!("{head}\nclass V({base}):\n    pass\n");
            assert!(classes(&[src.as_str()]).is_empty(), "{head} {base}");
        }
    }

    #[test]
    fn a_model_class_or_one_without_a_base_is_no_field_class() {
        let facts = analyze(
            "from odoo import fields\nclass X(models.Model):\n    _name = 'x'\nclass Plain:\n    pass\n\
             class Named(fields.Field):\n    _name = 'y'\n",
            None,
        );
        assert!(facts.field_classes.is_empty());
    }

    #[test]
    fn reads_a_call_of_a_field_class_only_once_the_scan_knows_it() {
        let src = "class X(models.Model):\n    _name = 'x'\n    v = Vector(size=3)\n    w = pkg.Link('y', 'Label')\n    \
                   z = Other()\n    alias = _set\n    if a:\n        u = Vector()\n";
        let unknown = analyze(src, None);
        assert_eq!(unknown.attr_calls, ["Vector", "Link", "Other"]);
        assert!(unknown.classes[0].fields.is_empty());
        let known = FieldClasses::from_iter([pair("Vector", "Vector"), pair("Link", "Many2one")]);
        let facts = analyze_with(src, None, &known, &TaintTable::default(), Scope::default());
        let class = &facts.classes[0];
        let declared: Vec<_> = class
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.ftype.as_str()))
            .collect();
        assert_eq!(declared, [("v", "Vector"), ("w", "Many2one")]);
        assert_eq!(class.fields[1].comodel.as_ref().unwrap().value, "y");
        assert_eq!(class.methods, ["z", "alias", "u"]);
    }

    #[test]
    fn an_attribute_read_as_a_method_by_name_keeps_its_call_for_the_second_read() {
        let src = "class X(models.Model):\n    _name = 'x'\n    _table_query = Tag()\n    \
                   _table_sql = pkg.Link()\n    _check_company_domain = Other()\n";
        let facts = analyze(src, None);
        assert_eq!(facts.attr_calls, ["Tag", "Link", "Other"]);
        assert_eq!(
            facts.classes[0].methods,
            ["_table_query", "_table_sql", "_check_company_domain"]
        );
    }

    #[test]
    fn a_field_class_counts_for_the_rules_of_a_field() {
        let known = FieldClasses::from_iter([pair("Link", "Many2one"), pair("Tag", "Char")]);
        let src = "class X(models.Model):\n    _name = 'x'\n    a = Link('p.q', delegate=True)\n    \
                   b = Link('r.s', delegate=flag)\n    c = Tag(groups='base.group_user', default=fields.Date.today())\n    \
                   _table_query = Tag()\n";
        let facts = analyze_with(src, None, &known, &TaintTable::default(), Scope::default());
        let class = &facts.classes[0];
        assert_eq!(class.inherits, [("p.q".to_owned(), "a".to_owned())]);
        assert!(class.delegate_unknown);
        assert!(!class.methods.contains(&"_table_query".to_owned()));
        assert_eq!(facts.group_refs[0].id, "base.group_user");
        assert!(facts.diags.iter().any(|d| d.code == "OD104"));
    }

    #[test]
    fn a_nested_class_keeps_its_super_calls() {
        let facts = analyze(
            "class X(models.Model):\n    _name = 'x'\n    def write(self, vals):\n        \
             @super().decorator()\n        class Inner(super().base()):\n            \
             def write(self, vals):\n                return super().write(vals)\n        \
             class Own(models.Model):\n            _inherit = 'x'\n            \
             def _own(self):\n                return super()._own()\n        \
             def helper():\n            return super().in_function()\n",
            None,
        );
        let called = |class: &ClassFact| class.super_calls.iter().map(|c| c.value.clone()).collect::<Vec<_>>();
        assert_eq!(called(&facts.classes[0]), ["decorator", "base", "in_function"]);
        assert_eq!(called(&facts.classes[1]), ["_own"]);
        assert!(facts.classes[0].extended.is_empty());
        assert!(!facts.diags.iter().any(|d| d.code == "OD119"));
    }

    #[test]
    fn a_hasattr_test_guards_only_its_own_super_call() {
        let called = |body: &str| {
            let facts = analyze(
                &format!("class X(models.Model):\n    _name = 'x'\n    def m(self):\n{body}"),
                None,
            );
            let class = &facts.classes[0];
            class.super_calls.iter().map(|c| c.value.clone()).collect::<Vec<_>>()
        };
        let none: [&str; 0] = [];
        for silent in [
            "        if hasattr(super(), 'a'):\n            super().a()\n",
            "        if hasattr(super(X, self), 'a'):\n            super(X, self).a()\n",
            "        if x and hasattr(super(), 'a'):\n            super().a()\n",
            "        if x:\n            pass\n        elif hasattr(super(), 'a'):\n            super().a()\n",
            "        if hasattr(super(), 'a'):\n            def f():\n                super().a()\n",
            "        return super().a() if hasattr(super(), 'a') else None\n",
        ] {
            assert_eq!(called(silent), none, "{silent}");
        }
        for reported in [
            "        super().a()\n",
            "        if hasattr(super(), 'b'):\n            super().a()\n",
            "        if hasattr(super(), 'a'):\n            pass\n        super().a()\n",
            "        if hasattr(super(), 'a'):\n            pass\n        else:\n            super().a()\n",
            "        if hasattr(super(), 'a'):\n            pass\n        elif x:\n            super().a()\n",
            "        if hasattr(super(), 'a') or x:\n            super().a()\n",
            "        if not hasattr(super(), 'a'):\n            super().a()\n",
            "        if hasattr(self, 'a'):\n            super().a()\n",
            "        if hasattr(super(Y, self), 'a'):\n            super(X, self).a()\n",
            "        return None if hasattr(super(), 'a') else super().a()\n",
        ] {
            assert_eq!(called(reported), ["a"], "{reported}");
        }
    }

    #[test]
    fn a_guarded_super_call_still_extends_the_method() {
        let facts = analyze(
            "class X(models.Model):\n    _name = 'x'\n    def write(self, vals):\n        \
             if hasattr(super(), 'write'):\n            super().write(vals)\n",
            None,
        );
        let class = &facts.classes[0];
        assert!(class.super_calls.is_empty());
        assert_eq!(class.extended, ["write"]);
        assert!(facts.diags.iter().any(|d| d.code == "OD119"));
    }

    /// Whether `body`, a method of a class, looks up `x.y` outside a guard.
    fn looks_up(body: &str) -> bool {
        let lines: String = body.lines().map(|line| format!("        {line}\n")).collect();
        let src = format!("class T(models.Model):\n    _name = 't'\n    def m(self):\n{lines}");
        let facts = analyze(&src, None);
        assert!(facts.diags.iter().all(|d| d.code != "OD000"), "{body}");
        facts.env_models.iter().any(|m| m.value == "x.y")
    }

    #[test]
    fn reads_a_runtime_test_of_a_soft_dependency_as_a_guard() {
        for test in [
            "'x.y' in self.env",
            "'x.y' in self.env.registry",
            "'f' in self._fields",
            "self.env.get('x.y')",
            "self.env.get('x.y') is not None",
            "(m := self.env.get('x.y')) is not None",
            "hasattr(self, 'f')",
            "hasattr(record.partner_id, 'f')",
            "x and hasattr(self, 'f')",
            "'m' in self.env['ir.module.module']._installed()",
            "self.env['ir.module.module']._get('m').state == 'installed'",
            "self.env['ir.module.module']._get('m').state in ('installed',)",
            "self.env['ir.module.module']._get('m').state != 'uninstalled'",
            "self.env['ir.module.module']._get('m').state not in ('uninstalled',)",
            "self.env['ir.module.module'].search([('name', '=', 'm')]).state == 'installed'",
            "self.env['ir.module.module'].sudo().search([('name', '=', 'm'), ('state', '=', 'installed')])",
            "self.env['ir.module.module'].search([('state', 'in', ['installed', 'to upgrade'])])",
            "self.env['ir.module.module'].search(domain=[('state', 'in', ('installed',))])",
            "request.env['ir.module.module'].search_count([('state', '=', 'installed')])",
            "'f' in self.env['a.b']",
            "not 'x.y' not in self.env",
        ] {
            assert!(!looks_up(&format!("if {test}:\n    self.env['x.y']")), "{test}");
        }
        for test in [
            "x",
            "hasattr(super(), 'f')",
            "hasattr(super(T, self), 'f')",
            "hasattr(self, name)",
            "hasattr(self)",
            "hasattr(request, 'f')",
            "hasattr(http.request, 'f')",
            "hasattr(self, '_f')",
            "self.env['ir.model']._get('x.y')",
            "self._get('m')",
            "self.env['res.partner']",
            "self.env[name]",
            "'f' in self.env[name]",
            "'f' in mapping['x.y']",
            "name in self.env",
            "name in self._fields",
            "self.env.get(name)",
            "self._fields.get(name)",
            "name in self.env['ir.module.module']._installed()",
            "self.env['ir.module.module']",
            "self.env['ir.module.module']._get('m')",
            "self.env['ir.module.module']._installed()",
            "'m' in self.env['ir.module.module'].search([])",
            "bool('x.y' in self.env)",
            "any('x.y' in e for e in envs)",
            "{'a': 'x.y' in self.env}",
            "('x.y' in self.env) if y else z",
        ] {
            assert!(looks_up(&format!("if {test}:\n    self.env['x.y']")), "{test}");
        }
    }

    #[test]
    fn a_test_opens_only_the_branch_that_shows_the_thing_there() {
        for reported in [
            "'x.y' not in self.env",
            "'f' not in request.env['a.b']",
            "not hasattr(self, 'f')",
            "not 'x.y' in self.env",
            "'m' not in self.env['ir.module.module']._installed()",
            "self.env['ir.module.module']._get('m').state != 'installed'",
            "self.env['ir.module.module']._get('m').state == 'uninstalled'",
            "self.env.get('x.y') is None",
        ] {
            assert!(looks_up(&format!("if {reported}:\n    self.env['x.y']")), "{reported}");
            assert!(
                !looks_up(&format!("if {reported}:\n    pass\nelse:\n    self.env['x.y']")),
                "{reported}"
            );
        }
        for silent in [
            "if 'x.y' not in self.env:\n    pass\nelse:\n    self.env['x.y']",
            "if not hasattr(self, 'f'):\n    pass\nelif y:\n    self.env['x.y']",
            "if not hasattr(self, 'f'):\n    pass\nelif y:\n    pass\nelse:\n    self.env['x.y']",
            "z = self.env['x.y'] if 'x.y' in self.env else None",
            "z = None if 'x.y' not in self.env else self.env['x.y']",
            "z = 'x.y' in self.env and self.env['x.y']",
            "z = y and hasattr(self, 'f') and self.env['x.y']",
        ] {
            assert!(!looks_up(silent), "{silent}");
        }
        for reported in [
            "if 'x.y' in self.env:\n    pass\nelse:\n    self.env['x.y']",
            "if 'x.y' in self.env or y:\n    self.env['x.y']",
            "if self.env['ir.module.module'].search_count([]) == 0:\n    self.env['x.y']",
            "if not ('x.y' in self.env and y):\n    self.env['x.y']",
            "z = self.env['x.y'] if 'x.y' not in self.env else None",
            "z = None if 'x.y' in self.env else self.env['x.y']",
            "z = 'x.y' not in self.env and self.env['x.y']",
            "z = not hasattr(self, 'f') and self.env['x.y']",
        ] {
            assert!(looks_up(reported), "{reported}");
        }
    }

    #[test]
    fn a_module_on_its_way_out_is_still_loaded() {
        let test = "self.env['ir.module.module']._get('m').state in ('installed', 'to upgrade', 'to remove')";
        assert!(!looks_up(&format!("if {test}:\n    self.env['x.y']")));
    }

    #[test]
    fn a_module_shows_itself_there_only_through_its_state() {
        let get = "self.env['ir.module.module']._get('m')";
        for test in [
            "state == 'uninstallable'",
            "state == 'to remove'",
            "state == 'to upgrade'",
            "state in ('to install', 'to upgrade')",
            "state in ('installed', 'to install')",
            "state in SOME_VARIABLE",
            "state == SOME_VARIABLE",
            "state in 'installed'",
            "state in ('installed', y)",
            "name == 'x'",
            "name in ('x', 'y')",
        ] {
            let test = format!("{get}.{test}");
            assert!(looks_up(&format!("if {test}:\n    self.env['x.y']")), "{test}");
            assert!(looks_up(&format!("if {test}:\n    return\nself.env['x.y']")), "{test}");
            assert!(
                looks_up(&format!("if {test}:\n    pass\nelse:\n    self.env['x.y']")),
                "{test}"
            );
        }
        let absent = [
            "m.state == 'uninstalled'",
            "m.state != 'installed'",
            "m.state in ('uninstalled', 'to remove')",
            "m.state in {'uninstalled', 'to remove'}",
            "m.state in ['uninstalled']",
            "m.state not in ('installed',)",
        ];
        for test in absent {
            assert!(
                looks_up(&format!("m = {get}\nif {test}:\n    self.env['x.y']")),
                "{test}"
            );
            assert!(
                !looks_up(&format!("m = {get}\nif {test}:\n    return\nself.env['x.y']")),
                "{test}"
            );
        }
        let both = format!("m = {get}\nif m and m.state == 'uninstalled':\n    self.env['x.y']");
        assert!(looks_up(&both));
        let present = format!("m = {get}\nif m and m.state == 'installed':\n    self.env['x.y']");
        assert!(!looks_up(&present));
        let through_a_holder =
            "m = self.env['ir.module.module']\nif m._get('m').state == 'installed':\n    self.env['x.y']";
        assert!(!looks_up(through_a_holder));
    }

    #[test]
    fn a_search_of_modules_shows_them_there_only_when_its_domain_asks_for_installed_ones() {
        let search = "self.env['ir.module.module'].search";
        for domain in [
            "",
            "[]",
            "domain",
            "[('name', '=', 'm')]",
            "[('state', '=', 'uninstalled')]",
            "[('name', '=', 'm'), ('state', '=', 'uninstalled')]",
            "[('state', '!=', 'installed')]",
            "[('state', 'not in', ['installed'])]",
            "[('state', 'in', ['uninstalled', 'to remove'])]",
            "[('state', '=', state)]",
            "[('state', 'in', states)]",
            "['|', ('state', '=', 'installed'), ('name', '=', 'm')]",
            "['!', ('state', '=', 'installed')]",
            "[term, ('state', '=', 'installed')]",
        ] {
            assert!(
                looks_up(&format!("if {search}({domain}):\n    self.env['x.y']")),
                "{domain}"
            );
        }
        let domain = "[('name', '=', 'm'), ('state', '=', 'uninstalled')]";
        assert!(looks_up(&format!(
            "if {search}({domain}):\n    return\nself.env['x.y']"
        )));
        let absent = format!("if {search}([('state', '=', 'installed')]):\n    return\nself.env['x.y']");
        assert!(looks_up(&absent));
        let elsewhere = "if self.env['res.partner'].search([('state', '=', 'installed')]):\n    self.env['x.y']";
        assert!(looks_up(elsewhere));
        let left_without = format!("if not {search}([('state', '=', 'installed')]):\n    return\nself.env['x.y']");
        assert!(!looks_up(&left_without));
    }

    #[test]
    fn a_test_that_fails_when_the_thing_is_absent_guards_the_rest_of_the_block() {
        for silent in [
            "if 'x.y' not in self.env:\n    self.skipTest('m')\nself.env['x.y']",
            "if 'x.y' not in cls.env:\n    cls.skipTest(cls, 'm')\ncls.env['x.y']",
            "if 'x.y' not in self.env:\n    return\nself.env['x.y']",
            "if 'x.y' not in self.env.registry:\n    return\nself.env['x.y']",
            "if not hasattr(self, 'f'):\n    raise E\nself.env['x.y']",
            "if self.env['ir.module.module']._get('m').state != 'installed':\n    return\nself.env['x.y']",
            "if self.env['ir.module.module']._get('m').state == 'uninstalled':\n    return\nself.env['x.y']",
            "if self.env.get('x.y') is None:\n    return\nself.env['x.y']",
            "if y or 'x.y' not in self.env:\n    return\nself.env['x.y']",
            "if not (y and 'x.y' in self.env):\n    return\nself.env['x.y']",
            "if 'x.y' not in self.env and 'f' not in self._fields:\n    return\nself.env['x.y']",
            "self.ensure_installed('m')\nself.env['x.y']",
            "cls.ensure_installed('m')\ncls.env['x.y']",
            "if a:\n    self.ensure_installed('m')\n    self.env['x.y']",
        ] {
            assert!(!looks_up(silent), "{silent}");
        }
        for reported in [
            "if x:\n    self.skipTest('m')\nself.env['x.y']",
            "if 'x.y' in self.env:\n    self.skipTest('m')\nself.env['x.y']",
            "if 'x.y' in self.env:\n    return\nself.env['x.y']",
            "if hasattr(self, 'f'):\n    return\nself.env['x.y']",
            "if self.env['ir.module.module']._get('m').state == 'installed':\n    return\nself.env['x.y']",
            "if y and 'x.y' not in self.env:\n    return\nself.env['x.y']",
            "if y and 'x.y' in self.env:\n    return\nself.env['x.y']",
            "if y or 'x.y' in self.env:\n    return\nself.env['x.y']",
            "if 'x.y' not in self.env:\n    pass\nself.env['x.y']",
            "if 'x.y' not in self.env:\n    self.other('m')\nself.env['x.y']",
            "self.env['x.y']\nif 'x.y' not in self.env:\n    self.skipTest('m')",
            "if a:\n    if 'x.y' not in self.env:\n        self.skipTest('m')\nself.env['x.y']",
            "ensure_installed('m')\nself.env['x.y']",
            "self.helper.ensure_installed('m')\nself.env['x.y']",
            "case.ensure_installed('m')\nself.env['x.y']",
            "try:\n    self.ensure_installed('m')\nexcept E:\n    pass\nself.env['x.y']",
            "self.ensure_installed(name)\nself.env['x.y']",
            "self.ensure_installed()\nself.env['x.y']",
            "self.env['x.y']\nself.ensure_installed('m')",
            "if a:\n    self.ensure_installed('m')\nself.env['x.y']",
        ] {
            assert!(looks_up(reported), "{reported}");
        }
    }

    #[test]
    fn a_try_guards_the_rest_when_it_looks_up_what_it_needs_and_every_handler_skips() {
        for silent in [
            "try:\n    self.env.ref('m.r')\nexcept ValueError:\n    self.skipTest('m')\nself.env['x.y']",
            "try:\n    z = self.env[name]\nexcept KeyError:\n    self.skipTest('m')\nself.env['x.y']",
            "try:\n    self.env.ref('m.r')\nexcept ValueError:\n    self.skipTest('m')\nexcept KeyError:\n    \
             self.skipTest('n')\nself.env['x.y']",
            "try:\n    self.env['x.y']\nexcept KeyError:\n    self.skipTest('m')",
            "try:\n    self.env['x.y']\nexcept (KeyError, ValueError):\n    self.skipTest('m')",
            "try:\n    self.env['x.y']\nexcept Exception:\n    self.skipTest('m')",
            "try:\n    self.env['x.y']\nexcept:\n    self.skipTest('m')",
            "try:\n    self.env.ref('m.r')\n    self.env['x.y']\nexcept ValueError:\n    self.skipTest('m')",
            "try:\n    self.env.ref('m.r')\nexcept ValueError:\n    self.skipTest('m')\nelse:\n    self.env['x.y']",
        ] {
            assert!(!looks_up(silent), "{silent}");
        }
        for reported in [
            "try:\n    a()\nexcept ValueError:\n    pass\nself.env['x.y']",
            "try:\n    requests.get(url)\nexcept ConnectionError:\n    self.skipTest('m')\nself.env['x.y']",
            "try:\n    self.env.ref('m.r')\nexcept ValueError:\n    b()\nexcept KeyError:\n    \
             self.skipTest('m')\nself.env['x.y']",
            "try:\n    self.env.ref('m.r')\nfinally:\n    b()\nself.env['x.y']",
            "try:\n    self.env['x.y']\nexcept KeyError:\n    pass",
            "try:\n    self.env['x.y']\nexcept ValueError:\n    self.skipTest('m')",
            "try:\n    self.env['x.y']\nexcept requests.ConnectionError:\n    self.skipTest('m')",
            "try:\n    self.env.ref('m.r')\nexcept KeyError:\n    self.skipTest('m')\nself.env['x.y']",
            "try:\n    self.env.ref('m.r')\nexcept ValueError:\n    self.env['x.y']\n    self.skipTest('m')",
            "try:\n    self.env.ref('m.r')\nexcept ValueError:\n    self.skipTest('m')\nfinally:\n    self.env['x.y']",
            "try:\n    self.env.ref('m.r', raise_if_not_found=False)\nexcept ValueError:\n    self.skipTest('m')\n\
             self.env['x.y']",
        ] {
            assert!(looks_up(reported), "{reported}");
        }
    }

    #[test]
    fn a_ref_behind_a_guard_is_kept_apart() {
        let ids = |refs: &[XmlRef]| refs.iter().map(|r| r.id.clone()).collect::<Vec<_>>();
        let facts = analyze(
            "class T(models.Model):\n    _name = 't'\n    def m(self):\n        \
             self.env.ref('a.before')\n        \
             if 'x.y' in self.env:\n            self.env.ref('a.opened')\n        \
             try:\n            self.env.ref('a.tried')\n        \
             except ValueError:\n            self.env.ref('a.caught')\n            self.skipTest('m')\n        \
             self.env.ref('a.after')\n",
            None,
        );
        assert_eq!(ids(&facts.xml_refs), ["a.before", "a.caught"]);
        let guarded: Vec<_> = facts.guarded_refs.into_iter().map(|g| g.xml_ref).collect();
        assert_eq!(ids(&guarded), ["a.opened", "a.tried", "a.after"]);
    }

    #[test]
    fn a_guard_names_what_it_proves() {
        let proven = |guard: &str| {
            let indent = if guard.ends_with(':') { "    " } else { "" };
            let lines: String = format!("{guard}\n{indent}self.env.ref('z.r')")
                .lines()
                .map(|line| format!("        {line}\n"))
                .collect();
            let src = format!("class T(models.Model):\n    _name = 't'\n    def m(self):\n{lines}");
            let mut facts = analyze(&src, None);
            assert!(facts.xml_refs.is_empty(), "{guard}");
            let names = |s: &Subject| match s {
                Subject::Module(m) => format!("module {m}"),
                Subject::Model(m) => format!("model {m}"),
            };
            let proven = facts.guarded_refs.pop().unwrap().proven;
            proven.map(|p| p.iter().map(names).collect::<Vec<_>>().join(", "))
        };
        let module_state = "self.env['ir.module.module']";
        for (guard, names) in [
            ("self.ensure_installed('m')", "module m"),
            ("cls.ensure_installed('m')", "module m"),
            (
                &format!("if {module_state}._get('m').state != 'installed':\n    return"),
                "module m",
            ),
            (
                &format!("if {module_state}.sudo()._get('m').state == 'installed':"),
                "module m",
            ),
            (
                &format!("if {module_state}.search([('name', '=', 'm')]).state == 'uninstalled':\n    return"),
                "module m",
            ),
            (
                &format!("if {module_state}.search_count([('name', 'in', ['m', 'n']), ('state', '=', 'installed')]):"),
                "module m, module n",
            ),
            (
                &format!("if 'm' not in {module_state}._installed():\n    self.skipTest('m')"),
                "module m",
            ),
            (
                "try:\n    self.env.ref('m.r')\nexcept ValueError:\n    self.skipTest('m')",
                "module m",
            ),
            ("if 'x.y' in self.env:", "model x.y"),
            ("if 'x.y' not in self.env.registry:\n    return", "model x.y"),
            ("if 'x.y' in self.pool:", "model x.y"),
            ("if self.env.get('x.y'):", "model x.y"),
            ("if (m := self.env.get('x.y')) is not None:", "model x.y"),
            (
                "try:\n    self.env['x.y']\nexcept KeyError:\n    self.skipTest('m')",
                "model x.y",
            ),
            (
                "try:\n    self.env.ref('n.r', raise_if_not_found=False)\n    self.env.ref('m.r')\n\
                 except ValueError:\n    self.skipTest('m')",
                "module m",
            ),
            (
                "try:\n    if y:\n        self.env.ref('n.r')\n    self.env.ref('m.r') or self.env.ref('n.s')\n\
                 except ValueError:\n    self.skipTest('m')",
                "module m",
            ),
            (
                "try:\n    try:\n        self.env.ref('n.r')\n    except ValueError:\n        pass\n\
                 \x20   self.env['x.y']\nexcept (KeyError, ValueError):\n    self.skipTest('m')",
                "model x.y",
            ),
            (
                "try:\n    with y:\n        self.env.ref('n.r')\n    [self.env.ref(r) for r in y]\n\
                 except ValueError:\n    self.skipTest('m')",
                "",
            ),
            (
                "self.ensure_installed('m')\nif y and 'x.y' in self.env:",
                "module m, model x.y",
            ),
            (
                "if 'x.y' in self.env and 'm' in self.env['ir.module.module']._installed():",
                "model x.y, module m",
            ),
        ] {
            assert_eq!(proven(guard).as_deref(), Some(names), "{guard}");
        }
        for guard in [
            "if hasattr(self, 'f'):",
            "if 'f' in self._fields:",
            "if 'f' in self.env['a.b']:",
            "a = 'x.y' in self.env\nif a:",
            "a = self.env['ir.module.module']._get('m')\nif a.state != 'installed':\n    return",
            &format!("if {module_state}.search([('state', '=', 'installed')]):"),
            &format!("if {module_state}._get(name).state == 'installed':"),
            "try:\n    self.env.ref(RECORD)\nexcept ValueError:\n    self.skipTest('m')",
            "try:\n    self.env[name]\nexcept KeyError:\n    self.skipTest('m')",
            "self.ensure_installed('m')\nif hasattr(self, 'f'):",
        ] {
            assert_eq!(proven(guard), None, "{guard}");
        }
    }

    #[test]
    fn a_value_that_holds_a_guard_guards_like_the_guard() {
        for silent in [
            "a = 'x.y' in self.env\nif a:\n    self.env['x.y']",
            "a: bool = 'x.y' in self.env\nif a:\n    self.env['x.y']",
            "a = 'x.y' in self.env\nb = a and y\nif b:\n    self.env['x.y']",
            "a = 'x.y' in self.env\nif y and a:\n    self.env['x.y']",
            "a = 'x.y' in self.env\nif not a:\n    return\nself.env['x.y']",
            "a = 'x.y' not in self.env\nif a:\n    return\nself.env['x.y']",
            "a = 'x.y' not in self.env\nif not a:\n    self.env['x.y']",
            "a = 'x.y' not in self.env\nb = a or y\nif b:\n    return\nself.env['x.y']",
            "a = self.env['ir.module.module']._get('m')\nif a.state != 'installed':\n    return\nself.env['x.y']",
            "a = self.env['ir.module.module'].sudo()\nif a.search([('state', '=', 'installed')]):\n    self.env['x.y']",
            "a = self.env['ir.module.module'].search([('state', '=', 'installed')])\nif a:\n    self.env['x.y']",
            "a = self.env['ir.module.module']._installed()\nif 'm' in a:\n    self.env['x.y']",
            "a = False\nif y:\n    a = 'x.y' in self.env\nif a:\n    self.env['x.y']",
            "self.a = 'x.y' in self.env\nif self.a:\n    self.env['x.y']",
            "self.a = 'x.y' not in self.env\nif self.a:\n    return\nself.env['x.y']",
            "self.a = 'x.y' in self.env\nself.a = False\nif self.a:\n    self.env['x.y']",
            "self.a = self.env['ir.module.module']._installed()\nif 'm' in self.a:\n    self.env['x.y']",
            "b = 'x.y' in self.env\ncls.a = b\nif self.a:\n    self.env['x.y']",
            "a = 'x.y' in self.env\ndef f():\n    if a:\n        self.env['x.y']",
            "a = 'x.y' in self.env\nif a:\n    self.env['x.y']\na = y",
            "a = 'x.y' in self.env\ndef f():\n    a = y\nif a:\n    self.env['x.y']",
        ] {
            assert!(!looks_up(silent), "{silent}");
        }
        for reported in [
            "a = y\nif a:\n    self.env['x.y']",
            "self.a = y\nif self.a:\n    self.env['x.y']",
            "other.a = 'x.y' in self.env\nif other.a:\n    self.env['x.y']",
            "a = 'x.y' in self.env\na = y\nif a:\n    self.env['x.y']",
            "a = 'x.y' in self.env\na += 1\nif a:\n    self.env['x.y']",
            "a = 'x.y' in self.env\na, b = y\nif a:\n    self.env['x.y']",
            "a = 'x.y' in self.env\nfor a in y:\n    if a:\n        self.env['x.y']",
            "a = y\nif a:\n    self.env['x.y']\na = 'x.y' in self.env",
            "a = 'x.y' in self.env\nif self.a:\n    self.env['x.y']",
            "self.a = 'x.y' in self.env\nif a:\n    self.env['x.y']",
            "a = 'x.y' not in self.env\nif a:\n    self.env['x.y']",
            "a = 'x.y' not in self.env\nif not a:\n    return\nself.env['x.y']",
            "a = hasattr(self, 'f') or y\nif a:\n    self.env['x.y']",
            "self.a = 'x.y' not in self.env\nif self.a:\n    self.env['x.y']",
            "a = self.env['ir.module.module']._get('m')\nif a:\n    self.env['x.y']",
            "a = self.env['ir.module.module'].search([('name', '=', 'm')])\nif a:\n    self.env['x.y']",
            "a = self.env['ir.module.module']._get('m')\nif not a:\n    return\nself.env['x.y']",
            "a = self.env['ir.module.module']._get('m')\nif 'm' in a:\n    self.env['x.y']",
            "a = self.env['ir.module.module']._installed()\nif a:\n    self.env['x.y']",
            "a = self.env['ir.module.module']._installed()\nif a.state == 'installed':\n    self.env['x.y']",
            "a = self._fields.get(name)\nif a:\n    self.env['x.y']",
            "a = self._fields.get(name)\nif not a:\n    raise E\nself.env['x.y']",
            "a = [n for n in y if 'x.y' in self.env]\nif a:\n    self.env['x.y']",
            "a = {'b': 'x.y' in self.env}\nif a:\n    self.env['x.y']",
            "a = b('x.y' in self.env)\nif a:\n    self.env['x.y']",
            "a = y if 'x.y' in self.env else z\nif a:\n    self.env['x.y']",
            "def f():\n    a = 'x.y' in self.env\nif a:\n    self.env['x.y']",
        ] {
            assert!(looks_up(reported), "{reported}");
        }
    }

    #[test]
    fn a_holder_stays_in_its_scope() {
        let reached = |src: &str| analyze(src, None).env_models.iter().any(|m| m.value == "x.y");
        let across_methods = "class T:\n    def a(self):\n        self.f = 'x.y' in self.env\n    \
                              def b(self):\n        if self.f:\n            self.env['x.y']\n";
        assert!(!reached(across_methods));
        let local = "class T:\n    def a(self):\n        f = 'x.y' in self.env\n    \
                     def b(self, f):\n        if f:\n            self.env['x.y']\n";
        assert!(reached(local));
        let other_class = "class S:\n    def a(self):\n        self.f = 'x.y' in self.env\n\
                           class T:\n    def b(self):\n        if self.f:\n            self.env['x.y']\n";
        assert!(reached(other_class));
        let nested_class = "class S:\n    class N:\n        def a(self):\n            self.f = 'x.y' in self.env\n    \
                            def b(self):\n        if self.f:\n            self.env['x.y']\n";
        assert!(reached(nested_class));
        let outside_a_class = "def a(self):\n    self.f = 'x.y' in self.env\n\
                               def b(self):\n    if self.f:\n        self.env['x.y']\n";
        assert!(reached(outside_a_class));
        let cleared_in_a_method = "class T:\n    def a(self):\n        self.f = 'x.y' in self.env\n    \
                                   def b(self):\n        self.f = False\n    \
                                   def c(self):\n        if self.f:\n            self.env['x.y']\n";
        assert!(!reached(cleared_in_a_method));
    }

    #[test]
    fn a_for_block_binds_no_method() {
        assert!(method_names("    for a in b:\n        def in_for(self):\n            pass\n").is_empty());
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
    fn counts_the_osv_aliases_as_model_bases() {
        for (base, kind) in [
            ("osv.osv", ModelKind::Model),
            ("osv", ModelKind::Model),
            ("osv.osv_memory", ModelKind::Transient),
            ("osv.osv_abstract", ModelKind::Abstract),
        ] {
            let facts = analyze(&format!("class X({base}):\n    _name = 'x'\n"), None);
            let class = &facts.classes[0];
            assert_eq!(class.kind, kind, "{base}");
            assert!(class.plain_bases.is_empty(), "{base}");
        }
    }

    #[test]
    fn counts_cached_model_as_a_model_base() {
        for base in ["models.CachedModel", "CachedModel"] {
            let facts = analyze(&format!("class X({base}):\n    _name = 'x'\n"), None);
            let class = &facts.classes[0];
            assert_eq!(class.kind, ModelKind::Model, "{base}");
            assert!(class.plain_bases.is_empty(), "{base}");
        }
        let facts = analyze("class X(models.CachedModel, Layout):\n    _name = 'x'\n", None);
        assert_eq!(facts.classes[0].plain_bases, ["Layout"]);
    }

    #[test]
    fn names_a_plain_base_only_when_written_as_a_bare_name() {
        let facts = analyze(
            "class X(models.Model, Layout, mixins.Layout, Layout()):\n    _name = 'x'\n",
            None,
        );
        assert_eq!(facts.classes[0].plain_bases, ["Layout", "", ""]);
    }

    #[test]
    fn keeps_a_class_of_the_file_that_holds_no_field_from_the_opaque_bases() {
        let class = "class X(models.Model, Layout):\n    _name = 'x'\n";
        for (layout, methods) in [
            ("class Layout:\n    pass\n", &[][..]),
            (
                "class Layout(object):\n    \"\"\"Doc.\"\"\"\n    __slots__ = ()\n    kind: str\n    def a(self):\n        pass\n",
                &["a"],
            ),
            ("class Layout:\n    ...\n", &[]),
            ("class Layout:\n    pass\nclass Layout:\n    size = 1\n", &["size"]),
        ] {
            let facts = analyze(&format!("{layout}{class}"), None);
            assert_eq!(facts.classes[0].plain_bases, ["Layout"], "{layout}");
            assert!(facts.classes[0].opaque_bases.is_empty(), "{layout}");
            assert_eq!(facts.classes[0].base_methods, methods, "{layout}");
        }
    }

    #[test]
    fn keeps_a_plain_base_that_may_hold_a_field_in_the_opaque_bases() {
        let class = "class X(models.Model, Layout):\n    _name = 'x'\n";
        for layout in [
            "",
            "from .mixins import Layout\n",
            "class Layout:\n    name = fields.Char()\n",
            "class Layout:\n    name = MADE\n",
            "class Layout:\n    if MADE:\n        name = fields.Char()\n",
            "class Layout:\n    _columns = {'name': MADE}\n",
            "class Layout:\n    _inherit = 'res.partner'\n",
            "class Layout(Other):\n    pass\n",
            "class Layout(metaclass=Made):\n    pass\n",
            "@made\nclass Layout:\n    pass\n",
            "if MADE:\n    class Layout:\n        pass\n",
            "try:\n    from .mixins import Layout\nexcept ImportError:\n    class Layout:\n        pass\n",
            "class Layout:\n    pass\nfrom .mixins import Layout\n",
            "class Layout:\n    pass\nLayout = make(Layout)\n",
            "class Layout:\n    pass\nclass Layout:\n    name = fields.Char()\n",
        ] {
            let facts = analyze(&format!("{layout}{class}"), None);
            let class = facts.classes.last().unwrap();
            assert_eq!(class.opaque_bases, ["Layout"], "{layout}");
            assert!(class.base_methods.is_empty(), "{layout}");
        }
    }

    #[test]
    fn reads_a_class_of_the_file_below_the_class_as_an_opaque_base() {
        let facts = analyze(
            "class X(models.Model, Layout):\n    _name = 'x'\nclass Layout:\n    pass\n",
            None,
        );
        assert_eq!(facts.classes[0].opaque_bases, ["Layout"]);
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
    fn marks_a_many2one_that_may_delegate_to_an_unread_parent() {
        let unknown = |field: &str| {
            let facts = analyze(
                &format!("class X(models.Model):\n    _name = 'x'\n    y_id = {field}\n"),
                None,
            );
            facts.classes[0].delegate_unknown
        };
        for field in [
            "fields.Many2one(PARTNER, delegate=True)",
            "fields.Many2one(comodel_name=PARTNER, delegate=True)",
            "fields.Many2one('res.partner', delegate=DELEGATE)",
            "fields.Many2one('res.partner', delegate=1)",
            "fields.Many2one('res.partner', **OPTIONS)",
        ] {
            assert!(unknown(field), "{field}");
        }
        for field in [
            "fields.Many2one('res.partner', delegate=True)",
            "fields.Many2one('res.partner', delegate=False)",
            "fields.Many2one('res.partner', delegate=None)",
            "fields.Many2one('res.partner')",
            "fields.Char(delegate=True)",
            "fields.Many2many('res.partner', delegate=True)",
        ] {
            assert!(!unknown(field), "{field}");
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
    fn reads_the_keys_of_a_literal_columns_as_fields_of_unread_type() {
        let src = "class X(orm.Model):
    _name = 'x'
    _columns = {
        'a': fields.char('A'),
        u'b': fields.function(_b, type='many2one', relation='y'),
    }
";
        let facts = analyze(src, Series::parse("9.0"));
        let class = &facts.classes[0];
        let names: Vec<&str> = class.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        assert!(!class.inherit_unknown);
        for field in &class.fields {
            assert_eq!(field.ftype, UNREAD_TYPE);
            assert!(field.splat && field.comodel.is_none() && field.options.is_empty());
        }
        assert_eq!((class.fields[0].name_pos.line, class.fields[1].name_pos.line), (4, 5));
    }

    #[test]
    fn reads_columns_below_ten_and_in_an_unknown_series() {
        let keys = |series: Option<&str>| {
            let src = "class X(orm.Model):\n    _name = 'x'\n    _columns = {'a': fields.char('A')}\n";
            analyze(src, series.and_then(Series::parse)).classes[0].fields.len()
        };
        for (series, want) in [
            (Some("9.0"), 1),
            (Some("saas~9.3"), 1),
            (None, 1),
            (Some("10.0"), 0),
            (Some("18.0"), 0),
        ] {
            assert_eq!(keys(series), want, "{series:?}");
        }
    }

    #[test]
    fn marks_a_columns_that_hides_a_key_unknown_on_either_side_of_inherit() {
        let unknown = |columns: &str, series: &str| {
            [
                format!("    _columns = {columns}\n    _inherit = 'a'\n"),
                format!("    _inherit = 'a'\n    _columns = {columns}\n"),
            ]
            .map(|body| {
                analyze(&format!("class X(orm.Model):\n{body}"), Series::parse(series)).classes[0].inherit_unknown
            })
        };
        for columns in [
            "COLUMNS",
            "dict(COLUMNS)",
            "{**COLUMNS}",
            "{'a': 1, **COLUMNS}",
            "{KEY: 1}",
            "{'a' + 'b': 1}",
            "{f'{a}': 1}",
        ] {
            assert_eq!(unknown(columns, "9.0"), [true, true], "{columns}");
            assert_eq!(unknown(columns, "10.0"), [false, false], "{columns}");
        }
        for columns in ["{}", "{'a': MADE}", "{'a' 'b': MADE}"] {
            assert_eq!(unknown(columns, "9.0"), [false, false], "{columns}");
        }
    }

    #[test]
    fn a_columns_that_hides_a_key_leaves_the_name_to_inherit() {
        let src = "class X(orm.Model):\n    _inherit = ['a', 'b']\n    _columns = COLUMNS\n";
        let facts = analyze(src, Series::parse("9.0"));
        assert_eq!(facts.classes[0].model, "X");
        assert!(facts.classes[0].inherit_unknown && !facts.dynamic_models);
    }

    /// The facts of a file that ends in a Python 2 `print`: unread, with OD000 when `body` is not
    /// Python 2 either.
    fn unparsed(body: &str) -> PyFacts {
        let facts = analyze(&format!("{body}\nprint 'x'\n"), Series::parse("10.0"));
        assert_eq!(facts.diags.len() + usize::from(facts.python2.is_some()), 1);
        facts
    }

    /// The models a file that stops at a Python 2 `print` names: (model, new_model) of each class.
    fn unread(body: &str) -> (Vec<(String, bool)>, bool, Vec<String>) {
        let facts = unparsed(body);
        assert!(
            facts
                .classes
                .iter()
                .all(|c| c.unread && c.inherit_unknown && c.inherit.is_empty())
        );
        let models = facts.classes.iter().map(|c| (c.model.clone(), c.new_model)).collect();
        (models, facts.dynamic_models, facts.built_model_starts)
    }

    #[test]
    fn a_file_that_does_not_parse_still_gives_the_table() {
        let table = |body: &str| unparsed(body).classes.remove(0).table;
        let literal = Some(OptionValue::Str("t".to_owned()));
        assert_eq!(
            table("class X(models.Model):\n    _name = 'a'\n    _table = 't'"),
            literal
        );
        assert_eq!(
            table("class X(models.Model):\n    _table = 't'\n    _name = 'a'"),
            literal
        );
        assert_eq!(table("class X(models.Model):\n    _name = _table = 't'"), literal);
        assert_eq!(
            table("class X(models.Model):\n    _inherit = 'a'\n    _table = 't'"),
            literal
        );
        let computed = Some(OptionValue::Other);
        assert_eq!(
            table("class X(models.Model):\n    _name = 'a'\n    _table = TABLE"),
            computed
        );
        assert_eq!(
            table("class X(models.Model):\n    _name = 'a'\n    _table = 1 +"),
            computed
        );
        assert_eq!(table("class X(models.Model):\n    _name = 'a'"), None);
    }

    #[test]
    fn a_file_that_does_not_parse_still_names_its_models() {
        let (a, b) = (("a".to_owned(), false), ("b".to_owned(), false));
        let new = ("a".to_owned(), true);
        let own = ("X".to_owned(), true);
        let cases = [
            ("class X(models.Model):\n    _name = 'a'", vec![new.clone()]),
            ("class X(models.Model):\n    _name = u\"a\"  # why", vec![new.clone()]),
            ("class X(osv.osv):\n\n    # why\n    _name = 'a'", vec![new.clone()]),
            ("class X(models.Model):\n    _inherit = 'a'", vec![a.clone()]),
            ("class X(models.Model):\n    _inherit = u'a'", vec![a.clone()]),
            ("class X(models.Model):\n    _inherit = ['a']", vec![a.clone()]),
            ("class X(models.Model):\n    _inherit = ['a', 'b']", vec![own.clone()]),
            ("class X(models.Model):\n    _inherit = ('a', 'b')", vec![own.clone()]),
            (
                "class X(models.Model):\n    _inherit = [\n        'a',  # why\n        'b',\n    ]",
                vec![own.clone()],
            ),
            (
                "class X(models.Model):\n    _name = _description = 'a'",
                vec![new.clone()],
            ),
            (
                "class X(models.Model):\n    _description = _name = 'a'",
                vec![new.clone()],
            ),
            ("class X(models.Model):\n    _name = 'a';", vec![new.clone()]),
            (
                "class X(models.Model):\n    _name = 'a'; _inherit = 'a'",
                vec![a.clone()],
            ),
            (
                "class X(models.Model):\n        x = 1\n\t_name = 'a'",
                vec![new.clone()],
            ),
            ("class X(object):\n    _name = 'a'", vec![new.clone()]),
            (
                "class X(models.Model):\n    _name = 'a'\n    _inherit = 'b'",
                vec![new.clone()],
            ),
            (
                "class X(models.Model):\n    _inherit = 'b'\n    _name = 'a'",
                vec![new.clone()],
            ),
            (
                "class X(models.Model):\n    _name = 'a'\n    _inherit = 'a'",
                vec![a.clone()],
            ),
            (
                "class X(models.Model):\n    _name = 'a'\n    _inherit = ['a', 'b']",
                vec![a.clone()],
            ),
            (
                "class X(models.Model):\n    _name = 'a'\n    _inherit = _name",
                vec![a.clone()],
            ),
            (
                "class X(models.Model):\n    _name = 'a'\n\n\nclass Y(models.Model):\n    _inherit = 'b'",
                vec![new.clone(), b.clone()],
            ),
            (
                "class X(models.Model):\n    class Meta:\n        _name = 'b'\n    _name = 'a'",
                vec![new.clone(), ("b".to_owned(), true)],
            ),
        ];
        for (body, models) in cases {
            assert_eq!(unread(body), (models, false, vec![]), "{body}");
        }
    }

    #[test]
    fn a_file_that_does_not_parse_reads_only_class_attributes() {
        for body in [
            "import os",
            "_name = 'a'",
            "class X(models.Model):\n    name = fields.Char()",
            "class X(models.Model):\n    def f(self):\n        _name = 'a'",
            "class X(models.Model):\n    if True:\n        _name = 'a'",
            "class X(models.Model):\n    pass\n\n\ndef f():\n    _inherit = 'a'",
            "class X(models.Model):\n    x = '_name = 1'",
            "class X(models.Model):\n    _inherit = []",
        ] {
            assert_eq!(unread(body), (vec![], false, vec![]), "{body}");
        }
    }

    #[test]
    fn a_file_that_does_not_parse_builds_what_it_hides() {
        let hidden = |models: Vec<(String, bool)>, start: &str| (models, true, vec![start.to_owned()]);
        let a = ("a".to_owned(), false);
        let cases = [
            ("class X(models.Model):\n    _name = MODEL", hidden(vec![], "")),
            (
                "class X(models.Model):\n    _name = 'a.%s' % KIND",
                hidden(vec![], "a."),
            ),
            ("class X(models.Model):\n    _name = print x", hidden(vec![], "")),
            ("class X(models.Model):\n    _inherit = PARENT", hidden(vec![], "")),
            ("class X(models.Model):\n    _inherit = _name", hidden(vec![], "")),
            (
                "class X(models.Model):\n    _inherit = ['a', PARENT]",
                hidden(vec![a.clone()], ""),
            ),
            ("class X(models.Model):\n    _inherit = ['a'", hidden(vec![], "")),
            ("class X(models.Model):\n    _name = x.y = 'a'", hidden(vec![], "")),
        ];
        for (body, expected) in cases {
            assert_eq!(unread(body), expected, "{body}");
        }
    }

    #[test]
    fn a_file_that_does_not_parse_lets_name_settle_what_inherit_hides() {
        let new = ("a".to_owned(), true);
        let hidden = "_inherit = ['mail.thread', MIXIN]";
        let cases = [
            (format!("_name = 'a'\n    {hidden}"), vec![new.clone()], false, vec![]),
            (format!("{hidden}\n    _name = 'a'"), vec![new.clone()], false, vec![]),
            (
                "_name = 'a'\n    _inherit = PARENT".to_owned(),
                vec![new],
                false,
                vec![],
            ),
            (
                "_name = 'a.%s' % KIND\n    _inherit = PARENT".to_owned(),
                vec![],
                true,
                vec!["a.".to_owned()],
            ),
            ("_name = False\n    _inherit = PARENT".to_owned(), vec![], true, vec![]),
        ];
        for (attrs, models, dynamic, starts) in cases {
            let body = format!("class X(models.Model):\n    {attrs}");
            assert_eq!(unread(&body), (models, dynamic, starts), "{body}");
        }
    }

    #[test]
    fn a_file_that_does_not_parse_guards_what_it_builds_as_a_parsed_class_does() {
        let a = ("a".to_owned(), false);
        let cases = [
            ("class X(models.Model):\n    _name = False", vec![]),
            ("class X(models.Model):\n    _name = False\n    _inherit = 'a'", vec![a]),
            ("class X(object):\n    _name = get()", vec![]),
            ("class X:\n    _name = get()", vec![]),
            (
                "class X(models.Model):\n    _name = get()\n    _register = False",
                vec![],
            ),
            ("class X(object):\n    _inherit = PARENT", vec![]),
        ];
        for (body, models) in cases {
            assert_eq!(unread(body), (models, true, vec![]), "{body}");
        }
    }

    #[test]
    fn a_file_that_does_not_parse_names_a_list_inherit_as_its_series_does() {
        let named = |series: &str| {
            let src = "class FooBar(models.Model):\n    _inherit = ['mail.thread']\nprint 'x'\n";
            let facts = analyze(src, Series::parse(series));
            let class = &facts.classes[0];
            (class.model.clone(), class.new_model)
        };
        assert_eq!(named("10.0"), ("mail.thread".to_owned(), false));
        assert_eq!(named("20.0"), ("foo.bar".to_owned(), true));
    }

    /// Each of these sits in its statement at an offset that, in the whole file, falls inside `©`.
    #[test]
    fn a_file_that_does_not_parse_reads_offsets_of_the_file() {
        let list = "# -*- coding: utf-8 -*-\n# © 2016 ...\nclass X(models.Model):\n    \
                    _inherit = ['mail.thread', 'ir.needaction_mixin']\nprint 'x'\n";
        let name = "# coding: ©\nclass X(models.Model):\n    _name = 'a'\n    _inherit = _name\nprint 'x'\n";
        let target = "# coding: ©\nclass X(models.Model):\n    _inherit = _register = 'a'\nprint 'x'\n";
        assert_eq!(list.as_bytes()[27] & 0xC0, 0x80);
        assert_eq!(name.as_bytes()[11] & 0xC0, 0x80);
        for (src, model) in [(list, "X"), (name, "a"), (target, "a")] {
            let facts = analyze(src, Series::parse("10.0"));
            assert_eq!(facts.classes[0].model, model, "{src}");
        }
    }

    #[test]
    fn a_file_that_parses_has_no_unread_class() {
        let facts = analyze("class X(models.Model):\n    _name = 'a'\n", Series::parse("10.0"));
        assert!(!facts.classes[0].unread);
        assert!(!facts.classes[0].inherit_unknown);
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
    fn reads_the_start_of_each_name_a_class_may_build() {
        let starts = |body: &str| analyze(&format!("class X(models.Model):\n{body}\n"), None).built_model_starts;
        for (name, start) in [
            ("'export.%s' % name", "export."),
            ("f'bad.generated.{kind}'", "bad.generated."),
            ("'a.' f'b.{kind}'", "a.b."),
            ("'a.' + b", "a."),
            ("'a.' + b + '.c'", "a."),
            ("'a.%s' % b + '.c'", "a."),
            ("'a.{}'.format(b)", "a."),
            ("'%s.line' % name", ""),
            ("f'{kind}.line'", ""),
            ("'{}.line'.format(b)", ""),
            ("PREFIX + '.line'", ""),
            ("MODEL", ""),
            ("self.env['a']._name", ""),
            ("'.'.join(parts)", ""),
        ] {
            assert_eq!(starts(&format!("    _name = {name}")), [start], "{name}");
        }
        for body in [
            "    _name = 'a'",
            "    _name = None",
            "    _name = False",
            "    _name = MODEL\n    _register = False",
            "    _register = False\n    _name = MODEL",
            "    _register = None\n    _name = f'a.{kind}'",
        ] {
            assert_eq!(starts(body), [""; 0], "{body}");
        }
        assert_eq!(starts("    _name = MODEL\n    _register = REGISTER"), [""]);
        assert_eq!(starts("    _name = MODEL\n    _name = 'a'"), [""; 0]);
        let bare = analyze("class Helper:\n    _name = None\n", None);
        assert!(bare.built_model_starts.is_empty());
        assert!(bare.dynamic_models);
    }

    #[test]
    fn a_class_that_cannot_reach_the_orm_builds_nothing() {
        let built = |src: &str| {
            let facts = analyze(src, None);
            (facts.dynamic_models, facts.built_model_starts, facts.built_model_names)
        };
        for src in [
            "class Helper:\n    _name = MODEL\n",
            "class Helper():\n    _name = MODEL\n",
            "class Helper(object):\n    _name = MODEL\n",
            "class Holder(models.Model):\n    _name = 'a'\n    class Helper:\n        _name = MODEL\n",
            "class ResPartnerX:\n    _inherit = ['a']\n",
            "class ResPartnerX(object):\n    _inherit = ['a']\n",
        ] {
            assert_eq!(built(src), (true, vec![], vec![]), "{src}");
        }
        for base in ["Mixin", "mixins.Layout", "object, Mixin"] {
            let src = format!("class Helper({base}):\n    _name = MODEL\n");
            assert_eq!(built(&src), (true, vec![String::new()], vec![]), "{base}");
        }
    }

    #[test]
    fn adds_the_reading_not_taken_of_a_class_of_unknown_series() {
        let read = |body: &str, series: Option<&str>| {
            let src = format!("class ResPartnerX(models.Model):\n{body}\n");
            let facts = analyze(&src, series.and_then(Series::parse));
            assert!(
                facts.built_model_starts.is_empty(),
                "a reading is a whole name, not a start"
            );
            (facts.dynamic_models, facts.built_model_names)
        };
        let parents = "    _inherit = ['a']";
        assert_eq!(read(parents, None), (true, vec!["res.partner.x".to_owned()]));
        for series in ["18.0", "20.0"] {
            assert_eq!(read(parents, Some(series)), (false, vec![]), "{series}");
        }
        assert_eq!(read("    _inherit = ['res.partner.x']", None), (false, vec![]));
        assert_eq!(
            read("    _inherit = ['a']\n    _register = False", None),
            (true, vec![])
        );
        // Each reading needs this parent from `depends`, so the module does not build it.
        assert_eq!(read("    _inherit = ['res.partner.x', 'a']", None), (true, vec![]));
        assert_eq!(read("    _inherit = ('a', 'res.partner.x')", None), (true, vec![]));
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

    fn class_defs(body: &str) -> ClassFact {
        let src = format!("from odoo import api, models\n\n\nclass X(models.Model):\n    _inherit = 'x'\n{body}");
        analyze(&src, None).classes.remove(0)
    }

    #[test]
    fn reads_each_parameter_kind_of_a_def() {
        let class = class_defs(
            "    def f(self, a, /, b, c=1 + 2, *rest, d, e=None, **more):\n        pass\n\n    \
             def g(self, *, k):\n        pass\n",
        );
        let f = class.signature("f").unwrap();
        assert_eq!(f.bound.as_deref(), Some("self"));
        let names: Vec<_> = f.positional.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c"]);
        assert_eq!(f.positional_only, 1);
        assert_eq!(f.positional[2].default.as_deref(), Some("1 + 2"));
        assert_eq!(f.var_positional.as_deref(), Some("rest"));
        assert_eq!(f.keyword_only.len(), 2);
        assert_eq!(f.var_keyword.as_deref(), Some("more"));
        assert_eq!(f.spelled("f"), "f(self, a, /, b, c=1 + 2, *rest, d, e=None, **more)");
        assert_eq!(class.signature("g").unwrap().spelled("g"), "g(self, *, k)");
    }

    #[test]
    fn a_staticmethod_binds_nothing_and_a_classmethod_its_class() {
        let class = class_defs(
            "    @staticmethod\n    def f(a):\n        pass\n\n    \
             @classmethod\n    def g(cls, a):\n        pass\n\n    \
             def h(*args):\n        pass\n",
        );
        let f = class.signature("f").unwrap();
        assert_eq!((f.bound.as_deref(), f.positional.len()), (None, 1));
        assert_eq!(class.signature("g").unwrap().bound.as_deref(), Some("cls"));
        let h = class.signature("h").unwrap();
        assert_eq!((h.bound.as_deref(), h.var_positional.as_deref()), (None, Some("args")));
    }

    #[test]
    fn only_decorators_that_keep_the_arity_keep_the_signature() {
        let class = class_defs(
            "    @api.model\n    @api.returns('self')\n    def a(self):\n        pass\n\n    \
             @tools.ormcache('self.env.uid')\n    @ormcache_context('x')\n    def b(self):\n        pass\n\n    \
             @typing.override\n    def c(self):\n        pass\n\n    \
             @logged\n    def d(self):\n        pass\n\n    \
             @property\n    def e(self):\n        pass\n\n    \
             @functools.wraps(other)\n    def f(self):\n        pass\n",
        );
        assert_eq!(class.defs[0].decorators, ["api.model", "api.returns"]);
        assert_eq!(class.defs[1].decorators, ["tools.ormcache", "ormcache_context"]);
        let kept: Vec<_> = ["a", "b", "c", "d", "e", "f"]
            .into_iter()
            .filter(|name| class.signature(name).is_some())
            .collect();
        assert_eq!(kept, ["a", "b", "c"]);
    }

    #[test]
    fn a_name_bound_otherwise_than_by_a_def_of_the_body_has_no_signature() {
        let class = class_defs(
            "    def a(self):\n        pass\n\n    a = helpers.run\n\n    \
             def b(self):\n        pass\n\n    if X:\n        def b(self, c):\n            pass\n\n    \
             def c(self):\n        pass\n\n    def c(self, d):\n        pass\n",
        );
        assert!(class.signature("a").is_none());
        assert!(class.signature("b").is_none());
        assert_eq!(class.signature("c").unwrap().positional.len(), 1);
        assert!(class.signature("missing").is_none());
        assert_eq!(class.defs.len(), 4);
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

    #[test]
    fn marks_the_queries_of_init_and_auto_init_as_ahead_of_the_data() {
        let src = "class X(models.Model):\n    _name = 'x'\n\n    \
                   def init(self):\n        self.env.cr.execute('SELECT a.x FROM t a')\n\n    \
                   def _auto_init(self):\n        self.env.cr.execute('SELECT b.y FROM t b')\n\n    \
                   def f(self):\n        self.env.cr.execute('SELECT c.z FROM t c')\n";
        let facts = analyze(src, None);
        let found: Vec<_> = facts
            .sql_columns
            .iter()
            .map(|c| (c.column.as_str(), c.before_data))
            .collect();
        assert_eq!(found, [("x", true), ("y", true), ("z", false)]);
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

    fn codes(src: &str, series: Option<Series>) -> Vec<&'static str> {
        analyze(src, series).diags.iter().map(|d| d.code).collect()
    }

    #[test]
    fn a_route_parameter_a_model_converter_fills_holds_records() {
        let route = |urls: &str| {
            let src = format!(
                "class C(http.Controller):\n    @http.route({urls})\n    def f(self, partner, id):\n        request.make_response('', [('X-Name', partner.name)])\n        request.make_response('', [('X-Id', id)])\n"
            );
            let diags = analyze(&src, None).diags;
            diags.into_iter().map(|d| d.message).collect::<Vec<_>>().join("\n")
        };
        let one = route("'/p/<model(\"res.partner\"):partner>/<int:id>'");
        assert!(one.contains("'id'") && !one.contains("'partner'"), "{one}");
        let list =
            route("['/p/<models(\"res.partner\", \"[]\"):partner>/<int:id>', '/q/<model(\"res.partner\"):partner>']");
        assert!(list.contains("'id'") && !list.contains("'partner'"), "{list}");
        // The query string fills a parameter a URL leaves out, with any value.
        let mixed = route("['/p/<models(\"res.partner\", \"[]\"):partner>', '/q/<int:id>']");
        assert!(mixed.contains("'id'") && mixed.contains("'partner'"), "{mixed}");
        let other = route("['/p/<model(\"res.partner\"):partner>', '/q/<partner>']");
        assert!(other.contains("'partner'"), "{other}");
        let both = route("route='/p/<int:partner>'");
        assert!(both.contains("'partner'") && both.contains("'id'"), "{both}");
    }

    fn route(decorator: &str, body: &str) -> Vec<&'static str> {
        let src = format!(
            "class C(http.Controller):\n    {decorator}\n    def f(self, order, buffer, path):\n        {body}\n"
        );
        codes(&src, None)
    }

    #[test]
    fn a_route_that_accepts_get_and_writes_records_is_reported() {
        let local = "lines = request.env['x'].browse(1)\n        lines.unlink()";
        let nested = "def inner():\n            request.env['x'].create({})\n        inner()";
        for (decorator, body) in [
            ("@http.route('/a')", "request.env['x'].browse(1).write({})"),
            (
                "@http.route('/a', methods=['GET', 'POST'])",
                "request.env['x'].create({})",
            ),
            (
                "@http.route('/a', type='http', methods=('get',))",
                "request.env.user.write({})",
            ),
            ("@http.route(route='/a')", local),
            ("@http.route('/a')", nested),
            ("@http.route('/a/<model(\"x\"):order>')", "order.write({})"),
            ("@http.route('/a')", "request.env['x'].with_user(1).create({})"),
            (
                "@http.route('/a')",
                "new_env = request.env(user=uid)\n        new_env['x'].create({})",
            ),
        ] {
            assert_eq!(route(decorator, body), ["OD127"], "{decorator} {body}");
        }
        let post = "if request.httprequest.method == 'POST':\n            request.env['x'].create({})";
        let token = "request.validate_csrf(kw)\n        request.env['x'].create({})";
        for (decorator, body) in [
            ("@http.route('/a', methods=['POST'])", "request.env['x'].create({})"),
            ("@http.route('/a', methods=METHODS)", "request.env['x'].create({})"),
            ("@http.route('/a', type='json')", "request.env['x'].create({})"),
            ("@http.route('/a', type=kind)", "request.env['x'].create({})"),
            ("@http.route()", "request.env['x'].create({})"),
            ("@http.route('/a')", post),
            ("@http.route('/a')", token),
            (
                "@http.route('/a')",
                "buffer.write(b'x')\n        Command.create({})\n        os.unlink(path)",
            ),
            ("@http.route('/a')", "order.write({})"),
        ] {
            assert_eq!(route(decorator, body), [""; 0], "{decorator} {body}");
        }
        let off = route("@http.route('/a', csrf=False)", "request.env['x'].create({})");
        assert_eq!(off, ["OD112"]);
    }

    #[test]
    fn a_route_open_to_any_origin_is_reported_at_its_cors() {
        let src =
            "class C(http.Controller):\n    @http.route('/a', type='json', cors='*')\n    def f(self):\n        pass\n";
        let diags = analyze(src, None).diags;
        assert_eq!(
            diags
                .iter()
                .map(|d| (d.code, d.pos.line, d.pos.col))
                .collect::<Vec<_>>(),
            [("OD128", 2, 36)]
        );
        assert_eq!(
            route("@http.route('/a', cors='https://shop.example.com')", "pass"),
            [""; 0]
        );
        assert_eq!(route("@http.route('/a', cors=ORIGIN)", "pass"), [""; 0]);
    }

    #[test]
    fn an_auth_none_route_that_reads_the_user_or_writes_without_setting_one_is_reported() {
        let none = |body: &str| route("@http.route('/a', auth='none', methods=['POST'])", body);
        assert_eq!(none("return request.env.user.name"), ["OD129"]);
        assert_eq!(none("request.env['x'].create({})"), ["OD129"]);
        assert_eq!(none("return http.request.env.user.name"), ["OD129"]);
        let check = "request.env['res.users'].authenticate(db, kw, env)\n        return request.env.user.name";
        assert_eq!(none(check), ["OD129"]);
        let cleared = "request.update_env(user=None)\n        return request.env.user.name";
        assert_eq!(none(cleared), ["OD129"]);
        for set in [
            "request.update_env(user=1)",
            "request.session.authenticate(request.db, kw)",
            "request.uid = 1",
            "http.request.uid = 1",
            "request.env = request.env(user=uid)",
            "authenticate(request.session, request.env, kw)",
        ] {
            let codes = none(&format!("{set}\n        return request.env.user.name"));
            assert!(!codes.contains(&"OD129"), "{set}");
        }
        for own in [
            "env = api.Environment(request.env.cr, SUPERUSER_ID, {})\n        env['x'].create({})",
            "env = request.env(user=SUPERUSER_ID)\n        env['x'].create({})",
            "request.env['x'].with_user(SUPERUSER_ID).create({})",
            "env = request.env(user=uid)\n        env(context={'lang': 'fr'})['x'].create({})",
            "base = request.env\n        base(user=uid)['x'].create({})",
            "env = api.Environment(cr, uid, {})\n        env(user=None)['x'].create({})",
        ] {
            assert!(!none(own).contains(&"OD129"), "{own}");
        }
        assert_eq!(none("return request.env.cr.dbname"), [""; 0]);
        let public = "@http.route('/a', auth='public', methods=['POST'])";
        assert_eq!(route(public, "return request.env.user.name"), [""; 0]);
        let src = "class C(http.Controller):\n    @http.route('/a', auth='none', methods=['POST'])\n    def f(self):\n        request.env['x'].create({})\n";
        assert_eq!(
            analyze(src, None).diags[0].message,
            "route with auth='none' calls create(), but runs with no user, so the write belongs to no one; use auth='public' or set the user with request.update_env()"
        );
    }

    #[test]
    fn a_public_route_that_runs_as_superuser_is_reported_with_the_form_it_uses() {
        let message = |body: &str| {
            let src = format!(
                "class C(http.Controller):\n    @http.route('/a', auth='public')\n    def f(self):\n        {body}\n"
            );
            let diags = analyze(&src, None).diags;
            diags.iter().find(|d| d.code == "OD111").map(|d| d.message.clone())
        };
        let through = |form: &str| {
            format!(
                "route with auth='public' runs as superuser through {form}; check that every record it reads or writes is filtered by an access check"
            )
        };
        for (body, form) in [
            (
                "return request.env['x'].with_user(SUPERUSER_ID).search([])",
                "with_user()",
            ),
            ("return request.env['x'].with_user(1).search([])", "with_user()"),
            ("return request.env(user=1, su=False)['x'].search([])", "env()"),
            ("return request.env(su=True)['x'].search([])", "env()"),
            ("request.update_env(user=SUPERUSER_ID)", "update_env()"),
            (
                "return api.Environment(request.cr, SUPERUSER_ID, {})['x'].search([])",
                "Environment()",
            ),
            (
                "return api.Environment(request.cr, uid, {}, su=True)['x'].search([])",
                "Environment()",
            ),
            (
                "return api.Environment(request.cr, uid=SUPERUSER_ID, context={})['x'].search([])",
                "Environment()",
            ),
            ("request.uid = SUPERUSER_ID", "request.uid"),
        ] {
            assert_eq!(message(body), Some(through(form)), "{body}");
        }
        for body in [
            "return request.env['x'].with_user(request.env.user).search([])",
            "return request.env(su=False)['x'].search([])",
            "return request.env['x'].sudo(False).search([])",
            "request.update_env(context={})",
        ] {
            assert_eq!(message(body), None, "{body}");
        }
        let sudo = message("request.env['x'].with_user(1)\n        return request.env['x'].sudo().search([])");
        assert!(sudo.is_some_and(|m| m.contains("calls sudo()")));
    }

    #[test]
    fn a_switch_of_the_request_or_session_user_is_reported_but_where_odoo_sets_it() {
        let user = |body: &str| route("@http.route('/a', methods=['POST'])", body);
        for body in [
            "request.update_env(user=uid)",
            "request.update_env(uid)",
            "http.request.update_env(user=request.env.ref('base.public_user').id)",
            "request.session.uid = uid",
            "uid = request.session.uid = SUPERUSER_ID",
            "request.session['uid'] = uid",
            "request.uid = uid",
        ] {
            assert_eq!(user(body), ["OD130"], "{body}");
        }
        for body in [
            "request.update_env(context={})",
            "request.update_env(user=None)",
            "request.session.uid = None",
            "request.session['uid'] = None",
            "request.session.context = {}",
            "uid = request.session.uid",
            "self.session.uid = uid",
            "request.update_env(user=request.session.uid)",
            "request.update_env(user=http.request.session['uid'])",
            "request.uid = request.session.uid",
            "request.uid = None",
        ] {
            assert_eq!(user(body), [""; 0], "{body}");
        }
        let src = "class C(http.Controller):\n    @http.route('/a', methods=['POST'])\n    def f(self):\n        request.uid = uid\n";
        let at = |series| codes(src, Series::parse(series));
        assert_eq!(at("15.0"), ["OD130"]);
        assert_eq!(at("16.0"), [""; 0]);
        let restore = |body: &str| {
            let src = format!(
                "class C(http.Controller):\n    @http.route('/a', methods=['POST'])\n    def f(self, uid):\n        old_uid = request.uid\n        {body}\n"
            );
            codes(&src, Series::parse("15.0"))
        };
        assert_eq!(restore("request.uid = old_uid"), [""; 0]);
        let failed =
            "try:\n            request.uid = uid\n        except Exception:\n            request.uid = old_uid";
        // Only the switch in the `try` is reported, not the restore in its handler.
        assert_eq!(restore(failed), ["OD130"]);
        assert_eq!(restore("request.uid = uid"), ["OD130"]);
        assert_eq!(restore("old_uid = uid\n        request.uid = old_uid"), ["OD130"]);
        let forward = "class R(Request):\n    def update_env(self, user=None, context=None, su=None):\n        Request.update_env(self, user, context, su)\n";
        assert_eq!(codes(forward, None), [""; 0]);
        let http = |base: &str| {
            format!(
                "class H(models.AbstractModel):\n    {base}\n\n    @classmethod\n    def _auth_method_key(cls):\n        request.update_env(user=uid)\n        request.session.uid = uid\n"
            )
        };
        let diags = analyze(&http("_inherit = 'ir.http'"), None).diags;
        let found: Vec<_> = diags.iter().map(|d| (d.code, d.pos.line, d.message.as_str())).collect();
        let message = "_auth_method_key() decides which user every route with auth='key' runs as; check that it sets only a user the request has proven to be";
        assert_eq!(found, [("OD130", 5, message)]);
        assert_eq!(codes(&http("_name = 'ir.http'"), None), [""; 0]);
        assert_eq!(codes(&http("_inherit = 'res.users'"), None), [""; 0]);
    }

    #[test]
    fn two_secrets_compared_with_eq_are_reported_and_a_secret_against_anything_else_is_not() {
        let compare = |test: &str| codes(&format!("if {test}:\n    pass\n"), None);
        for test in [
            "token != order.access_token",
            "kw['token'] == record.signature",
            "kw.get('access_token') == self._get_token()",
            "API_SECRET == request.params.get('secret')",
            "a < b == access_token != self.access_token",
        ] {
            assert_eq!(compare(test), ["OD131"], "{test}");
        }
        for test in [
            "consteq(token, order.access_token)",
            "token_type == 'bearer'",
            "token == False",
            "token is order.access_token",
            "token == tokens[0]",
            "kw.get(key) == order.access_token",
            "token_count == order.token_ids",
            "password == user.new_password",
        ] {
            assert_eq!(compare(test), [""; 0], "{test}");
        }
        let message = &analyze("token != order.access_token\n", None).diags[0].message;
        assert_eq!(
            message,
            "!= between secrets takes longer the more of the secret a guess gets right, which lets a caller find it a character at a time; compare them with consteq() or hmac.compare_digest()"
        );
    }

    #[test]
    fn a_change_check_compares_a_secret_field_with_only_the_value_it_is_set_to() {
        let model = |head: &str, body: &str| {
            let src = format!("class M(models.Model):\n    _inherit = 'x'\n\n    {head}\n        {body}\n");
            codes(&src, None).contains(&"OD131")
        };
        let write = |body: &str| model("def write(self, vals):", body);
        for body in [
            "if vals.get('webhook_secret') != self.webhook_secret:\n            pass",
            "if self.webhook_secret != vals['webhook_secret']:\n            pass",
            "new = vals.get('webhook_secret')\n        if record.webhook_secret != new:\n            pass",
            "old_secret = self.webhook_secret\n        super().write(vals)\n        return old_secret != self.webhook_secret",
        ] {
            assert!(!write(body), "{body}");
        }
        let change = "return vals.get('webhook_secret') == self.webhook_secret";
        for (head, vals) in [
            ("def write(self, values):", "values"),
            ("def write(self, changes):", "changes"),
            ("def write(self, cr, uid, ids, vals, context=None):", "vals"),
        ] {
            let body = format!("return {vals}.get('webhook_secret') == self.webhook_secret");
            assert!(!model(head, &body), "{head}");
        }
        for body in [
            "return self.env.context.get('access_token') == self.access_token",
            "return vals.get('webhook_secret') == self.access_token",
            "def inner(vals):\n            return vals.get('webhook_secret') == self.webhook_secret",
        ] {
            assert!(write(body), "{body}");
        }
        assert!(model("def check(self, vals):", change));
        let controller = format!("class C(http.Controller):\n    def write(self, vals):\n        {change}\n");
        assert_eq!(codes(&controller, None), ["OD131"]);
        for head in [
            "def _onchange_secret(self):",
            "@api.onchange('webhook_secret')\n    def _check(self):",
        ] {
            for (body, reported) in [
                ("return self.webhook_secret != self._origin.webhook_secret", false),
                (
                    "for rec in self:\n            if rec.webhook_secret != rec._origin.webhook_secret:\n                pass",
                    false,
                ),
                (
                    "origin = self._origin\n        return self.webhook_secret != origin.webhook_secret",
                    false,
                ),
                (
                    "webhook_secret = 'x'\n        return self.webhook_secret != webhook_secret",
                    false,
                ),
                (
                    "webhook_secret = 'x'\n        for rec in self:\n            if rec.webhook_secret != webhook_secret:\n                pass",
                    false,
                ),
                (
                    "webhook_secret = 'x'\n        for rec in self.search([]):\n            if rec.webhook_secret != webhook_secret:\n                pass",
                    true,
                ),
                ("return self.webhook_secret != self._origin.access_token", true),
            ] {
                assert_eq!(model(head, body), reported, "{head} {body}");
            }
        }
    }

    #[test]
    fn the_signatures_of_two_records_compare_with_eq_and_other_secrets_do_not() {
        let compare = |test: &str| codes(&format!("if {test}:\n    pass\n"), None);
        for test in [
            "get_signature(a) == get_signature(b)",
            "a._get_signature() != b._get_signature()",
            "a.signature == b.signature",
            "a._get_signature(kind='sale') == b._get_signature(kind='sale')",
        ] {
            assert_eq!(compare(test), [""; 0], "{test}");
        }
        for test in [
            "get_signature(a) == kw['signature']",
            "self._get_signature(kw) == order._get_signature()",
            "kw.get('signature') == other.get('signature')",
            "a.access_token == b.access_token",
            "get_token(a) == get_token(b)",
            "a.signature == b.user_signature_hash_token",
        ] {
            assert_eq!(compare(test), ["OD131"], "{test}");
        }
    }

    /// The routes an override of `SHOP_ROUTES` inherits from.
    const SHOP_ROUTES: &str = "from odoo import http\n\nclass Shop(http.Controller):\n    @http.route('/public', auth='public')\n    def public(self):\n        pass\n\n    @http.route('/none', auth='none')\n    def none(self):\n        pass\n\n    @http.route('/get')\n    def get(self):\n        pass\n\n    @http.route('/post', methods=['POST'])\n    def post(self):\n        pass\n\n    @http.route('/hook', csrf=False)\n    def hook(self):\n        pass\n\n    @http.route('/data', type='json')\n    def data(self):\n        pass\n\n    @http.route('/record/<model(\"x\"):order>')\n    def record(self, order):\n        pass\n\n    @http.route(['/mixed', '/mixed/<model(\"x\"):order>'])\n    def mixed(self, order=None):\n        pass\n";

    /// The diagnostics of an override of `method` of `SHOP_ROUTES` in a module of `series`, with
    /// `decorator` and `body`, read with the routes of both files when `bases` names `Shop` from
    /// its addon.
    fn override_diags(series: &str, bases: &str, decorator: &str, method: &str, body: &str) -> Vec<RawDiag> {
        let series = Series::parse(series);
        let leaf = format!(
            "from odoo import http\nfrom odoo.addons.shop.controllers.main import Shop\nfrom odoo.http import request\n\nclass Ext({bases}):\n    {decorator}\n    def {method}(self, order=None):\n        {body}\n"
        );
        let files = [
            ("shop", "controllers/main.py", SHOP_ROUTES),
            ("ext", "controllers/main.py", leaf.as_str()),
        ];
        let facts: Vec<PyFacts> = files.iter().map(|(_, _, src)| analyze(src, series)).collect();
        let table = routing::Table::of(files.iter().zip(&facts).map(|((addon, rel, _), f)| (*addon, *rel, f)));
        let scope = Scope {
            table: &table,
            addon: "ext",
            rel: "controllers/main.py",
        };
        analyze_with(&leaf, series, &FieldClasses::default(), &TaintTable::default(), scope).diags
    }

    fn override_codes(decorator: &str, method: &str, body: &str) -> Vec<&'static str> {
        let diags = override_diags("18.0", "Shop", decorator, method, body);
        diags.iter().map(|d| d.code).collect()
    }

    const WRITE: &str = "request.env['x'].create({})";

    #[test]
    fn an_override_reads_the_auth_it_leaves_out_from_the_route_it_overrides() {
        let sudo = "request.env['x'].sudo().search([])";
        let diags = override_diags("18.0", "Shop", "@http.route()", "public", sudo);
        let message = "route with auth='public' calls sudo(); check that every record it reads or writes is filtered by an access check";
        assert_eq!(
            diags.iter().map(|d| (d.code, d.message.as_str())).collect::<Vec<_>>(),
            [("OD111", message)]
        );
        assert_eq!(override_codes("@http.route(auth='user')", "public", sudo), [""; 0]);
        assert_eq!(
            override_codes("@http.route()", "none", "return request.env.user.name"),
            ["OD129"]
        );
        assert_eq!(
            override_codes("@http.route('/none', methods=['POST'])", "none", WRITE),
            ["OD129"]
        );
        // OD130 and OD131 read no routing, so an override reports them as a route does.
        let switch = "request.update_env(user=order)\n        return order.token == request.params['token']";
        assert_eq!(
            override_codes("@http.route(methods=['POST'])", "get", switch),
            ["OD130", "OD131"]
        );
    }

    #[test]
    fn an_override_reads_the_methods_urls_type_and_csrf_it_leaves_out() {
        assert_eq!(override_codes("@http.route()", "get", WRITE), ["OD127"]);
        assert_eq!(override_codes("@http.route('/get')", "get", WRITE), ["OD127"]);
        assert_eq!(override_codes("@http.route('/post')", "post", WRITE), [""; 0]);
        assert_eq!(override_codes("@http.route()", "hook", WRITE), [""; 0]);
        assert_eq!(override_codes("@http.route()", "data", WRITE), [""; 0]);
        // Odoo keeps the type of the first definition, whatever an override gives.
        assert_eq!(override_codes("@http.route(type='http')", "data", WRITE), [""; 0]);
        assert_eq!(override_codes("@http.route(type='json')", "get", WRITE), ["OD127"]);
        assert_eq!(override_codes("@http.route(methods=['POST'])", "get", WRITE), [""; 0]);
    }

    #[test]
    fn an_override_reads_its_own_csrf_type_and_cors_for_od112_od125_and_od128() {
        assert_eq!(override_codes("@http.route(csrf=False)", "data", "pass"), [""; 0]);
        assert_eq!(override_codes("@http.route(csrf=False)", "get", "pass"), ["OD112"]);
        let json = override_diags("saas~18.1", "Shop", "@http.route(type='json')", "get", "pass");
        assert_eq!(json.iter().map(|d| d.code).collect::<Vec<_>>(), ["OD125"]);
        assert_eq!(override_codes("@http.route(cors='*')", "data", "pass"), ["OD128"]);
    }

    #[test]
    fn an_override_reads_its_records_from_the_urls_it_inherits() {
        let header = "return request.make_response('', [('X-Order', order.name)])";
        assert_eq!(override_codes("@http.route()", "record", header), [""; 0]);
        assert_eq!(override_codes("@http.route()", "mixed", header), ["OD711"]);
        assert_eq!(override_codes("@http.route()", "record", "order.write({})"), ["OD127"]);
    }

    #[test]
    fn an_override_of_a_base_the_scan_does_not_read_reads_as_its_own_route() {
        let codes = |bases: &str, method: &str, body: &str| {
            let diags = override_diags("18.0", bases, "@http.route()", method, body);
            diags.iter().map(|d| d.code).collect::<Vec<_>>()
        };
        let header = "return request.make_response('', [('X-Order', order.name)])";
        for bases in ["Mixin", "Mixin, Shop"] {
            assert_eq!(codes(bases, "get", WRITE), [""; 0], "{bases}");
            assert_eq!(codes(bases, "public", "request.env['x'].sudo()"), [""; 0], "{bases}");
            assert_eq!(codes(bases, "record", header), ["OD711"], "{bases}");
        }
        // Shop comes after Mixin in the merge, so its URLs and auth stand, while the methods Mixin
        // may give read as the override's own.
        assert_eq!(codes("Shop, Mixin", "get", WRITE), ["OD127"]);
        assert_eq!(codes("Shop, Mixin", "public", "request.env['x'].sudo()"), ["OD111"]);
        assert_eq!(codes("Shop, Mixin", "record", header), [""; 0]);
    }

    #[test]
    fn each_of_two_controllers_of_one_name_reads_its_own_auth() {
        let class = |auth: &str| {
            format!(
                "class Main(http.Controller):\n    @http.route('/stock', auth='{auth}')\n    def stock(self):\n        return request.env['x'].sudo().search([])\n"
            )
        };
        let src = format!(
            "from odoo import http\nfrom odoo.http import request\n\n{}\n{}",
            class("public"),
            class("user")
        );
        let facts = analyze(&src, None);
        let table = routing::Table::of([("shop", "main.py", &facts)].into_iter());
        let scope = Scope {
            table: &table,
            addon: "shop",
            rel: "main.py",
        };
        let diags = analyze_with(&src, None, &FieldClasses::default(), &TaintTable::default(), scope).diags;
        let found: Vec<_> = diags.iter().map(|d| (d.code, d.pos.line)).collect();
        assert_eq!(found, [("OD111", 6)]);
    }

    /// `route_check` raises every route rule, so a change to one must leave the others as they were.
    #[test]
    fn the_route_rules_sharing_route_check_keep_their_findings() {
        let src = "class C(http.Controller):\n    @http.route('/a', auth='public', methods=['POST'])\n    def f(self):\n        return request.env['x'].sudo().search([]).name\n";
        let diags = analyze(src, None).diags;
        let od111 = (
            diags[0].code,
            diags[0].pos.line,
            diags[0].pos.col,
            diags[0].message.as_str(),
        );
        let message = "route with auth='public' calls sudo(); check that every record it reads or writes is filtered by an access check";
        assert_eq!((diags.len(), od111), (1, ("OD111", 3, 9, message)));
        let csrf = route("@http.route('/a', type=kind, methods=['POST'], csrf=False)", "pass");
        assert_eq!(csrf, ["OD112"]);
        let src = "class C(http.Controller):\n    @http.route('/a', type='json')\n    def f(self):\n        pass\n";
        assert_eq!(codes(src, Series::parse("saas~18.1")), ["OD125"]);
        assert_eq!(codes(src, Series::parse("18.0")), [""; 0]);
    }

    #[test]
    fn async_and_await_are_names_below_15_0_and_keywords_from_it() {
        for src in ["def f(async=False):\n    pass\n", "async = 1\n", "a.async\n"] {
            assert_eq!(codes(src, Series::parse("13.0")), [""; 0], "{src}");
            assert_eq!(codes(src, None), [""; 0], "{src}");
            assert_eq!(codes(src, Series::parse("15.0")), ["OD000"], "{src}");
        }
    }

    #[test]
    fn python_2_forms_give_a_notice_below_11_0_and_od000_elsewhere() {
        for src in [
            "print 'x'\n",
            "exec 'x'\n",
            "raise ValueError, 'x'\n",
            "x = a <> b\n",
            "x = 0755\n",
            "x = 10L\n",
            "x = ur'a'\n",
            "x = `a`\n",
            "def f(self, (a, b)):\n    pass\n",
            "def f(True=1):\n    pass\n",
            "if a:\n\tx = 1\n        y = 2\n",
        ] {
            let facts = analyze(src, Series::parse("10.0"));
            assert!(facts.python2.is_some() && facts.diags.is_empty(), "{src}");
            assert_eq!(codes(src, Series::parse("15.0")), ["OD000"], "{src}");
            assert_eq!(codes(src, None), ["OD000"], "{src}");
        }
    }

    #[test]
    fn a_file_python_2_refuses_too_keeps_od000_below_11_0() {
        for src in [
            "None = 1\n",
            "x = 089\n",
            "x = 1e5L\n",
            "x = (\n",
            "from __future__ import print_function\nprint \"x\"\n",
        ] {
            let facts = analyze(src, Series::parse("10.0"));
            assert_eq!(
                facts.diags.iter().map(|d| d.code).collect::<Vec<_>>(),
                ["OD000"],
                "{src}"
            );
            assert!(facts.python2.is_none(), "{src}");
        }
        let facts = analyze("print \"x\"\nx = (", Series::parse("9.0"));
        assert_eq!(facts.diags[0].pos.line, 2);
        let facts = analyze("if a:\n\t  x = 089\n", Series::parse("9.0"));
        assert_eq!((facts.diags[0].pos.line, facts.diags[0].pos.col), (2, 8));
    }

    #[test]
    fn a_name_keeps_the_position_of_an_error_after_it() {
        let facts = analyze("async = 1\n(", Series::parse("13.0"));
        assert_eq!((facts.diags[0].pos.line, facts.diags[0].pos.col), (2, 2));
    }

    #[test]
    fn the_class_facts_hold_each_name_as_the_file_spells_it() {
        let src = "class X(models.Model):\n    _name = 'x'\n    async = fields.Boolean()\n    \
                   ASYNC = fields.Boolean()\n    def await(self):\n        pass\n    def AWAIT(self):\n        pass\n";
        let class = analyze(src, Series::parse("13.0")).classes.remove(0);
        let fields: Vec<_> = class.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(fields, ["async", "ASYNC"]);
        assert_eq!(class.methods, ["await", "AWAIT"]);
    }

    #[test]
    fn sudo_is_given_a_user_only_by_an_id_a_record_or_a_user_name() {
        let src = |arg: &str| {
            format!("class X(models.Model):\n    _inherit = 'x'\n\n    def f(self):\n        return self.sudo({arg})\n")
        };
        let users = [
            "user",
            "2",
            "SUPERUSER_ID",
            "superuser.id",
            "self.env.ref('base.user_admin')",
            "self.env.ref('base.user_admin').id",
            "users.browse(2).id",
        ];
        for arg in users {
            assert_eq!(codes(&src(arg), Series::parse("16.0")), ["OD126"], "{arg}");
        }
        let flags = [
            "is_user",
            "has_uid",
            "as_superuser",
            "force_superuser",
            "self.use_superuser",
            "is_superuser",
            "self.env.su",
        ];
        for arg in flags {
            assert_eq!(codes(&src(arg), Series::parse("16.0")), [""; 0], "{arg}");
        }
    }

    #[test]
    fn a_method_the_class_defines_is_its_own_unless_it_extends_the_removed_one() {
        let src = "class X(models.Model):\n    _inherit = 'x'\n\n    def refresh(self):\n        return True\n\n    \
                   def user_has_groups(self, groups):\n        return self.env.user.has_groups(groups)\n\n    \
                   def f(self):\n        self.refresh()\n        self.sudo().refresh()\n        \
                   self.with_context(a=1).refresh()\n        self.env['x'].refresh()\n        \
                   self.user_has_groups('base.group_user')\n        self.env['y'].refresh()\n        self.flush()\n\n\
                   class Y(models.Model):\n    _inherit = 'y'\n\n    def refresh(self):\n        \
                   return super().refresh()\n\n    def f(self):\n        self.refresh()\n";
        let facts = analyze(src, Series::parse("saas~17.2"));
        let found: Vec<_> = facts.diags.iter().map(|d| (d.code, d.pos.line)).collect();
        assert_eq!(found, [("OD126", 16), ("OD126", 17), ("OD126", 26)]);
    }

    #[test]
    fn check_access_rights_without_raising_advises_has_access() {
        let src = |args: &str| format!("def f(records):\n    records.check_access_rights({args})\n");
        let message = |args: &str| analyze(&src(args), Series::parse("18.0")).diags[0].message.clone();
        for args in ["'read', raise_exception=False", "'read', False"] {
            assert!(message(args).ends_with("; use has_access()"), "{args}");
        }
        for args in ["'read'", "'read', raise_exception=True", "'read', True"] {
            assert!(message(args).ends_with("; use check_access()"), "{args}");
        }
    }

    #[test]
    fn force_company_counts_as_a_keyword_or_a_literal_key_of_with_context() {
        let src = |line: &str| format!("def f(records, company):\n    return {line}\n");
        let keyed = [
            "records.with_context(force_company=company.id)",
            "records.with_context({'force_company': company.id})",
        ];
        for line in keyed {
            assert_eq!(codes(&src(line), Series::parse("saas~13.1")), ["OD125"], "{line}");
            assert_eq!(codes(&src(line), Series::parse("13.0")), [""; 0], "{line}");
        }
        let unkeyed = [
            "records.env.context.get('force_company')",
            "records.with_context(company_id=company.id)",
            "records.with_company(company)",
        ];
        for line in unkeyed {
            assert_eq!(codes(&src(line), Series::parse("saas~13.1")), [""; 0], "{line}");
        }
    }

    #[test]
    fn cron_fields_count_in_the_values_create_and_write_give_ir_cron() {
        let src = |line: &str| format!("def f(env, cron):\n    return {line}\n");
        let written = [
            "env['ir.cron'].create({'name': 'x', 'numbercall': -1})",
            "env['ir.cron'].sudo().create([{'name': 'x'}, {'name': 'y', 'doall': True}])",
            "env['ir.cron'].with_context(a=1).write({'numbercall': 1})",
        ];
        for line in written {
            assert_eq!(codes(&src(line), Series::parse("saas~17.3")), ["OD126"], "{line}");
            assert_eq!(codes(&src(line), Series::parse("saas~17.2")), [""; 0], "{line}");
        }
        let unwritten = [
            "env['res.partner'].create({'numbercall': 1})",
            "cron.write({'doall': True})",
            "env['ir.cron'].create({'name': 'x', 'active': False})",
            "end_previous(doall=True)",
        ];
        for line in unwritten {
            assert_eq!(codes(&src(line), Series::parse("saas~17.3")), [""; 0], "{line}");
        }
        let own = "class X(models.Model):\n    _inherit = 'ir.cron'\n\n    def f(self):\n        \
                   return self.write({'doall': False})\n";
        assert_eq!(codes(own, Series::parse("18.0")), ["OD126"]);
    }

    #[test]
    fn a_numbercall_without_limit_is_advised_away() {
        let message = |value: &str| {
            let src = format!("def f(env, n):\n    return env['ir.cron'].create([{{'numbercall': {value}}}])\n");
            analyze(&src, Series::parse("saas~17.3")).diags[0].message.clone()
        };
        assert!(message("-1").ends_with("; remove it, as a cron repeats until deactivated"));
        for value in ["1", "0", "n", "-n"] {
            assert!(message(value).ends_with("; use active=False or _trigger()"), "{value}");
        }
    }

    #[test]
    fn an_onchange_returning_a_domain_warns_then_is_ignored() {
        let src = |decorator: &str| {
            format!(
                "class X(models.Model):\n    _inherit = 'x'\n\n    {decorator}\n    def f(self):\n        \
                 if self.a:\n            return {{'warning': {{}}, 'domain': {{'b': []}}}}\n        return {{}}\n"
            )
        };
        let onchange = src("@api.onchange('a')");
        assert_eq!(codes(&onchange, Series::parse("saas~13.1")), [""; 0]);
        assert_eq!(codes(&onchange, Series::parse("saas~13.2")), ["OD125"]);
        assert_eq!(codes(&onchange, Series::parse("saas~16.3")), ["OD125"]);
        assert_eq!(codes(&src("@api.depends('a')"), Series::parse("17.0")), [""; 0]);
    }

    #[test]
    fn overrides_count_in_model_classes() {
        let src = |base: &str, method: &str| {
            format!(
                "class X({base}):\n    _inherit = 'x'\n\n    def {method}(self, *args, **kwargs):\n        return []\n"
            )
        };
        assert_eq!(
            codes(&src("models.Model", "_name_search"), Series::parse("saas~17.4")),
            [""; 0]
        );
        assert_eq!(
            codes(&src("models.Model", "_name_search"), Series::parse("18.0")),
            ["OD125"]
        );
        assert_eq!(
            codes(&src("models.Model", "read_group"), Series::parse("20.0")),
            ["OD126"]
        );
        let plain = "class Helper:\n    def _name_search(self, name):\n        return []\n";
        assert_eq!(codes(plain, Series::parse("18.0")), [""; 0]);
    }

    #[test]
    fn an_override_of_read_group_is_reported_once_for_its_super_call() {
        let src = |method: &str, sup: &str| {
            format!(
                "class X(models.Model):\n    _inherit = 'x'\n\n    def {method}(self, domain, fields, groupby):\n        \
                 return {sup}.read_group(domain, fields, groupby)\n"
            )
        };
        for series in ["saas~18.2", "saas~19.2"] {
            for sup in ["super()", "super(X, self)"] {
                let found: Vec<_> = analyze(&src("read_group", sup), Series::parse(series))
                    .diags
                    .iter()
                    .map(|d| (d.pos.line, d.message.starts_with("overriding")))
                    .collect();
                assert_eq!(found, [(4, true)], "{sup} on {series}");
            }
            let other = analyze(&src("_x_totals", "super()"), Series::parse(series));
            assert_eq!(
                other.diags.iter().map(|d| d.pos.line).collect::<Vec<_>>(),
                [5],
                "{series}"
            );
        }
        let keyword = "class X(models.Model):\n    _inherit = 'x'\n\n    def read_group(self, domain, groupby, lazy=True):\n        \
                       return super().read_group(domain, groupby=groupby, lazy=lazy)\n";
        let found: Vec<_> = analyze(keyword, Series::parse("20.0"))
            .diags
            .iter()
            .map(|d| (d.code, d.pos.line, d.message.starts_with("read_group(fields=")))
            .collect();
        assert_eq!(found, [("OD126", 4, false), ("OD126", 5, true)]);
    }

    #[test]
    fn an_import_a_try_falls_back_from_is_no_error() {
        let src = |caught: &str| {
            format!("try:\n    from odoo.osv import expression\nexcept {caught}:\n    expression = None\n")
        };
        for caught in [
            "ImportError",
            "ModuleNotFoundError",
            "(ImportError, KeyError)",
            "Exception",
        ] {
            assert_eq!(codes(&src(caught), Series::parse("saas~19.1")), [""; 0], "{caught}");
            assert_eq!(codes(&src(caught), Series::parse("19.0")), ["OD125"], "{caught}");
        }
        assert_eq!(codes(&src("ValueError"), Series::parse("saas~19.1")), ["OD126"]);
        let fallback = "try:\n    import lxml\nexcept ImportError:\n    from odoo.osv import expression\n";
        assert_eq!(codes(fallback, Series::parse("saas~19.1")), ["OD126"]);
    }

    /// `src` with the fixes of its `code` findings applied, and the findings left unfixed.
    fn fixed(src: &str, series: &str, code: &str) -> (String, usize) {
        let facts = analyze(src, Series::parse(series));
        let found: Vec<&RawDiag> = facts.diags.iter().filter(|d| d.code == code).collect();
        assert!(!found.is_empty(), "{code} on {src}");
        let fixes: Vec<&Fix> = found.iter().filter_map(|d| d.fix.as_ref()).collect();
        let (out, outcomes) = fix::apply(src.as_bytes(), &fixes);
        assert!(outcomes.iter().all(|o| *o == fix::Outcome::Applied), "{src}");
        (String::from_utf8(out).unwrap(), found.len() - fixes.len())
    }

    /// Fixes every `code` finding of `src`, then checks the result reports `code` no more.
    fn rewrites(src: &str, series: &str, code: &str) -> String {
        let (out, left) = fixed(src, series, code);
        assert_eq!(left, 0, "{src}");
        let again = analyze(&out, Series::parse(series));
        assert!(again.diags.iter().all(|d| d.code != code && d.code != "OD000"), "{out}");
        out
    }

    fn unfixed(src: &str, series: &str, code: &str) {
        assert_eq!(fixed(src, series, code), (src.to_owned(), 1), "{src}");
    }

    const MODEL: &str =
        "from odoo import api, fields, models\n\n\nclass X(models.Model):\n    _inherit = 'res.partner'\n";

    #[test]
    fn api_multi_alone_on_its_line_is_dropped() {
        let src = format!("{MODEL}\n    @api.multi\n    def f(self):\n        return self\n");
        let out = rewrites(&src, "13.0", "OD107");
        assert_eq!(out, format!("{MODEL}\n    def f(self):\n        return self\n"));
        let stacked = format!("{MODEL}\n    @api.depends('name')\n    @api.multi\n    def f(self):\n        pass\n");
        assert!(rewrites(&stacked, "13.0", "OD107").contains("    @api.depends('name')\n    def f"));
    }

    #[test]
    fn api_one_and_a_commented_api_multi_stay() {
        unfixed(
            &format!("{MODEL}\n    @api.one\n    def f(self):\n        pass\n"),
            "13.0",
            "OD107",
        );
        unfixed(
            &format!("{MODEL}\n    @api.multi  # batch\n    def f(self):\n        pass\n"),
            "13.0",
            "OD107",
        );
    }

    #[test]
    fn group_operator_becomes_aggregator() {
        let src = format!("{MODEL}\n    x = fields.Float(\n        group_operator = 'avg',\n    )\n");
        let out = rewrites(&src, "18.0", "OD108");
        assert!(out.contains("        aggregator = 'avg',\n"), "{out}");
    }

    #[test]
    fn group_operator_beside_an_aggregator_stays() {
        unfixed(
            &format!("{MODEL}\n    x = fields.Float(group_operator='avg', aggregator='sum')\n"),
            "18.0",
            "OD108",
        );
        unfixed(
            &format!("{MODEL}\n    x = fields.Float(group_operator='avg', **OPTS)\n"),
            "18.0",
            "OD108",
        );
    }

    #[test]
    fn private_env_shortcuts_read_on_a_model_go_through_env() {
        let src = format!(
            "{MODEL}\n    def f(self):\n        self._cr.execute('SELECT 1')\n        \
             return self._context.get('lang'), self._uid\n"
        );
        let out = rewrites(&src, "19.0", "OD109");
        assert!(
            out.contains(
                "self.env.cr.execute('SELECT 1')\n        return self.env.context.get('lang'), self.env.uid\n"
            ),
            "{out}"
        );
    }

    #[test]
    fn a_private_env_shortcut_written_or_outside_a_model_stays() {
        unfixed(
            &format!("{MODEL}\n    def f(self):\n        self._context = {{}}\n"),
            "19.0",
            "OD109",
        );
        unfixed(
            "class Helper:\n    def f(self):\n        self._cr.execute('SELECT 1')\n",
            "19.0",
            "OD109",
        );
        unfixed("def f(self):\n    return self._uid\n", "19.0", "OD109");
    }

    #[test]
    fn sudo_given_a_user_becomes_with_user() {
        let src = format!("{MODEL}\n    def f(self, user):\n        return self.sudo(user).name\n");
        let out = rewrites(&src, "saas~12.5", "OD125");
        assert!(out.contains("return self.with_user(user).name\n"), "{out}");
        rewrites(&src, "saas~15.3", "OD126");
    }

    #[test]
    fn sudo_given_more_than_the_user_stays() {
        for args in ["user, True", "user, **opts"] {
            unfixed(
                &format!("{MODEL}\n    def f(self, user):\n        return self.sudo({args}).name\n"),
                "saas~12.5",
                "OD125",
            );
        }
    }

    #[test]
    fn track_visibility_becomes_tracking() {
        let src = format!("{MODEL}\n    x = fields.Char(track_visibility=\"onchange\", index=True)\n");
        let out = rewrites(&src, "saas~13.4", "OD125");
        assert!(out.contains("fields.Char(tracking=True, index=True)\n"), "{out}");
        assert_eq!(rewrites(&src, "saas~18.1", "OD125"), out);
    }

    #[test]
    fn track_visibility_that_is_false_unknown_or_beside_tracking_stays() {
        for option in [
            "track_visibility=False",
            "track_visibility=''",
            "track_visibility=MODE",
            "track_visibility='always', tracking=10",
            "track_visibility='always', track_sequence=2",
            "track_visibility='always', **OPTS",
        ] {
            unfixed(
                &format!("{MODEL}\n    x = fields.Char({option})\n"),
                "saas~13.4",
                "OD125",
            );
        }
    }

    /// Passes with no guard, because `unread_facts` reads only the attributes that name models and
    /// none of them carries a fix; it pins that invariant for a reader that would read more.
    #[test]
    fn a_file_that_does_not_parse_has_no_fix() {
        let src = format!("{MODEL}\n    x = fields.Float(group_operator='avg')\n    def (\n");
        let facts = analyze(&src, Series::parse("18.0"));
        assert!(facts.diags.iter().any(|d| d.code == "OD000"));
        assert!(facts.diags.iter().all(|d| d.fix.is_none()));
    }
}
