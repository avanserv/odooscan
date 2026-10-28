use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{DataFacts, JsFacts, PyFacts, TemplateFacts};
use crate::manifest::{self, Manifest};
use crate::series::Series;
use crate::suppress::Suppressions;
use crate::{csvdata, js, owl, python, xml};

const MANIFESTS: &[&str] = &["__manifest__.py", "__openerp__.py"];

/// Module subdirectories that hold neither models nor data loaded from the manifest.
const SKIPPED_DIRS: &[&str] = &["static", "node_modules", "__pycache__"];

pub struct SourceFile<T> {
    pub path: PathBuf,
    /// Path relative to the module directory, with `/` separators, as the manifest writes it.
    pub rel: String,
    /// Load position among the manifest's data then demo files; `None` when Odoo never loads it.
    pub rank: Option<usize>,
    pub facts: T,
    pub suppressions: Suppressions,
}

pub struct Module {
    pub name: String,
    pub dir: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: Manifest,
    pub series: Option<Series>,
    /// Diagnostics are reported for checked modules; the others only feed the index.
    pub checked: bool,
    pub python: Vec<SourceFile<PyFacts>>,
    pub data: Vec<SourceFile<DataFacts>>,
    /// Scripts under `static/src`, the only JS the web client loads as Odoo modules.
    pub js: Vec<SourceFile<JsFacts>>,
    /// Owl templates: the XML files under `static/src`.
    pub templates: Vec<SourceFile<TemplateFacts>>,
}

fn manifest_in(dir: &Path) -> Option<PathBuf> {
    MANIFESTS.iter().map(|m| dir.join(m)).find(|p| p.is_file())
}

/// Module directories anywhere under `path`, or `path` itself when it is a module. Copies nested
/// inside a module, such as the posbox image under point_of_sale, are not modules.
fn modules_under(path: &Path) -> Vec<PathBuf> {
    if manifest_in(path).is_some() {
        return vec![path.to_path_buf()];
    }
    let mut dirs: Vec<PathBuf> = WalkBuilder::new(path)
        .filter_entry(|e| !SKIPPED_DIRS.contains(&e.file_name().to_str().unwrap_or_default()))
        .build()
        .filter_map(Result::ok)
        .filter(|e| MANIFESTS.contains(&e.file_name().to_str().unwrap_or_default()))
        .filter_map(|e| e.path().parent().map(Path::to_path_buf))
        .collect();
    dirs.sort();
    let mut modules: Vec<PathBuf> = vec![];
    for dir in dirs {
        if !modules.last().is_some_and(|m| dir.starts_with(m)) {
            modules.push(dir);
        }
    }
    modules
}

/// Modules directly inside an addons directory, as Odoo reads `--addons-path`. An Odoo source
/// checkout stands for its two addons directories.
fn modules_in_addons_path(path: &Path) -> Vec<PathBuf> {
    if path.join("odoo/addons/base").is_dir() {
        return [path.join("odoo/addons"), path.join("addons")]
            .iter()
            .flat_map(|p| modules_in_addons_path(p))
            .collect();
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return vec![];
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| manifest_in(p).is_some())
        .collect();
    dirs.sort();
    dirs
}

fn module_files(dir: &Path) -> Vec<PathBuf> {
    let root = dir.to_path_buf();
    WalkBuilder::new(dir)
        .filter_entry(move |e| {
            let name = e.file_name().to_str().unwrap_or_default();
            let nested_module =
                e.path() != root && e.file_type().is_some_and(|t| t.is_dir()) && manifest_in(e.path()).is_some();
            !SKIPPED_DIRS.contains(&name) && !nested_module
        })
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(|e| e.into_path())
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("py" | "xml" | "csv")))
        .collect()
}

fn static_files(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let src = dir.join("static/src");
    if !src.is_dir() {
        return vec![];
    }
    WalkBuilder::new(src)
        .filter_entry(|e| e.file_name() != "node_modules")
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(|e| e.into_path())
        .filter(|p| p.extension().is_some_and(|e| e == extension))
        .collect()
}

fn normalize(rel: &str) -> String {
    rel.trim_start_matches("./").replace('\\', "/")
}

pub struct Discovery {
    pub modules: Vec<Module>,
    pub notices: Vec<String>,
}

