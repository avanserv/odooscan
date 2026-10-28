//! Literal Odoo domains, and the field paths their terms name.

use ruff_python_ast::{self as ast, Expr};
use ruff_python_parser::parse_expression;

fn items(expr: &Expr) -> Option<&[Expr]> {
    match expr {
        Expr::List(l) => Some(&l.elts),
        Expr::Tuple(t) => Some(&t.elts),
        _ => None,
    }
}

/// The string naming the field of each term, or None when a term is built at runtime, so the
/// domain cannot be read. The values of the terms may be any expression.
pub fn fields(expr: &Expr) -> Option<Vec<&ast::ExprStringLiteral>> {
    let mut out = vec![];
    for term in items(expr)? {
        match term {
            Expr::StringLiteral(s) if matches!(s.value.to_str(), "&" | "|" | "!") => {}
            // `(1, '=', 1)` is always true and names no field.
            _ => match items(term)?.first()? {
                Expr::StringLiteral(s) => out.push(s),
                Expr::NumberLiteral(_) => {}
                _ => return None,
            },
        }
    }
    Some(out)
}

/// The field paths of a domain written as text, as XML and string field attributes hold it.
pub fn parse(text: &str) -> Option<Vec<String>> {
    let parsed = parse_expression(text.trim()).ok()?;
    let fields = fields(parsed.expr())?;
    Some(fields.iter().map(|s| s.value.to_str().to_owned()).collect())
}

/// The fields a context's `group_by` key names, without a `:month`-style granularity.
pub fn group_by(context: &str) -> Vec<String> {
    let Ok(parsed) = parse_expression(context.trim()) else {
        return vec![];
    };
    let Expr::Dict(dict) = parsed.expr() else {
        return vec![];
    };
    let value = dict.items.iter().find_map(|i| match &i.key {
        Some(Expr::StringLiteral(k)) if k.value.to_str() == "group_by" => Some(&i.value),
        _ => None,
    });
    let names: Vec<&str> = match value {
        Some(Expr::StringLiteral(s)) => s.value.to_str().split(',').collect(),
        Some(v) => items(v)
            .unwrap_or_default()
            .iter()
            .filter_map(|e| e.as_string_literal_expr().map(|s| s.value.to_str()))
            .collect(),
        None => vec![],
    };
    names
        .into_iter()
        .map(|n| n.split(':').next().unwrap_or_default().trim().to_owned())
        .filter(|n| !n.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_fields_with_any_values() {
        let fields = parse("['|', ('state', '=', 'done'), ('user_id', '=', uid), (1, '=', 1)]");
        assert_eq!(fields, Some(vec!["state".to_owned(), "user_id".to_owned()]));
    }

    #[test]
    fn runtime_terms_skip_the_domain() {
        assert_eq!(parse("[('state', '=', 'done')] + extra"), None);
        assert_eq!(parse("[(field, '=', 1)]"), None);
        assert_eq!(parse("[*base, ('a', '=', 1)]"), None);
    }

    #[test]
    fn group_by_drops_granularity() {
        assert_eq!(group_by("{'group_by': 'date:month'}"), vec!["date"]);
        assert_eq!(
            group_by("{'group_by': ['stage_id', 'user_id']}"),
            vec!["stage_id", "user_id"]
        );
        assert!(group_by("{'search_default_mine': 1}").is_empty());
    }
}
