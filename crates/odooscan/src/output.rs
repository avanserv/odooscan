use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use odooscan_core::Diagnostic;
use odooscan_core::rules::{Kind, RULES, Severity, rule};
use odooscan_core::series::Series;
use serde_json::json;

/// Where the release workflow publishes `docs/`.
const DOCS: &str = "https://avanserv.github.io/odooscan/";

/// How many related lines the text, GitHub and GitLab reports name. JSON lists them all.
const RELATED_SHOWN: usize = 20;

/// How many related locations a SARIF result lists. The cap follows GitHub code scanning's limit
/// of 1,000 locations per result, past which it rejects the whole file.
const SARIF_RELATED: usize = 1000;

/// A rule's page on the site, derived from its code. Every report written links here, so the path
/// never changes.
pub fn help_url(code: &str) -> String {
    format!("{DOCS}rules/{code}/")
}

/// The page of rule `code` with its links into the rules section made absolute, for a reader
/// outside the site: SARIF viewers and `oscn explain`.
pub fn help_markdown(code: &str, doc: &str) -> String {
    let mut out = String::with_capacity(doc.len());
    let mut rest = doc;
    while let Some(at) = rest.find("](") {
        let (head, tail) = rest.split_at(at + 2);
        out.push_str(head);
        rest = tail;
        if let Some(end) = rest.find(')')
            && let Some(url) = absolute(code, &rest[..end])
        {
            out.push_str(&url);
            rest = &rest[end..];
        }
    }
    out.push_str(rest);
    out
}

/// The site URL of a link target on the page of `code`: `ODxxx.md`, `index.md` or a same-page
/// anchor. Any other target gives `None` and stays as written, whether absolute, a relative form
/// this does not map, or not a link at all, such as `](float(amount)` in a code block.
fn absolute(code: &str, target: &str) -> Option<String> {
    let (path, anchor) = match target.split_once('#') {
        Some((path, anchor)) => (path, format!("#{anchor}")),
        None => (target, String::new()),
    };
    let page = match path {
        "" if !anchor.is_empty() => help_url(code),
        "index.md" => format!("{DOCS}rules/"),
        _ => help_url(
            path.strip_suffix(".md")
                .filter(|stem| RULES.iter().any(|r| r.code == *stem))?,
        ),
    };
    Some(format!("{page}{anchor}"))
}

/// `n` and `noun`, plural unless `n` is 1. Every noun the summary lines count is regular.
pub fn count(n: usize, noun: &str) -> String {
    let s = if n == 1 { "" } else { "s" };
    format!("{n} {noun}{s}")
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

/// The counts per kind, every kind listed so that they add up to the total, then per rule.
pub fn print_summary(out: &mut impl Write, diags: &[&Diagnostic], target: Series) {
    let _ = writeln!(out, "Findings by kind, checked as {target}:");
    for kind in Kind::ALL {
        let n = diags.iter().filter(|d| rule(d.code).kind(d.severity) == kind).count();
        let _ = writeln!(out, "{n:>6}  {}", kind.label());
    }
    let _ = writeln!(out);
    print_statistics(out, diags);
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
        let related = d
            .related_lines(RELATED_SHOWN)
            .map(|l| format!(" ({l})"))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "{}:{}:{}: {code} {}{related}",
            rel(&d.path),
            d.line,
            d.column,
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
        .map(|r| {
            json!({
                "id": r.code,
                "name": r.name,
                "shortDescription": {"text": r.summary},
                "helpUri": help_url(r.code),
                "help": {"text": r.doc, "markdown": help_markdown(r.code, r.doc)},
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
            let mut result = json!({
                "ruleId": d.code,
                "level": level,
                "message": {"text": d.message},
                "locations": [{"physicalLocation": physical(rel(&d.path), d.line, d.column)}],
            });
            if !d.related.is_empty() {
                let related: Vec<_> = d
                    .related
                    .iter()
                    .take(SARIF_RELATED)
                    .enumerate()
                    .map(|(id, s)| {
                        json!({
                            "id": id,
                            "physicalLocation": physical(rel(&s.path), s.line, s.column),
                            "message": {"text": s.label},
                        })
                    })
                    .collect();
                result["relatedLocations"] = json!(related);
                let omitted = d.related.len() - related.len();
                if omitted > 0 {
                    result["properties"] = json!({"relatedLocationsOmitted": omitted});
                }
            }
            if !d.flow.is_empty() {
                let steps: Vec<_> = d
                    .flow
                    .iter()
                    .map(|s| {
                        json!({"location": {
                            "physicalLocation": physical(rel(&s.path), s.line, s.column),
                            "message": {"text": s.label},
                        }})
                    })
                    .collect();
                result["codeFlows"] = json!([{"threadFlows": [{"locations": steps}]}]);
            }
            result
        })
        .collect();
    json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{"tool": {"driver": {"name": "odooscan", "version": env!("CARGO_PKG_VERSION"), "rules": rules}}, "results": results}],
    })
}

