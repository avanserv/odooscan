//! Literal Odoo domains, and the field paths their terms name.

use ruff_python_ast::{self as ast, Expr};
use ruff_python_parser::parse_expression;

use crate::facts::{ActionValue, Spanned};

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

/// The field paths of the domain a record rule writes, at that domain.
pub fn written_fields(domain: &ActionValue) -> Vec<Spanned<String>> {
    let ActionValue::Literal(text) = domain else {
        return vec![];
    };
    let spanned = |value| Spanned { value, pos: text.pos };
    parse(&text.value)
        .unwrap_or_default()
        .into_iter()
        .map(spanned)
        .collect()
}

/// The name of the model a `search` of `ir.model` finds. Only one term `('model', '=', name)` tells
/// it without a run.
pub fn searched_model(search: &str) -> Option<String> {
    let parsed = parse_expression(search.trim()).ok()?;
    let [term] = items(parsed.expr())? else {
        return None;
    };
    let [
        Expr::StringLiteral(field),
        Expr::StringLiteral(op),
        Expr::StringLiteral(model),
    ] = items(term)?
    else {
        return None;
    };
    (field.value.to_str() == "model" && op.value.to_str() == "=").then(|| model.value.to_str().to_owned())
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
    fn unreadable_domains_name_no_field() {
        for text in [
            "[('a', '=', %(x)d)]",
            "[('a', '=', 1)] if x else []",
            "[('a', '=', 1)] + [('b', '=', 2)]",
            "dict(a=1)",
            "[(x.y, '=', 1)]",
        ] {
            assert_eq!(parse(text), None, "{text}");
        }
    }

    #[test]
    fn written_fields_sit_at_the_domain() {
        let pos = crate::facts::Pos { line: 3, col: 4 };
        let text = Spanned {
            value: "[('a.b', '=', 1)]".to_owned(),
            pos,
        };
        let fields = written_fields(&ActionValue::Literal(text));
        assert_eq!((fields[0].value.as_str(), fields[0].pos), ("a.b", pos));
        assert!(written_fields(&ActionValue::Opaque).is_empty());
        assert!(written_fields(&ActionValue::Unset).is_empty());
    }

    #[test]
    fn reads_the_model_a_search_finds() {
        assert_eq!(searched_model("[('model', '=', 'a.b')]").as_deref(), Some("a.b"));
        assert_eq!(
            searched_model(" [(\"model\", \"=\", \"a.b\")] ").as_deref(),
            Some("a.b")
        );
        for search in [
            "[('name', '=', 'a.b')]",
            "[('model', '!=', 'a.b')]",
            "[('model', 'in', ['a.b'])]",
            "[('model', '=', name)]",
            "[('model', '=', 'a.b'), ('name', '=', 'c')]",
            "[]",
            "[",
        ] {
            assert_eq!(searched_model(search), None, "{search}");
        }
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
