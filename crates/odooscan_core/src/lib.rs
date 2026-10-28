mod checks;
mod csvdata;
mod facts;
mod index;
mod manifest;
mod project;
mod python;
pub mod rules;
pub mod series;
mod suppress;
mod xml;

use std::path::PathBuf;

use rayon::prelude::*;
use serde::Serialize;

use crate::facts::RawDiag;
use crate::rules::{Severity, rule};
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
}

pub struct Report {
    pub diagnostics: Vec<Diagnostic>,
    pub modules_checked: usize,
    pub modules_indexed: usize,
    pub files: usize,
    pub notices: Vec<String>,
}

pub fn scan(opts: &ScanOptions) -> Report {
    let discovery = project::load(&opts.paths, &opts.addons_paths);
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
                .map(|(p, s, d)| {
                    (
                        p.to_path_buf(),
                        s.allows(d.pos.line, d.code),
                        RawDiag::new(d.code, d.pos, d.message.clone()),
                    )
                });
            let cross = checks::check_module(&index, mi).into_iter().map(|(p, d)| {
                let allowed = module
                    .python
                    .iter()
                    .map(|f| (&f.path, &f.suppressions))
                    .chain(module.data.iter().map(|f| (&f.path, &f.suppressions)))
                    .find(|(fp, _)| fp.as_path() == p)
                    .is_none_or(|(_, s)| s.allows(d.pos.line, d.code));
                (p.to_path_buf(), allowed, d)
            });
            local
                .chain(cross)
                .filter(move |(_, allowed, d)| *allowed && rule(d.code).applies_to(series))
                .map(|(path, _, d)| Diagnostic {
                    code: d.code,
                    severity: rule(d.code).severity_for(series),
                    path,
                    line: d.pos.line,
                    column: d.pos.col,
                    message: d.message,
                })
                .collect::<Vec<_>>()
        })
        .collect();
    diagnostics.sort_by(|a, b| (&a.path, a.line, a.column, a.code).cmp(&(&b.path, b.line, b.column, b.code)));
    Report {
        diagnostics,
        modules_checked: modules.iter().filter(|m| m.checked).count(),
        modules_indexed: modules.len(),
        files: modules.iter().map(|m| m.python.len() + m.data.len()).sum(),
        notices,
    }
}