fn physical(uri: String, line: u32, column: u32) -> serde_json::Value {
    json!({"artifactLocation": {"uri": uri}, "region": {"startLine": line, "startColumn": column}})
}

/// The steps of a data flow, one `path:line:column label` line each, after a blank line.
fn flow_lines(d: &Diagnostic, rel: &dyn Fn(&Path) -> String, bullet: &str) -> String {
    if d.flow.is_empty() {
        return String::new();
    }
    d.flow
        .iter()
        .map(|s| format!("\n{bullet}{}:{}:{} {}", rel(&s.path), s.line, s.column, s.label))
        .fold("\n".to_owned(), |acc, l| acc + &l)
}

/// The related lines, as a paragraph after a blank line.
fn related_paragraph(d: &Diagnostic) -> String {
    d.related_lines(RELATED_SHOWN).map_or_else(String::new, |l| {
        let mut chars = l.chars();
        let first = chars.next().map(|c| c.to_ascii_uppercase()).unwrap_or_default();
        format!("\n\n{first}{}", chars.as_str())
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
            escape_data(&format!(
                "{}{}{}\n\nSee {}",
                d.message,
                flow_lines(d, rel, "  "),
                related_paragraph(d),
                help_url(d.code)
            ))
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
                "content": {"body": format!("[{} {}]({}){}{}", d.code, rule(d.code).name, help_url(d.code), flow_lines(d, rel, "- "), related_paragraph(d))},
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

    use odooscan_core::FlowStep;

    use super::*;

    fn diags() -> Vec<Diagnostic> {
        let at = |code, severity, line, message: &str| Diagnostic {
            code,
            severity,
            path: PathBuf::from("bad_sale/models/sale_order.py"),
            line,
            column: 5,
            message: message.to_owned(),
            flow: vec![],
            related: vec![],
            fix: None,
        };
        let step = |line, label: &str| FlowStep {
            path: PathBuf::from("bad_sale/controllers/main.py"),
            line,
            column: 9,
            label: label.to_owned(),
        };
        let mut flow = at(
            "OD701",
            Severity::Error,
            14,
            "route parameter 'term' reaches the query of cr.execute() through 'query'",
        );
        flow.path = PathBuf::from("bad_sale/controllers/main.py");
        flow.flow = vec![
            step(11, "route parameter 'term'"),
            step(12, "query"),
            step(14, "cr.execute()"),
        ];
        vec![
            at(
                "OD203",
                Severity::Error,
                7,
                "'partner_id.cde': res.partner has no field 'cde'",
            ),
            at("OD111", Severity::Info, 12, "route with auth='public' calls sudo()"),
            flow,
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
    fn summary_counts_every_kind_then_every_rule() {
        let mut diags = diags();
        diags[0].code = "OD126";
        diags[1].code = "OD324";
        let mut out = vec![];
        print_summary(
            &mut out,
            &diags.iter().collect::<Vec<_>>(),
            Series::parse("18.0").unwrap(),
        );
        let expected = "Findings by kind, checked as 18.0:
     1  removed API
     0  deprecated API
     1  missing upstream
     1  other

     1  OD126  python-api-removed
     1  OD324  missing-required-field
     1  OD701  sql-injection-flow
";
        assert_eq!(String::from_utf8(out).unwrap(), expected);
    }

    #[test]
    fn a_count_of_one_is_singular() {
        assert_eq!(
            [count(0, "file"), count(1, "file"), count(2, "file")],
            ["0 files", "1 file", "2 files"]
        );
    }

    #[test]
    fn summary_kinds_add_up_to_the_total() {
        let diags: Vec<_> = RULES
            .iter()
            .map(|r| Diagnostic {
                code: r.code,
                severity: r.severity,
                path: PathBuf::from("bad_sale/models/sale_order.py"),
                line: 1,
                column: 1,
                message: String::new(),
                flow: vec![],
                related: vec![],
                fix: None,
            })
            .collect();
        let mut out = vec![];
        print_summary(
            &mut out,
            &diags.iter().collect::<Vec<_>>(),
            Series::parse("18.0").unwrap(),
        );
        let out = String::from_utf8(out).unwrap();
        let kinds: usize = out
            .lines()
            .skip(1)
            .take(Kind::ALL.len())
            .map(|l| l.split_whitespace().next().unwrap().parse::<usize>().unwrap())
            .sum();
        assert_eq!(kinds, RULES.len());
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

    /// An OD803 finding on `fr.po` whose `count` untranslated entries are at lines 10, 20 and on.
    fn untranslated(count: u32) -> Diagnostic {
        let path = PathBuf::from("bad_sale/i18n/fr.po");
        let related = (1..=count)
            .map(|n| FlowStep {
                path: path.clone(),
                line: n * 10,
                column: 1,
                label: format!("untranslated: Message {n}"),
            })
            .collect();
        Diagnostic {
            code: "OD803",
            severity: Severity::Info,
            path,
            line: 10,
            column: 1,
            message: format!("{count} of 40 messages are untranslated, so Odoo shows them in the source language"),
            flow: vec![],
            related,
            fix: None,
        }
    }

    #[test]
    fn text_github_and_gitlab_name_related_lines_up_to_the_cap() {
        let diags = [untranslated(2), untranslated(22)];
        let diags: Vec<_> = diags.iter().collect();
        let mut out = vec![];
        print_text(&mut out, &diags, &rel, false);
        let shown: Vec<String> = (1..=20).map(|n| (n * 10).to_string()).collect();
        let capped = format!("lines {} and 2 more, which --format json lists", shown.join(", "));
        let expected = format!(
            "bad_sale/i18n/fr.po:10:1: OD803 2 of 40 messages are untranslated, so Odoo shows them in the source language (lines 10, 20)
bad_sale/i18n/fr.po:10:1: OD803 22 of 40 messages are untranslated, so Odoo shows them in the source language ({capped})
"
        );
        assert_eq!(String::from_utf8(out).unwrap(), expected);
        let mut out = vec![];
        print_github(&mut out, &diags[..1], &rel);
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("source language%0A%0ALines 10, 20%0A%0ASee ")
        );
        let report = gitlab(&diags, &rel);
        assert_eq!(
            report[0]["content"]["body"],
            "[OD803 translation-empty](https://avanserv.github.io/odooscan/rules/OD803/)\n\nLines 10, 20"
        );
        assert!(
            report[1]["content"]["body"]
                .as_str()
                .unwrap()
                .ends_with("and 2 more, which --format json lists")
        );
    }

    #[test]
    fn sarif_and_json_list_every_related_line_under_the_cap() {
        let diags = [untranslated(22), untranslated(0)];
        let report = sarif(&diags.iter().collect::<Vec<_>>(), &rel);
        let results = &report["runs"][0]["results"];
        let related = results[0]["relatedLocations"].as_array().unwrap();
        assert_eq!(related.len(), 22);
        assert_eq!(
            related[21],
            json!({
                "id": 21,
                "physicalLocation": physical("bad_sale/i18n/fr.po".to_owned(), 220, 1),
                "message": {"text": "untranslated: Message 22"},
            })
        );
        assert!(results[0].get("properties").is_none());
        assert!(results[1].get("relatedLocations").is_none());
        let json = serde_json::to_value(&diags).unwrap();
        assert_eq!(json[0]["related"].as_array().unwrap().len(), 22);
        assert_eq!(json[0]["related"][0]["line"], 10);
        assert!(json[1].get("related").is_none());
    }

    #[test]
    fn sarif_lists_the_first_thousand_related_lines_and_counts_the_rest() {
        let diags = [untranslated(1000), untranslated(1003)];
        let report = sarif(&diags.iter().collect::<Vec<_>>(), &rel);
        let results = &report["runs"][0]["results"];
        assert_eq!(results[0]["relatedLocations"].as_array().unwrap().len(), 1000);
        assert!(results[0].get("properties").is_none());
        let capped = &results[1];
        assert_eq!(capped["relatedLocations"].as_array().unwrap().len(), 1000);
        assert_eq!(
            capped["relatedLocations"][999]["physicalLocation"]["region"]["startLine"],
            10_000
        );
        assert_eq!(capped["properties"], json!({"relatedLocationsOmitted": 3}));
        assert_eq!(capped["message"]["text"], diags[1].message);
        let json = serde_json::to_value(&diags).unwrap();
        assert_eq!(json[1]["related"].as_array().unwrap().len(), 1003);
    }

    #[test]
    fn help_markdown_makes_rule_links_absolute() {
        let md = help_markdown(
            "OD312",
            "is [OD201](OD201.md), [x](OD312.md#what-it-reports), [JS](index.md#od6-javascript-and-owl) \
             and [why](#why-it-matters)",
        );
        assert_eq!(
            md,
            "is [OD201](https://avanserv.github.io/odooscan/rules/OD201/), \
             [x](https://avanserv.github.io/odooscan/rules/OD312/#what-it-reports), \
             [JS](https://avanserv.github.io/odooscan/rules/#od6-javascript-and-owl) \
             and [why](https://avanserv.github.io/odooscan/rules/OD312/#why-it-matters)"
        );
    }

    #[test]
    fn help_markdown_keeps_other_targets() {
        let doc = "[s](https://example.com/a.md#b) F[f](float(x)) [n](OD999.md) [open](OD201.md";
        assert_eq!(help_markdown("OD312", doc), doc);
    }

    /// An inline link the site resolves but a SARIF viewer cannot, such as a future
    /// `../fixing.md`, would otherwise go dead outside the site without a test noticing.
    /// Reference-style definitions and targets without `.md` are not checked.
    #[test]
    fn every_rule_page_reads_absolute_outside_the_site() {
        for r in RULES {
            let md = help_markdown(r.code, r.doc);
            for target in md.split("](").skip(1) {
                assert!(
                    !target.starts_with('#') && !target.split(')').next().unwrap().contains(".md"),
                    "{}: ]({}",
                    r.code,
                    target.lines().next().unwrap()
                );
            }
        }
    }
}
