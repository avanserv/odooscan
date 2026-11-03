use std::borrow::Cow;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{DataFacts, JsFacts, PyFacts, TemplateFacts};
use crate::manifest::{self, Manifest, normalize};
use crate::series::Series;
use crate::suppress::Suppressions;
use crate::{csvdata, decode, js, owl, python, sql, xml};

const MANIFESTS: &[&str] = &["__manifest__.py", "__openerp__.py"];

const ACCESS_MODELS: &[&str] = &["ir.model.access", "ir.access"];

/// Module subdirectories that hold neither models nor data loaded from the manifest.
const SKIPPED_DIRS: &[&str] = &["static", "node_modules", "__pycache__"];

pub struct SourceFile<T> {
    pub path: PathBuf,
    /// Path relative to the module directory, with `/` separators, as the manifest writes it.
    pub rel: String,
    /// Load position among the files the manifest loads in the module's series, as
    /// `Manifest::loaded_files` orders them; `None` when Odoo never loads it.
    pub rank: Option<usize>,
    /// Odoo first loads it as demo data, which a database may skip.
    pub demo: bool,
    pub facts: T,
    pub suppressions: Suppressions,
}

impl SourceFile<()> {
    fn with<T>(self, facts: T) -> SourceFile<T> {
        SourceFile {
            path: self.path,
            rel: self.rel,
            rank: self.rank,
            demo: self.demo,
            facts,
            suppressions: self.suppressions,
        }
    }
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
    /// The views the SQL files the manifest loads in the module's series create.
    pub sql_views: Vec<String>,
}

impl Module {
    /// Whether the manifest loads a data file the scan cannot read, so that the records of the
    /// module are not all known.
    pub fn lost_data(&self) -> bool {
        self.lost_files().next().is_some()
    }

    /// Whether `lost_data` covers a file that may hold an access row. A CSV loads into the model it
    /// is named after, so one of another model holds none.
    pub fn lost_access(&self) -> bool {
        self.lost_files().any(|f| {
            let path = Path::new(&f.rel);
            path.extension().is_none_or(|e| e != "csv") || ACCESS_MODELS.contains(&loads_into(path).as_str())
        })
    }

    fn lost_files(&self) -> impl Iterator<Item = &SourceFile<DataFacts>> {
        self.data.iter().filter(|f| f.facts.unreadable && f.rank.is_some())
    }

    /// The `.xml` and `.csv` file names the module's Python mentions, its manifest aside.
    pub fn code_mentions(&self) -> FxHashSet<&str> {
        let code = self.python.iter().filter(|f| f.path != self.manifest_path);
        code.flat_map(|f| &f.facts.file_mentions).map(String::as_str).collect()
    }
}

/// Whether the module loads the file itself, given its `Module::code_mentions`: chart templates,
/// any file its Python names by base name, and any file the manifest lists under a key Odoo loads
/// nothing from, unless a legacy key `series` ignores lists it too: that file was meant to load
/// from there. A manifest entry matches by path: a base name would also match each entry of
/// `data`, and so every `tests/` copy of a listed file.
pub fn code_loaded(rel: &str, mentions: &FxHashSet<&str>, manifest: &Manifest, series: Option<Series>) -> bool {
    let base = rel.rsplit('/').next().unwrap_or_default();
    mentions.contains(base)
        || rel.starts_with("data/template/")
        || (manifest.unread.iter().any(|f| f == rel) && manifest.ignored_key(rel, series).is_none())
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
    modules
        .par_iter_mut()
        .for_each(|module| parse_module_files(module, default_series));
    let known = python::field_classes(modules.iter().flat_map(|m| &m.python).map(|f| &f.facts));
    if !known.is_empty() {
        modules
            .par_iter_mut()
            .for_each(|module| read_field_calls(module, default_series, &known));
    }
    notices.extend(python2_notices(&modules, default_series));
    Discovery { modules, notices }
}

