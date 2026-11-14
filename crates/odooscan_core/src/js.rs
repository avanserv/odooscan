use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, AssignmentExpression, AssignmentTarget, CallExpression, Class, ClassElement,
    Expression, Function, ObjectExpression, ObjectPropertyKind, Program, Statement, StaticMemberExpression,
    StringLiteral,
};
use oxc_ast_visit::Visit;
use oxc_ast_visit::walk::{walk_assignment_expression, walk_call_expression, walk_class};
use oxc_parser::Parser;
use oxc_span::SourceType;
use oxc_syntax::module_record::{ExportExportName, ExportImportName, ImportImportName, ModuleRecord};
use oxc_syntax::scope::ScopeFlags;

use crate::facts::{
    Annotation, Heritage, JsAlias, JsClass, JsDefine, JsFacts, JsName, JsPatch, Lines, OrmCall, Pos, Spanned,
};
use crate::series::Series;

/// Methods of the `orm` service whose first argument is a model name.
const ORM_METHODS: &[&str] = &[
    "call",
    "create",
    "nameGet",
    "read",
    "readGroup",
    "search",
    "searchCount",
    "searchRead",
    "unlink",
    "webRead",
    "webReadGroup",
    "webSave",
    "webSearchRead",
    "write",
];

pub fn analyze(text: &str) -> JsFacts {
    let lines = Lines::new(text);
    let mut facts = JsFacts {
        annotation: annotation(text),
        alias: alias(text),
        ..JsFacts::default()
    };
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, text, SourceType::mjs()).parse();
    if let Some(error) = parsed.diagnostics.errors().next() {
        let offset = error.labels.first().map_or(0, |l| l.offset());
        facts.syntax_error = Some(Spanned {
            value: error.message.to_string(),
            pos: lines.pos(offset as usize),
        });
        if parsed.fatal_error {
            return facts;
        }
    }
    module_record(&parsed.module_record, &lines, &mut facts);
    facts.defines = defines(&parsed.program, &lines);
    let mut collector = Collector {
        lines: &lines,
        facts: &mut facts,
        templates: vec![],
    };
    collector.visit_program(&parsed.program);
    for (class, template) in collector.templates {
        if let Some(c) = facts.classes.iter_mut().find(|c| c.name == class) {
            c.template = Some(template);
        }
    }
    facts
}

/// The text after `@odoo-module` in the comments that open the file, where Odoo reads it.
fn module_tag(text: &str) -> Option<&str> {
    let mut rest = text.trim_start();
    loop {
        let (comment, after) = if let Some(r) = rest.strip_prefix("//") {
            r.split_once('\n').unwrap_or((r, ""))
        } else {
            let r = rest.strip_prefix("/*")?;
            r.split_once("*/").unwrap_or((r, ""))
        };
        if let Some((_, tail)) = comment.split_once("@odoo-module") {
            return Some(tail);
        }
        rest = after.trim_start();
    }
}

fn annotation(text: &str) -> Annotation {
    let Some(tail) = module_tag(text) else {
        return Annotation::None;
    };
    let ignore = tail.starts_with(char::is_whitespace) && tail.split_whitespace().next() == Some("ignore");
    if ignore { Annotation::Ignore } else { Annotation::Module }
}

/// `alias=<name>` right after `@odoo-module`, then optionally `default=<value>`, each up to a space
/// or a `*`. An ignored file has none.
fn alias(text: &str) -> Option<JsAlias> {
    let token = |s: &str| -> (String, usize) {
        let end = s.find(|c: char| c.is_whitespace() || c == '*').unwrap_or(s.len());
        (s[..end].to_owned(), end)
    };
    let tail = module_tag(text)?;
    let rest = tail
        .strip_prefix(char::is_whitespace)?
        .trim_start()
        .strip_prefix("alias=")?;
    let (name, end) = token(rest);
    let default = rest[end..]
        .strip_prefix(char::is_whitespace)
        .and_then(|a| a.trim_start().strip_prefix("default="))
        .map(|d| token(d).0);
    (!name.is_empty()).then_some(JsAlias { name, default })
}

