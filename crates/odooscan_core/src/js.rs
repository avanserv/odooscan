use oxc_allocator::Allocator;
use oxc_ast::ast::{Argument, Expression, Program, Statement};
use oxc_parser::Parser;
use oxc_span::SourceType;
use oxc_syntax::module_record::{ExportExportName, ExportImportName, ImportImportName, ModuleRecord};

use crate::facts::{Annotation, JsFacts, JsName, Lines, Pos, Spanned};

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
        facts
            .names
            .push(js_name(&entry.module_request.name, name, lines.pos(start as usize)));
    }
}

fn exports(record: &ModuleRecord, lines: &Lines, facts: &mut JsFacts) {
    for entry in record
        .local_export_entries
        .iter()
        .chain(&record.indirect_export_entries)
    {
        match &entry.export_name {
            ExportExportName::Name(n) => facts.exports.push(n.name.to_string()),
            ExportExportName::Default(_) => facts.exports.push("default".to_owned()),
            ExportExportName::Null => {}
        }
        if let (Some(request), ExportImportName::Name(n)) = (&entry.module_request, &entry.import_name) {
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

fn js_name(specifier: &str, name: String, pos: Pos) -> JsName {
    JsName {
        specifier: specifier.to_owned(),
        name: Spanned { value: name, pos },
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
        assert!(facts.syntax_error.is_none());
    }

    #[test]
    fn reports_the_first_syntax_error() {
        let facts = analyze("const a = ;\n");
        assert_eq!(facts.syntax_error.map(|e| e.pos.line), Some(1));
    }
}