/// One notice per file of a checked module that only parses as the Python 2 its series runs, by
/// path. Such a file gets no finding, so this says it went unchecked.
fn python2_notices(modules: &[Module], default_series: Option<Series>) -> Vec<String> {
    let mut notices: Vec<String> = modules
        .iter()
        .filter(|m| m.checked)
        .flat_map(|m| {
            let series = m.series.or(default_series);
            m.python.iter().filter_map(move |f| {
                let pos = f.facts.python2?;
                Some(format!(
                    "{}/{}:{}:{}: Python 2 syntax, which {} runs; this file is not checked",
                    m.name, f.rel, pos.line, pos.col, series?
                ))
            })
        })
        .collect();
    notices.sort();
    notices
}

/// Reads again the files whose class attributes call a field class of the scan. A class may live
/// in any module, so the first pass takes such a call for a method.
fn read_field_calls(module: &mut Module, default_series: Option<Series>, known: &python::FieldClasses) {
    let series = module.series.or(default_series);
    module
        .python
        .par_iter_mut()
        .filter(|file| file.facts.attr_calls.iter().any(|name| known.contains_key(name)))
        .for_each(|file| {
            if let Ok(text) = std::fs::read_to_string(&file.path) {
                file.facts = python::analyze_with(&text, series, known);
            }
        });
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
        sql_views: vec![],
    })
}

/// Each file the manifest loads in `series` by its first load position, where a reference to an
/// id it defines can first fail; Odoo loads a file again at each later place it is listed.
fn ranks(manifest: &Manifest, series: Option<Series>) -> FxHashMap<String, usize> {
    let mut ranks = FxHashMap::default();
    for (i, file) in manifest.loaded_files(series).enumerate() {
        ranks.entry(normalize(file)).or_insert(i);
    }
    ranks
}

/// What reading a file of a module takes, besides the file.
struct ModuleContext<'a> {
    dir: &'a Path,
    ranks: FxHashMap<String, usize>,
    data_entries: usize,
    series: Option<Series>,
    checked: bool,
}

/// A Python file on the left, a data file on the right.
type ParsedFile = Result<SourceFile<PyFacts>, SourceFile<DataFacts>>;

impl ModuleContext<'_> {
    /// Reads `path`, none when it cannot be read or is Python that does not decode. A data file that
    /// does not decode stays, since what it may define matters to the rules.
    fn file(&self, path: PathBuf) -> Option<ParsedFile> {
        let bytes = std::fs::read(&path).ok()?;
        let text = decoded(&path, &bytes, self.series);
        let text = text.as_deref();
        if path.extension().is_some_and(|e| e == "py") {
            let facts = python::analyze(text?, self.series);
            return Some(Ok(self.source(path, text)?.with(facts)));
        }
        let facts = self.data_facts(&path, text);
        Some(Err(self.source(path, text)?.with(facts)))
    }

    /// `path` as a file of the module, its facts to come.
    fn source(&self, path: PathBuf, text: Option<&str>) -> Option<SourceFile<()>> {
        let rel = normalize(&path.strip_prefix(self.dir).ok()?.to_string_lossy());
        let rank = self.ranks.get(&rel).copied();
        let suppressions = text.filter(|_| self.checked).map(Suppressions::parse);
        Some(SourceFile {
            demo: rank.is_some_and(|r| r >= self.data_entries),
            facts: (),
            path,
            rel,
            rank,
            suppressions: suppressions.unwrap_or_default(),
        })
    }

    fn data_facts(&self, path: &Path, text: Option<&str>) -> DataFacts {
        let Some(text) = text else {
            return DataFacts {
                unreadable: true,
                ..DataFacts::default()
            };
        };
        if path.extension().is_some_and(|e| e == "xml") {
            return xml::analyze(text, self.series);
        }
        csvdata::analyze(text, &loads_into(path))
    }
}

/// The model a CSV at `path` loads into: `model-suffix.csv` loads into model, so one model can have
/// several files.
fn loads_into(path: &Path) -> String {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    stem.split('-').next().unwrap_or_default().to_owned()
}

