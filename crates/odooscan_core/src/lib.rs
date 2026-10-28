mod checks;
mod csvdata;
mod domain;
mod facts;
mod flows;
mod index;
mod js;
mod manifest;
mod owl;
mod project;
mod python;
pub mod rules;
pub mod series;
mod suppress;
mod taint;
mod xml;

use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::Serialize;

use crate::facts::RawDiag;
use crate::rules::{SUPERSEDED, Severity, rule};
use crate::series::Series;

pub struct ScanOptions {
    /// Modules to report on: module directories, or directories searched for modules.
    pub paths: Vec<PathBuf>,
    /// Addons directories, or Odoo checkouts, indexed for context and never reported on.
    pub addons_paths: Vec<PathBuf>,
    /// Series for modules whose manifest version does not name one.
    pub default_series: Option<Series>,
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
}

#[derive(Debug, Serialize)]
pub struct FlowStep {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
    pub label: String,
}

impl Diagnostic {
    fn new(path: PathBuf, d: RawDiag, series: Option<Series>) -> Self {
        let flow = d
            .flow
            .into_iter()
            .map(|s| FlowStep {
                path: s.path.unwrap_or_else(|| path.clone()),
                line: s.pos.line,
                column: s.pos.col,
                label: s.label,
            })
            .collect();
        Self {
            code: d.code,
            severity: rule(d.code).severity_for(series),
            path,
            line: d.pos.line,
            column: d.pos.col,
            message: d.message,
            flow,
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

pub fn scan(opts: &ScanOptions) -> Report {
    let discovery = project::load(&opts.paths, &opts.addons_paths, opts.default_series);
    let mut notices = discovery.notices;
    let modules = discovery.modules;
    let index = index::Index::build(&modules);
    if !index.has_base && modules.iter().any(|m| m.checked) {
        notices.push(
            "module base is not indexed, so cross-module rules are off; pass --addons-path with an Odoo checkout"
                .to_owned(),
        );
    }
    let mut diagnostics: Vec<Diagnostic> = modules
        .par_iter()
        .enumerate()
        .filter(|(_, m)| m.checked)
        .flat_map_iter(|(mi, module)| {
            let series = module.series.or(opts.default_series);
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
                .map(|(p, s, d): (&Path, _, &RawDiag)| (p.to_path_buf(), s.allows(d.pos.line, d.code), d.clone()));
            let cross = checks::check_module(&index, mi, series).into_iter().map(|(p, d)| {
                let allowed = module
                    .python
                    .iter()
                    .map(|f| (&f.path, &f.suppressions))
                    .chain(module.data.iter().map(|f| (&f.path, &f.suppressions)))
                    .chain(module.js.iter().map(|f| (&f.path, &f.suppressions)))
                    .chain(module.templates.iter().map(|f| (&f.path, &f.suppressions)))
                    .find(|(fp, _)| fp.as_path() == p)
                    .is_none_or(|(_, s)| s.allows(d.pos.line, d.code));
                (p.to_path_buf(), allowed, d)
            });
            let mut diags: Vec<_> = local
                .chain(cross)
                .filter(move |(_, allowed, d)| *allowed && rule(d.code).applies_to(series))
                .map(|(path, _, d)| Diagnostic::new(path, d, series))
                .collect();
            drop_superseded(&mut diags);
            diags
        })
        .collect();
    diagnostics.sort_by(|a, b| (&a.path, a.line, a.column, a.code).cmp(&(&b.path, b.line, b.column, b.code)));
    Report {
        diagnostics,
        modules_checked: modules.iter().filter(|m| m.checked).count(),
        modules_indexed: modules.len(),
        files: modules
            .iter()
            .map(|m| m.python.len() + m.data.len() + m.js.len() + m.templates.len())
            .sum(),
        notices,
    }
}