/// From saas~17.2 an alias is any name and any `default=` value makes it stand for the file's
/// exports; before, a name of word characters and dots, and `default=` one of `false`, `False` or
/// `0` (17.0 and 18.0 `odoo/tools/js_transpiler.py`, `ODOO_MODULE_RE`, commit `3ca925b8a82e`).
const PERMISSIVE_ALIAS: Series = Series { major: 17, minor: 2 };

impl JsAlias {
    /// The name the series registers and whether it returns the file's exports rather than its
    /// default export. Odoo's pattern has no end anchor, so it takes the longest prefix it accepts.
    pub fn registered(&self, series: Option<Series>) -> Option<(&str, bool)> {
        let word = |c: char| c.is_alphanumeric() || c == '_';
        let default = self.default.as_deref();
        if series.is_none_or(|s| s >= PERMISSIVE_ALIAS) {
            let proxies = default.is_some_and(|d| d.starts_with(|c| word(c) || c == '$'));
            return Some((&self.name, proxies));
        }
        let end = self.name.find(|c| !(word(c) || c == '.')).unwrap_or(self.name.len());
        // A name cut short leaves no space before `default=`.
        let proxies =
            end == self.name.len() && default.is_some_and(|d| ["false", "False", "0"].iter().any(|v| d.starts_with(v)));
        (end > 0).then_some((&self.name[..end], proxies))
    }
}

fn module_record(record: &ModuleRecord, lines: &Lines, facts: &mut JsFacts) {
    let mut specifiers: Vec<(u32, String)> = record
        .requested_modules
        .iter()
        .filter_map(|(name, uses)| Some((uses.iter().map(|u| u.span.start).min()?, name.to_string())))
        .collect();
    specifiers.sort();
    facts.specifiers = specifiers
        .into_iter()
        .map(|(start, value)| Spanned {
            value,
            pos: lines.pos(start as usize),
        })
        .collect();
    imports(record, lines, facts);
    exports(record, lines, facts);
    // `export { x }` of an imported `x` is an indirect export of the same name at the same place.
    facts.names.sort_by_key(|n| n.name.pos);
    facts
        .names
        .dedup_by(|a, b| a.name.pos == b.name.pos && a.specifier == b.specifier && a.name.value == b.name.value);
}

fn imports(record: &ModuleRecord, lines: &Lines, facts: &mut JsFacts) {
    for entry in &record.import_entries {
        let (name, start) = match &entry.import_name {
            ImportImportName::Name(n) => (n.name.to_string(), n.span.start),
            ImportImportName::Default(span) => ("default".to_owned(), span.start),
            ImportImportName::NamespaceObject => continue,
        };
        let mut name = js_name(&entry.module_request.name, name, lines.pos(start as usize));
        name.local = Some(entry.local_name.name.to_string());
        facts.names.push(name);
    }
}

fn exports(record: &ModuleRecord, lines: &Lines, facts: &mut JsFacts) {
    for entry in record
        .local_export_entries
        .iter()
        .chain(&record.indirect_export_entries)
    {
        facts.exports.extend(export_name(&entry.export_name));
        if let Some(local) = entry.local_name.name().filter(|_| entry.module_request.is_none())
            && let Some(exported) = export_name(&entry.export_name)
        {
            facts.export_locals.push((exported, local.to_string()));
        }
        if let (Some(request), ExportImportName::Name(n)) = (&entry.module_request, &entry.import_name) {
            if let Some(exported) = export_name(&entry.export_name) {
                facts
                    .reexports
                    .push((exported, request.name.to_string(), n.name.to_string()));
            }
            facts.names.push(js_name(
                &request.name,
                n.name.to_string(),
                lines.pos(n.span.start as usize),
            ));
        }
    }
    for entry in &record.star_export_entries {
        facts
            .star_exports
            .extend(entry.module_request.as_ref().map(|r| r.name.to_string()));
    }
}

fn export_name(name: &ExportExportName) -> Option<String> {
    match name {
        ExportExportName::Name(n) => Some(n.name.to_string()),
        ExportExportName::Default(_) => Some("default".to_owned()),
        ExportExportName::Null => None,
    }
}

fn js_name(specifier: &str, name: String, pos: Pos) -> JsName {
    JsName {
        specifier: specifier.to_owned(),
        name: Spanned { value: name, pos },
        local: None,
    }
}

