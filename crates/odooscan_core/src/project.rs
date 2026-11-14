use std::borrow::Cow;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use rayon::iter::Either;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{DataFacts, JsFacts, Pos, PyFacts, RawDiag, TemplateFacts};
use crate::manifest::{self, Manifest, OdooReads, Rejection, normalize};
use crate::mentions::{Mentions, Words};
use crate::series::Series;
use crate::suppress::Suppressions;
use crate::taint::TaintTable;
use crate::{csvdata, decode, js, owl, python, routing, sql, xml};

const MANIFESTS: &[&str] = &["__manifest__.py", "__openerp__.py"];

const ACCESS_MODELS: &[&str] = &["ir.model.access", "ir.access"];

/// The extensions of the files a bundle takes (18.0 `odoo/tools/constants.py:4-7`).
pub const ASSET_EXTENSIONS: [&str; 6] = ["js", "css", "scss", "sass", "less", "xml"];

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
    /// The hook names its text mentions, comments included.
    pub mentions: Mentions,
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
            mentions: self.mentions,
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
    /// The files under `static/` Odoo may serve as assets, as `<module>/<path>`, for a checked
    /// module whose manifest has `assets`.
    pub static_assets: Vec<String>,
    /// A file whose text may name a method could not be read: Python, XML or CSV that does not
    /// decode, or a script or template under `static/src` that is not UTF-8.
    pub lost_text: bool,
    /// The words of every file of a checked module read with text, the manifest included; none
    /// for an indexed one.
    pub words: Words,
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

/// A checked module with no files, for tests to fill in.
#[cfg(test)]
pub(crate) fn module(name: &str, manifest: &str) -> Module {
    let manifest = manifest::parse(manifest).unwrap();
    Module {
        name: name.to_owned(),
        dir: PathBuf::new(),
        manifest_path: PathBuf::new(),
        series: manifest.series(),
        manifest,
        checked: true,
        python: vec![],
        data: vec![],
        js: vec![],
        templates: vec![],
        sql_views: vec![],
        static_assets: vec![],
        lost_text: false,
        words: Words::default(),
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

/// Whether `path` is an Odoo source checkout rather than an addons directory.
fn is_checkout(path: &Path) -> bool {
    path.join("odoo/addons/base").is_dir()
}

/// Whether the scan is given an Odoo core: an addons entry that is a checkout, a `base` module in
/// an addons entry that is not a checkout, or a `base` module among the paths. The paths, which
/// may be deep trees, are walked only when no addons entry gives a core.
pub fn has_core(paths: &[PathBuf], addons_paths: &[PathBuf]) -> bool {
    let is_base = |dir: &PathBuf| dir.canonicalize().is_ok_and(|d| d.file_name() == Some("base".as_ref()));
    addons_paths.iter().any(|p| is_checkout(p))
        || addons_paths
            .iter()
            .flat_map(|p| modules_in_addons_path(p))
            .chain(paths.iter().flat_map(|p| modules_under(p)))
            .any(|dir| is_base(&dir))
}

/// The series a checkout's `release.py` names in `version_info`, such as `(18, 0, ...)` or
/// `('saas~18', 1, ...)`.
fn checkout_series(release: &Path) -> Option<Series> {
    let text = std::fs::read_to_string(release).ok()?;
    let line = text.lines().find(|l| l.trim_start().starts_with("version_info"))?;
    let (_, tuple) = line.split_once('(')?;
    let mut items = tuple.split(',').map(|i| i.trim().trim_matches(['\'', '"']));
    Series::parse(&format!("{}.{}", items.next()?, items.next()?))
}

/// One notice per checkout on the addons path whose series is not the target, since its core is
/// what the checked modules are held to. Only an entry that is a whole checkout counts: its
/// `odoo/addons` alone does not hold `release.py`, and the scan reads nothing outside its paths.
fn checkout_notices(addons_paths: &[PathBuf], target: Series) -> Vec<String> {
    let mut seen = FxHashSet::default();
    let differing = addons_paths.iter().filter(|p| is_checkout(p)).filter_map(|p| {
        let release = p.join("odoo/release.py");
        let series = checkout_series(&release).filter(|s| *s != target)?;
        seen.insert(release.canonicalize().unwrap_or(release)).then(|| {
            format!(
                "{}: the Odoo checkout is {series}, not the target series {target}",
                p.display()
            )
        })
    });
    differing.collect()
}

/// Modules directly inside an addons directory, as Odoo reads `--addons-path`. An Odoo source
/// checkout stands for its two addons directories.
fn modules_in_addons_path(path: &Path) -> Vec<PathBuf> {
    if is_checkout(path) {
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

/// The words of the scripts and templates under `dir/static` outside `static/src`, which the scan
/// reads already, and the third-party `static/lib`. A tour under `static/tests` that a test bundle
/// loads uses a module as `static/src` does. Words are ASCII, so a file that is not UTF-8 loses none.
fn static_words(dir: &Path) -> Words {
    let root = dir.join("static");
    if !root.is_dir() {
        return Words::default();
    }
    let skipped = [root.join("src"), root.join("lib")];
    let paths: Vec<PathBuf> = WalkBuilder::new(&root)
        .filter_entry(move |e| e.file_name() != "node_modules" && !skipped.iter().any(|s| s == e.path()))
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(|e| e.into_path())
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("js" | "xml")))
        .collect();
    let words: Vec<Words> = paths
        .into_par_iter()
        .filter_map(|path| std::fs::read(path).ok())
        .map(|bytes| Words::scan(&String::from_utf8_lossy(&bytes)))
        .collect();
    Words::merge(&words)
}

/// The files under `dir/static` whose extension Odoo serves as an asset, as `name/<path>`. Odoo
/// globs the disk, so files an ignore file names or hidden ones count, and links are followed.
fn static_assets(dir: &Path, name: &str) -> Vec<String> {
    let root = dir.join("static");
    if !root.is_dir() {
        return vec![];
    }
    let served = |p: &Path| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ASSET_EXTENSIONS.contains(&e))
    };
    WalkBuilder::new(root)
        .standard_filters(false)
        .follow_links(true)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()) && served(e.path()))
        .filter_map(|e| {
            Some(format!(
                "{name}/{}",
                normalize(&e.path().strip_prefix(dir).ok()?.to_string_lossy())
            ))
        })
        .collect()
}

