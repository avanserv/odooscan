mod api_table;
mod async_names;
mod bundles;
mod checks;
mod csvdata;
mod decode;
mod domain;
mod facts;
pub mod fix;
mod flows;
mod grants;
mod imports;
mod index;
mod js;
mod manifest;
mod mentions;
mod messages;
mod method_body;
mod modifier;
mod module_state;
mod orm_queries;
mod owl;
mod placeholders;
mod po;
mod project;
mod python;
mod python2;
mod record_eval;
mod route_body;
mod routing;
pub mod rules;
pub mod series;
mod server_code;
mod sql;
mod superuser;
mod suppress;
mod taint;
mod xml;

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::Serialize;

use crate::facts::{RawDiag, Step};
use crate::fix::Fix;
use crate::rules::{SUPERSEDED, Severity, rule};
use crate::series::Series;
pub use crate::taint::TaintTable;

#[derive(Default)]
pub struct ScanOptions {
    /// Modules to report on: module directories, or directories searched for modules.
    pub paths: Vec<PathBuf>,
    /// Addons directories, or Odoo checkouts, indexed for context and never reported on.
    pub addons_paths: Vec<PathBuf>,
    /// Series for modules whose manifest version does not name one.
    pub default_series: Option<Series>,
    /// Series every checked module is checked as, whatever its manifest version names, and the
    /// default for the rest.
    pub target_series: Option<Series>,
    /// Sources, sinks and sanitizers the OD7 rules follow besides their own.
    pub taint: TaintTable,
}

#[derive(Debug, Serialize)]
pub struct Diagnostic {
    pub code: &'static str,
    pub severity: Severity,
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
    pub message: String,
    /// The path a data flow takes from its source to the finding.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flow: Vec<FlowStep>,
    /// Further places the finding stands for, such as each untranslated entry of an OD803 count.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<FlowStep>,
    /// The rewrite `oscn check --fix` applies, which no report format carries.
    #[serde(skip)]
    pub fix: Option<Fix>,
}

#[derive(Debug, Serialize)]
pub struct FlowStep {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
    pub label: String,
}

impl Diagnostic {
    /// Borrows `d`, as a per-file finding such as OD803 can carry thousands of related steps and
    /// cloning it first would hold them a third time.
    fn new(path: PathBuf, d: &RawDiag, series: Option<Series>) -> Self {
        let steps = |steps: &[Step]| {
            steps
                .iter()
                .map(|s| FlowStep {
                    path: s.path.clone().unwrap_or_else(|| path.clone()),
                    line: s.pos.line,
                    column: s.pos.col,
                    label: s.label.clone(),
                })
                .collect()
        };
        let flow = steps(&d.flow);
        let related = steps(&d.related);
        Self {
            code: d.code,
            severity: d.severity.unwrap_or_else(|| rule(d.code).severity_for(series)),
            path,
            line: d.pos.line,
            column: d.pos.col,
            message: d.message.clone(),
            flow,
            related,
            fix: d.fix.clone(),
        }
    }

    /// The lines of the related locations, the first `cap` of them named, for a report that
    /// cannot list them all. Lines alone name a place only in the finding's file, so a rule that
    /// gives related locations must keep them there.
    pub fn related_lines(&self, cap: usize) -> Option<String> {
        debug_assert!(
            self.related.iter().all(|s| s.path == self.path),
            "{} gives a related location outside {}",
            self.code,
            self.path.display()
        );
        let shown: Vec<String> = self.related.iter().take(cap).map(|s| s.line.to_string()).collect();
        match (shown.as_slice(), self.related.len() - shown.len()) {
            ([], _) => None,
            ([one], 0) => Some(format!("line {one}")),
            (lines, 0) => Some(format!("lines {}", lines.join(", "))),
            (lines, rest) => Some(format!(
                "lines {} and {rest} more, which --format json lists",
                lines.join(", ")
            )),
        }
    }
}

/// Drops each finding a rule in `SUPERSEDED` makes at the same position.
fn drop_superseded(diags: &mut Vec<Diagnostic>) {
    let at = |d: &Diagnostic| (d.path.clone(), d.line, d.column);
    let mut by: Vec<(&str, (PathBuf, u32, u32))> = vec![];
    for d in diags.iter() {
        if SUPERSEDED.iter().any(|(_, b)| *b == d.code) {
            by.push((d.code, at(d)));
        }
    }
    diags.retain(|d| {
        !SUPERSEDED
            .iter()
            .filter(|(code, _)| *code == d.code)
            .any(|(_, b)| by.iter().any(|(c, pos)| c == b && *pos == at(d)))
    });
}

