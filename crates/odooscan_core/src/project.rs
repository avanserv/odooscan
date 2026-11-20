use std::borrow::Cow;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use rayon::iter::Either;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{DataFacts, JsFacts, PoFacts, Pos, PyFacts, RawDiag, TemplateEntry, TemplateFacts};
use crate::manifest::{self, Manifest, OdooReads, Rejection, Taken, normalize};
use crate::mentions::{Mentions, Words};
use crate::module_state::UPGRADE_DIRS;
use crate::series::Series;
use crate::suppress::Suppressions;
use crate::taint::TaintTable;
use crate::{csvdata, decode, imports, index, js, owl, po, python, routing, sql, xml};

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

impl<T> SourceFile<T> {
    /// Whether the file is a test by its path: under `static/tests`, or in a `tests` folder under
    /// `static/src`, which a test bundle lists from the manifests, as 18.0 and 20.0
    /// `addons/barcodes_gs1_nomenclature/__manifest__.py:19-20` do. A script is a test also by the
    /// bundles that load it: ask `Index::js_test` for those.
    pub fn in_tests(&self) -> bool {
        test_path(&self.rel)
    }
}

/// `SourceFile::in_tests` of a file at `rel`.
fn test_path(rel: &str) -> bool {
    rel.starts_with("static/tests/")
        || rel.strip_prefix("static/src/").is_some_and(|rest| {
            let mut dirs = rest.split('/');
            dirs.next_back();
            dirs.any(|d| d == "tests")
        })
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
    /// Scripts under `static/src`, then those under `static/tests`: the JS Odoo names as modules,
    /// but for the libraries under `static/lib`.
    pub js: Vec<SourceFile<JsFacts>>,
    /// Owl templates: the XML files under `static/src`.
    pub templates: Vec<SourceFile<TemplateFacts>>,
    /// The `.po` files directly in `i18n/` and `i18n_extra/` of a checked module, where Odoo looks
    /// for a language's catalog (18.0 `odoo/tools/translate.py:1788-1790`).
    pub translations: Vec<SourceFile<PoFacts>>,
    /// The messages of `i18n/<module>.pot` of a checked module, the template Odoo's export writes.
    pub template: Option<SourceFile<Vec<TemplateEntry>>>,
    /// The views the SQL files the manifest loads in the module's series create.
    pub sql_views: Vec<String>,
    /// The files under `static/` Odoo may serve as assets, as `<module>/<path>`, for a checked
    /// module whose manifest has `assets`.
    pub static_assets: Vec<String>,
    /// The plain `assets` paths of the module outside `static/` that name a file, normalized to
    /// `<module>/<path>`, for a checked module whose manifest has `assets`.
    pub outside_assets: Vec<String>,
    /// A file whose text may name a method could not be read: Python, XML or CSV that does not
    /// decode, or a script or template under `static/src` that is not UTF-8.
    pub lost_text: bool,
    /// One of the files of `lost_text` is one the scan cannot read at all, so none of its words is
    /// known.
    pub unread_text: bool,
    /// The hook names of the files of `lost_text` that do not decode, scripts under a test path
    /// aside, as `Loss::undecoded` reads them. A script only a test bundle loads still counts.
    pub lost_mentions: Vec<Mentions>,
    /// The words of every file of a checked module read with text, the manifest included; none
    /// for an indexed one.
    pub words: Words,
}

impl Module {
    /// Whether the scan lists `static_assets` and `outside_assets`, so that an empty list means no
    /// file.
    pub fn reads_static_assets(&self) -> bool {
        self.checked && !self.manifest.bundles.is_empty()
    }

    /// Whether the manifest loads a data file the scan cannot read, so that the records of the
    /// module are not all known.
    pub fn lost_data(&self) -> bool {
        self.lost_files().next().is_some()
    }

    /// Whether `lost_data` covers a file that may hold an access row.
    pub fn lost_access(&self) -> bool {
        self.lost_into(ACCESS_MODELS)
    }

    /// Whether `lost_data` covers a file that may hold a view.
    pub fn lost_views(&self) -> bool {
        self.lost_into(&["ir.ui.view"])
    }