pub struct Discovery {
    pub modules: Vec<Module>,
    pub notices: Vec<String>,
    /// The checked modules whose manifest Odoo cannot read, by path.
    pub broken: Vec<Broken>,
}

/// The manifest of a checked module that Odoo cannot read, so the module is not loaded.
pub struct Broken {
    pub path: PathBuf,
    /// The OD000 it gets.
    pub diag: RawDiag,
    pub suppressions: Suppressions,
}

/// A manifest `manifest::read` rejects, of the module `name`.
struct Rejected {
    name: String,
    path: PathBuf,
    checked: bool,
    bytes: Vec<u8>,
    rejection: Rejection,
}

/// Reads the modules, `target` standing for the series of each checked one.
pub fn load(
    paths: &[PathBuf],
    addons_paths: &[PathBuf],
    default_series: Option<Series>,
    target: Option<Series>,
    taint: &TaintTable,
) -> Discovery {
    let mut notices = target.map_or_else(Vec::new, |t| checkout_notices(addons_paths, t));
    let (mut modules, rejected): (Vec<Module>, Vec<Rejected>) = candidates(paths, addons_paths)
        .into_par_iter()
        .filter_map(|(name, dir, checked)| read_module(name, dir, checked, target, default_series))
        .partition_map(|read| match read {
            Ok(module) => Either::Left(module),
            Err(rejected) => Either::Right(rejected),
        });
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
        .for_each(|module| parse_module_files(module, default_series, taint));
    let known = python::field_classes(modules.iter().flat_map(|m| &m.python).map(|f| &f.facts));
    let routes = routing::Table::of_modules(&modules);
    modules
        .par_iter_mut()
        .for_each(|module| read_again(module, default_series, &known, &routes, taint));
    notices.extend(python2_notices(&modules, default_series));
    let (broken, rejected_notices) = rejected_manifests(rejected);
    notices.extend(rejected_notices);
    Discovery {
        modules,
        notices,
        broken,
    }
}

/// The checked manifests of `rejected` that get OD000, by path, and a sorted notice for each other
/// one: its module is not loaded, so the modules that depend on it get OD503. A manifest the Odoo
/// of its series may read gets a notice alone, as a Python 2 file does.
fn rejected_manifests(rejected: Vec<Rejected>) -> (Vec<Broken>, Vec<String>) {
    let mut broken = vec![];
    let mut notices = vec![];
    for r in rejected {
        let state = if r.checked { "checked" } else { "indexed" };
        let file = r.path.file_name().unwrap_or_default().to_string_lossy();
        let series = r.rejection.series.map(|s| s.to_string()).unwrap_or_default();
        let what = match r.rejection.odoo {
            OdooReads::AsPython2 => format!("Python 2 syntax, which {series} runs"),
            OdooReads::AsExpression => {
                format!("{series} evaluates the manifest, which odooscan reads only as literal key/value pairs")
            }
            OdooReads::No if r.checked => {
                broken.push(Broken {
                    diag: r.rejection.diag(),
                    suppressions: Suppressions::parse(&String::from_utf8_lossy(&r.bytes)),
                    path: r.path,
                });
                continue;
            }
            OdooReads::No => r.rejection.error.value.clone(),
        };
        let what = format!("{what}; this module is not {state}");
        notices.push(notice(&r.name, &file, r.rejection.error.pos, &what));
    }
    broken.sort_by(|a, b| a.path.cmp(&b.path));
    notices.sort();
    (broken, notices)
}