pub fn load(paths: &[PathBuf], addons_paths: &[PathBuf], default_series: Option<Series>) -> Discovery {
    let mut notices = vec![];
    let mut modules: Vec<Module> = candidates(paths, addons_paths)
        .into_par_iter()
        .filter_map(|(name, dir, checked)| read_module(name, dir, checked))
        .collect();
    for m in modules.iter().filter(|_| default_series.is_none()) {
        if m.checked && m.series.is_none() {
            notices.push(format!(
                "{}: the manifest version does not name a series; pass --odoo-version",
                m.name
            ));
        }
    }
    modules.par_iter_mut().for_each(parse_module_files);
    Discovery { modules, notices }
}

/// Module directories by name, with whether each is checked, keeping the first of each name.
fn candidates(paths: &[PathBuf], addons_paths: &[PathBuf]) -> Vec<(String, PathBuf, bool)> {
    let mut seen = FxHashSet::default();
    // Checked modules come first, so they shadow a same-named module on the addons path.
    let checked = paths.iter().flat_map(|p| modules_under(p)).map(|dir| (dir, true));
    let indexed = addons_paths
        .iter()
        .flat_map(|p| modules_in_addons_path(p))
        .map(|dir| (dir, false));
    let mut chosen = vec![];
    for (dir, checked) in checked.chain(indexed) {
        let Some(name) = dir
            .canonicalize()
            .ok()
            .and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
        else {
            continue;
        };
        if seen.insert(name.clone()) {
            chosen.push((name, dir, checked));
        }
    }
    chosen
}

fn read_module(name: String, dir: PathBuf, checked: bool) -> Option<Module> {
    let manifest_path = manifest_in(&dir)?;
    let manifest = manifest::parse(&std::fs::read_to_string(&manifest_path).ok()?)?;
    manifest.installable.then(|| Module {
        series: manifest.series(),
        name,
        dir,
        manifest_path,
        manifest,
        checked,
        python: vec![],
        data: vec![],
        js: vec![],
        templates: vec![],
    })
}

fn parse_module_files(module: &mut Module) {
    let ranks: FxHashMap<String, usize> = module
        .manifest
        .data
        .iter()
        .chain(&module.manifest.demo)
        .enumerate()
        .map(|(i, f)| (normalize(&f.value), i))
        .collect();
    let files = module_files(&module.dir);
    let checked = module.checked;
    let parsed: Vec<_> = files
        .into_par_iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            let rel = normalize(&path.strip_prefix(&module.dir).ok()?.to_string_lossy());
            let rank = ranks.get(&rel).copied();
            let suppressions = if checked {
                Suppressions::parse(&text)
            } else {
                Suppressions::default()
            };
            Some(match path.extension().and_then(|e| e.to_str()) {
                Some("py") => Ok(SourceFile {
                    facts: python::analyze(&text),
                    path,
                    rel,
                    rank,
                    suppressions,
                }),
                Some("xml") => Err(SourceFile {
                    facts: xml::analyze(&text),
                    path,
                    rel,
                    rank,
                    suppressions,
                }),
                _ => {
                    // `model-suffix.csv` loads into model, so one model can have several files.
                    let stem = path.file_stem()?.to_string_lossy();
                    let model = stem.split('-').next().unwrap_or_default().to_owned();
                    Err(SourceFile {
                        facts: csvdata::analyze(&text, &model),
                        path,
                        rel,
                        rank,
                        suppressions,
                    })
                }
            })
        })
        .collect();
    for file in parsed {
        match file {
            Ok(py) => module.python.push(py),
            Err(data) => module.data.push(data),
        }
    }
    module
        .data
        .sort_by_key(|f| (f.rank.unwrap_or(usize::MAX), f.rel.clone()));
    module.js = parse_static_files(&module.dir, "js", checked, js::analyze);
    module.templates = parse_static_files(&module.dir, "xml", checked, owl::analyze);
}

fn parse_static_files<T: Send>(
    dir: &Path,
    extension: &str,
    checked: bool,
    analyze: fn(&str) -> T,
) -> Vec<SourceFile<T>> {
    let mut files: Vec<_> = static_files(dir, extension)
        .into_par_iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            Some(SourceFile {
                rel: normalize(&path.strip_prefix(dir).ok()?.to_string_lossy()),
                rank: None,
                suppressions: if checked {
                    Suppressions::parse(&text)
                } else {
                    Suppressions::default()
                },
                facts: analyze(&text),
                path,
            })
        })
        .collect();
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    files
}