/// Named classes and prototype patches, wherever they are.
struct Collector<'f> {
    lines: &'f Lines,
    facts: &'f mut JsFacts,
    /// `Class.template = "..."` assignments, which may precede the class.
    templates: Vec<(String, Spanned<String>)>,
}

impl<'a> Visit<'a> for Collector<'_> {
    fn visit_class(&mut self, class: &Class<'a>) {
        if let Some(id) = &class.id {
            self.facts.classes.push(JsClass {
                name: id.name.to_string(),
                extends: match class.heritage.as_ref().map(|h| &h.expression) {
                    None => Heritage::None,
                    Some(Expression::Identifier(i)) => Heritage::Name(i.name.to_string()),
                    Some(_) => Heritage::Other,
                },
                members: class
                    .body
                    .body
                    .iter()
                    .filter(|e| !e.r#static())
                    .filter_map(|e| Some(e.static_name()?.as_str()?.to_owned()))
                    .collect(),
                template: class.body.body.iter().find_map(|e| match e {
                    ClassElement::PropertyDefinition(p)
                        if p.r#static && p.key.static_name().is_some_and(|n| n == "template") =>
                    {
                        match &p.value {
                            // An abstract component leaves its template empty for subclasses to set.
                            Some(Expression::StringLiteral(s)) => {
                                literal(s, self.lines).filter(|t| !t.value.is_empty())
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                }),
            });
        }
        walk_class(self, class);
    }

    fn visit_assignment_expression(&mut self, assignment: &AssignmentExpression<'a>) {
        if let AssignmentTarget::StaticMemberExpression(m) = &assignment.left
            && m.property.name == "template"
            && let Expression::Identifier(class) = &m.object
            && let Expression::StringLiteral(s) = &assignment.right
            && let Some(template) = literal(s, self.lines)
        {
            self.templates.push((class.name.to_string(), template));
        }
        walk_assignment_expression(self, assignment);
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if let Some(patch) = prototype_patch(call, self.lines) {
            self.facts.patches.push(patch);
        }
        if let Some(orm) = orm_call(call, self.lines).or_else(|| legacy_rpc(call, self.lines)) {
            self.facts.orm_calls.push(orm);
        }
        self.facts.view_kinds.extend(view_kind(call));
        walk_call_expression(self, call);
    }
}

fn prototype_patch(call: &CallExpression, lines: &Lines) -> Option<JsPatch> {
    if !matches!(&call.callee, Expression::Identifier(i) if i.name == "patch") {
        return None;
    }
    let target = match call.arguments.first()? {
        Argument::StaticMemberExpression(m) if m.property.name == "prototype" => match &m.object {
            Expression::Identifier(i) => i.name.to_string(),
            _ => return None,
        },
        _ => return None,
    };
    let (extension, named) = match (call.arguments.get(1)?, call.arguments.get(2)) {
        (Argument::ObjectExpression(extension), _) => (extension, false),
        (_, Some(Argument::ObjectExpression(extension))) => (extension, true),
        _ => return None,
    };
    let supers = if named {
        this_supers(extension, lines)
    } else {
        let mut supers = Supers { lines, found: vec![] };
        supers.visit_object_expression(extension);
        supers.found
    };
    Some(JsPatch {
        target,
        members: members(extension),
        supers,
        named,
    })
}

fn orm_call(call: &CallExpression, lines: &Lines) -> Option<OrmCall> {
    let Expression::StaticMemberExpression(callee) = &call.callee else {
        return None;
    };
    let method = callee.property.name.as_str();
    if !ORM_METHODS.contains(&method) || !is_orm(&callee.object) {
        return None;
    }
    let literal = |i: usize| match call.arguments.get(i)? {
        Argument::StringLiteral(s) => literal(s, lines),
        _ => None,
    };
    let call = OrmCall {
        model: literal(0),
        method: if method == "call" { literal(1) } else { None },
    };
    (call.model.is_some() || call.method.is_some()).then_some(call)
}

/// `x._rpc({...})` or `rpc.query({...})` given the `model` and `method` that `buildQuery` routes to
/// `call_kw` (14.0 `addons/web/static/src/js/core/rpc.js:16-19`, `:37-45`, and `service_mixins.js:146`
/// beside it).
fn legacy_rpc(call: &CallExpression, lines: &Lines) -> Option<OrmCall> {
    let Argument::ObjectExpression(options) = call.arguments.first().filter(|_| is_legacy_rpc(&call.callee))? else {
        return None;
    };
    let (mut model, mut method) = (None, None);
    for property in &options.properties {
        let (key, value) = option(property, lines)?;
        match key.as_str() {
            "model" => model = Some(value),
            "method" => method = Some(value),
            _ => {}
        }
    }
    let call = OrmCall {
        model: model?,
        method: method?,
    };
    (call.model.is_some() || call.method.is_some()).then_some(call)
}

fn is_legacy_rpc(callee: &Expression) -> bool {
    let Expression::StaticMemberExpression(callee) = callee else {
        return false;
    };
    callee.property.name == "_rpc"
        || (callee.property.name == "query" && matches!(&callee.object, Expression::Identifier(i) if i.name == "rpc"))
}

/// A property's name and its value when a literal, or None for a spread, a computed key or a
/// `route`, any of which may send the call elsewhere.
fn option(property: &ObjectPropertyKind, lines: &Lines) -> Option<(String, Option<Spanned<String>>)> {
    let ObjectPropertyKind::ObjectProperty(p) = property else {
        return None;
    };
    let value = match &p.value {
        Expression::StringLiteral(s) => literal(s, lines),
        _ => None,
    };
    let key = p.key.static_name()?.as_str()?.to_owned();
    (key != "route").then_some((key, value))
}

/// The kind a `view_registry.add("kind", View)` call registers.
fn view_kind(call: &CallExpression) -> Option<String> {
    let Expression::StaticMemberExpression(callee) = &call.callee else {
        return None;
    };
    if callee.property.name != "add" || !is_view_registry(&callee.object) {
        return None;
    }
    let Argument::StringLiteral(kind) = call.arguments.first()? else {
        return None;
    };
    Some(kind.value.as_str()?.to_owned())
}

/// `view_registry`, `x.view_registry`, and those through a chain of `.add()`.
fn is_view_registry(e: &Expression) -> bool {
    match e {
        Expression::Identifier(i) => i.name == "view_registry",
        Expression::StaticMemberExpression(m) => m.property.name == "view_registry",
        Expression::CallExpression(c) => match &c.callee {
            Expression::StaticMemberExpression(m) => m.property.name == "add" && is_view_registry(&m.object),
            _ => false,
        },
        _ => false,
    }
}

/// A string's value, placed after its opening quote.
fn literal(s: &StringLiteral, lines: &Lines) -> Option<Spanned<String>> {
    Some(Spanned {
        value: s.value.as_str()?.to_owned(),
        pos: lines.pos(s.span.start as usize + 1),
    })
}

/// `orm`, `x.orm`, and those through `.silent` or `.cache({...})`.
fn is_orm(e: &Expression) -> bool {
    match e {
        Expression::Identifier(i) => i.name == "orm",
        Expression::StaticMemberExpression(m) => {
            m.property.name == "orm" || (m.property.name == "silent" && is_orm(&m.object))
        }
        Expression::CallExpression(c) => match &c.callee {
            Expression::StaticMemberExpression(m) => m.property.name == "cache" && is_orm(&m.object),
            _ => false,
        },
        _ => false,
    }
}

fn members(object: &ObjectExpression) -> Vec<String> {
    object
        .properties
        .iter()
        .filter_map(|p| match p {
            ObjectPropertyKind::ObjectProperty(p) => Some(p.key.static_name()?.as_str()?.to_owned()),
            ObjectPropertyKind::SpreadProperty(_) => None,
        })
        .collect()
}

/// `super.name()` calls. A read such as `super.name || fallback` tolerates a missing member.
struct Supers<'f> {
    lines: &'f Lines,
    found: Vec<Spanned<String>>,
}

impl<'a> Visit<'a> for Supers<'_> {
    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if let Expression::StaticMemberExpression(member) = &call.callee
            && matches!(member.object, Expression::Super(_))
        {
            self.found.push(Spanned {
                value: member.property.name.to_string(),
                pos: self.lines.pos(member.property.span.start as usize),
            });
        }
        walk_call_expression(self, call);
    }
}

