use std::collections::HashMap;
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

/// Workflow commands, which GitHub turns into annotations on the pull request without the
/// Advanced Security that SARIF upload needs on a private repository.
pub fn print_github(out: &mut impl Write, diags: &[&Diagnostic], rel: &dyn Fn(&Path) -> String) {
    for d in diags {
        let level = match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "notice",
        };
        let title = format!("{} {}", d.code, odooscan_core::rules::rule(d.code).name);
        let _ = writeln!(
            out,
            "::{level} file={},line={},col={},title={}::{}",
            escape_property(&rel(&d.path)),
            d.line,
            d.column,
            escape_property(&title),
            escape_data(&d.message)
        );
    }
}

fn escape_data(s: &str) -> String {
    s.replace('%', "%25").replace('\r', "%0D").replace('\n', "%0A")
}

fn escape_property(s: &str) -> String {
    escape_data(s).replace(':', "%3A").replace(',', "%2C")
}

/// GitLab's Code Quality report, shown in the merge request widget and diff.
pub fn gitlab(diags: &[&Diagnostic], rel: &dyn Fn(&Path) -> String) -> serde_json::Value {
    let mut seen: HashMap<u64, u32> = HashMap::new();
    let issues: Vec<_> = diags
        .iter()
        .map(|d| {
            let path = rel(&d.path);
            // GitLab tells new findings from old by fingerprint, so it leaves the line out, like the
            // baseline does, and counts repeats of the same finding in a file to keep each unique.
            let base = fnv1a(&[d.code, &path, &d.message]);
            let nth = seen.entry(base).or_default();
            *nth += 1;
            let severity = match d.severity {
                Severity::Error => "critical",
                Severity::Warning => "major",
                Severity::Info => "info",
            };
            json!({
                "description": format!("{}: {}", d.code, d.message),
                "check_name": d.code,
                "fingerprint": format!("{:016x}", fnv1a(&[&base.to_string(), &nth.to_string()])),
                "severity": severity,
                "location": {"path": path, "lines": {"begin": d.line}},
            })
        })
        .collect();
    json!(issues)
}

/// FNV-1a over the parts with a separator, stable across builds, unlike std's hasher.
fn fnv1a(parts: &[&str]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in parts.iter().flat_map(|p| p.bytes().chain([0])) {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    hash
}