pub struct Report {
    pub diagnostics: Vec<Diagnostic>,
    pub modules_checked: usize,
    pub modules_indexed: usize,
    pub files: usize,
    pub notices: Vec<String>,
}

/// Whether `opts` give an Odoo core for the cross-module rules to check against.
pub fn has_core(opts: &ScanOptions) -> bool {
    project::has_core(&opts.paths, &opts.addons_paths)
}

pub fn scan(opts: &ScanOptions) -> Report {
    let default_series = opts.target_series.or(opts.default_series);
    let discovery = project::load(
        &opts.paths,
        &opts.addons_paths,
        default_series,
        opts.target_series,
        &opts.taint,
    );
    let mut notices = discovery.notices;
    let modules = discovery.modules;
    let broken = discovery.broken;
    let mut index = index::Index::build(&modules, default_series);
    index.rejected = discovery.rejected;
    index.uninstallable = discovery.uninstallable;
    if !index.has_base && modules.iter().any(|m| m.checked) {
        notices.push(
            "module base is not indexed, so cross-module rules are off; pass --addons-path with an Odoo checkout"
                .to_owned(),
        );
    }
    notices.extend(deep_gap_notices(&index));
    let mut diagnostics: Vec<Diagnostic> = modules
        .par_iter()
        .enumerate()
        .filter(|(_, m)| m.checked)
        .flat_map_iter(|(mi, module)| {
            let series = module.series.or(default_series);
            let local = module
                .python
                .iter()
                .flat_map(|f| {
                    f.facts
                        .diags
                        .iter()
                        .map(move |d| (f.path.as_path(), &f.suppressions, d))
                })
                .chain(module.data.iter().flat_map(|f| {
                    f.facts
                        .diags
                        .iter()
                        .map(move |d| (f.path.as_path(), &f.suppressions, d))
                }))
                .chain(module.translations.iter().flat_map(|f| {
                    f.facts
                        .diags
                        .iter()
                        .map(move |d| (f.path.as_path(), &f.suppressions, d))
                }))
                .map(|(p, s, d): (&Path, _, &RawDiag)| {
                    (p.to_path_buf(), s.allows(d.pos.line, d.code), Cow::Borrowed(d))
                });
            let cross = checks::check_module(&index, mi, series).into_iter().map(|(p, d)| {
                let allowed = module
                    .python
                    .iter()
                    .map(|f| (&f.path, &f.suppressions))
                    .chain(module.data.iter().map(|f| (&f.path, &f.suppressions)))
                    .chain(module.js.iter().map(|f| (&f.path, &f.suppressions)))
                    .chain(module.templates.iter().map(|f| (&f.path, &f.suppressions)))
                    .chain(module.translations.iter().map(|f| (&f.path, &f.suppressions)))
                    .chain(module.template.iter().map(|f| (&f.path, &f.suppressions)))
                    .find(|(fp, _)| fp.as_path() == p)
                    .is_none_or(|(_, s)| s.allows(d.pos.line, d.code));
                (p.to_path_buf(), allowed, Cow::Owned(d))
            });
            let mut diags: Vec<_> = local
                .chain(cross)
                .filter(move |(_, allowed, d)| *allowed && rule(d.code).applies_to(series))
                .map(|(path, _, d)| Diagnostic::new(path, &d, series))
                .collect();
            drop_superseded(&mut diags);
            diags
        })
        .collect();
    // Odoo cannot read the manifest, so the module is not loaded and this is all it gets.
    let broken_diags = broken
        .iter()
        .filter(|b| b.suppressions.allows(b.diag.pos.line, b.diag.code) && rule(b.diag.code).applies_to(default_series))
        .map(|b| Diagnostic::new(b.path.clone(), &b.diag, default_series));
    diagnostics.extend(broken_diags);
    diagnostics.sort_by(|a, b| (&a.path, a.line, a.column, a.code).cmp(&(&b.path, b.line, b.column, b.code)));
    Report {
        diagnostics,
        modules_checked: modules.iter().filter(|m| m.checked).count() + broken.len(),
        modules_indexed: modules.len(),
        files: modules
            .iter()
            .map(|m| m.python.len() + m.data.len() + m.js.len() + m.templates.len() + m.translations.len())
            .sum(),
        notices,
    }
}