/// The `this._super` calls of each method or accessor of a named patch, as the member's name: the
/// patch binds `this._super` to the member it replaces only when the target or its prototype chain
/// has one (16.0 `addons/web/static/src/core/utils/patch.js:37-56`, `:79-107`).
fn this_supers(extension: &ObjectExpression, lines: &Lines) -> Vec<Spanned<String>> {
    let mut found = vec![];
    for property in &extension.properties {
        if let ObjectPropertyKind::ObjectProperty(p) = property
            && let Expression::FunctionExpression(function) = &p.value
            && let Some(name) = p.key.static_name()
            && let Some(name) = name.as_str()
            && let Some(body) = &function.body
        {
            let mut calls = ThisSupers {
                name,
                lines,
                found: &mut found,
            };
            calls.visit_function_body(body);
        }
    }
    found
}

/// `this._super(...)`, `this._super.apply(...)` and `this._super.call(...)`. An arrow function keeps
/// the method's `this`; a nested function or class has its own.
struct ThisSupers<'f> {
    name: &'f str,
    lines: &'f Lines,
    found: &'f mut Vec<Spanned<String>>,
}

impl<'a> Visit<'a> for ThisSupers<'_> {
    fn visit_function(&mut self, _: &Function<'a>, _: ScopeFlags) {}

    fn visit_class(&mut self, _: &Class<'a>) {}

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if let Expression::StaticMemberExpression(member) = &call.callee {
            let target = match &member.object {
                Expression::StaticMemberExpression(inner)
                    if ["apply", "call"].contains(&member.property.name.as_str()) =>
                {
                    inner
                }
                _ => member,
            };
            if this_super(target) {
                self.found.push(Spanned {
                    value: self.name.to_owned(),
                    pos: self.lines.pos(target.property.span.start as usize),
                });
            }
        }
        walk_call_expression(self, call);
    }
}

