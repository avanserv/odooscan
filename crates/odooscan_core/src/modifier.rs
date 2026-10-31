//! The expressions of view modifiers, read as Odoo validates them when it loads a view.

use std::collections::BTreeSet;
use std::iter::once;
use std::sync::LazyLock;

use regex::{Captures, Regex};
use ruff_python_ast::Expr;
use ruff_python_parser::parse_expression;

/// What the data loader rewrites in an arch: `%(id)d` and `%(id)s` to a database id, `%%` to `%`.
/// Taking `%%` first, as the loader does, leaves `%%(id)d` a literal `%(id)d`.
static LOADER_ESCAPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"%%|%\(.*?\)[ds]").unwrap());

/// Names Odoo's view validation ignores, and `id`, which every record has. They only count at the
/// root of a name: `parent.uid` is a field of the parent.
const CONTEXT: &[&str] = &[
    "abs",
    "allowed_company_ids",
    "bool",
    "context",
    "context_today",
    "current_company_id",
    "current_date",
    "datetime",
    "float",
    "id",
    "len",
    "now",
    "relativedelta",
    "self",
    "set",
    "str",
    "time",
    "today",
    "uid",
    "unicode",
];

const REFUSED: &str = "Odoo does not read this construct, such as a comprehension, a set or a lambda";

/// Whether Odoo takes `value` without parsing it. It compares the value as written, so blanks around
/// one of these make it an expression. Empty attributes are no expression.
pub fn constant(value: &str) -> bool {
    value.is_empty()
        || ["0", "1", "true", "false"]
            .iter()
            .any(|c| value.eq_ignore_ascii_case(c))
}

/// The fields an expression reads: `parent.x` whole, and otherwise the name a path starts from. An
/// expression Odoo refuses, for its syntax or a construct its walk leaves out, is the error.
pub fn names(text: &str) -> Result<Vec<String>, String> {
    let text = LOADER_ESCAPE.replace_all(text, |c: &Captures| if &c[0] == "%%" { "%" } else { "0" });
    let parsed = parse_expression(text.trim()).map_err(|e| e.error.to_string())?;
    let mut names = BTreeSet::new();
    for name in values(parsed.expr())? {
        let root = name.split('.').next().unwrap_or_default();
        let read = match root {
            "parent" => (name != root).then_some(name.as_str()),
            _ => (!CONTEXT.contains(&root)).then_some(root),
        };
        names.extend(read.map(str::to_owned));
    }
    Ok(names.into_iter().collect())
}

/// Every name and dotted path under `expr`. A path off several names, as in `(a or b).c`, stops at
/// those names, and one off none, as in `'a'.b`, is nothing.
fn values(expr: &Expr) -> Result<BTreeSet<String>, String> {
    let mut found = BTreeSet::new();
    match expr {
        Expr::Name(name) => {
            found.insert(name.id.to_string());
        }
        Expr::Attribute(attribute) => {
            found = values(&attribute.value)?;
            if found.len() == 1 {
                found = found.iter().map(|path| format!("{path}.{}", attribute.attr)).collect();
            }
        }
        _ => {
            for child in children(expr)? {
                found.extend(values(child)?);
            }
        }
    }
    Ok(found)
}

/// The operands of the nodes Odoo's walk enters. Keyword arguments are not among them, so a name
/// there goes unread.
fn children(expr: &Expr) -> Result<Vec<&Expr>, String> {
    match expr {
        _ if expr.is_literal_expr() => Ok(vec![]),
        Expr::Dict(e) => e
            .items
            .iter()
            .flat_map(|item| [item.key.as_ref().ok_or(REFUSED.to_owned()), Ok(&item.value)])
            .collect(),
        _ => sequence(expr).or_else(|| operands(expr)).ok_or(REFUSED.to_owned()),
    }
}

