use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, CallExpression, Class, Expression, ObjectExpression, ObjectPropertyKind, Program, Statement,
};
use oxc_ast_visit::Visit;
use oxc_ast_visit::walk::{walk_call_expression, walk_class};
use oxc_parser::Parser;
use oxc_span::SourceType;
use oxc_syntax::module_record::{ExportExportName, ExportImportName, ImportImportName, ModuleRecord};

use crate::facts::{Annotation, Heritage, JsClass, JsFacts, JsName, JsPatch, Lines, Pos, Spanned};

pub fn analyze(text: &str) -> JsFacts {
    let lines = Lines::new(text);
    let mut facts = JsFacts {
        annotation: annotation(text),
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
    facts.defines = defines(&parsed.program);
    Collector {
        lines: &lines,
        facts: &mut facts,
    }
    .visit_program(&parsed.program);
    facts
}

/// The `@odoo-module` annotation, which Odoo reads in the comments that open the file.
fn annotation(text: &str) -> Annotation {
    let mut rest = text.trim_start();
    loop {
        let (comment, after) = if let Some(r) = rest.strip_prefix("//") {
            r.split_once('\n').unwrap_or((r, ""))
        } else if let Some(r) = rest.strip_prefix("/*") {
            r.split_once("*/").unwrap_or((r, ""))
        } else {
            return Annotation::None;
        };
        if let Some((_, tail)) = comment.split_once("@odoo-module") {
            let ignore = tail.starts_with(char::is_whitespace) && tail.split_whitespace().next() == Some("ignore");
            return if ignore { Annotation::Ignore } else { Annotation::Module };
        }
        rest = after.trim_start();
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
            });
        }
        walk_class(self, class);
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if let Some(patch) = prototype_patch(call, self.lines) {
            self.facts.patches.push(patch);
        }
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
    let Argument::ObjectExpression(extension) = call.arguments.get(1)? else {
        return None;
    };
    let mut supers = Supers { lines, found: vec![] };
    supers.visit_object_expression(extension);
    Some(JsPatch {
        target,
        members: members(extension),
        supers: supers.found,
    })
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

/// Names given to top-level `odoo.define("name", ...)` calls.
fn defines(program: &Program) -> Vec<String> {
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
        .filter_map(|call| match call.arguments.first() {
            Some(Argument::StringLiteral(s)) => s.value.as_str().map(str::to_owned),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(facts.defines, ["@web/legacy"]);
        assert_eq!(
            facts.export_locals,
            [("f".to_owned(), "f".to_owned()), ("default".to_owned(), "G".to_owned())]
        );
        assert_eq!(facts.reexports, [("e".to_owned(), "@web/w".to_owned(), "e".to_owned())]);
        assert_eq!(facts.names[1].local.as_deref(), Some("c"));
        assert!(facts.syntax_error.is_none());
    }

    #[test]
    fn collects_classes_and_prototype_patches() {
        let facts = analyze(
            r#"class A extends B { static s() {} m() {} get g() {} f = 1; }
class C extends mixin(B) {}
patch(A.prototype, { m() { return super.m() + super.g; }, n: 1 });
patch(A, { s() { super.s(); } });
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
        assert_eq!(facts.patches.len(), 1);
        assert_eq!(facts.patches[0].target, "A");
        assert_eq!(facts.patches[0].members, ["m", "n"]);
        let supers: Vec<_> = facts.patches[0].supers.iter().map(|s| s.value.as_str()).collect();
        assert_eq!(supers, ["m"]);
    }

    #[test]
    fn reports_the_first_syntax_error() {
        let facts = analyze("const a = ;\n");
        assert_eq!(facts.syntax_error.map(|e| e.pos.line), Some(1));
    }
}