fn this_super(member: &StaticMemberExpression) -> bool {
    member.property.name == "_super" && matches!(member.object, Expression::ThisExpression(_))
}

/// The top-level `odoo.define()` calls. Odoo takes a leading string as the name, a list after it as
/// the dependencies, and otherwise each `require("x")` of the factory's source (14.0
/// `addons/web/static/src/js/boot.js:38-39`, `:106-121`; 16.0 `addons/web/static/src/boot.js:39`,
/// `:205-219`).
fn defines(program: &Program, lines: &Lines) -> Vec<JsDefine> {
    program
        .body
        .iter()
        .filter_map(|s| match s {
            Statement::ExpressionStatement(e) => match &e.expression {
                Expression::CallExpression(call) => Some(call),
                _ => None,
            },
            _ => None,
        })
        .filter(|call| match &call.callee {
            Expression::StaticMemberExpression(m) => {
                m.property.name == "define" && matches!(&m.object, Expression::Identifier(i) if i.name == "odoo")
            }
            _ => false,
        })
        .map(|call| define(call, lines))
        .collect()
}

fn define(call: &CallExpression, lines: &Lines) -> JsDefine {
    let args = call.arguments.as_slice();
    let (name, rest) = match args.split_first() {
        Some((Argument::StringLiteral(s), rest)) => (s.value.as_str().map(str::to_owned), rest),
        _ => (None, args),
    };
    let Some((factory, before)) = rest.split_last() else {
        return JsDefine {
            name,
            ..JsDefine::default()
        };
    };
    let deps = match before.first() {
        Some(Argument::ArrayExpression(list)) => list
            .elements
            .iter()
            .filter_map(|e| match e {
                ArrayExpressionElement::StringLiteral(s) => literal(s, lines),
                _ => None,
            })
            .collect(),
        // Odoo tests `instanceof Array`, which a value read from a name or a call may pass. Its
        // items are out of reach, but the factory's calls are then no dependencies.
        Some(
            Argument::Identifier(_)
            | Argument::StaticMemberExpression(_)
            | Argument::ComputedMemberExpression(_)
            | Argument::CallExpression(_),
        ) => vec![],
        _ => return implicit(name, factory, lines),
    };
    JsDefine {
        name,
        deps,
        listed: true,
    }
}