/// An XML file in the encoding it declares, a CSV as `series` reads it, any other as UTF-8.
fn decoded<'b>(path: &Path, bytes: &'b [u8], series: Option<Series>) -> Option<Cow<'b, str>> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("xml") => decode::xml_text(bytes),
        Some("csv") => decode::csv_text(bytes, series),
        _ => std::str::from_utf8(bytes).ok().map(Cow::Borrowed),
    }
}

fn parse_module_files(module: &mut Module, default_series: Option<Series>) {
    let series = module.series.or(default_series);
    let checked = module.checked;
    let context = ModuleContext {
        dir: &module.dir,
        ranks: ranks(&module.manifest, series),
        data_entries: module.manifest.data_entries(series),
        series,
        checked,
    };
    let files = module_files(&module.dir);
    let parsed: Vec<_> = files.into_par_iter().filter_map(|path| context.file(path)).collect();
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
    module.sql_views = sql_views(module, series);
}

/// The views the SQL files that a module's manifest loads in `series` create by name.
fn sql_views(module: &Module, series: Option<Series>) -> Vec<String> {
    let texts = module
        .manifest
        .sql_files(series)
        .filter_map(|file| std::fs::read_to_string(module.dir.join(normalize(file))).ok());
    texts.flat_map(|text| sql::file_views(&text)).collect()
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
            // Odoo decodes assets as UTF-8 whatever they declare (18.0 `odoo/addons/base/models/assetsbundle.py:776`).
            let text = std::fs::read_to_string(&path).ok()?;
            Some(SourceFile {
                rel: normalize(&path.strip_prefix(dir).ok()?.to_string_lossy()),
                rank: None,
                demo: false,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_loads_the_files_it_names_and_chart_templates() {
        let mentions = FxHashSet::from_iter(["pages.xml"]);
        let none = Manifest::default();
        assert!(code_loaded("data/pages.xml", &mentions, &none, None));
        assert!(code_loaded("pages.xml", &mentions, &none, None));
        assert!(code_loaded(
            "data/template/account.tax.csv",
            &FxHashSet::default(),
            &none,
            None
        ));
        assert!(!code_loaded("views/page.xml", &mentions, &none, None));
        assert!(!code_loaded("template/account.tax.csv", &mentions, &none, None));
    }

    #[test]
    fn code_loads_the_files_a_key_odoo_ignores_lists_by_path() {
        let m = manifest::parse(r#"{"data": ["views/a.xml"], "other_files": ["data/b.xml"]}"#).unwrap();
        let mentions = FxHashSet::default();
        assert!(code_loaded("data/b.xml", &mentions, &m, None));
        assert!(!code_loaded("tests/b.xml", &mentions, &m, None));
        assert!(!code_loaded("tests/a.xml", &mentions, &m, None));
    }

    #[test]
    fn code_does_not_load_a_file_a_legacy_key_was_meant_to() {
        let m = manifest::parse(r#"{"update_xml": ["views/a.xml"], "other_files": ["views/a.xml"]}"#).unwrap();
        let mentions = FxHashSet::default();
        assert!(!code_loaded("views/a.xml", &mentions, &m, Series::parse("17.0")));
        assert!(code_loaded("views/a.xml", &mentions, &m, Series::parse("14.0")));
    }

    #[test]
    fn a_file_listed_as_data_too_is_no_demo() {
        let m = manifest::parse(r#"{"data": ["a.xml", "b.xml"], "demo": ["b.xml", "c.xml"]}"#).unwrap();
        let series = Series::parse("18.0");
        let ranks = ranks(&m, series);
        assert!(ranks["b.xml"] < m.data_entries(series));
        assert!(ranks["c.xml"] >= m.data_entries(series));
    }

    #[test]
    fn ranks_a_file_listed_twice_at_its_first_place() {
        let m = manifest::parse(r#"{"init_xml": ["./a.xml"], "data": ["b.xml", "a.xml"], "demo": ["c.xml"]}"#).unwrap();
        let ranks = ranks(&m, Series::parse("14.0"));
        assert_eq!(ranks.get("a.xml"), Some(&0));
        assert_eq!(ranks.get("b.xml"), Some(&1));
        assert_eq!(ranks.get("c.xml"), Some(&3));
    }
}