fn sequence(expr: &Expr) -> Option<Vec<&Expr>> {
    match expr {
        Expr::List(e) => Some(e.elts.iter().collect()),
        Expr::Tuple(e) => Some(e.elts.iter().collect()),
        Expr::BoolOp(e) => Some(e.values.iter().collect()),
        Expr::Compare(e) => Some(e.operands.iter().collect()),
        Expr::Call(e) => Some(once(&*e.func).chain(&e.arguments.args).collect()),
        _ => None,
    }
}

fn operands(expr: &Expr) -> Option<Vec<&Expr>> {
    match expr {
        Expr::Subscript(e) => Some(vec![&e.value, &e.slice]),
        Expr::BinOp(e) => Some(vec![&e.left, &e.right]),
        Expr::UnaryOp(e) => Some(vec![&e.operand]),
        Expr::If(e) => Some(vec![&e.test, &e.body, &e.orelse]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Vec<String> {
        names(text).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    #[test]
    fn a_constant_is_the_value_as_written_in_any_case() {
        for value in ["", "0", "1", "true", "false", "TRUE", "True", "fAlSe"] {
            assert!(constant(value), "{value:?}");
        }
        for value in [" ", " true", "true ", "\n    true\n", "\n0\n", "None", "2", "not true"] {
            assert!(!constant(value), "{value:?}");
        }
    }

    #[test]
    fn reads_the_names_of_each_node_kind() {
        assert_eq!(read("a == 1 and (b or not c)"), ["a", "b", "c"]);
        assert_eq!(read("a if b else [c, (d,)]"), ["a", "b", "c", "d"]);
        assert_eq!(read("{'x': a, b: 1}[c] + d"), ["a", "b", "c", "d"]);
        assert_eq!(read("a == ... or b == b'' or c == 1.5"), ["a", "b", "c"]);
    }

    #[test]
    fn a_call_reads_its_positional_arguments_only() {
        assert_eq!(read("f(a, key=b)"), ["a", "f"]);
    }

    #[test]
    fn a_path_counts_from_its_root_except_under_parent() {
        assert_eq!(read("a.b.c"), ["a"]);
        assert_eq!(read("parent.a.b"), ["parent.a.b"]);
        assert_eq!(read("parent.a"), ["parent.a"]);
        assert_eq!(read("(a or b).c"), ["a", "b"]);
        assert!(read("'a'.upper").is_empty());
    }

    #[test]
    fn context_names_and_bare_parent_read_no_field() {
        assert!(read("context.get('a') and uid and id and parent and True and None").is_empty());
        assert_eq!(read("parent.uid"), ["parent.uid"]);
        assert_eq!(read("parent.parent.a"), ["parent.parent.a"]);
    }

    #[test]
    fn lowercase_constants_are_names() {
        assert_eq!(read("a and true"), ["a", "true"]);
    }

    #[test]
    fn substitutes_what_the_loader_rewrites() {
        assert_eq!(read("a == %(base.x)d"), ["a"]);
        assert_eq!(read("a == %(base.x)s"), ["a"]);
        assert_eq!(read("a %% 2"), ["a"]);
        assert!(names("a == %%(base.x)d").is_err());
    }

    #[test]
    fn refuses_a_syntax_error() {
        assert!(names("a and").is_err());
        assert!(names("  ").is_err());
        assert!(names("a = 1").is_err());
        assert!(names("a and\nb").is_err());
    }

    #[test]
    fn refuses_what_the_walk_leaves_out() {
        for text in [
            "[a for a in b]",
            "{a for a in b}",
            "(a for a in b)",
            "{a: 1 for a in b}",
            "lambda a: a",
            "{a, b}",
            "f'{a}'",
            "a[1:2]",
            "(a := 1)",
            "[*a]",
            "f(*a)",
            "{**a}",
        ] {
            assert_eq!(names(text), Err(REFUSED.to_owned()), "{text}");
        }
    }

    #[test]
    fn trims_what_an_attribute_spec_leaves_around_the_text() {
        assert_eq!(read("\n    a\n"), ["a"]);
    }
}
