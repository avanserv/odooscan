//! What an `eval` writes, as far as it reads without a run: `ref()`, x2many commands, a string.

use ruff_python_ast::{self as ast, Expr};
use ruff_python_parser::parse_expression;

use crate::facts::{Spanned, WrittenValue};

/// The value of an `eval`. Most are other expressions, so only those that may hold a value are parsed.
pub fn written(eval: &Spanned<String>) -> WrittenValue {
    let text = eval.value.trim();
    if !text.contains("ref(") && !text.starts_with(['\'', '"']) {
        return WrittenValue::Opaque;
    }
    let Ok(parsed) = parse_expression(text) else {
        return WrittenValue::Opaque;
    };
    let spanned = |value: String| Spanned { value, pos: eval.pos };
    let ids = |ids: Vec<String>| WrittenValue::Ids(ids.into_iter().map(spanned).collect());
    match parsed.expr() {
        // An empty string clears the field, as Odoo validates only what is true.
        Expr::StringLiteral(s) if s.value.is_empty() => WrittenValue::Opaque,
        Expr::StringLiteral(s) => WrittenValue::Text(spanned(s.value.to_str().to_owned())),
        Expr::List(list) => commands(&list.elts).map_or(WrittenValue::Opaque, ids),
        expr => ref_id(expr).map_or(WrittenValue::Opaque, |id| ids(vec![id])),
    }
}

/// The id of a `ref()` call: its second argument only says what a missing id gives.
fn ref_id(expr: &Expr) -> Option<String> {
    let Expr::Call(call) = expr else { return None };
    let Expr::Name(name) = &*call.func else { return None };
    if name.id.as_str() != "ref" {
        return None;
    }
    let Expr::StringLiteral(id) = call.arguments.args.first()? else {
        return None;
    };
    Some(id.value.to_str().to_owned())
}

/// The ids a list of x2many commands names. None when an item is no command, or holds an id only a
/// run tells.
fn commands(items: &[Expr]) -> Option<Vec<String>> {
    let mut ids = vec![];
    for item in items {
        match item {
            Expr::Tuple(t) => tuple_command(&t.elts, &mut ids)?,
            Expr::Call(c) => call_command(c, &mut ids)?,
            _ => return None,
        }
    }
    Some(ids)
}

/// `(0, ...)` creates a record and `(5, ...)` clears the field, so neither names one.
fn tuple_command(elts: &[Expr], ids: &mut Vec<String>) -> Option<()> {
    let Expr::NumberLiteral(code) = elts.first()? else {
        return None;
    };
    let ast::Number::Int(code) = &code.value else {
        return None;
    };
    match code.as_u64()? {
        0 | 5 => Some(()),
        1..=4 => push_ref(elts.get(1)?, ids),
        6 => push_refs(elts.get(2)?, ids),
        _ => None,
    }
}

fn call_command(call: &ast::ExprCall, ids: &mut Vec<String>) -> Option<()> {
    let Expr::Attribute(method) = &*call.func else {
        return None;
    };
    if !matches!(&*method.value, Expr::Name(n) if n.id.as_str() == "Command") {
        return None;
    }
    let first = call.arguments.args.first();
    match method.attr.as_str() {
        "create" | "clear" => Some(()),
        "link" | "unlink" | "update" | "delete" => push_ref(first?, ids),
        "set" => push_refs(first?, ids),
        _ => None,
    }
}

fn push_ref(expr: &Expr, ids: &mut Vec<String>) -> Option<()> {
    ids.push(ref_id(expr)?);
    Some(())
}

fn push_refs(expr: &Expr, ids: &mut Vec<String>) -> Option<()> {
    let items = match expr {
        Expr::List(l) => &l.elts,
        Expr::Tuple(t) => &t.elts,
        _ => return None,
    };
    items.iter().try_for_each(|item| push_ref(item, ids))
}

#[cfg(test)]
mod tests {
    use crate::facts::Pos;

    use super::*;

    fn read(eval: &str) -> Option<Vec<String>> {
        let value = eval.to_owned();
        let pos = Pos { line: 1, col: 1 };
        match written(&Spanned { value, pos }) {
            WrittenValue::Ids(ids) => Some(ids.into_iter().map(|i| i.value).collect()),
            WrittenValue::Text(text) => Some(vec![format!("text:{}", text.value)]),
            WrittenValue::Opaque => None,
        }
    }

    fn ids(ids: &[&str]) -> Option<Vec<String>> {
        Some(ids.iter().map(|i| (*i).to_owned()).collect())
    }

    #[test]
    fn reads_a_whole_ref() {
        assert_eq!(read(" ref('a.b') "), ids(&["a.b"]));
        assert_eq!(read("ref('a', False)"), ids(&["a"]));
        assert_eq!(read("ref('a', raise_if_not_found=False)"), ids(&["a"]));
    }

    #[test]
    fn reads_the_ids_of_x2many_commands() {
        assert_eq!(
            read("[(1, ref('a'), {}), (2, ref('b')), (3, ref('c')), (4, ref('d'))]"),
            ids(&["a", "b", "c", "d"])
        );
        assert_eq!(read("[(6, 0, [ref('a'), ref('b')])]"), ids(&["a", "b"]));
        assert_eq!(read("[(6, 0, (ref('a'),))]"), ids(&["a"]));
        assert_eq!(
            read("[Command.link(ref('a')), Command.unlink(ref('b'))]"),
            ids(&["a", "b"])
        );
        assert_eq!(
            read("[Command.update(ref('a'), {}), Command.delete(ref('b'))]"),
            ids(&["a", "b"])
        );
        assert_eq!(read("[Command.set([ref('a')])]"), ids(&["a"]));
    }

    #[test]
    fn commands_that_name_no_record_pass() {
        assert_eq!(read("[(5, 0, 0), (4, ref('a'))]"), ids(&["a"]));
        assert_eq!(read("[(0, 0, {'x': ref('a')}), (4, ref('b'))]"), ids(&["b"]));
        assert_eq!(read("[Command.clear(), Command.link(ref('a'))]"), ids(&["a"]));
        assert_eq!(read("[Command.create({'x': ref('a')})]"), ids(&[]));
    }

    #[test]
    fn reads_a_whole_string() {
        assert_eq!(read("'draft'"), ids(&["text:draft"]));
        assert_eq!(read("\"ref(x)\""), ids(&["text:ref(x)"]));
        assert_eq!(read("'a' 'b'"), ids(&["text:ab"]));
        assert_eq!(read("' a '"), ids(&["text: a "]));
    }

    #[test]
    fn leaves_what_only_a_run_tells() {
        for eval in [
            "",
            "''",
            "False",
            "1",
            "'a' + 'b'",
            "'a' if x else 'b'",
            "ref('a') or False",
            "ref('a.' + name)",
            "ref(name)",
            "obj().env.ref('a').id",
            "[ref('a')]",
            "[(4, ref('a')), (4, 7)]",
            "[(4, 7)]",
            "[(6, 0, [ref('a'), 7])]",
            "[(7, ref('a'))]",
            "[Command.link(7)]",
            "[Command.other(ref('a'))]",
            "[Other.link(ref('a'))]",
            "(4, ref('a'))",
            "ref(",
        ] {
            assert_eq!(read(eval), None, "{eval}");
        }
    }
}