/// A define whose dependencies are the `require` calls of its factory.
fn implicit(name: Option<String>, factory: &Argument, lines: &Lines) -> JsDefine {
    let mut requires = Requires { lines, found: vec![] };
    requires.visit_argument(factory);
    JsDefine {
        name,
        deps: requires.found,
        listed: false,
    }
}

/// The `require("x")` calls Odoo's pattern reads: a plain `require` given one string without a
/// quote or a blank.
struct Requires<'f> {
    lines: &'f Lines,
    found: Vec<Spanned<String>>,
}

impl<'a> Visit<'a> for Requires<'_> {
    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if let Expression::Identifier(callee) = &call.callee
            && callee.name == "require"
            && let [Argument::StringLiteral(s)] = call.arguments.as_slice()
            && let Some(dep) = literal(s, self.lines)
            && !dep.value.is_empty()
            && !dep.value.contains(|c: char| c.is_whitespace() || c == '\'' || c == '"')
        {
            self.found.push(dep);
        }
        walk_call_expression(self, call);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_kinds_the_view_registry_adds() {
        let source = r#"
            view_registry.add("a", A);
            core.view_registry.add("b", B);
            view_registry.add("c", C).add("d", D);
            view_registry.add(kind, E);
            core.one2many_view_registry.add("f", F);
            registry.add("g", G);
        "#;
        assert_eq!(analyze(source).view_kinds, ["a", "b", "d", "c"]);
    }

    #[test]
    fn reads_the_annotation_from_the_leading_comments() {
        assert_eq!(
            annotation("/** @odoo-module **/\nimport x from 'y';"),
            Annotation::Module
        );
        assert_eq!(annotation("// @odoo-module ignore\n"), Annotation::Ignore);
        assert_eq!(
            annotation("/* license */\n/** @odoo-module alias=a.b **/"),
            Annotation::Module
        );
        assert_eq!(annotation("import x from 'y'; // @odoo-module"), Annotation::None);
    }

    #[test]
    fn reads_the_alias_after_the_annotation() {
        let read = |text: &str| alias(text).map(|a| (a.name, a.default));
        let some = |name: &str, default: Option<&str>| Some((name.to_owned(), default.map(str::to_owned)));
        assert_eq!(
            read("/** @odoo-module alias=@web/../tests/utils default=false */"),
            some("@web/../tests/utils", Some("false"))
        );
        assert_eq!(read("// @odoo-module alias=web.Widget"), some("web.Widget", None));
        assert_eq!(read("/** @odoo-module alias=a.b**/"), some("a.b", None));
        assert_eq!(read("/* @odoo-module alias=a.b default=0*/"), some("a.b", Some("0")));
        assert_eq!(read("/** @odoo-module ignore alias=a.b */"), None);
        assert_eq!(read("/** @odoo-module default=false alias=a.b */"), None);
        assert_eq!(read("/** @odoo-module alias= */"), None);
        assert_eq!(read("/** @odoo-module **/"), None);
    }

    #[test]
    fn a_series_reads_its_own_part_of_the_alias() {
        let registered = |text: &str, series: &str| {
            let alias = alias(text).unwrap();
            alias.registered(Series::parse(series)).map(|(n, p)| (n.to_owned(), p))
        };
        let tests = "/** @odoo-module alias=@web/../tests/utils default=false */";
        assert_eq!(registered(tests, "17.0"), None);
        assert_eq!(
            registered(tests, "saas~17.2"),
            Some(("@web/../tests/utils".to_owned(), true))
        );
        let widget = "/** @odoo-module alias=web.Widget default=true */";
        assert_eq!(registered(widget, "17.0"), Some(("web.Widget".to_owned(), false)));
        assert_eq!(registered(widget, "18.0"), Some(("web.Widget".to_owned(), true)));
        let zero = "/** @odoo-module alias=web.Widget default=0 */";
        assert_eq!(registered(zero, "16.0"), Some(("web.Widget".to_owned(), true)));
        let cut = "/** @odoo-module alias=web.Widget/x default=0 */";
        assert_eq!(registered(cut, "16.0"), Some(("web.Widget".to_owned(), false)));
        let bare = "/** @odoo-module alias=web.Widget */";
        assert_eq!(registered(bare, "20.0"), Some(("web.Widget".to_owned(), false)));
    }

    #[test]
    fn collects_imports_exports_and_defines() {
        let facts = analyze(
            r#"import { a, b as c } from "@web/x";
import d from "./y";
import * as ns from "@web/z";
export { e } from "@web/w";
export * from "@web/v";
export const f = 1;
export default class G {}
odoo.define("@web/legacy", [], function () {});
"#,
        );
        let specifiers: Vec<_> = facts.specifiers.iter().map(|s| s.value.as_str()).collect();
        assert_eq!(specifiers, ["@web/x", "./y", "@web/z", "@web/w", "@web/v"]);
        let names: Vec<_> = facts
            .names
            .iter()
            .map(|n| format!("{}:{}", n.specifier, n.name.value))
            .collect();
        assert_eq!(names, ["@web/x:a", "@web/x:b", "./y:default", "@web/w:e"]);
        assert_eq!(facts.exports, ["f", "default", "e"]);
        assert_eq!(facts.star_exports, ["@web/v"]);
        let names: Vec<_> = facts.defines.iter().map(|d| d.name.as_deref()).collect();
        assert_eq!(names, [Some("@web/legacy")]);
        assert_eq!(
            facts.export_locals,
            [("f".to_owned(), "f".to_owned()), ("default".to_owned(), "G".to_owned())]
        );
        assert_eq!(facts.reexports, [("e".to_owned(), "@web/w".to_owned(), "e".to_owned())]);
        assert_eq!(facts.names[1].local.as_deref(), Some("c"));
        assert!(facts.syntax_error.is_none());
    }

    #[test]
    fn collects_orm_calls_with_literal_names() {
        let facts = analyze(
            r#"this.orm.call("res.partner", "action_x", []);
orm.silent.searchRead("res.users", [], []);
this.env.services.orm.cache({ type: "disk" }).call("res.company", "read", []);
this.orm.call(this.props.resModel, "write", []);
this.orm.call("res.partner", method, []);
this.orm.call(model, method, []);
this.rpc.call("res.partner", "x");
"#,
        );
        let calls: Vec<_> = facts
            .orm_calls
            .iter()
            .map(|c| {
                let method = c.method.as_ref().map_or("", |m| m.value.as_str());
                let model = c.model.as_ref().map_or("", |m| m.value.as_str());
                let col = c.model.as_ref().or(c.method.as_ref()).map_or(0, |s| s.pos.col);
                format!("{col}:{model}:{method}")
            })
            .collect();
        assert_eq!(
            calls,
            [
                "16:res.partner:action_x",
                "24:res.users:",
                "53:res.company:read",
                "37::write",
                "16:res.partner:"
            ]
        );
    }

    #[test]
    fn reads_legacy_rpc_calls_routed_to_call_kw() {
        let facts = analyze(
            r#"this._rpc({ model: "res.partner", method: "read", args: [] });
self._rpc({"model": "res.users", "method": "_private"});
rpc.query({ model: this.modelName, method: "_hidden" });
widget.parent._rpc({ model: "res.company", method: name });
this._rpc({ route: "/web/x", model: "res.partner", method: "read" });
this._rpc({ ...options, model: "res.partner", method: "read" });
this._rpc({ [key]: "x", model: "res.partner", method: "read" });
this._rpc({ model: "res.partner", args: [] });
this._rpc({ model, method });
this._rpc("res.partner", "read");
other.query({ model: "res.partner", method: "read" });
_rpc({ model: "res.partner", method: "read" });
"#,
        );
        let calls: Vec<_> = facts
            .orm_calls
            .iter()
            .map(|c| {
                let show = |s: &Option<Spanned<String>>| {
                    s.as_ref()
                        .map_or(String::new(), |s| format!("{}@{}:{}", s.value, s.pos.line, s.pos.col))
                };
                format!("{}/{}", show(&c.model), show(&c.method))
            })
            .collect();
        assert_eq!(
            calls,
            [
                "res.partner@1:21/read@1:44",
                "res.users@2:22/_private@2:45",
                "/_hidden@3:45",
                "res.company@4:30/",
            ]
        );
    }

    #[test]
    fn reads_the_this_super_calls_of_a_named_patch() {
        let facts = analyze(
            r#"patch(A.prototype, "mod.A", {
    setup() { this._super(...arguments); },
    save: function () { return this._super.apply(this, arguments); },
    get value() { return this._super.call(this); },
    load() {
        const run = () => this._super();
        function nested() { return this._super(); }
        class Inner { m() { this._super(); } }
        return this.other._super();
    },
    arrow: () => this._super(),
    plain() { return super.plain(); },
});
patch(A.prototype, { m() { this._super(); return super.m(); } });
patch(A.prototype, name);
"#,
        );
        let shown: Vec<_> = facts
            .patches
            .iter()
            .map(|p| {
                let supers: Vec<_> = p
                    .supers
                    .iter()
                    .map(|s| format!("{}@{}:{}", s.value, s.pos.line, s.pos.col))
                    .collect();
                (p.named, p.members.join(","), supers.join(" "))
            })
            .collect();
        assert_eq!(
            shown,
            [
                (
                    true,
                    "setup,save,value,load,arrow,plain".to_owned(),
                    "setup@2:20 save@3:37 value@4:31 load@6:32".to_owned()
                ),
                (false, "m".to_owned(), "m@14:56".to_owned()),
            ]
        );
    }

    #[test]
    fn collects_classes_and_prototype_patches() {
        let facts = analyze(
            r#"class A extends B { static s() {} static template = "x.A"; m() {} get g() {} f = 1; }
class C extends mixin(B) {}
patch(A.prototype, { m() { return super.m() + super.g; }, n: 1 });
patch(A, { s() { super.s(); } });
C.template = "x.C";
"#,
        );
        let classes: Vec<_> = facts
            .classes
            .iter()
            .map(|c| (c.name.as_str(), &c.extends, c.members.join(",")))
            .collect();
        assert_eq!(
            classes,
            [
                ("A", &Heritage::Name("B".to_owned()), "m,g,f".to_owned()),
                ("C", &Heritage::Other, String::new())
            ]
        );
        let templates: Vec<_> = facts
            .classes
            .iter()
            .filter_map(|c| c.template.as_ref().map(|t| (t.value.as_str(), t.pos.line)))
            .collect();
        assert_eq!(templates, [("x.A", 1), ("x.C", 5)]);
        assert_eq!(facts.patches.len(), 1);
        assert_eq!(facts.patches[0].target, "A");
        assert_eq!(facts.patches[0].members, ["m", "n"]);
        let supers: Vec<_> = facts.patches[0].supers.iter().map(|s| s.value.as_str()).collect();
        assert_eq!(supers, ["m"]);
    }

    #[test]
    fn reads_the_dependencies_of_each_define() {
        let facts = analyze(
            r#"odoo.define("a.listed", ["web.core", "@web/x", 1], function (require) {
    require("not.read");
});
odoo.define("a.implicit", function (require) {
    "use strict";
    var core = require("web.core");
    x.require("member.call");
    require("two", "args");
    require(`template`);
    require("with space");
    function nested() {
        return require('./sibling');
    }
});
odoo.define(function (require) {
    require("anonymous.dep");
});
odoo.define("a.variable", deps, function (require) {
    require("not.read.either");
});
if (x) {
    odoo.define("nested.statement", function (require) { require("skipped"); });
}
"#,
        );
        let shown: Vec<_> = facts
            .defines
            .iter()
            .map(|d| {
                let deps: Vec<_> = d
                    .deps
                    .iter()
                    .map(|s| format!("{}@{}:{}", s.value, s.pos.line, s.pos.col))
                    .collect();
                (d.name.as_deref(), d.listed, deps.join(" "))
            })
            .collect();
        assert_eq!(
            shown,
            [
                (Some("a.listed"), true, "web.core@1:27 @web/x@1:39".to_owned()),
                (Some("a.implicit"), false, "web.core@6:25 ./sibling@12:25".to_owned()),
                (None, false, "anonymous.dep@16:14".to_owned()),
                (Some("a.variable"), true, String::new()),
            ]
        );
    }

    #[test]
    fn reports_the_first_syntax_error() {
        let facts = analyze("const a = ;\n");
        assert_eq!(facts.syntax_error.map(|e| e.pos.line), Some(1));
    }
}