    /// Whether `lost_data` covers a file that may hold a record of one of `models`. A CSV loads into
    /// the model it is named after, so one of another model holds none.
    fn lost_into(&self, models: &[&str]) -> bool {
        self.lost_files().any(|f| {
            let path = Path::new(&f.rel);
            path.extension().is_none_or(|e| e != "csv") || models.contains(&loads_into(path).as_str())
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
        translations: vec![],
        template: None,
        sql_views: vec![],
        static_assets: vec![],
        outside_assets: vec![],
        lost_text: false,
        unread_text: false,
        lost_mentions: vec![],
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

fn static_files(dir: &Path, folder: &str, extension: &str) -> Vec<PathBuf> {
    let root = dir.join("static").join(folder);
    if !root.is_dir() {
        return vec![];
    }
    WalkBuilder::new(root)
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

/// The paths of `outside_assets` that are files under `module.dir`. Odoo globs a plain path to
/// itself, so one `is_file` per path stands for its glob.
fn outside_assets(module: &Module) -> Vec<String> {
    let paths = module.manifest.bundles.iter().flat_map(|b| &b.paths);
    paths
        .filter_map(|path| plain_outside(&module.name, &path.value))
        .filter(|rel| {
            let in_module = rel.split_once('/').map_or("", |(_, p)| p);
            module.dir.join(in_module).is_file()
        })
        .collect()
}

/// The normalized `<module>/<path>` of an `assets` path of `name`'s that holds no wildcard and
/// stays inside the module but outside `static/`.
fn plain_outside(name: &str, path: &str) -> Option<String> {
    if index::wildcard(path) || path.split('/').find(|p| !p.is_empty()) != Some(name) {
        return None;
    }
    let parts = index::normal_parts(path);
    (parts.len() > 1 && parts[0] == name && !index::in_static(name, &parts)).then(|| parts.join("/"))
}

pub struct Discovery {
    pub modules: Vec<Module>,
    pub notices: Vec<String>,
    /// The checked modules whose manifest Odoo cannot read, by path.
    pub broken: Vec<Broken>,
    /// The modules, checked or indexed, whose manifest `manifest::read` rejects, by name.
    pub rejected: FxHashMap<String, Unindexed>,
    /// The modules, checked or indexed, whose manifest sets `installable` to false, so Odoo never
    /// installs them, by name. A rejected one is not among them: whether it is installable
    /// depends on the series reading it (`Unindexed::on`).
    pub uninstallable: FxHashSet<String>,
}

/// The manifest of a checked module that Odoo cannot read, so the module is not loaded.
pub struct Broken {
    pub path: PathBuf,
    /// The OD000 it gets.
    pub diag: RawDiag,
    pub suppressions: Suppressions,
}

/// How the Odoo of each checked module's series takes a manifest `manifest::read` rejects.
#[derive(Debug)]
pub struct Unindexed {
    /// Why the series odooscan read the manifest as rejects it.
    rejection: Rejection,
    /// The manifest read again as the series of each checked module but that one.
    again: Vec<(Option<Series>, Taken)>,
}

impl Unindexed {
    /// How the Odoo of `series`, a checked module's, takes the manifest.
    pub fn on(&self, series: Option<Series>) -> Taken {
        let again = self.again.iter().find(|(s, _)| *s == series);
        again.map_or_else(|| self.rejection.taken_on(series), |&(_, taken)| taken)
    }
}

/// A manifest `manifest::read` rejects, of the module `name`.
struct Rejected {
    name: String,
    path: PathBuf,
    checked: bool,
    bytes: Vec<u8>,
    rejection: Rejection,
}

/// Why `read_module` reads no module from a manifest.
enum Unread {
    Rejected(Rejected),
    /// The module of this name sets `installable` to false.
    Uninstallable(String),
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
    let (mut modules, unread): (Vec<Module>, Vec<Unread>) = candidates(paths, addons_paths)
        .into_par_iter()
        .filter_map(|(name, dir, checked)| read_module(name, dir, checked, target, default_series))
        .partition_map(|read| match read {
            Ok(module) => Either::Left(module),
            Err(unread) => Either::Right(unread),
        });
    let (rejected, uninstallable) = split_unread(unread);
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
    let known = module_field_classes(&modules, default_series);
    let routes = routing::Table::of_modules(&modules);
    modules
        .par_iter_mut()
        .zip(&known)
        .for_each(|(module, known)| read_again(module, default_series, known, &routes, taint));
    notices.extend(python2_notices(&modules, default_series));
    let (broken, rejected_notices) = rejected_manifests(&rejected);
    notices.extend(rejected_notices);
    let series = checked_series(&modules, default_series);
    let unindexed = rejected.into_iter().map(|r| unindexed(r, &series)).collect();
    Discovery {
        modules,
        notices,
        broken,
        rejected: unindexed,
        uninstallable,
    }
}

/// The rejected manifests of `unread`, and the names of the other modules, which are not
/// installable.
fn split_unread(unread: Vec<Unread>) -> (Vec<Rejected>, FxHashSet<String>) {
    let mut rejected = vec![];
    let mut uninstallable = FxHashSet::default();
    for u in unread {
        match u {
            Unread::Rejected(r) => rejected.push(r),
            Unread::Uninstallable(name) => {
                uninstallable.insert(name);
            }
        }
    }
    (rejected, uninstallable)
}

/// The series of the checked modules, sorted and each once.
fn checked_series(modules: &[Module], default_series: Option<Series>) -> Vec<Option<Series>> {
    let checked = modules.iter().filter(|m| m.checked);
    let mut series: Vec<_> = checked.map(|m| m.series.or(default_series)).collect();
    series.sort_unstable();
    series.dedup();
    series
}

/// The name of `r` and how the Odoo of each of `series` takes its manifest. Odooscan read it as
/// the series it names, else the default one, and a checked one as `target` when given, while
/// the Odoo of the module installing it reads it whatever series it names, so it is read again
/// as each other one.
fn unindexed(r: Rejected, series: &[Option<Series>]) -> (String, Unindexed) {
    let other = series.iter().copied().filter(|&s| s != r.rejection.series);
    let again = other.map(|s| (s, manifest::taken_as(&r.bytes, s))).collect();
    let unindexed = Unindexed {
        rejection: r.rejection,
        again,
    };
    (r.name, unindexed)
}

/// The checked manifests of `rejected` that get OD000, by path, and a sorted notice for each other
/// one: its module is not indexed. A module depending on it gets OD503 unless the Odoo of that
/// module's series reads the manifest, as 9.0 does a Python 2 file, and the manifest leaves the
/// module installable; then this notice alone names it.
fn rejected_manifests(rejected: &[Rejected]) -> (Vec<Broken>, Vec<String>) {
    let mut broken = vec![];
    let mut notices = vec![];
    for r in rejected {
        let state = if r.checked { "checked" } else { "indexed" };
        let file = r.path.file_name().unwrap_or_default().to_string_lossy();
        let series = r.rejection.series.map(|s| s.to_string()).unwrap_or_default();
        let what = match r.rejection.odoo {
            OdooReads::AsPython2 | OdooReads::AsPython2Expression => format!("Python 2 syntax, which {series} runs"),
            OdooReads::AsExpression => {
                format!("{series} evaluates the manifest, which odooscan reads only as literal key/value pairs")
            }
            OdooReads::No if r.checked => {
                broken.push(Broken {
                    diag: r.rejection.diag(),
                    suppressions: Suppressions::parse(&String::from_utf8_lossy(&r.bytes)),
                    path: r.path.clone(),
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

/// The field classes the files of a module may call, by `module_field_classes`.
#[derive(Default)]
struct ScopedFieldClasses {
    /// Those of the module's scope.
    module: python::FieldClasses,
    /// Those of the module's scope widened by the addons that a group of its files, which Odoo does
    /// not load with the module, imports outside that scope.
    widened: Vec<python::FieldClasses>,
    /// By file index, the index in `widened` of the classes a file of such a group may call.
    files: FxHashMap<usize, usize>,
}

impl ScopedFieldClasses {
    /// The field classes the file at index `fi` may call.
    fn of_file(&self, fi: usize) -> &python::FieldClasses {
        self.files.get(&fi).map_or(&self.module, |&w| &self.widened[w])
    }
}

/// When Odoo imports a file of a module.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Loading {
    /// With the module.
    Module,
    /// For a test run alone, with every other test file: Odoo imports `<addon>.tests`, the folder
    /// or a root `tests.py` (18.0 `odoo/tests/loader.py:57-62`; 20.0 `:56-61`).
    Tests,
    /// On an upgrade, one script at a time by its path under `odoo.upgrade` (18.0
    /// `odoo/modules/migration.py:139-140`, `:237`; 20.0 `:140-141`, `:249`).
    Upgrade,
}

/// When Odoo imports the file at `rel`; only the top-level folders count.
pub(crate) fn loading(rel: &str) -> Loading {
    let dir = rel.split('/').next().unwrap_or_default();
    if dir == "tests" || rel == "tests.py" {
        Loading::Tests
    } else if UPGRADE_DIRS.contains(&dir) {
        Loading::Upgrade
    } else {
        Loading::Module
    }
}

/// The files of `module` that Odoo imports together and not with the module, by file index: its
/// test files as one group, and each upgrade script alone.
fn unloaded_groups(module: &Module) -> Vec<Vec<(usize, &SourceFile<PyFacts>)>> {
    let files = module.python.iter().enumerate();
    let (tests, upgrades): (Vec<_>, Vec<_>) = files
        .filter(|(_, f)| loading(&f.rel) != Loading::Module)
        .partition(|(_, f)| loading(&f.rel) == Loading::Tests);
    let upgrades = upgrades.into_iter().map(|file| vec![file]);
    [tests].into_iter().chain(upgrades).collect()
}

/// What `module_field_classes` works out the scope of each module from.
struct FieldScope<'a> {
    modules: &'a [Module],
    by_name: FxHashMap<&'a str, usize>,
    always: Vec<bool>,
    /// The field classes of every module of the scan.
    all: python::FieldClasses,
    series: Option<Series>,
}

impl FieldScope<'_> {
    /// Whether `file` calls a field class of the scan.
    fn calls(&self, file: &SourceFile<PyFacts>) -> bool {
        file.facts.attr_calls.iter().any(|name| self.all.contains_key(name))
    }

    /// The scope of `seeds`, and the field classes it defines.
    fn classes(&self, seeds: impl IntoIterator<Item = usize>) -> (Vec<bool>, python::FieldClasses) {
        let scope = index::scope_of(self.modules, &self.by_name, &self.always, seeds, self.series);
        let in_scope = self.modules.iter().zip(&scope).filter_map(|(m, &s)| s.then_some(m));
        let classes = python::field_classes(in_scope.flat_map(|m| &m.python).map(|f| &f.facts));
        (scope, classes)
    }

    /// The field classes the files of the module at `mi` may call.
    fn of_module(&self, mi: usize, module: &Module) -> ScopedFieldClasses {
        if !module.python.iter().any(|f| self.calls(f)) {
            return ScopedFieldClasses::default();
        }
        let loaded = module.python.iter().filter(|f| loading(&f.rel) == Loading::Module);
        let seeds: Vec<usize> = loaded
            .flat_map(|f| imported_modules(f, &self.by_name))
            .chain([mi])
            .collect();
        let (scope, classes) = self.classes(seeds.iter().copied());
        let mut scoped = ScopedFieldClasses {
            module: classes,
            ..ScopedFieldClasses::default()
        };
        for group in unloaded_groups(module) {
            let callers = group.iter().filter(|(_, f)| self.calls(f)).map(|&(fi, _)| fi);
            let callers: Vec<usize> = callers.collect();
            let imported = group.iter().flat_map(|(_, f)| imported_modules(f, &self.by_name));
            let imported: Vec<usize> = imported.collect();
            if callers.is_empty() || imported.iter().all(|&a| scope[a]) {
                continue;
            }
            let widened = self.classes(seeds.iter().copied().chain(imported)).1;
            let slot = scoped.widened.len();
            scoped.files.extend(callers.into_iter().map(|fi| (fi, slot)));
            scoped.widened.push(widened);
        }
        scoped
    }
}

/// The field classes each module may call, those its scope defines, bases included. The scope is
/// that of `Index::scope`, seeded with the addons the module's files that load with it import by
/// path too, so a same-named class outside it does not count (OD210's page). The test files, and
/// each upgrade script alone, widen that scope by their own imports for themselves. A module that
/// calls none of the scan's field classes gets none, without its scope being worked out.
fn module_field_classes(modules: &[Module], series: Option<Series>) -> Vec<ScopedFieldClasses> {
    let by_name: FxHashMap<&str, usize> = modules.iter().enumerate().map(|(i, m)| (m.name.as_str(), i)).collect();
    let ctx = FieldScope {
        modules,
        always: index::always_installed(modules, &by_name, series),
        by_name,
        all: python::field_classes(modules.iter().flat_map(|m| &m.python).map(|f| &f.facts)),
        series,
    };
    modules
        .par_iter()
        .enumerate()
        .map(|(mi, m)| ctx.of_module(mi, m))
        .collect()
}

/// The indexes of the modules of `by_name` that `file` imports by path.
fn imported_modules<'a>(
    file: &'a SourceFile<PyFacts>,
    by_name: &'a FxHashMap<&str, usize>,
) -> impl Iterator<Item = usize> + 'a {
    let rel = file.rel.as_str();
    let facts = file.facts.imports.iter().flat_map(|i| &i.list);
    let addons = facts.flat_map(move |fact| imports::imported_addons(rel, fact));
    addons.filter_map(|addon| by_name.get(addon.as_str()).copied())
}

/// Reads again the files the first pass could not read whole: those whose class attributes call
/// a field class `known` gives the file, which the first pass takes for a method, and those with a
/// route that inherits from a controller of the scan.
fn read_again(
    module: &mut Module,
    default_series: Option<Series>,
    known: &ScopedFieldClasses,
    routes: &routing::Table,
    taint: &TaintTable,
) {
    let series = module.series.or(default_series);
    let addon = module.name.as_str();
    let unread = python::FieldClasses::default();
    module.python.par_iter_mut().enumerate().for_each(|(fi, file)| {
        let known = known.of_file(fi);
        let calls_fields = file.facts.attr_calls.iter().any(|name| known.contains_key(name));
        if !(calls_fields || file.facts.inherits_routes) {
            return;
        }
        // A file read again for its routes alone reads no field class of its scope, as the first
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

/// The module in `dir`, or why it is not read: odooscan rejects its manifest, or the module is not
/// installable. None when the manifest cannot be read at all. A checked module takes `target` for
/// its series, an indexed one keeps its manifest's, and either falls back to `default_series`.
fn read_module(
    name: String,
    dir: PathBuf,
    checked: bool,
    target: Option<Series>,
    default_series: Option<Series>,
) -> Option<Result<Module, Unread>> {
    let manifest_path = manifest_in(&dir)?;
    let bytes = std::fs::read(&manifest_path).ok()?;
    let series = |own: Option<Series>| target.filter(|_| checked).or(own).or(default_series);
    let manifest = match manifest::read(&bytes, series) {
        Ok(manifest) => manifest,
        Err(rejection) => {
            return Some(Err(Unread::Rejected(Rejected {
                name,
                path: manifest_path,
                checked,
                bytes,
                rejection,
            })));
        }
    };
    if !manifest.installable {
        return Some(Err(Unread::Uninstallable(name)));
    }
    Some(Ok(Module {
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
        translations: vec![],
        template: None,
        sql_views: vec![],
        static_assets: vec![],
        outside_assets: vec![],
        lost_text: false,
        unread_text: false,
        lost_mentions: vec![],
        words: Words::default(),
    }))
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

/// What the scan lost of the text of some files that may name a method.
#[derive(Default)]
struct Loss {
    /// One does not decode or cannot be read.
    text: bool,
    /// One cannot be read at all.
    unread: bool,
    /// The hook names of those that do not decode, as `Loss::undecoded` reads them.
    mentions: Vec<Mentions>,
}

impl Loss {
    fn unread() -> Self {
        Self {
            text: true,
            unread: true,
            mentions: vec![],
        }
    }

    /// A file that does not decode, read as UTF-16 or UTF-32 when `decode::wide_text` takes it, and
    /// otherwise with each byte that is not UTF-8 replaced, which keeps every ASCII byte, so every
    /// hook name, in place.
    fn undecoded(bytes: &[u8]) -> Self {
        let text = decode::wide_text(bytes).map_or_else(|| String::from_utf8_lossy(bytes), Cow::Owned);
        Self {
            text: true,
            unread: false,
            mentions: vec![Mentions::scan(&text)],
        }
    }

    fn add(&mut self, other: Self) {
        self.text |= other.text;
        self.unread |= other.unread;
        self.mentions.extend(other.mentions);
    }

    fn into_module(self, module: &mut Module) {
        module.lost_text |= self.text;
        module.unread_text |= self.unread;
        module.lost_mentions.extend(self.mentions);
    }
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
    /// not refuse, what of its text is lost, and its words in a checked module. A data file that does
    /// not decode stays, since what it may define matters to the rules; one that decodes but does
    /// not parse keeps its text.
    fn file(&self, path: PathBuf) -> (Option<ParsedFile>, Loss, Words) {
        let Ok(bytes) = std::fs::read(&path) else {
            return (None, Loss::unread(), Words::default());
        };
        let Some(text) = decoded(&path, &bytes, self.series) else {
            return (self.undecoded(path, &bytes), Loss::undecoded(&bytes), Words::default());
        };
        let words = if self.checked {
            Words::scan(&text)
        } else {
            Words::default()
        };
        (self.parsed(path, &text, &bytes), Loss::default(), words)
    }

    fn parsed(&self, path: PathBuf, text: &str, bytes: &[u8]) -> Option<ParsedFile> {
        let source = self.source(path, Some(text))?;
        if is_python(&source.path) {
            let unread = python::FieldClasses::default();
            let scope = routing::Scope {
                rel: &source.rel,
                ..routing::Scope::default()
            };
            let facts = python::analyze_with(text, self.series, &unread, self.taint, scope);
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
    /// reads it from 11.0, which refuses what the series before leave to the reader, with the line
    /// breaks of every series the module may target.
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
        let lines = if strict {
            csvdata::LineBreaks::of(self.series)
        } else {
            &[]
        };
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
    let mut loss = Loss::default();
    for (file, file_loss, file_words) in parsed {
        loss.add(file_loss);
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
    let (mut js, js_loss, js_words) = parse_static_files(&module.dir, "src", "js", checked, js::analyze, None);
    // `static_words` reads the words of tests already, and a test that does not decode hides no hook
    // name, as `hook_mentions` takes no name from a test.
    let (tests, _, _) = parse_static_files(&module.dir, "tests", "js", checked, js::analyze, None);
    js.extend(tests);
    module.js = js;
    let (templates, template_loss, template_words) =
        parse_static_files(&module.dir, "src", "xml", checked, owl::analyze, Some(owl::undecoded));
    module.templates = templates;
    loss.add(js_loss);
    loss.add(template_loss);
    loss.into_module(module);
    words.extend([js_words, template_words]);
    if checked {
        words.push(static_words(&module.dir));
    }
    module.words = Words::merge(&words);
    if checked {
        let (translations, template) = translations(&module.dir, &module.name);
        module.translations = translations;
        module.template = template.map(|mut template| {
            mark_unread(&mut template.facts, &module.python, &module.dir, &module.name);
            template
        });
    }
    module.sql_views = sql_views(module, series);
    if module.reads_static_assets() {
        module.static_assets = static_assets(&module.dir, &module.name);
        module.outside_assets = outside_assets(module);
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

/// The `.po` catalogs of the module `name` in `dir` that decode as UTF-8, each merged with the
/// `<name>.pot` of its folder as Odoo merges them, and the template of `i18n/`. Odoo names the
/// template after the module's folder (18.0 `odoo/tools/translate.py:807-809`), which `dir` does
/// not name when it is `.`. Their words name no hook, so they add none.
fn translations(dir: &Path, name: &str) -> (Vec<SourceFile<PoFacts>>, Option<SourceFile<Vec<TemplateEntry>>>) {
    let read: Vec<_> = ["i18n", "i18n_extra"]
        .par_iter()
        .map(|&folder| {
            let messages = folder == "i18n";
            let folder = dir.join(folder);
            let path = folder.join(format!("{name}.pot"));
            let template = std::fs::read_to_string(&path).ok().map(|text| po::template(&text));
            let (merge, template) = match template {
                Some((merge, entries, suppressions)) => (Some(merge), Some((path, entries, suppressions))),
                None => (None, None),
            };
            (catalogs(dir, &folder, merge.as_ref()), template.filter(|_| messages))
        })
        .collect();
    let mut files = vec![];
    let mut template = None;
    for (catalogs, pot) in read {
        files.extend(catalogs);
        template = template.or(pot);
    }
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    let template = template.and_then(|(path, entries, suppressions)| {
        Some(SourceFile {
            rel: normalize(&path.strip_prefix(dir).ok()?.to_string_lossy()),
            rank: None,
            demo: false,
            suppressions,
            facts: entries,
            mentions: Mentions::default(),
            path,
        })
    });
    (files, template)
}

/// The `.po` files directly in `folder` of the module in `dir`, merged with `template`.
fn catalogs(dir: &Path, folder: &Path, template: Option<&po::Template>) -> Vec<SourceFile<PoFacts>> {
    let paths = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .filter_map(|e| Some(e.ok()?.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "po"));
    paths
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            let (facts, suppressions) = po::analyze(&text, template);
            Some(SourceFile {
                rel: normalize(&path.strip_prefix(dir).ok()?.to_string_lossy()),
                rank: None,
                demo: false,
                suppressions,
                facts,
                mentions: Mentions::default(),
                path,
            })
        })
        .collect()
}

/// Marks each entry of the template of the module `name` in `dir` with a reference to one of its
/// `.py` files that exists outside `python`: one under `static/`, in a nested module, ignored by
/// git, or not UTF-8. Odoo's export reads every `.py` file of the module (18.0
/// `odoo/tools/translate.py:1443-1447`), so such a file may hold the message.
fn mark_unread(entries: &mut [TemplateEntry], python: &[SourceFile<PyFacts>], dir: &Path, name: &str) {
    let read: FxHashSet<&str> = python.iter().map(|f| f.rel.as_str()).collect();
    let prefix = format!("code:addons/{name}/");
    for entry in entries {
        entry.unread = entry.references.iter().any(|reference| {
            let Some(rel) = reference.strip_prefix(&prefix).map(po::reference_path) else {
                return false;
            };
            rel.ends_with(".py") && !read.contains(rel) && dir.join(rel).is_file()
        });
    }
}

/// The files of `extension` under `static/<folder>`, what of their text is lost, and their words
/// when the module is checked. One that is not UTF-8 is kept with the facts `undecoded` gives, and
/// dropped without it.
fn parse_static_files<T: Send>(
    dir: &Path,
    folder: &str,
    extension: &str,
    checked: bool,
    analyze: fn(&str) -> T,
    undecoded: Option<fn() -> T>,
) -> (Vec<SourceFile<T>>, Loss, Words) {
    let paths = static_files(dir, folder, extension);
    let read: Vec<_> = paths
        .into_par_iter()
        .map(|path| static_file(dir, path, checked, analyze, undecoded))
        .collect();
    let mut files = Vec::with_capacity(read.len());
    let mut loss = Loss::default();
    let mut words = Vec::with_capacity(read.len());
    for (file, file_loss, file_words) in read {
        files.extend(file);
        loss.add(file_loss);
        words.push(file_words);
    }
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    (files, loss, Words::merge(&words))
}

/// A file of `parse_static_files`, none when it cannot be read or is not UTF-8 without `undecoded`,
/// what of its text is lost, and its words.
fn static_file<T>(
    dir: &Path,
    path: PathBuf,
    checked: bool,
    analyze: fn(&str) -> T,
    undecoded: Option<fn() -> T>,
) -> (Option<SourceFile<T>>, Loss, Words) {
    let rel = path.strip_prefix(dir).ok().map(|rel| normalize(&rel.to_string_lossy()));
    let (Some(rel), Ok(bytes)) = (rel, std::fs::read(&path)) else {
        return (None, Loss::unread(), Words::default());
    };
    // Odoo decodes assets as UTF-8 whatever they declare (18.0 `odoo/addons/base/models/assetsbundle.py:776`).
    let Ok(text) = std::str::from_utf8(&bytes) else {
        let mut loss = Loss::undecoded(&bytes);
        // `hook_mentions` takes no name from a test script, which runs on a mock server. Only the
        // path tells one here: the bundles that load the script are not known yet.
        if path.extension().is_some_and(|e| e == "js") && test_path(&rel) {
            loss.mentions.clear();
        }
        let file = undecoded.map(|facts| SourceFile {
            rel,
            rank: None,
            demo: false,
            suppressions: Suppressions::default(),
            facts: facts(),
            mentions: Mentions::default(),
            path,
        });
        return (file, loss, Words::default());
    };
    let checked_text = checked.then_some(text);
    let file = SourceFile {
        rel,
        rank: None,
        demo: false,
        suppressions: checked_text.map(Suppressions::parse).unwrap_or_default(),
        facts: analyze(text),
        mentions: Mentions::scan(text),
        path,
    };
    (
        Some(file),
        Loss::default(),
        checked_text.map(Words::scan).unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_a_test_under_static_tests_or_a_tests_folder_under_static_src() {
        let file = |rel: &str| SourceFile {
            path: PathBuf::new(),
            rel: rel.to_owned(),
            rank: None,
            demo: false,
            facts: (),
            suppressions: Suppressions::default(),
            mentions: Mentions::default(),
        };
        for rel in [
            "static/tests/a.js",
            "static/src/tests/a.js",
            "static/src/c/x/tests/a.js",
        ] {
            assert!(file(rel).in_tests(), "{rel}");
        }
        for rel in [
            "static/src/js/tests.js",
            "static/src/tests_shared/a.js",
            "static/src/e/test/a.js",
        ] {
            assert!(!file(rel).in_tests(), "{rel}");
        }
    }

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
        assert!(lost("encoded_lost_csv"));
        assert!(lost("encoded_python"));
        let dir = scratch("lost-xml");
        let module = written(&dir, "lost_xml", "{'version': '18.0.1.0.0', 'data': ['data/a.xml']}");
        std::fs::create_dir_all(module.join("data")).unwrap();
        std::fs::write(module.join("data/a.xml"), b"<odoo><!-- \xE9 --></odoo>").unwrap();
        let modules = load(&[module], &[], None, None, &TaintTable::default()).modules;
        let _ = std::fs::remove_dir_all(&dir);
        assert!(modules[0].lost_text && !modules[0].unread_text);
    }

    #[test]
    fn a_file_that_does_not_decode_keeps_the_hook_names_it_spells() {
        let dir = scratch("lost-mentions");
        let module = written(&dir, "lost_mentions", "{'version': '18.0.1.0.0'}");
        std::fs::create_dir_all(module.join("models")).unwrap();
        std::fs::create_dir_all(module.join("static/src")).unwrap();
        let python = b"# -*- coding: latin-1 -*-\nX = '\xE9'\nY = '_compute_after'\n";
        std::fs::write(module.join("models/a.py"), python).unwrap();
        std::fs::write(module.join("static/src/b.js"), b"// \xE9 _inverse_after\n").unwrap();
        std::fs::create_dir_all(module.join("static/src/tests")).unwrap();
        std::fs::write(module.join("static/src/tests/c.js"), b"// \xE9 _search_in_test\n").unwrap();
        let wide = "\u{FEFF}<odoo><!-- \u{E9} _onchange_wide --></odoo>";
        let wide: Vec<u8> = wide.encode_utf16().flat_map(u16::to_le_bytes).collect();
        std::fs::write(module.join("models/d.xml"), wide).unwrap();
        let modules = load(&[module], &[], None, None, &TaintTable::default()).modules;
        let _ = std::fs::remove_dir_all(&dir);
        let mut names: Vec<_> = modules[0].lost_mentions.iter().flat_map(|m| &m.names).collect();
        names.sort();
        assert_eq!(names, ["_compute_after", "_inverse_after", "_onchange_wide"]);
        assert!(modules[0].lost_text && !modules[0].unread_text);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_the_scan_cannot_open_is_unread() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("unread-text");
        let module = written(&dir, "unread_text", "{'version': '18.0.1.0.0'}");
        std::fs::create_dir_all(module.join("models")).unwrap();
        let path = module.join("models/a.py");
        std::fs::write(&path, "X = 1\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&path).is_ok() {
            let _ = std::fs::remove_dir_all(&dir);
            eprintln!("skipped: this user reads a file without permissions, as root does");
            return;
        }
        let modules = load(&[module], &[], None, None, &TaintTable::default()).modules;
        let _ = std::fs::remove_dir_all(&dir);
        assert!(modules[0].lost_text && modules[0].unread_text);
    }

    #[test]
    fn a_module_given_by_a_path_ending_in_dots_keeps_its_template() {
        let findings = |dir: PathBuf| {
            let modules = load(&[dir], &[], None, None, &TaintTable::default()).modules;
            let module = modules.iter().find(|m| m.name == "translated_shop").unwrap();
            let diags = module.translations.iter().flat_map(|f| &f.facts.diags);
            diags.map(|d| (d.pos.line, d.message.clone())).collect::<Vec<_>>()
        };
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/custom/translated_shop");
        let dotted = findings(dir.join("i18n/.."));
        assert!(dotted.iter().all(|(line, _)| *line != 110), "{dotted:?}");
        assert_eq!(dotted, findings(dir));
    }

    #[test]
    fn a_template_entry_of_a_python_file_the_scan_does_not_read_is_unread() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/custom/exported_stall");
        let modules = load(&[dir], &[], None, None, &TaintTable::default()).modules;
        let template = modules[0].template.as_ref().unwrap();
        assert_eq!(template.rel, "i18n/exported_stall.pot");
        let unread: Vec<_> = template
            .facts
            .iter()
            .filter(|e| e.unread)
            .map(|e| e.msgid.as_str())
            .collect();
        assert_eq!(unread, ["Stall tool"]);
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

    #[test]
    fn a_call_is_a_field_only_when_its_class_is_in_the_module_scope() {
        let dir = scratch("field-scope");
        let model = |name: &str| format!("class X(models.Model):\n    _name = 'x.{name}'\n");
        let files = [
            (
                "orm_vec",
                "{'version': '18.0.1.0.0'}",
                "orm.py",
                "from odoo import fields\n\nclass Vector(fields.Field):\n    pass\n".to_owned(),
            ),
            (
                "user",
                "{'version': '18.0.1.0.0', 'depends': ['orm_vec']}",
                "models.py",
                format!("from odoo import models\n\n{}    v = Vector()\n", model("user")),
            ),
            (
                "plain",
                "{'version': '18.0.1.0.0'}",
                "models.py",
                format!(
                    "from odoo import models\n\nclass Vector:\n    pass\n\nclass Grid(Vector):\n    pass\n\n{}    v = Vector()\n    g = Grid()\n",
                    model("plain")
                ),
            ),
            (
                "importer",
                "{'version': '18.0.1.0.0'}",
                "models.py",
                format!(
                    "from odoo import models\nfrom odoo.addons.orm_vec.orm import Vector\n\n{}    v = Vector()\n",
                    model("importer")
                ),
            ),
        ];
        let paths = files.map(|(name, manifest, rel, src)| {
            let module = written(&dir, name, manifest);
            std::fs::write(module.join(rel), src).unwrap();
            module
        });
        let modules = load(&paths, &[], None, None, &TaintTable::default()).modules;
        let _ = std::fs::remove_dir_all(&dir);
        let read = |name: &str| {
            let module = modules.iter().find(|m| m.name == name).unwrap();
            let class = &module
                .python
                .iter()
                .find(|f| f.rel == "models.py")
                .unwrap()
                .facts
                .classes[0];
            let fields: Vec<&str> = class.fields.iter().map(|f| f.name.as_str()).collect();
            (fields, class.methods.clone())
        };
        let none: Vec<String> = vec![];
        assert_eq!(read("user"), (vec!["v"], none.clone()));
        assert_eq!(read("plain"), (vec![], vec!["v".to_owned(), "g".to_owned()]));
        assert_eq!(read("importer"), (vec!["v"], none));
    }

    #[test]
    fn only_module_code_loads_with_the_module() {
        for rel in [
            "models/a.py",
            "a.py",
            "models/tests/a.py",
            "models/tests.py",
            "migrations.py",
        ] {
            assert_eq!(loading(rel), Loading::Module, "{rel}");
        }
        for rel in ["tests/test_a.py", "tests/common/a.py", "tests.py"] {
            assert_eq!(loading(rel), Loading::Tests, "{rel}");
        }
        for rel in ["migrations/1.0/post-a.py", "upgrades/1.0/post-a.py"] {
            assert_eq!(loading(rel), Loading::Upgrade, "{rel}");
        }
    }

    #[test]
    fn the_test_files_and_each_upgrade_script_widen_the_scope_for_themselves() {
        let dir = scratch("test-field-scope");
        let orm = written(&dir, "orm_vec", "{'version': '18.0.1.0.0'}");
        std::fs::write(
            orm.join("orm.py"),
            "from odoo import fields\n\nclass Vector(fields.Field):\n    pass\n",
        )
        .unwrap();
        let user = written(&dir, "user", "{'version': '18.0.1.0.0', 'depends': ['base']}");
        for sub in ["tests", "migrations/1.0", "upgrades/1.0"] {
            std::fs::create_dir_all(user.join(sub)).unwrap();
        }
        let by_path = "from odoo import models\nfrom odoo.addons.orm_vec.orm import Vector\n\n";
        let plain = "from odoo import models\n\nclass Vector:\n    pass\n\n";
        let files = [
            ("models.py", format!("{plain}class X(models.Model):\n    _name = 'x.user'\n    v = Vector()\n")),
            ("tests/common.py", by_path.to_owned()),
            (
                "tests/test_a.py",
                "from odoo import models\nfrom .common import Vector\n\nclass Y(models.Model):\n    _name = 'x.probe'\n    w = Vector()\n".to_owned(),
            ),
            (
                "migrations/1.0/post-a.py",
                format!("{by_path}class Z(models.Model):\n    _name = 'x.migrated'\n    u = Vector()\n"),
            ),
            (
                "upgrades/1.0/post-b.py",
                format!("{plain}class Q(models.Model):\n    _name = 'x.upgraded'\n    q = Vector()\n"),
            ),
        ];
        for (rel, src) in files {
            std::fs::write(user.join(rel), src).unwrap();
        }
        let modules = load(&[orm, user], &[], None, None, &TaintTable::default()).modules;
        let _ = std::fs::remove_dir_all(&dir);
        let module = modules.iter().find(|m| m.name == "user").unwrap();
        let fields = |rel: &str| {
            let file = module.python.iter().find(|f| f.rel == rel).unwrap();
            file.facts.classes[0]
                .fields
                .iter()
                .map(|f| f.name.clone())
                .collect::<Vec<_>>()
        };
        assert!(fields("models.py").is_empty());
        assert_eq!(fields("tests/test_a.py"), ["w"]);
        assert_eq!(fields("migrations/1.0/post-a.py"), ["u"]);
        assert!(fields("upgrades/1.0/post-b.py").is_empty());
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
        assert_eq!(discovery.rejected.len(), 1);
        assert_eq!(
            discovery.rejected.get("open").map(|u| u.rejection.odoo),
            Some(OdooReads::No)
        );
    }

    #[test]
    fn a_module_that_is_not_installable_is_named_checked_or_indexed() {
        let dir = scratch("manifest-uninstallable");
        let checked = written(&dir.join("checked"), "retired", "{'installable': False}");
        let addons = dir.join("addons");
        written(&addons, "off", "{'installable': False}");
        written(&addons, "legacy", "{'sequence': 0755, 'installable': False}");
        written(&addons, "on", "{'installable': True}");
        let discovery = load(
            &[checked],
            &[addons],
            Series::parse("10.0"),
            None,
            &TaintTable::default(),
        );
        let _ = std::fs::remove_dir_all(&dir);
        let mut uninstallable: Vec<_> = discovery.uninstallable.iter().map(String::as_str).collect();
        uninstallable.sort_unstable();
        assert_eq!(uninstallable, ["off", "retired"]);
        let modules: Vec<_> = discovery.modules.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(modules, ["on"]);
        let legacy = &discovery.rejected["legacy"];
        assert_eq!(legacy.rejection.odoo, OdooReads::AsPython2);
        let ten = Taken {
            reads: true,
            installable: false,
        };
        assert_eq!(legacy.on(Series::parse("10.0")), ten);
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
    fn a_python_2_manifest_10_0_cannot_evaluate_is_broken() {
        let dir = scratch("manifest-python2-eval");
        let module = written(&dir, "legacy", "{'name': 'leg' + 'acy', 'sequence': 0755}");
        let read = |series: &str| {
            load(
                std::slice::from_ref(&module),
                &[],
                Series::parse(series),
                None,
                &TaintTable::default(),
            )
        };
        let (nine, ten) = (read("9.0"), read("10.0"));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(nine.broken.is_empty());
        let notice = "legacy/__manifest__.py:1:37: Python 2 syntax, which 9.0 runs; this module is not checked";
        assert_eq!(nine.notices, [notice]);
        let [broken] = &ten.broken[..] else {
            panic!("{} broken", ten.broken.len());
        };
        assert_eq!((broken.diag.pos.line, broken.diag.pos.col), (1, 10));
        assert_eq!(
            broken.diag.message,
            "not a literal: Odoo reads the manifest with ast.literal_eval"
        );
        assert!(ten.notices.is_empty(), "{:#?}", ten.notices);
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
        assert_eq!(discovery.rejected.len(), 2);
        assert_eq!(
            discovery.rejected.get("legacy").map(|u| u.rejection.odoo),
            Some(OdooReads::AsPython2)
        );
        assert_eq!(
            discovery.rejected.get("latin").map(|u| u.rejection.odoo),
            Some(OdooReads::No)
        );
    }

    #[test]
    fn a_rejected_manifest_is_read_as_each_dependent_series() {
        let dir = scratch("manifest-dependent-series");
        let checked = dir.join("checked");
        let nine = written(&checked, "nine", "{'version': '9.0.1.0.0', 'depends': ['legacy']}");
        let eighteen = written(&checked, "eighteen", "{'version': '18.0.1.0.0', 'depends': ['legacy']}");
        let addons = dir.join("addons");
        written(&addons, "legacy", "{'sequence': 0755}");
        written(&addons, "versioned", "{'version': '10.0.1.0.0', 'name': 'a' + 'b'}");
        let discovery = load(
            &[nine, eighteen],
            &[addons],
            Series::parse("18.0"),
            None,
            &TaintTable::default(),
        );
        let _ = std::fs::remove_dir_all(&dir);
        let legacy = &discovery.rejected["legacy"];
        assert!(legacy.on(Series::parse("9.0")).reads);
        assert!(!legacy.on(Series::parse("18.0")).reads);
        let versioned = &discovery.rejected["versioned"];
        assert!(versioned.on(Series::parse("9.0")).reads);
        assert!(!versioned.on(Series::parse("18.0")).reads);
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
    fn a_refused_csv_loses_its_module_data() {
        let lost = |series: &str| {
            let name = format!("refused-csv-{series}");
            let dir = scratch(&name);
            let manifest = format!("{{'version': '{series}.1.0.0', 'data': ['data/res.partner.csv']}}");
            let module = written(&dir, "short", &manifest);
            std::fs::create_dir_all(module.join("data")).unwrap();
            std::fs::write(module.join("data/res.partner.csv"), "id,name,parent_id:id\nr,a\n").unwrap();
            let modules = load(&[module], &[], None, None, &TaintTable::default()).modules;
            let _ = std::fs::remove_dir_all(&dir);
            modules[0].lost_data()
        };
        assert!(lost("18.0"));
        assert!(!lost("10.0"));
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

    #[test]
    fn a_template_that_is_not_utf8_stays_unreadable() {
        let dir = scratch("undecoded-template");
        let module = written(&dir, "undecoded_template", "{'version': '18.0.1.0.0'}");
        std::fs::create_dir_all(module.join("static/src")).unwrap();
        std::fs::write(module.join("static/src/a.xml"), b"<templates>\xE9</templates>").unwrap();
        // A test script's loss leaves `lost_text` alone, so the template alone sets it.
        std::fs::create_dir_all(module.join("static/tests")).unwrap();
        std::fs::write(module.join("static/tests/b.js"), b"// \xE9\n").unwrap();
        let modules = load(&[module], &[], None, None, &TaintTable::default()).modules;
        let _ = std::fs::remove_dir_all(&dir);
        let rels: Vec<_> = modules[0].templates.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, ["static/src/a.xml"]);
        assert!(modules[0].templates[0].facts.unreadable);
        assert!(modules[0].lost_text);
        assert!(modules[0].js.is_empty());
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
        let nine = checkout("nine", "version_info = (9, 'saas~10', 0, FINAL, 0, 'c')\n");
        let release = |dir: &Path| checkout_series(&dir.join("odoo/release.py"));
        assert_eq!(release(&stable), Series::parse("18.0"));
        assert_eq!(release(&saas), Series::parse("saas~18.1"));
        assert_eq!(release(&nine), Some(Series { major: 9, minor: 10 }));
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
        for dir in [stable, saas, nine] {
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