/// One notice per module the index lacks that a checked module reaches only through its
/// dependencies: OD503, or the notice on a manifest the dependent's Odoo reads but odooscan does
/// not, tells each module that names an unindexed one, and this tells the rest. Without `base` the cross-module
/// rules are off, so nothing is silenced by the gap alone.
fn deep_gap_notices(index: &index::Index) -> Vec<String> {
    if !index.has_base {
        return vec![];
    }
    let gaps = index.deep_gaps().into_iter();
    gaps.map(|(name, modules)| deep_gap_notice(name, &modules)).collect()
}

/// Names past this many are counted, as a large scan may reach the gap from hundreds of modules.
const GAP_NAMES: usize = 5;

fn deep_gap_notice(name: &str, modules: &[&str]) -> String {
    let count = modules.len();
    let (noun, verb) = if count == 1 {
        ("module", "depends")
    } else {
        ("modules", "depend")
    };
    let mut listed = modules[..count.min(GAP_NAMES)].join(", ");
    if count > GAP_NAMES {
        listed = format!("{listed} and {} more", count - GAP_NAMES);
    }
    format!(
        "module '{name}' is not indexed, so most cross-module rules stay silent in {count} {noun} that {verb} on it through other modules: {listed}; add its checkout to --addons-path"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diag(code: &'static str, line: u32) -> Diagnostic {
        Diagnostic {
            code,
            severity: Severity::Error,
            path: PathBuf::from("views/assets.xml"),
            line,
            column: 5,
            message: String::new(),
            flow: vec![],
            related: vec![],
            fix: None,
        }
    }

    fn related(count: u32) -> Diagnostic {
        let mut d = diag("OD803", 3);
        d.related = (1..=count)
            .map(|n| FlowStep {
                path: d.path.clone(),
                line: n * 10,
                column: 1,
                label: String::new(),
            })
            .collect();
        d
    }

    #[test]
    fn related_lines_name_each_line_up_to_the_cap() {
        assert_eq!(related(0).related_lines(3), None);
        assert_eq!(related(1).related_lines(3).as_deref(), Some("line 10"));
        assert_eq!(related(3).related_lines(3).as_deref(), Some("lines 10, 20, 30"));
        assert_eq!(
            related(5).related_lines(3).as_deref(),
            Some("lines 10, 20, 30 and 2 more, which --format json lists")
        );
        assert_eq!(
            related(5).related_lines(usize::MAX).as_deref(),
            Some("lines 10, 20, 30, 40, 50")
        );
    }

    #[test]
    fn a_deep_gap_notice_names_up_to_five_modules() {
        assert_eq!(
            deep_gap_notice("ghost_enterprise", &["partial_sites_dependent"]),
            "module 'ghost_enterprise' is not indexed, so most cross-module rules stay silent in 1 module that depends on it through other modules: partial_sites_dependent; add its checkout to --addons-path"
        );
        assert_eq!(
            deep_gap_notice("ghost", &["a", "b", "c", "d", "e", "f", "g"]),
            "module 'ghost' is not indexed, so most cross-module rules stay silent in 7 modules that depend on it through other modules: a, b, c, d, e and 2 more; add its checkout to --addons-path"
        );
    }

    #[test]
    fn a_deep_gap_gets_no_notice_without_base() {
        let manifest = |depends: &str| format!("{{'depends': [{depends}]}}");
        let mut modules = vec![
            project::module("mid", &manifest("'base', 'ghost'")),
            project::module("top", &manifest("'mid'")),
        ];
        assert!(deep_gap_notices(&index::Index::build(&modules, None)).is_empty());
        modules.insert(0, project::module("base", &manifest("")));
        assert_eq!(deep_gap_notices(&index::Index::build(&modules, None)).len(), 1);
    }

    #[test]
    fn a_finding_severity_replaces_the_series_one() {
        let at = |raw: RawDiag| Diagnostic::new(PathBuf::from("models/site.py"), &raw, Series::parse("16.0")).severity;
        let raw = || RawDiag::new("OD222", facts::Pos { line: 3, col: 5 }, "");
        assert_eq!(at(raw()), Severity::Warning);
        assert_eq!(at(raw().with_severity(Some(Severity::Error))), Severity::Error);
    }

    #[test]
    fn a_bundle_reference_reports_only_od328() {
        let codes = [("OD301", 2), ("OD328", 2), ("OD302", 10), ("OD328", 10), ("OD302", 14)];
        let mut diags: Vec<_> = codes.iter().map(|&(code, line)| diag(code, line)).collect();
        drop_superseded(&mut diags);
        let left: Vec<_> = diags.iter().map(|d| (d.code, d.line)).collect();
        assert_eq!(left, [("OD328", 2), ("OD328", 10), ("OD302", 14)]);
    }
}
