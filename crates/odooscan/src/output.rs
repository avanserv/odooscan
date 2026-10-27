use std::io::{IsTerminal, Write};
use std::path::Path;

use odooscan_core::Diagnostic;
use odooscan_core::rules::{RULES, Severity};
use serde_json::json;

pub fn print_statistics(out: &mut impl Write, diags: &[&Diagnostic]) {
    let mut counts: Vec<(&str, usize)> = vec![];
    for d in diags {
        match counts.iter_mut().find(|(c, _)| *c == d.code) {
            Some((_, n)) => *n += 1,
            None => counts.push((d.code, 1)),
        }
    }
    counts.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    for (code, n) in counts {
        let _ = writeln!(out, "{n:>6}  {code}  {}", odooscan_core::rules::rule(code).name);
    }
}

pub fn print_text(out: &mut impl Write, diags: &[&Diagnostic], rel: &dyn Fn(&Path) -> String) {
    let color = std::io::stdout().is_terminal();
    for d in diags {
        let (on, off) = if color { ansi(d.severity) } else { ("", "") };
        let _ = writeln!(
            out,
            "{}:{}:{}: {on}{}{off} {}",
            rel(&d.path),
            d.line,
            d.column,
            d.code,
            d.message
        );
    }
}

fn ansi(severity: Severity) -> (&'static str, &'static str) {
    match severity {
        Severity::Error => ("\x1b[1;31m", "\x1b[0m"),
        Severity::Warning => ("\x1b[1;33m", "\x1b[0m"),
        Severity::Info => ("\x1b[1;36m", "\x1b[0m"),
    }
}

pub fn sarif(diags: &[&Diagnostic], rel: &dyn Fn(&Path) -> String) -> serde_json::Value {
    let rules: Vec<_> = RULES
        .iter()
        .map(|r| json!({"id": r.code, "name": r.name, "shortDescription": {"text": r.summary}, "properties": {"category": r.category}}))
        .collect();
    let results: Vec<_> = diags
        .iter()
        .map(|d| {
            let level = match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Info => "note",
            };
            json!({
                "ruleId": d.code,
                "level": level,
                "message": {"text": d.message},
                "locations": [{"physicalLocation": {
                    "artifactLocation": {"uri": rel(&d.path)},
                    "region": {"startLine": d.line, "startColumn": d.column},
                }}],
            })
        })
        .collect();
    json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{"tool": {"driver": {"name": "odooscan", "version": env!("CARGO_PKG_VERSION"), "rules": rules}}, "results": results}],
    })
}
