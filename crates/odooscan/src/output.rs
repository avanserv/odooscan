use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use odooscan_core::Diagnostic;
use odooscan_core::rules::{RULES, Severity, rule};
use serde_json::json;

/// Where the release workflow publishes `docs/`.
const DOCS: &str = "https://avanserv.github.io/odooscan/";

/// A rule's page on the site, derived from its code. Every report written links here, so the path
/// never changes.
pub fn help_url(code: &str) -> String {
    format!("{DOCS}rules/{code}/")
}

/// The page with its links to other rule pages made absolute, for a reader outside the site.
fn help_markdown(doc: &str) -> String {
    RULES.iter().fold(doc.to_owned(), |doc, r| {
        doc.replace(&format!("]({}.md)", r.code), &format!("]({})", help_url(r.code)))
    })
}

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
        let _ = writeln!(out, "{n:>6}  {code}  {}", rule(code).name);
    }
}

/// On a terminal, the code is coloured and links to its page (OSC 8), which terminals that do not
/// support links print as plain text.
pub fn print_text(out: &mut impl Write, diags: &[&Diagnostic], rel: &dyn Fn(&Path) -> String, terminal: bool) {
    for d in diags {
        let code = if terminal {
            let (on, off) = ansi(d.severity);
            format!("\x1b]8;;{}\x1b\\{on}{}{off}\x1b]8;;\x1b\\", help_url(d.code), d.code)
        } else {
            d.code.to_owned()
        };
        let _ = writeln!(out, "{}:{}:{}: {code} {}", rel(&d.path), d.line, d.column, d.message);
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
        .map(|r| {
            json!({
                "id": r.code,
                "name": r.name,
                "shortDescription": {"text": r.summary},
                "helpUri": help_url(r.code),
                "help": {"text": r.doc, "markdown": help_markdown(r.doc)},
                "properties": {"category": r.category},
            })
        })
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
        let title = format!("{} {}", d.code, rule(d.code).name);
        let _ = writeln!(
            out,
            "::{level} file={},line={},col={},title={}::{}",
            escape_property(&rel(&d.path)),
            d.line,
            d.column,
            escape_property(&title),
            escape_data(&format!("{}\n\nSee {}", d.message, help_url(d.code)))
        );
    }
}

fn escape_data(s: &str) -> String {
    s.replace('%', "%25").replace('\r', "%0D").replace('\n', "%0A")
}

fn escape_property(s: &str) -> String {
    escape_data(s).replace(':', "%3A").replace(',', "%2C")
}

/// GitLab's Code Quality report, shown in the merge request widget and diff. `content.body` is the
/// Code Climate field for a finding's documentation.
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
                "content": {"body": format!("[{} {}]({})", d.code, rule(d.code).name, help_url(d.code))},
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn diags() -> Vec<Diagnostic> {
        let at = |code, severity, line, message: &str| Diagnostic {
            code,
            severity,
            path: PathBuf::from("bad_sale/models/sale_order.py"),
            line,
            column: 5,
            message: message.to_owned(),
        };
        vec![
            at(
                "OD203",
                Severity::Error,
                7,
                "'partner_id.cde': res.partner has no field 'cde'",
            ),
            at("OD111", Severity::Info, 12, "route with auth='public' calls sudo()"),
        ]
    }

    fn rel(p: &Path) -> String {
        p.display().to_string()
    }

    fn text(terminal: bool) -> String {
        let diags = diags();
        let mut out = vec![];
        print_text(&mut out, &diags.iter().collect::<Vec<_>>(), &rel, terminal);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn text_links_the_code_on_a_terminal_only() {
        insta::assert_snapshot!("text_piped", text(false));
        insta::assert_snapshot!("text_terminal", text(true).escape_debug().to_string());
    }

    #[test]
    fn github_annotations_end_with_the_page() {
        let diags = diags();
        let mut out = vec![];
        print_github(&mut out, &diags.iter().collect::<Vec<_>>(), &rel);
        insta::assert_snapshot!(String::from_utf8(out).unwrap());
    }

    #[test]
    fn gitlab_issues_link_the_page() {
        let diags = diags();
        let report = gitlab(&diags.iter().collect::<Vec<_>>(), &rel);
        insta::assert_snapshot!(serde_json::to_string_pretty(&report).unwrap());
    }

    #[test]
    fn sarif_rules_carry_their_page() {
        let diags = diags();
        let mut report = sarif(&diags.iter().collect::<Vec<_>>(), &rel);
        let rules = &mut report["runs"][0]["tool"]["driver"]["rules"];
        let mut od203 = rules
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "OD203")
            .unwrap()
            .clone();
        assert_eq!(od203["helpUri"], "https://avanserv.github.io/odooscan/rules/OD203/");
        assert!(
            od203["help"]["markdown"]
                .as_str()
                .unwrap()
                .starts_with("# OD203 unresolved-field-path")
        );
        // Every rule and its whole page is in the report; the snapshot keeps one, without the page.
        od203["help"] = json!("<docs/rules/OD203.md>");
        *rules = json!([od203]);
        report["runs"][0]["tool"]["driver"]["version"] = json!("<version>");
        insta::assert_snapshot!(serde_json::to_string_pretty(&report).unwrap());
    }

    #[test]
    fn help_markdown_makes_rule_links_absolute() {
        let md = help_markdown("is [OD201](OD201.md) instead");
        assert_eq!(
            md,
            "is [OD201](https://avanserv.github.io/odooscan/rules/OD201/) instead"
        );
    }
}