/// A notice on the file `rel` of module `module` at `pos`.
fn notice(module: &str, rel: &str, pos: Pos, what: &str) -> String {
    format!("{module}/{rel}:{}:{}: {what}", pos.line, pos.col)
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
                let what = format!("Python 2 syntax, which {} runs; this file is not checked", series?);
                Some(notice(&m.name, &f.rel, pos, &what))
            })
        })
        .collect();
    notices.sort();
    notices
}

/// Reads again the files the first pass could not read whole: those whose class attributes call
/// a field class of the scan, which the first pass takes for a method, and those with a route that
/// inherits from a controller of the scan. Either may live in any module.
fn read_again(
    module: &mut Module,
    default_series: Option<Series>,
    known: &python::FieldClasses,
    routes: &routing::Table,
    taint: &TaintTable,
) {
    let series = module.series.or(default_series);
    let addon = module.name.as_str();
    let unread = python::FieldClasses::default();
    module.python.par_iter_mut().for_each(|file| {
        let calls_fields = file.facts.attr_calls.iter().any(|name| known.contains_key(name));
        if !(calls_fields || file.facts.inherits_routes) {
            return;
        }
        // A file read again for its routes alone reads no field class of the scan, as the first
        // pass did: they also decide the groups a call outside a class attribute references.
        let fields = if calls_fields { known } else { &unread };
        if let Ok(text) = std::fs::read_to_string(&file.path) {
            let scope = routing::Scope {
                table: routes,
                addon,
                rel: &file.rel,
            };
            file.facts = python::analyze_with(&text, series, fields, taint, scope);
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

/// The module in `dir`, or its manifest when odooscan cannot read it; none when the manifest cannot
/// be read at all or the module is not installable. A checked module takes `target` for its series,
/// an indexed one keeps its manifest's, and either falls back to `default_series`.
fn read_module(
    name: String,
    dir: PathBuf,
    checked: bool,
    target: Option<Series>,
    default_series: Option<Series>,
) -> Option<Result<Module, Rejected>> {
    let manifest_path = manifest_in(&dir)?;
    let bytes = std::fs::read(&manifest_path).ok()?;
    let series = |own: Option<Series>| target.filter(|_| checked).or(own).or(default_series);
    let manifest = match manifest::read(&bytes, series) {
        Ok(manifest) => manifest,
        Err(rejection) => {
            return Some(Err(Rejected {
                name,
                path: manifest_path,
                checked,
                bytes,
                rejection,
            }));
        }
    };
    manifest.installable.then(|| {
        Ok(Module {
            series: target.filter(|_| checked).or(manifest.series()),
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
            static_assets: vec![],
            lost_text: false,
            words: Words::default(),
        })
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
    manifest: &'a Path,
    ranks: FxHashMap<String, usize>,
    data_entries: usize,
    series: Option<Series>,
    checked: bool,
    taint: &'a TaintTable,
}

/// A Python file on the left, a data file on the right.
type ParsedFile = Result<SourceFile<PyFacts>, SourceFile<DataFacts>>;

impl ModuleContext<'_> {
    /// Reads `path`, none when it cannot be read or is Python that does not decode and that Odoo does
    /// not refuse, whether its text is lost, and its words in a checked module. A data file that does
    /// not decode stays, since what it may define matters to the rules; one that decodes but does
    /// not parse keeps its text.
    fn file(&self, path: PathBuf) -> (Option<ParsedFile>, bool, Words) {
        let Ok(bytes) = std::fs::read(&path) else {
            return (None, true, Words::default());
        };
        let Some(text) = decoded(&path, &bytes, self.series) else {
            return (self.undecoded(path, &bytes), true, Words::default());
        };
        let words = if self.checked {
            Words::scan(&text)
        } else {
            Words::default()
        };
        (self.parsed(path, &text, &bytes), false, words)
    }

    fn parsed(&self, path: PathBuf, text: &str, bytes: &[u8]) -> Option<ParsedFile> {
        let source = self.source(path, Some(text))?;
        if is_python(&source.path) {
            let unread = python::FieldClasses::default();
            let facts = python::analyze_with(text, self.series, &unread, self.taint, routing::Scope::default());
            return Some(Ok(source.with(facts)));
        }
        let facts = self.data_facts(&source, text, bytes);
        Some(Err(source.with(facts)))
    }

    /// A file that does not decode. One Odoo refuses gets OD000 where it stops, and is read for the
    /// suppressions of its text with each byte that is not UTF-8 replaced.
    fn undecoded(&self, path: PathBuf, bytes: &[u8]) -> Option<ParsedFile> {
        let Some(pos) = self.refused(&path, bytes) else {
            if is_python(&path) {
                return None;
            }
            let unreadable = DataFacts {
                unreadable: true,
                ..DataFacts::default()
            };
            return Some(Err(self.source(path, None)?.with(unreadable)));
        };
        let lossy = String::from_utf8_lossy(bytes);
        let source = self.source(path, Some(&lossy))?;
        Some(match source.path.extension().and_then(|e| e.to_str()) {
            Some("py") => Ok(source.with(python::undecoded(&lossy, pos, self.series, self.taint))),
            Some("xml") => Err(source.with(xml::undecoded(pos))),
            _ => Err(source.with(csvdata::undecoded(pos))),
        })
    }

    /// Where Odoo stops decoding `bytes`: Python but the manifest, and a data file the manifest
    /// loads.
    fn refused(&self, path: &Path, bytes: &[u8]) -> Option<Pos> {
        let loaded = || self.rel(path).is_some_and(|rel| self.ranks.contains_key(&rel));
        match path.extension().and_then(|e| e.to_str())? {
            "py" if path != self.manifest => decode::python_refused(bytes),
            "xml" if loaded() => decode::xml_refused(bytes),
            "csv" if loaded() && decode::csv_strict(self.series) => decode::invalid_utf8(bytes),
            _ => None,
        }
    }

    /// `path` relative to the module, as the manifest writes it.
    fn rel(&self, path: &Path) -> Option<String> {
        Some(normalize(&path.strip_prefix(self.dir).ok()?.to_string_lossy()))
    }

    /// `path` as a file of the module, its facts to come.
    fn source(&self, path: PathBuf, text: Option<&str>) -> Option<SourceFile<()>> {
        let rel = self.rel(&path)?;
        let rank = self.ranks.get(&rel).copied();
        let suppressions = text.filter(|_| self.checked).map(Suppressions::parse);
        Some(SourceFile {
            demo: rank.is_some_and(|r| r >= self.data_entries),
            facts: (),
            path,
            rel,
            rank,
            suppressions: suppressions.unwrap_or_default(),
            mentions: text.map(Mentions::scan).unwrap_or_default(),
        })
    }

    /// The facts of a data file that decodes. A CSV the manifest loads is read as Odoo's import
    /// reads it from 11.0, which refuses what the series before leave to the reader, on the lines
    /// of every series the module may target.
    fn data_facts(&self, source: &SourceFile<()>, text: &str, bytes: &[u8]) -> DataFacts {
        let path = &source.path;
        if path.extension().is_some_and(|e| e == "xml") {
            let mut facts = xml::analyze(text, self.series);
            if text.as_bytes() != bytes {
                facts.drop_fixes();
            }
            return facts;
        }
        let strict = source.rank.is_some() && decode::csv_strict(self.series);
        let lines = if strict { csvdata::Lines::of(self.series) } else { &[] };
        csvdata::analyze(text, &loads_into(path), lines)
    }
}

fn is_python(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "py")
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

fn parse_module_files(module: &mut Module, default_series: Option<Series>, taint: &TaintTable) {
    let series = module.series.or(default_series);
    let checked = module.checked;
    let context = ModuleContext {
        dir: &module.dir,
        manifest: &module.manifest_path,
        ranks: ranks(&module.manifest, series),
        data_entries: module.manifest.data_entries(series),
        series,
        checked,
        taint,
    };
    let files = module_files(&module.dir);
    let parsed: Vec<_> = files.into_par_iter().map(|path| context.file(path)).collect();
    let mut words = Vec::with_capacity(parsed.len() + 2);
    for (file, lost, file_words) in parsed {
        module.lost_text |= lost;
        words.push(file_words);
        match file {
            Some(Ok(py)) => module.python.push(py),
            Some(Err(data)) => module.data.push(data),
            None => {}
        }
    }
    module
        .data
        .sort_by_key(|f| (f.rank.unwrap_or(usize::MAX), f.rel.clone()));
    let (js, lost_js, js_words) = parse_static_files(&module.dir, "js", checked, js::analyze);
    module.js = js;
    let (templates, lost_templates, template_words) = parse_static_files(&module.dir, "xml", checked, owl::analyze);
    module.templates = templates;
    module.lost_text |= lost_js || lost_templates;
    words.extend([js_words, template_words]);
    if checked {
        words.push(static_words(&module.dir));
    }
    module.words = Words::merge(&words);
    module.sql_views = sql_views(module, series);
    if checked && !module.manifest.bundles.is_empty() {
        module.static_assets = static_assets(&module.dir, &module.name);
    }
}

/// The views the SQL files that a module's manifest loads in `series` create by name.
fn sql_views(module: &Module, series: Option<Series>) -> Vec<String> {
    let texts = module
        .manifest
        .sql_files(series)
        .filter_map(|file| std::fs::read_to_string(module.dir.join(normalize(file))).ok());
    texts.flat_map(|text| sql::file_views(&text)).collect()
}

/// The files of `extension` under `static/src`, whether one of them could not be read, and their
/// words when the module is checked.
fn parse_static_files<T: Send>(
    dir: &Path,
    extension: &str,
    checked: bool,
    analyze: fn(&str) -> T,
) -> (Vec<SourceFile<T>>, bool, Words) {
    let paths = static_files(dir, extension);
    let count = paths.len();
    let (mut files, words): (Vec<_>, Vec<_>) = paths
        .into_par_iter()
        .filter_map(|path| {
            // Odoo decodes assets as UTF-8 whatever they declare (18.0 `odoo/addons/base/models/assetsbundle.py:776`).
            let text = std::fs::read_to_string(&path).ok()?;
            let words = if checked { Words::scan(&text) } else { Words::default() };
            let file = SourceFile {
                rel: normalize(&path.strip_prefix(dir).ok()?.to_string_lossy()),
                rank: None,
                demo: false,
                suppressions: if checked {
                    Suppressions::parse(&text)
                } else {
                    Suppressions::default()
                },
                facts: analyze(&text),
                mentions: Mentions::scan(&text),
                path,
            };
            Some((file, words))
        })
        .unzip();
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    let lost = files.len() < count;
    (files, lost, Words::merge(&words))
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
    fn only_a_file_that_does_not_decode_loses_its_text() {
        let lost = |name: &str| {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/custom")
                .join(name);
            let modules = load(&[dir], &[], None, None, &TaintTable::default()).modules;
            modules.iter().find(|m| m.name == name).unwrap().lost_text
        };
        assert!(!lost("encoded_broken"));
        assert!(lost("orphan_hooks_lost"));
    }

    #[test]
    fn a_file_whose_route_inherits_is_read_again_with_the_routes_of_the_scan() {
        let custom = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/custom");
        let paths = [custom.join("route_overrides"), custom.join("bad_sale")];
        let modules = load(&paths, &[], None, None, &TaintTable::default()).modules;
        let module = modules.iter().find(|m| m.name == "route_overrides").unwrap();
        let file = module.python.iter().find(|f| f.rel == "controllers/routes.py").unwrap();
        assert!(file.facts.inherits_routes);
        let lines: Vec<(&str, u32)> = file.facts.diags.iter().map(|d| (d.code, d.pos.line)).collect();
        assert_eq!(lines, [("OD127", 8), ("OD127", 13), ("OD111", 18), ("OD129", 22)]);
    }

    #[test]
    fn a_file_read_again_for_its_routes_alone_reads_no_field_class_of_the_scan() {
        let dir = std::env::temp_dir().join(format!("odooscan-route-reread-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let files = [
            (
                "orm_tags/orm/tag.py",
                "from odoo import fields\n\nclass Tag(fields.Char):\n    pass\n",
            ),
            (
                "shop/controllers/main.py",
                "from odoo import http\n\nclass Shop(http.Controller):\n    @http.route('/a', auth='public')\n    def a(self):\n        pass\n",
            ),
            (
                "ext/controllers/main.py",
                "from odoo import http\nfrom odoo.addons.orm_tags.orm.tag import Tag\nfrom odoo.addons.shop.controllers.main import Shop\nfrom odoo.http import request\n\nLABEL = Tag(groups='base.group_user')\n\nclass Ext(Shop):\n    @http.route()\n    def a(self):\n        return request.env['x'].sudo().search([])\n",
            ),
        ];
        for (rel, src) in files {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, src).unwrap();
        }
        let paths = ["orm_tags", "shop", "ext"].map(|name| {
            std::fs::write(dir.join(name).join("__manifest__.py"), "{'version': '18.0.1.0.0'}").unwrap();
            dir.join(name)
        });
        let modules = load(&paths, &[], None, None, &TaintTable::default()).modules;
        let _ = std::fs::remove_dir_all(&dir);
        let module = modules.iter().find(|m| m.name == "ext").unwrap();
        let facts = &module
            .python
            .iter()
            .find(|f| f.rel == "controllers/main.py")
            .unwrap()
            .facts;
        assert_eq!(facts.diags.iter().map(|d| d.code).collect::<Vec<_>>(), ["OD111"]);
        assert!(facts.group_refs.is_empty(), "{:?}", facts.group_refs);
    }

    /// A module `name` in `dir`, created when missing, whose manifest holds `manifest`.
    fn written(dir: &Path, name: &str, manifest: &str) -> PathBuf {
        let module = dir.join(name);
        std::fs::create_dir_all(&module).unwrap();
        std::fs::write(module.join("__manifest__.py"), manifest).unwrap();
        module
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("odooscan-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn only_a_checked_module_keeps_the_words_of_its_text() {
        let dir = scratch("words");
        let own = written(&dir, "own", "{'depends': ['lib']}");
        std::fs::create_dir_all(own.join("static/src")).unwrap();
        std::fs::write(own.join("static/src/a.js"), "import '@lib/b';").unwrap();
        std::fs::write(own.join("a.py"), "x = 'lib.b'  # odoo.addons.lib").unwrap();
        std::fs::create_dir_all(own.join("static/tests/tours")).unwrap();
        std::fs::write(
            own.join("static/tests/tours/t.js"),
            b"// tour \xff\nimport '@lib/../c';",
        )
        .unwrap();
        std::fs::create_dir_all(own.join("static/lib")).unwrap();
        std::fs::write(own.join("static/lib/vendor.js"), "vendored").unwrap();
        let addons = dir.join("addons");
        written(&addons, "lib", "{'name': 'lib.c'}");
        let modules = load(&[own], &[addons], None, None, &TaintTable::default()).modules;
        let _ = std::fs::remove_dir_all(&dir);
        let words = |name: &str| {
            let module = modules.iter().find(|m| m.name == name).unwrap();
            module.words.iter().map(str::to_owned).collect::<Vec<_>>()
        };
        assert_eq!(
            words("own"),
            [
                "depends",
                "import",
                "lib",
                "lib.b",
                "lib/../c",
                "lib/b",
                "odoo.addons.lib",
                "tour",
                "x"
            ]
        );
        assert!(words("lib").is_empty());
    }

    #[test]
    fn a_checked_manifest_that_does_not_parse_is_broken() {
        let dir = scratch("manifest-checked");
        let module = written(&dir, "open", "# odooscan: ignore[OD101]\n{'name': 'open',\n");
        let discovery = load(&[module], &[], None, None, &TaintTable::default());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(discovery.modules.is_empty() && discovery.notices.is_empty());
        let [broken] = &discovery.broken[..] else {
            panic!("{} broken", discovery.broken.len());
        };
        assert!(broken.path.ends_with("open/__manifest__.py"));
        assert_eq!((broken.diag.code, broken.diag.pos.line), ("OD000", 3));
        assert!(!broken.suppressions.allows(2, "OD101"));
    }

    #[test]
    fn a_checked_manifest_of_pairs_is_read() {
        let dir = scratch("manifest-pairs");
        let module = written(
            &dir,
            "paired",
            "[('name', 'x'), ('version', '18.0.1.0.0'), ('depends', ['base'])]",
        );
        let discovery = load(&[module], &[], None, None, &TaintTable::default());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(discovery.broken.is_empty() && discovery.notices.is_empty());
        let [module] = &discovery.modules[..] else {
            panic!("{} modules", discovery.modules.len());
        };
        assert_eq!(module.series, Series::parse("18.0"));
        let depends: Vec<_> = module.manifest.depends.iter().map(|d| d.value.as_str()).collect();
        assert_eq!(depends, ["base"]);
    }

    #[test]
    fn an_indexed_manifest_that_does_not_parse_gets_a_notice() {
        let dir = scratch("manifest-indexed");
        let checked = written(&dir.join("checked"), "main", "{'depends': ['open']}");
        written(&dir.join("addons"), "open", "{'name': OPEN}");
        let discovery = load(&[checked], &[dir.join("addons")], None, None, &TaintTable::default());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(discovery.broken.is_empty());
        assert_eq!(discovery.modules.len(), 1);
        let notice = "open/__manifest__.py:1:10: not a literal: Odoo reads the manifest with ast.literal_eval; this module is not indexed";
        assert!(
            discovery.notices.iter().any(|n| n == notice),
            "{:#?}",
            discovery.notices
        );
    }

    #[test]
    fn a_python_2_manifest_gets_a_notice_on_the_series_that_runs_it() {
        let dir = scratch("manifest-python2");
        let module = written(&dir, "legacy", "{'name': 'legacy', 'sequence': 0755}");
        let read = |series: &str| {
            load(
                std::slice::from_ref(&module),
                &[],
                Series::parse(series),
                None,
                &TaintTable::default(),
            )
        };
        let ten = read("10.0");
        let eleven = read("11.0");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(ten.broken.is_empty());
        let notice = "legacy/__manifest__.py:1:32: Python 2 syntax, which 10.0 runs; this module is not checked";
        assert_eq!(ten.notices, [notice]);
        assert_eq!(eleven.broken.len(), 1);
        assert!(eleven.notices.is_empty(), "{:#?}", eleven.notices);
    }

    #[test]
    fn an_indexed_python_2_or_non_utf_8_manifest_gets_a_notice() {
        let dir = scratch("manifest-indexed-python2");
        let checked = written(&dir.join("checked"), "main", "{'depends': ['legacy', 'latin']}");
        let addons = dir.join("addons");
        written(&addons, "legacy", "{'sequence': 0755}");
        written(&addons, "latin", "");
        std::fs::write(
            addons.join("latin/__manifest__.py"),
            b"{'version': '11.0.1.0.0', 'category': 'r\xe9'}",
        )
        .unwrap();
        let discovery = load(
            &[checked],
            &[addons],
            Series::parse("10.0"),
            None,
            &TaintTable::default(),
        );
        let _ = std::fs::remove_dir_all(&dir);
        assert!(discovery.broken.is_empty());
        assert_eq!(
            discovery.notices,
            [
                "latin/__manifest__.py:1:41: the manifest is not UTF-8; this module is not indexed",
                "legacy/__manifest__.py:1:14: Python 2 syntax, which 10.0 runs; this module is not indexed",
            ]
        );
    }

    #[test]
    fn a_manifest_python_2_odoo_reads_is_not_broken() {
        let dir = scratch("manifest-python2-reads");
        let latin1 = dir.join("latin1");
        std::fs::create_dir_all(&latin1).unwrap();
        std::fs::write(latin1.join("__manifest__.py"), b"{'category': 'r\xe9sum\xe9'}").unwrap();
        let called = written(&dir, "called", "dict(name='called')");
        let read = |series: &str| {
            load(
                &[latin1.clone(), called.clone()],
                &[],
                Series::parse(series),
                Series::parse(series),
                &TaintTable::default(),
            )
        };
        let (nine, ten) = (read("9.0"), read("10.0"));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(nine.broken.is_empty());
        assert_eq!(nine.modules.len(), 1);
        let notice = "called/__manifest__.py:1:1: 9.0 evaluates the manifest, which odooscan reads only as literal key/value pairs; this module is not checked";
        assert_eq!(nine.notices, [notice]);
        assert_eq!(ten.modules.len(), 1);
        let [broken] = &ten.broken[..] else {
            panic!("{} broken", ten.broken.len());
        };
        assert!(broken.path.ends_with("called/__manifest__.py"));
    }

    /// The OD000 of each file of module `name`, as `rel:line`, with `files` written into it.
    fn refusals(name: &str, version: &str, data: &str, files: &[(&str, &[u8])]) -> Vec<String> {
        let dir = scratch(name);
        let manifest = format!("{{'version': '{version}', 'data': [{data}]}}");
        let module = written(&dir, name, &manifest);
        for (rel, bytes) in files {
            let path = module.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let modules = load(&[module], &[], None, None, &TaintTable::default()).modules;
        let _ = std::fs::remove_dir_all(&dir);
        let module = &modules[0];
        let python = module.python.iter().map(|f| (&f.rel, &f.facts.diags));
        let data = module.data.iter().map(|f| (&f.rel, &f.facts.diags));
        let mut found: Vec<String> = python
            .chain(data)
            .flat_map(|(rel, diags)| {
                diags
                    .iter()
                    .filter(|d| d.code == "OD000")
                    .map(move |d| format!("{rel}:{}", d.pos.line))
            })
            .collect();
        found.sort();
        found
    }

    #[test]
    fn a_loaded_file_odoo_cannot_decode_is_refused() {
        let files: &[(&str, &[u8])] = &[
            ("models/latin1.py", b"class A:\n    _name = 'x.a'\n    s = '\xE9'\n"),
            ("models/coded.py", b"# coding: latin-1\ns = '\xE9'\n"),
            ("data/a.xml", b"<odoo>\n\xE9</odoo>"),
            (
                "data/b.xml",
                b"<?xml version='1.0' encoding='windows-1252'?>\n<odoo>\xE9</odoo>",
            ),
            ("data/stray.xml", b"<odoo>\n\xE9</odoo>"),
            ("data/res.partner.csv", b"id,name\nr,\xE9\n"),
        ];
        let data = "'data/a.xml', 'data/b.xml', 'data/res.partner.csv'";
        assert_eq!(
            refusals("refused18", "18.0.1.0.0", data, files),
            ["data/a.xml:2", "data/res.partner.csv:2", "models/latin1.py:3"]
        );
        assert_eq!(
            refusals("refused10", "10.0.1.0.0", data, files),
            ["data/a.xml:2", "models/latin1.py:3"]
        );
    }

    #[test]
    fn a_refused_python_file_still_names_its_model() {
        let dir = scratch("refused-model");
        let module = written(&dir, "refused_model", "{'version': '18.0.1.0.0'}");
        std::fs::create_dir_all(module.join("models")).unwrap();
        let source = b"class A(models.Model):\n    _name = 'x.a'\n    # odooscan: ignore[OD000]\n    s = '\xE9'\n";
        std::fs::write(module.join("models/a.py"), source).unwrap();
        let modules = load(&[module], &[], None, None, &TaintTable::default()).modules;
        let _ = std::fs::remove_dir_all(&dir);
        let file = modules[0].python.iter().find(|f| f.rel == "models/a.py").unwrap();
        assert!(modules[0].lost_text);
        assert_eq!(file.facts.diags[0].pos.line, 4);
        assert!(file.facts.classes[0].model == "x.a" && file.facts.classes[0].unread);
        assert!(!file.suppressions.allows(4, "OD000"));
    }

    #[test]
    fn a_manifest_odoo_reads_is_not_refused_as_python() {
        let dir = scratch("refused-manifest");
        let module = dir.join("legacy");
        std::fs::create_dir_all(&module).unwrap();
        std::fs::write(
            module.join("__manifest__.py"),
            b"{'version': '10.0.1.0.0', 'author': 'Ren\xE9'}",
        )
        .unwrap();
        let modules = load(&[module], &[], None, None, &TaintTable::default()).modules;
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(modules.len(), 1);
        assert!(modules[0].python.is_empty());
    }

    fn checkout(name: &str, release: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("odooscan-checkout-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("odoo/addons/base")).unwrap();
        std::fs::create_dir_all(dir.join("addons/web")).unwrap();
        std::fs::write(dir.join("odoo/release.py"), release).unwrap();
        dir
    }

    #[test]
    fn a_checkout_names_its_series_in_version_info() {
        let stable = checkout(
            "stable",
            "RELEASE_LEVELS = [ALPHA]\nversion_info = (18, 0, 0, FINAL, 0, '')\n",
        );
        let saas = checkout("saas", "version_info = ('saas~18', 1, 0, ALPHA, 1, '')\n");
        let release = |dir: &Path| checkout_series(&dir.join("odoo/release.py"));
        assert_eq!(release(&stable), Series::parse("18.0"));
        assert_eq!(release(&saas), Series::parse("saas~18.1"));
        let entries = [
            stable.clone(),
            saas.clone(),
            saas.join("."),
            saas.join("odoo/addons"),
            saas.join("addons"),
        ];
        let notices = checkout_notices(&entries, Series::parse("18.0").unwrap());
        let differing = format!(
            "{}: the Odoo checkout is 18.1, not the target series 18.0",
            saas.display()
        );
        assert_eq!(notices, [differing]);
        for dir in [stable, saas] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn a_core_is_a_base_module_or_a_checkout() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let module = fixtures.join("target/port17");
        assert!(has_core(std::slice::from_ref(&module), &[fixtures.join("core")]));
        assert!(has_core(&[fixtures.join("core/base")], &[]));
        // Without a manifest, base is no module, yet the directory makes the entry a checkout.
        let bare = checkout("bare", "");
        assert!(has_core(std::slice::from_ref(&module), std::slice::from_ref(&bare)));
        assert!(!has_core(std::slice::from_ref(&module), &[bare.join("addons")]));
        assert!(!has_core(&[module], &[fixtures.join("target")]));
        let _ = std::fs::remove_dir_all(bare);
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
