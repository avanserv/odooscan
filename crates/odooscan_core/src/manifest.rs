use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;
use ruff_python_ast::{self as ast, Expr, Number, Operator, Stmt, UnaryOp};
use ruff_python_parser::parse_module;
use ruff_text_size::{Ranged, TextSize};
use rustc_hash::FxHashSet;

use crate::decode;
use crate::facts::{Pos, RawDiag, Spanned};
use crate::module_state;
use crate::python::{Locator, str_lit};
use crate::python2;
use crate::series::Series;

/// The first series that reads the manifest with `ast.literal_eval` (`10.0`), where 9.0 calls
/// `eval`.
const LITERAL_EVAL: Series = Series { major: 10, minor: 0 };

/// The first series that loads a module's data from `data` alone (`saas~15.2`).
const DATA_ONLY: Series = Series { major: 15, minor: 2 };

/// The first series that loads the files of `init_xml` with those of `data` again (`19.0`).
const INIT_XML_BACK: Series = Series { major: 19, minor: 0 };

/// The first series that loads the files of `data` alone again (`20.0`).
const INIT_XML_GONE: Series = Series { major: 20, minor: 0 };

/// The keys Odoo loads a module's data from, in its load order.
const DATA_KEYS: [&str; 3] = ["init_xml", "update_xml", "data"];

/// The keys Odoo loads a module's demo data from, in its load order, after the data keys.
const DEMO_KEYS: [&str; 2] = ["demo_xml", "demo"];

/// Whether Odoo loads the files listed under `key` in `series`. An unknown series reads every key.
fn reads(key: &str, series: Option<Series>) -> bool {
    match series {
        Some(s) if s >= DATA_ONLY => match key {
            "data" | "demo" => true,
            "init_xml" => (INIT_XML_BACK..INIT_XML_GONE).contains(&s),
            _ => false,
        },
        _ => true,
    }
}

/// The first series that refuses a module version of a form other than two or three numbers,
/// maybe behind the series (`saas~16.3`).
const VERSION_FORM: Series = Series { major: 16, minor: 3 };

/// The values of `ir.module.module`'s `license` selection, which every series from 10.0 shares
/// (18.0 `odoo/addons/base/models/ir_module.py:320-330`). 9.0 lacks `OPL-1`
/// (`openerp/addons/base/module/module.py:284-294`).
const LICENSES: [&str; 10] = [
    "GPL-2",
    "GPL-2 or any later version",
    "GPL-3",
    "GPL-3 or any later version",
    "AGPL-3",
    "LGPL-3",
    "Other OSI approved licence",
    "OEEL-1",
    "OPL-1",
    "Other proprietary",
];

/// The first series whose license selection has `OPL-1` (`10.0`).
const OPL: Series = Series { major: 10, minor: 0 };

/// The keys Odoo loads a module's files from in `series`, in its load order.
pub fn loaded_keys(series: Option<Series>) -> impl Iterator<Item = &'static str> {
    DATA_KEYS.into_iter().chain(DEMO_KEYS).filter(move |k| reads(k, series))
}

/// The `auto_install` key as written: `True` waits on every depend. The index reads `Listed` by series,
/// as only from `saas~12.4` does Odoo take a list for the depends it waits on.
#[derive(Debug, Default, PartialEq, Eq)]
pub enum AutoInstall {
    #[default]
    Off,
    Depends,
    Listed(Vec<Spanned<String>>),
}

impl AutoInstall {
    /// A collection with an entry that is not a string literal names unknown triggers, so it reads as `Off`.
    fn read(loc: &Source, value: &Expr) -> Self {
        let entries = match value {
            Expr::BooleanLiteral(b) if b.value => return Self::Depends,
            Expr::List(l) => &l.elts[..],
            Expr::Tuple(t) => &t.elts[..],
            Expr::Set(s) => &s.elts[..],
            _ => return Self::Off,
        };
        if entries.iter().any(|e| str_lit(e).is_none()) {
            return Self::Off;
        }
        Self::Listed(list(loc, value))
    }
}

/// A bundle of the manifest's `assets`, at its key.
#[derive(Debug)]
pub struct Bundle {
    pub name: Spanned<String>,
    /// What `asset_globs` reads from its list.
    pub globs: Vec<String>,
    /// The path or glob of each entry but an `include`, as written, at the string.
    pub paths: Vec<Spanned<String>>,
}

/// The `license` key's value, at the value.
#[derive(Debug)]
enum License {
    /// A value Odoo replaces with `LGPL-3`: an empty string, `False` or `None`.
    Empty,
    Named(String),
    /// Any other literal, which this scan does not judge.
    Other,
}

/// The manifest keys odooscan reads, evaluated as literals; the file is never executed.
#[derive(Debug, Default)]
pub struct Manifest {
    pub version: Option<Spanned<String>>,
    license: Option<Spanned<License>>,
    pub depends: Vec<Spanned<String>>,
    pub data: Vec<Spanned<String>>,
    pub init_xml: Vec<String>,
    pub update_xml: Vec<String>,
    pub demo_xml: Vec<String>,
    pub demo: Vec<Spanned<String>>,
    pub category: Option<String>,
    pub installable: bool,
    pub auto_install: AutoInstall,
    /// The `countries` key lists a name.
    pub countries: bool,
    /// Globs the `assets` bundles add, relative to the addons directory.
    pub assets: Vec<String>,
    /// Each bundle `assets` gives a non-empty list.
    pub bundles: Vec<Bundle>,
    /// The entries of the `qweb` key, normalized, at the key, when its list names a file.
    pub qweb: Option<Spanned<Vec<String>>>,
    /// Entries of the list keys Odoo loads nothing from, such as `other_files`, normalized.
    pub unread: Vec<String>,
    /// The findings the manifest alone gives: OD507, OD508, OD511 and OD512.
    pub diags: Vec<RawDiag>,
}

impl Manifest {
    pub fn series(&self) -> Option<Series> {
        self.version
            .as_ref()
            .and_then(|v| Series::from_module_version(&v.value))
    }

    /// The files Odoo loads for the module in `series`, in its load order: those of the data keys
    /// the series reads, then those of its demo keys. A file listed twice appears at each place.
    pub fn loaded_files(&self, series: Option<Series>) -> impl Iterator<Item = &str> {
        self.read_files(&DATA_KEYS, series)
            .chain(self.read_files(&DEMO_KEYS, series))
    }

    /// How many entries of `loaded_files` come from the data keys, ahead of the demo keys.
    pub fn data_entries(&self, series: Option<Series>) -> usize {
        self.read_files(&DATA_KEYS, series).count()
    }

    /// The SQL files Odoo runs when it loads the data of the module in `series`, in load order.
    pub fn sql_files(&self, series: Option<Series>) -> impl Iterator<Item = &str> {
        self.read_files(&DATA_KEYS, series).filter(|f| f.ends_with(".sql"))
    }

    /// The legacy key listing `file`, a path relative to the module, that Odoo ignores in `series`,
    /// when no key it reads lists the file too.
    pub fn ignored_key(&self, file: &str, series: Option<Series>) -> Option<&'static str> {
        let lists = |key: &str| self.listed(key).any(|f| normalize(f) == file);
        let keys = DATA_KEYS.into_iter().chain(DEMO_KEYS);
        if keys.clone().any(|k| reads(k, series) && lists(k)) {
            return None;
        }
        keys.filter(|k| !reads(k, series)).find(|k| lists(k))
    }

    /// OD507 and OD508 on the license, OD511 on an `auto_install` trigger outside `depends`, and
    /// OD512 on a file listed again under `data` or `demo`. `at` is where the dict opens.
    fn findings(&self, at: Pos, series: Option<Series>) -> Vec<RawDiag> {
        let mut diags = vec![];
        match &self.license {
            None => diags.push(RawDiag::new("OD507", at, NO_LICENSE)),
            Some(Spanned {
                value: License::Empty,
                pos,
            }) => diags.push(RawDiag::new("OD507", *pos, NO_LICENSE)),
            Some(Spanned {
                value: License::Named(name),
                pos,
            }) if !known_license(name, series) => diags.push(RawDiag::new(
                "OD508",
                *pos,
                format!("Odoo knows no license '{name}', so updating the module list fails"),
            )),
            Some(_) => {}
        }
        diags.extend(self.foreign_triggers());
        diags.extend(repeated(&self.data, "data"));
        diags.extend(repeated(&self.demo, "demo"));
        diags
    }

    /// The `auto_install` entries `depends` lacks, which fail an assertion as Odoo reads the
    /// manifest (18.0 `odoo/modules/module.py:331-338`). With no `depends`, 20.0 depends on `base`
    /// first (`odoo/modules/module.py:471-475`) where 18.0 does not, so that case stays silent.
    fn foreign_triggers(&self) -> Vec<RawDiag> {
        let AutoInstall::Listed(names) = &self.auto_install else {
            return vec![];
        };
        if self.depends.is_empty() {
            return vec![];
        }
        names
            .iter()
            .filter(|n| !self.depends.iter().any(|d| d.value == n.value))
            .map(|n| {
                let message = format!(
                    "auto_install names '{}', which is not in depends, so Odoo fails to read the manifest",
                    n.value
                );
                RawDiag::new("OD511", n.pos, message)
            })
            .collect()
    }

    fn read_files(&self, keys: &'static [&'static str], series: Option<Series>) -> impl Iterator<Item = &str> {
        keys.iter()
            .filter(move |k| reads(k, series))
            .flat_map(|k| self.listed(k))
    }

    fn listed(&self, key: &str) -> impl Iterator<Item = &str> {
        let (spanned, plain): (&[Spanned<String>], &[String]) = match key {
            "data" => (&self.data, &[]),
            "demo" => (&self.demo, &[]),
            "init_xml" => (&[], &self.init_xml),
            "update_xml" => (&[], &self.update_xml),
            "demo_xml" => (&[], &self.demo_xml),
            _ => (&[], &[]),
        };
        spanned
            .iter()
            .map(|f| f.value.as_str())
            .chain(plain.iter().map(String::as_str))
    }

    fn set(&mut self, loc: &Source, key: &str, at: Pos, value: &Expr) {
        match key {
            "version" | "category" | "installable" => self.set_scalar(loc, key, value),
            "license" => self.set_license(loc, value),
            "depends" | "data" | "demo" | "init_xml" | "update_xml" | "demo_xml" => self.set_files(loc, key, value),
            "assets" => self.set_assets(loc, value),
            "qweb" => self.set_qweb(loc, at, value),
            "auto_install" | "countries" => self.set_install(loc, key, value),
            _ => self.unread.extend(names(loc, value).iter().map(|f| normalize(f))),
        }
    }

    fn set_scalar(&mut self, loc: &Source, key: &str, value: &Expr) {
        match key {
            "version" => {
                self.version = str_lit(value).map(|v| Spanned {
                    value: v.to_owned(),
                    pos: loc.at(value),
                });
            }
            "category" => self.category = str_lit(value).map(str::to_owned),
            "installable" => self.installable = !matches!(value, Expr::BooleanLiteral(b) if !b.value),
            _ => {}
        }
    }

    /// Odoo takes `LGPL-3` for a value that is not truthy (18.0 `odoo/modules/module.py:322-324`).
    fn set_license(&mut self, loc: &Source, value: &Expr) {
        let license = match value {
            Expr::BooleanLiteral(b) if !b.value => License::Empty,
            Expr::NoneLiteral(_) => License::Empty,
            e => match str_lit(e) {
                Some("") => License::Empty,
                Some(name) => License::Named(name.to_owned()),
                None => License::Other,
            },
        };
        self.license = Some(Spanned {
            value: license,
            pos: loc.at(value),
        });
    }

    fn set_files(&mut self, loc: &Source, key: &str, value: &Expr) {
        match key {
            "depends" => self.depends = list(loc, value),
            "data" => self.data = list(loc, value),
            "demo" => self.demo = list(loc, value),
            "init_xml" => self.init_xml = names(loc, value),
            "update_xml" => self.update_xml = names(loc, value),
            "demo_xml" => self.demo_xml = names(loc, value),
            _ => {}
        }
    }

    fn set_assets(&mut self, loc: &Source, value: &Expr) {
        let Expr::Dict(bundles) = value else {
            return;
        };
        for item in &bundles.items {
            let Expr::List(list) = &item.value else {
                continue;
            };
            let globs = asset_globs(&list.elts);
            self.assets.extend(globs.iter().cloned());
            let Some(key) = item.key.as_ref().filter(|_| !list.elts.is_empty()) else {
                continue;
            };
            if let Some(name) = str_lit(key) {
                let name = Spanned {
                    value: name.to_owned(),
                    pos: loc.at(key),
                };
                let paths = asset_paths(loc, &list.elts);
                self.bundles.push(Bundle { name, globs, paths });
            }
        }
    }

    fn set_qweb(&mut self, loc: &Source, at: Pos, value: &Expr) {
        let files: Vec<String> = names(loc, value).iter().map(|f| normalize(f)).collect();
        // Odoo loads no data from `qweb`, so like any such key it exempts its files from OD502.
        self.unread.extend(files.iter().cloned());
        self.qweb = (!files.is_empty()).then_some(Spanned { value: files, pos: at });
    }

    fn set_install(&mut self, loc: &Source, key: &str, value: &Expr) {
        match key {
            "auto_install" => self.auto_install = AutoInstall::read(loc, value),
            "countries" => self.countries = !list(loc, value).is_empty(),
            _ => {}
        }
    }
}

/// Why the Odoo of `series` will not install a module of `version`, none when it does or when the
/// series disagree. From saas~16.3 a version is two or three numbers, maybe behind the series (18.0
/// `odoo/modules/module.py:454-466`); 20.0 also takes `S.x` but makes a module whose version has
/// four or five parts not behind the series uninstallable (`odoo/modules/module.py:579-608`). `S`
/// and `S.x` stay silent, since 18.0 takes only the first and 20.0 only the second. On an unknown
/// series, only the forms every series from saas~16.3 refuses count.
pub fn version_refusal(version: &str, series: Option<Series>) -> Option<String> {
    if series.is_some_and(|s| s < VERSION_FORM) {
        return None;
    }
    let number = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    let behind = series.and_then(|s| prefixes(s).into_iter().find(|p| version.starts_with(&format!("{p}."))));
    let taken = match behind {
        Some(prefix) => {
            let rest: Vec<&str> = version[prefix.len() + 1..].split('.').collect();
            rest.len() <= 3 && rest.iter().all(|p| number(p))
        }
        None => {
            // A saas series writes its own as `saas~M.m`, which a version of unknown series may carry.
            let bare = if series.is_none() {
                version.trim_start_matches("saas~")
            } else {
                version
            };
            let parts: Vec<&str> = bare.split('.').collect();
            let most = if series.is_some() { 3 } else { 5 };
            parts.iter().all(|p| number(p)) && (2..=most).contains(&parts.len())
        }
    };
    if taken {
        return None;
    }
    Some(match series {
        Some(s) => {
            let own = prefixes(s).swap_remove(0);
            format!(
                "version '{version}' is not of the form x.y, x.y.z, {own}.x.y or {own}.x.y.z, so Odoo {own} will not install the module"
            )
        }
        None => format!(
            "version '{version}' is not of the form x.y or x.y.z, behind the series or not, so Odoo will not install the module"
        ),
    })
}

/// How a version starts with `series` behind it: Odoo's own name of the series first, then the
/// `major.minor` a saas series is read from too.
fn prefixes(series: Series) -> Vec<String> {
    let plain = format!("{}.{}", series.major, series.minor);
    if series.minor == 0 {
        vec![plain]
    } else {
        vec![format!("saas~{plain}"), plain]
    }
}

/// The names Odoo takes for a folder of upgrade scripts: two or three numbers, maybe behind a
/// series written `6.1`, `N.0` from 6, `N.saas~M` for N of 7 to 10, or `saas~N.m` for N from 11 but
/// not 100 to 199, and m of 1 to 9 (18.0 `odoo/modules/migration.py:23-47`).
static UPGRADE_FOLDER: LazyLock<Regex> = LazyLock::new(|| {
    let series = r"6\.1|[6-9]\.0|[1-9][0-9]+\.0|(?:7|8|9|10)\.saas~[1-9][0-9]*|saas~(?:1[1-9]|[2-9][0-9]+)\.[1-9]";
    Regex::new(&format!(r"^(?:(?:{series})\.)?[0-9]+\.[0-9]+(?:\.[0-9]+)?$")).unwrap()
});

/// Why the Odoo of `series` never runs the scripts of the upgrade folder `name` of a module of
/// `version`, a version `version_refusal` takes: Odoo skips a name outside `UPGRADE_FOLDER` with a
/// warning, and runs a folder only up to the module version (18.0 `odoo/modules/migration.py:103-115`,
/// `:198-211`). A name of at most three numbers takes the series and drops it again to compare, so
/// it compares with the module version without its series. `0.0.0` runs on every update.
pub fn upgrade_folder_refusal(name: &str, version: &str, series: Series) -> Option<String> {
    if !UPGRADE_FOLDER.is_match(name) {
        return Some(format!(
            "folder '{name}' is not a version of two or three numbers, maybe behind a series, so Odoo skips its scripts"
        ));
    }
    if name == "0.0.0" {
        return None;
    }
    let prefixes = prefixes(series);
    let own = &prefixes[0];
    let adapted = if prefixes.iter().any(|p| version.starts_with(&format!("{p}."))) {
        version.to_owned()
    } else {
        format!("{own}.{version}")
    };
    // Odoo compares the version as it compares a folder, so the `S.x` that 20.0 keeps unprefixed
    // takes the series a second time.
    let current = version_key(&if adapted.matches('.').count() > 2 {
        adapted
    } else {
        format!("{own}.{adapted}")
    });
    let whole = name.matches('.').count() > 2;
    let folder = version_key(&if whole {
        name.to_owned()
    } else {
        format!("{own}.{name}")
    });
    let above = if whole {
        folder > current
    } else {
        folder.get(2..).unwrap_or_default() > current.get(2..).unwrap_or_default()
    };
    above.then(|| format!("folder '{name}' is above the module version {version}, so Odoo never runs its scripts"))
}

/// The order Odoo's `parse_version` gives a version of numbers and `saas~` (18.0
/// `odoo/tools/parse_version.py:12-64`): the numbers, each one up, without trailing zeros, then a
/// 0 for the end, which comes before any number.
fn version_key(version: &str) -> Vec<u64> {
    let mut key: Vec<u64> = version
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<u64>().unwrap_or(u64::MAX - 1) + 1)
        .collect();
    while key.last() == Some(&1) {
        key.pop();
    }
    key.push(0);
    key
}

const NO_LICENSE: &str = "the manifest names no license, which Odoo reads as LGPL-3";

/// Whether `ir.module.module` of `series` takes `name` for its license; one of unknown series may
/// run on any series from 10.0.
fn known_license(name: &str, series: Option<Series>) -> bool {
    LICENSES.contains(&name) && (name != "OPL-1" || series.is_none_or(|s| s >= OPL))
}

/// OD512 on each entry of `files` naming a file an earlier entry names, which Odoo loads again
/// (18.0 `odoo/modules/loading.py:41-53`).
fn repeated(files: &[Spanned<String>], key: &str) -> Vec<RawDiag> {
    let mut seen = FxHashSet::default();
    files
        .iter()
        .filter(|f| !seen.insert(normalize(&f.value)))
        .map(|f| {
            let message = format!("'{}' is listed again under {key}, so Odoo loads it twice", f.value);
            RawDiag::new("OD512", f.pos, message)
        })
        .collect()
}

/// Positions in the manifest as written, of the nodes parsed from it without its leading blanks.
struct Source<'a> {
    loc: Locator<'a>,
    blanks: TextSize,
}

impl Source<'_> {
    fn pos(&self, offset: TextSize) -> Pos {
        self.loc.pos(offset + self.blanks)
    }

    fn at(&self, node: &impl Ranged) -> Pos {
        self.pos(node.start())
    }
}

/// Why odooscan reads no module from a manifest, in the series it read the manifest as.
#[derive(Debug)]
pub struct Refusal {
    pub error: Spanned<String>,
    pub series: Option<Series>,
    pub odoo: OdooReads,
}

/// Whether the Odoo of the series reads a manifest odooscan refuses.
#[derive(Debug, PartialEq, Eq)]
pub enum OdooReads {
    No,
    /// It parses only as the Python 2 the series runs.
    AsPython2,
    /// It is an expression but no dict literal, which 9.0 evaluates.
    AsExpression,
}

/// The manifest in `bytes`, or why the Odoo of `series` cannot read it. `series` is handed the
/// manifest's own series, none when that cannot be read. From 11.0 Odoo reads the file as UTF-8
/// with `ast.literal_eval` (18.0 `odoo/modules/module.py:312-313`, `odoo/tools/misc.py:257`). 10.0
/// hands `literal_eval` the bytes, which Python 2 takes in any encoding
/// (`odoo/modules/module.py:330-332`), and 9.0 hands them to `eval`
/// (`openerp/modules/module.py:263-265`), which takes any expression.
pub fn read(bytes: &[u8], series: impl FnOnce(Option<Series>) -> Option<Series>) -> Result<Manifest, Refusal> {
    let (src, not_utf8) = decode(bytes);
    let body = src.trim_start_matches([' ', '\t']);
    let loc = Source {
        loc: Locator::new(&src),
        blanks: TextSize::of(&src[..src.len() - body.len()]),
    };
    let parsed = parse_module(body);
    // The flag tells a syntax error, which may be Python 2 the series runs, from the others.
    let top = match &parsed {
        Ok(p) => top_expression(&loc, &p.syntax().body).map_err(|e| (e, false)),
        Err(e) => Err((error(loc.pos(e.location.start()), e.error.to_string()), true)),
    };
    let dict = top.as_ref().ok().and_then(|top| top.as_dict_expr());
    let mut manifest = dict.map(|d| entries(&loc, d));
    let series = series(manifest.as_ref().and_then(Manifest::series));
    if let (Some(m), Some(d)) = (manifest.as_mut(), dict) {
        m.diags = m.findings(loc.at(d), series);
    }
    let refuse = |(error, odoo)| Refusal { error, series, odoo };
    if let Some(pos) = not_utf8.filter(|_| !python2::runs_on(series)) {
        return Err(refuse((error(pos, "the manifest is not UTF-8"), OdooReads::No)));
    }
    let top = top.map_err(|(e, syntax)| {
        let python2 = syntax && python2::runs_on(series) && python2::remaining_error(body, series).is_none();
        refuse((e, if python2 { OdooReads::AsPython2 } else { OdooReads::No }))
    })?;
    literal_dict(&loc, top, manifest, series).map_err(refuse)
}

/// `bytes` as text, each byte one character when they are not UTF-8, with the position of the
/// first byte that is not.
fn decode(bytes: &[u8]) -> (Cow<'_, str>, Option<Pos>) {
    match decode::invalid_utf8(bytes) {
        None => (String::from_utf8_lossy(bytes), None),
        Some(pos) => (Cow::Owned(bytes.iter().copied().map(char::from).collect()), Some(pos)),
    }
}

/// `manifest`, read from `top`, when the Odoo of `series` reads `top` as a dict of literals.
fn literal_dict(
    loc: &Source,
    top: &Expr,
    manifest: Option<Manifest>,
    series: Option<Series>,
) -> Result<Manifest, (Spanned<String>, OdooReads)> {
    let literal = series.is_none_or(|s| s >= LITERAL_EVAL);
    let (Some(dict), Some(manifest)) = (top.as_dict_expr(), manifest) else {
        let odoo = if literal {
            OdooReads::No
        } else {
            OdooReads::AsExpression
        };
        return Err((error(loc.at(top), "not a dict literal"), odoo));
    };
    match unliteral_items(dict).filter(|_| literal) {
        Some(node) => Err((
            error(
                loc.at(node),
                "not a literal: Odoo reads the manifest with ast.literal_eval",
            ),
            OdooReads::No,
        )),
        None => Ok(manifest),
    }
}

/// The manifest `src` holds as a series from 11.0 reads it, or the first place it refuses: one
/// expression of literals, the spaces and tabs ahead of the first line stripped.
#[cfg(test)]
pub fn parse(src: &str) -> Result<Manifest, Spanned<String>> {
    read(src.as_bytes(), |_| None).map_err(|r| r.error)
}

/// The manifest of the entries of `dict` it reads, skipping those that are no literal.
fn entries(loc: &Source, dict: &ast::ExprDict) -> Manifest {
    let mut m = Manifest {
        installable: true,
        ..Manifest::default()
    };
    for item in &dict.items {
        let Some((key, at)) = item.key.as_ref().and_then(|k| Some((str_lit(k)?, loc.at(k)))) else {
            continue;
        };
        m.set(loc, key, at, &item.value);
    }
    m
}

fn error(pos: Pos, message: impl Into<String>) -> Spanned<String> {
    Spanned {
        value: message.into(),
        pos,
    }
}

/// The expression of a body that is that one expression.
fn top_expression<'a>(loc: &Source, body: &'a [Stmt]) -> Result<&'a Expr, Spanned<String>> {
    let Some(first) = body.first() else {
        return Err(error(Pos { line: 1, col: 1 }, "the manifest is empty"));
    };
    if let Some(second) = body.get(1) {
        return Err(error(
            loc.at(second),
            "a second statement: the manifest is one dict literal",
        ));
    }
    let Stmt::Expr(e) = first else {
        return Err(error(loc.at(first), "a statement: the manifest is one dict literal"));
    };
    Ok(&e.value)
}

/// The first key or value of `dict` that `ast.literal_eval` refuses, a `**` entry at its value.
fn unliteral_items(dict: &ast::ExprDict) -> Option<&Expr> {
    dict.items.iter().find_map(|item| match &item.key {
        Some(key) => unliteral(key).or_else(|| unliteral(&item.value)),
        None => Some(&item.value),
    })
}

/// The first node of `expr` that `ast.literal_eval` refuses, `expr` itself when it is no literal.
fn unliteral(expr: &Expr) -> Option<&Expr> {
    if leaf(expr) {
        return None;
    }
    if let Expr::Dict(dict) = expr {
        return unliteral_items(dict);
    }
    match module_state::items(expr) {
        Some(items) => items.into_iter().flatten().find_map(unliteral),
        None => Some(expr),
    }
}

/// A literal that holds no other.
fn leaf(expr: &Expr) -> bool {
    module_state::scalar(expr) || expr.is_ellipsis_literal_expr() || signed(expr) || complex(expr) || empty_set(expr)
}

/// A number with a unary `+` or `-`.
fn signed(expr: &Expr) -> bool {
    matches!(expr, Expr::UnaryOp(u) if matches!(u.op, UnaryOp::UAdd | UnaryOp::USub) && u.operand.is_number_literal_expr())
}

/// `real + imaginary` or `real - imaginary`, the real part an int or a float, maybe signed.
fn complex(expr: &Expr) -> bool {
    let Expr::BinOp(b) = expr else {
        return false;
    };
    let real = |e: &Expr| matches!(number(e), Some(Number::Int(_) | Number::Float(_)));
    let left = match &*b.left {
        Expr::UnaryOp(u) if signed(&b.left) => real(&u.operand),
        e => real(e),
    };
    matches!(b.op, Operator::Add | Operator::Sub) && left && matches!(number(&b.right), Some(Number::Complex { .. }))
}

fn number(expr: &Expr) -> Option<&Number> {
    expr.as_number_literal_expr().map(|n| &n.value)
}

/// `set()`, the only call `ast.literal_eval` takes.
fn empty_set(expr: &Expr) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    call.arguments.is_empty() && call.func.as_name_expr().is_some_and(|n| n.id.as_str() == "set")
}

/// `rel`, relative to the module, in the one form the manifest's entries and the files on disk
/// compare in: without a leading `./`, with `/` separators.
pub fn normalize(rel: &str) -> String {
    rel.trim_start_matches("./").replace('\\', "/")
}

fn names(loc: &Source, e: &Expr) -> Vec<String> {
    list(loc, e).into_iter().map(|f| f.value).collect()
}

fn list(loc: &Source, e: &Expr) -> Vec<Spanned<String>> {
    let elts = match e {
        Expr::List(l) => &l.elts[..],
        Expr::Tuple(t) => &t.elts[..],
        Expr::Set(s) => &s.elts[..],
        _ => &[],
    };
    elts.iter()
        .filter_map(|e| {
            Some(Spanned {
                value: str_lit(e)?.to_owned(),
                pos: loc.at(e),
            })
        })
        .collect()
}

/// The path of each entry of a bundle's list but an `include`, which names a bundle: a plain entry,
/// or the last item of an `(op, ..., path)` one. A URL is left out.
fn asset_paths(loc: &Source, entries: &[Expr]) -> Vec<Spanned<String>> {
    entries
        .iter()
        .filter_map(|entry| {
            let path = match entry {
                Expr::Tuple(t) if t.elts.len() >= 2 => {
                    if t.elts.first().and_then(str_lit)? == "include" {
                        return None;
                    }
                    t.elts.last()?
                }
                e => e,
            };
            let value = str_lit(path).filter(|p| !p.contains("://"))?;
            Some(Spanned {
                value: value.to_owned(),
                pos: loc.at(path),
            })
        })
        .collect()
}

/// The globs of a bundle's list, whether a plain entry or the last item of an `(op, ..., glob)` one.
/// `include` names a bundle and `remove` takes files out, so neither adds anything.
fn asset_globs(entries: &[Expr]) -> Vec<String> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            Expr::Tuple(t) => {
                let op = t.elts.first().and_then(str_lit)?;
                (!matches!(op, "include" | "remove")).then(|| t.elts.last().and_then(str_lit))?
            }
            e => str_lit(e),
        })
        .filter(|glob| !glob.contains("://"))
        .map(|glob| glob.trim_start_matches('/').to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_globs_bundles_add() {
        let m = parse(
            r#"{"assets": {"web.assets_backend": [
                "x/static/src/**/*",
                ("include", "web._assets_core"),
                ("remove", "x/static/src/old.js"),
                ("after", "web/static/src/a.js", "/x/static/src/b.js"),
                "https://cdn.example.com/lib.js",
            ]}}"#,
        )
        .unwrap();
        assert_eq!(m.assets, ["x/static/src/**/*", "x/static/src/b.js"]);
    }

    #[test]
    fn reads_the_bundles_and_the_qweb_key() {
        let m = parse(
            r#"{"qweb": ["static/src/xml/a.xml"],
                "assets": {"web.assets_qweb": ["x/static/src/xml/*.xml"], "web.assets_tests": []}}"#,
        )
        .unwrap();
        let qweb = m.qweb.unwrap();
        assert_eq!(
            (qweb.value, qweb.pos),
            (vec!["static/src/xml/a.xml".to_owned()], Pos { line: 1, col: 2 })
        );
        let bundles: Vec<_> = m
            .bundles
            .iter()
            .map(|b| (b.name.value.as_str(), b.name.pos, &b.globs[..]))
            .collect();
        assert_eq!(
            bundles,
            [(
                "web.assets_qweb",
                Pos { line: 2, col: 28 },
                &["x/static/src/xml/*.xml".to_owned()][..]
            )]
        );
        assert_eq!(m.unread, ["static/src/xml/a.xml"]);
        assert!(parse(r#"{"qweb": []}"#).unwrap().qweb.is_none());
        assert!(parse("{}").unwrap().qweb.is_none());
    }

    #[test]
    fn reads_auto_install_forms() {
        let read = |src: &str| parse(src).unwrap().auto_install;
        let web = |col| {
            AutoInstall::Listed(vec![Spanned {
                value: "web".to_owned(),
                pos: Pos { line: 1, col },
            }])
        };
        assert_eq!(read(r#"{"auto_install": True}"#), AutoInstall::Depends);
        assert_eq!(read(r#"{"auto_install": ["web"]}"#), web(19));
        assert_eq!(read(r#"{"auto_install": []}"#), AutoInstall::Listed(vec![]));
        assert_eq!(read(r#"{"auto_install": {"web"}}"#), web(19));
        assert_eq!(read(r#"{"auto_install": ["web", 1]}"#), AutoInstall::Off);
        assert_eq!(read(r#"{"auto_install": False}"#), AutoInstall::Off);
        assert_eq!(read(r#"{"name": "x"}"#), AutoInstall::Off);
    }

    /// The findings of the manifest `src` read as `series`, as `(code, line, col)`.
    fn findings(src: &str, series: Option<&str>) -> Vec<(&'static str, u32, u32)> {
        let m = read(src.as_bytes(), |_| series.and_then(Series::parse)).unwrap();
        m.diags.iter().map(|d| (d.code, d.pos.line, d.pos.col)).collect()
    }

    const LICENSED: &str = r#""license": "LGPL-3""#;

    #[test]
    fn reports_a_missing_or_empty_license() {
        assert_eq!(findings(r#"{"name": "x"}"#, None), [("OD507", 1, 1)]);
        assert_eq!(findings("\n{'name': 'x'}", None), [("OD507", 2, 1)]);
        assert_eq!(findings("  {'name': 'x'}", None), [("OD507", 1, 3)]);
        for value in ["''", "False", "None"] {
            assert_eq!(
                findings(&format!("{{'license': {value}}}"), None),
                [("OD507", 1, 13)],
                "{value}"
            );
        }
        assert!(findings(&format!("{{{LICENSED}}}"), None).is_empty());
    }

    #[test]
    fn reports_a_license_the_series_does_not_know() {
        assert_eq!(findings(r#"{"license": "MIT"}"#, None), [("OD508", 1, 13)]);
        assert_eq!(findings(r#"{"license": "lgpl-3"}"#, None), [("OD508", 1, 13)]);
        for license in LICENSES {
            assert!(findings(&format!("{{'license': '{license}'}}"), Some("18.0")).is_empty());
        }
        assert!(findings(r#"{"license": "OPL-1"}"#, None).is_empty());
        assert!(findings(r#"{"license": "OPL-1"}"#, Some("10.0")).is_empty());
        assert_eq!(findings(r#"{"license": "OPL-1"}"#, Some("9.0")), [("OD508", 1, 13)]);
        assert!(findings(r#"{"license": ["LGPL-3"]}"#, None).is_empty());
    }

    #[test]
    fn reports_a_trigger_outside_depends() {
        let src = |rest: &str| format!("{{{LICENSED}, {rest}}}");
        let outside = src(r#""depends": ["sale"], "auto_install": ["sale", "stock"]"#);
        assert_eq!(findings(&outside, None), [("OD511", 1, 69)]);
        assert!(findings(&src(r#""depends": ["sale"], "auto_install": ["sale"]"#), None).is_empty());
        assert!(findings(&src(r#""depends": ["sale"], "auto_install": True"#), None).is_empty());
        assert!(findings(&src(r#""depends": [], "auto_install": ["sale"]"#), None).is_empty());
        assert!(findings(&src(r#""auto_install": ["sale"]"#), None).is_empty());
    }

    #[test]
    fn reports_a_file_listed_again_at_the_later_entry() {
        let src = format!(
            r#"{{{LICENSED},
                "data": ["views/a.xml", "data/b.xml", "./views/a.xml", "views/a.xml"],
                "demo": ["views/a.xml", "demo/c.xml"]}}"#
        );
        assert_eq!(findings(&src, None), [("OD512", 2, 55), ("OD512", 2, 72)]);
    }

    #[test]
    fn refuses_versions_from_saas_16_3() {
        let refused =
            |version: &str, series: Option<&str>| version_refusal(version, series.and_then(Series::parse)).is_some();
        for version in ["1.0", "1.2.3", "18.0", "18.0.1", "18.0.1.0", "18.0.1.0.0"] {
            assert!(!refused(version, Some("18.0")), "{version}");
        }
        for version in [
            "1",
            "1.0-beta",
            "1.0.0rc1",
            "1..0",
            "1.2.3.4",
            "17.0.1.0.0",
            "18.0.1.2.3.4",
            "18.0.x",
        ] {
            assert!(refused(version, Some("18.0")), "{version}");
        }
        assert!(!refused("20.0.1.0.0", Some("20.0")) && refused("18.0.1.0.0", Some("20.0")));
        assert!(!refused("saas~18.1.1.0.0", Some("saas~18.1")) && !refused("18.1.1.0.0", Some("saas~18.1")));
        assert!(refused("18.0.1.0.0", Some("saas~18.1")));
        assert!(!refused("1.0-beta", Some("saas~16.2")) && !refused("17.0.1.0.0", Some("16.0")));
        assert!(refused("1.0-beta", Some("saas~16.3")));
        assert!(refused("1.0-beta", None) && refused("1", None) && refused("1.2.3.4.5.6", None));
        assert!(!refused("1.2.3.4", None) && !refused("saas~18.1.1.0.0", None));
    }

    #[test]
    fn names_the_series_a_refused_version_must_start_with() {
        let message = |version: &str, series: &str| version_refusal(version, Series::parse(series)).unwrap();
        assert_eq!(
            message("17.0.1.0.0", "18.0"),
            "version '17.0.1.0.0' is not of the form x.y, x.y.z, 18.0.x.y or 18.0.x.y.z, so Odoo 18.0 will not install the module"
        );
        assert!(message("1", "saas~18.1").contains("saas~18.1.x.y"));
    }

    #[test]
    fn skips_an_upgrade_folder_of_another_form_or_above_the_module_version() {
        let eighteen = Series::parse("18.0").unwrap();
        let refused = |name: &str, version: &str| upgrade_folder_refusal(name, version, eighteen).is_some();
        for name in [
            "1.2.0",
            "1.1",
            "1.2",
            "17.0.1.5",
            "18.0.1.2",
            "9.0.9.0",
            "0.0.0",
            "6.1.1.0",
            "8.saas~6.1.0",
        ] {
            assert!(!refused(name, "18.0.1.2.0"), "{name}");
        }
        for name in [
            "18.0.1.3.0",
            "1.3",
            "1.2.1",
            "13.0.2",
            "19.0.1.0",
            "saas~19.5.2.2",
            "5.0.1.0",
            "v1.1",
            "17.2.1.0",
            "1",
            "1.2.3.4",
            "1.1.0.0",
            "saas~10.1.1.0",
            "saas~100.1.1.0",
            "saas~18.0.1.0",
        ] {
            assert!(refused(name, "18.0.1.2.0"), "{name}");
        }
        // A version without the series takes it, and `18.0` alone is the module version 18.0.
        assert!(!refused("1.2", "1.2.0") && refused("1.3", "1.2"));
        assert!(!refused("18.0", "18.0") && refused("18.1", "18.0"));
        // Trailing zeros do not count, on either side.
        assert!(!refused("1.2.0", "18.0.1.2") && !refused("1.2.0", "1.2") && !refused("18.0.1.2.0", "1.2"));
        // 20.0 keeps `S.x` unprefixed, and Odoo then compares it behind the series once more.
        let twenty = Series::parse("20.0").unwrap();
        let twenty_refused = |name: &str, version: &str| upgrade_folder_refusal(name, version, twenty).is_some();
        assert!(!twenty_refused("5.0", "20.0.3") && !twenty_refused("20.0.4.0", "20.0.3"));
        assert!(twenty_refused("20.0.21.0.0", "20.0.3") && !twenty_refused("1.2", "20.0.1.2"));
        let saas = Series::parse("saas~18.2").unwrap();
        let saas_refused = |name: &str, version: &str| upgrade_folder_refusal(name, version, saas).is_some();
        assert!(!saas_refused("1.0", "saas~18.2.1.0") && !saas_refused("saas~18.2.1.0", "1.0"));
        assert!(saas_refused("1.1", "saas~18.2.1.0") && saas_refused("18.2.1.1", "18.2.1.0"));
        assert!(!saas_refused("18.0.5.0", "1.0") && saas_refused("saas~18.3.1.0", "1.0"));
    }

    #[test]
    fn names_why_odoo_skips_an_upgrade_folder() {
        let eighteen = Series::parse("18.0").unwrap();
        assert_eq!(
            upgrade_folder_refusal("v1.1", "18.0.1.2.0", eighteen).as_deref(),
            Some(
                "folder 'v1.1' is not a version of two or three numbers, maybe behind a series, so Odoo skips its scripts"
            )
        );
        assert_eq!(
            upgrade_folder_refusal("1.3", "18.0.1.2.0", eighteen).as_deref(),
            Some("folder '1.3' is above the module version 18.0.1.2.0, so Odoo never runs its scripts")
        );
    }

    #[test]
    fn reads_the_path_of_every_entry_but_an_include() {
        let m = parse(
            r#"{"assets": {"web.assets_backend": [
                "x/static/src/**/*",
                ("include", "web._assets_core"),
                ("remove", "x/static/src/old.js"),
                ("after", "web/static/src/a.js", "/x/static/src/b.js"),
                "https://cdn.example.com/lib.js",
            ]}}"#,
        )
        .unwrap();
        let paths: Vec<_> = m.bundles[0]
            .paths
            .iter()
            .map(|p| (p.value.as_str(), p.pos.line))
            .collect();
        assert_eq!(
            paths,
            [
                ("x/static/src/**/*", 2),
                ("x/static/src/old.js", 4),
                ("/x/static/src/b.js", 5)
            ]
        );
    }

    #[test]
    fn lists_the_files_a_series_loads_in_load_order() {
        let m = parse(
            r#"{"data": ["d.xml"], "init_xml": ["i.xml"], "update_xml": ["u.xml"],
                "demo": ["e.xml"], "demo_xml": ["x.xml"]}"#,
        )
        .unwrap();
        let files = |series: Option<&str>| m.loaded_files(series.and_then(Series::parse)).collect::<Vec<_>>();
        assert_eq!(files(Some("14.0")), ["i.xml", "u.xml", "d.xml", "x.xml", "e.xml"]);
        assert_eq!(files(Some("saas~15.1")), ["i.xml", "u.xml", "d.xml", "x.xml", "e.xml"]);
        assert_eq!(files(Some("saas~15.2")), ["d.xml", "e.xml"]);
        assert_eq!(files(Some("17.0")), ["d.xml", "e.xml"]);
        assert_eq!(files(Some("19.0")), ["i.xml", "d.xml", "e.xml"]);
        assert_eq!(files(Some("20.0")), ["d.xml", "e.xml"]);
        assert_eq!(files(None), ["i.xml", "u.xml", "d.xml", "x.xml", "e.xml"]);
    }

    #[test]
    fn lists_the_sql_files_of_the_data_keys_a_series_reads() {
        let m = parse(
            r#"{"data": ["a.xml", "a.sql"], "init_xml": ["b.sql"], "update_xml": ["c.sql", "c.csv"],
                "demo": ["e.sql"]}"#,
        )
        .unwrap();
        let files = |series: Option<&str>| m.sql_files(series.and_then(Series::parse)).collect::<Vec<_>>();
        assert_eq!(files(Some("14.0")), ["b.sql", "c.sql", "a.sql"]);
        assert_eq!(files(Some("saas~15.1")), ["b.sql", "c.sql", "a.sql"]);
        assert_eq!(files(Some("saas~15.2")), ["a.sql"]);
        assert_eq!(files(Some("17.0")), ["a.sql"]);
        assert_eq!(files(Some("19.0")), ["b.sql", "a.sql"]);
        assert_eq!(files(Some("20.0")), ["a.sql"]);
        assert_eq!(files(None), ["b.sql", "c.sql", "a.sql"]);
    }

    #[test]
    fn names_the_ignored_key_listing_a_file() {
        let m = parse(
            r#"{"data": ["views/both.xml"], "init_xml": ["./views/init.xml", "views/both.xml"],
                "update_xml": ["views/update.xml"], "demo_xml": ["demo/demo.xml"]}"#,
        )
        .unwrap();
        let key = |file: &str, series: Option<&str>| m.ignored_key(file, series.and_then(Series::parse));
        assert_eq!(key("views/init.xml", Some("17.0")), Some("init_xml"));
        assert_eq!(key("views/init.xml", Some("19.0")), None);
        assert_eq!(key("views/init.xml", Some("20.0")), Some("init_xml"));
        assert_eq!(key("views/update.xml", Some("19.0")), Some("update_xml"));
        assert_eq!(key("demo/demo.xml", Some("saas~15.2")), Some("demo_xml"));
        assert_eq!(key("views/both.xml", Some("17.0")), None);
        assert_eq!(key("views/update.xml", Some("14.0")), None);
        assert_eq!(key("views/update.xml", None), None);
        assert_eq!(key("views/other.xml", Some("17.0")), None);
        assert_eq!(m.listed("other_files").count(), 0);
    }

    #[test]
    fn reads_a_set_as_odoo_iterates_it() {
        let m = parse(r#"{"data": {"views/a.xml"}, "demo": {"data/b.xml"}}"#).unwrap();
        let files: Vec<_> = m.loaded_files(Series::parse("16.0")).collect();
        assert_eq!(files, ["views/a.xml", "data/b.xml"]);
    }

    #[test]
    fn reads_countries() {
        let countries = |src: &str| parse(src).unwrap().countries;
        assert!(countries(r#"{"countries": ["be"]}"#));
        assert!(!countries(r#"{"countries": []}"#));
        assert!(!countries(r#"{"countries": True}"#));
        assert!(!countries("{}"));
    }

    /// Where `parse` refuses `src`, as `(line, col)`.
    fn refused(src: &str) -> (u32, u32) {
        let error = parse(src).unwrap_err();
        (error.pos.line, error.pos.col)
    }

    #[test]
    fn reads_the_literals_literal_eval_takes() {
        for src in [
            " {}",
            "\t{}",
            "({})",
            "# -*- coding: utf-8 -*-\n{\n    'name': 'x'  # a comment\n    ' y',\n}\n",
            "{'a': set(), 'b': -1, 'c': +1.5, 'd': 1+2j, 'e': -1-2j, 'f': ..., 'g': None, 'h': b'x'}",
            "{'a': [1, (2,), {3}, {'b': [True]}], 4: 'c',}",
        ] {
            assert!(parse(src).is_ok(), "{src:?}");
        }
    }

    #[test]
    fn refuses_a_file_that_is_not_one_expression() {
        assert_eq!(refused(""), (1, 1));
        assert_eq!(refused("# only a comment\n"), (1, 1));
        assert_eq!(refused("\"doc\"\n{}"), (2, 1));
        assert_eq!(refused("x = {}"), (1, 1));
        assert_eq!(refused("\n  {}").0, 2);
        assert_eq!(refused("{'name': 'x',\n").0, 2);
    }

    #[test]
    fn refuses_a_top_expression_that_is_not_a_dict() {
        assert_eq!(refused("[('name', 'x')]"), (1, 1));
        assert_eq!(refused("dict(name='x')"), (1, 1));
    }

    #[test]
    fn refuses_a_key_or_value_that_is_not_a_literal() {
        assert_eq!(refused("  {'a': OTHER}"), (1, 9));
        assert_eq!(refused("{'a': 1, **{}}"), (1, 12));
        assert_eq!(refused("{'a': f''}"), (1, 7));
        assert_eq!(refused("{'a': 'x' f'y'}"), (1, 7));
        assert_eq!(refused("{('a', b): 1}"), (1, 8));
        assert_eq!(refused("{'a': [1, *b]}"), (1, 11));
        assert_eq!(refused("{'a': set([1])}"), (1, 7));
        assert_eq!(refused("{'a': 1 + 2}"), (1, 7));
        assert_eq!(refused("{'a': 1j + 2}"), (1, 7));
        assert_eq!(refused("{'a': -True}"), (1, 7));
        assert_eq!(refused("{'a': - -1}"), (1, 7));
    }

    #[test]
    fn refuses_a_file_that_is_not_utf_8() {
        let error = parse_bytes(b"{'name': 'r\xe9sum\xe9'}", "11.0").unwrap_err();
        assert_eq!(error.pos, Pos { line: 1, col: 12 });
        assert!(parse_bytes("{'name': 'r\u{e9}sum\u{e9}'}".as_bytes(), "11.0").is_ok());
    }

    /// The manifest in `bytes` as `series` reads it, the error alone.
    fn parse_bytes(bytes: &[u8], series: &str) -> Result<Manifest, Spanned<String>> {
        read(bytes, |_| Series::parse(series)).map_err(|r| r.error)
    }

    /// How the Odoo of `series` reads `src`, none when odooscan reads it too.
    fn odoo_reads(src: &[u8], series: &str) -> Option<OdooReads> {
        read(src, |_| Series::parse(series)).err().map(|r| r.odoo)
    }

    #[test]
    fn python_2_reads_a_manifest_in_any_encoding() {
        let latin1 = b"{'category': 'r\xe9sum\xe9'}";
        let category = parse_bytes(latin1, "10.0").unwrap().category;
        assert_eq!(category.as_deref(), Some("r\u{e9}sum\u{e9}"));
        assert!(parse_bytes(latin1, "9.0").is_ok());
        assert!(parse_bytes(latin1, "11.0").is_err());
        assert!(read(b"{'version': '10.0.1.0.0', 'category': 'r\xe9'}", |own| own).is_ok());
        let error = read(b"{'version': '11.0.1.0.0', 'category': 'r\xe9'}", |own| own).unwrap_err();
        assert_eq!(error.error.pos, Pos { line: 1, col: 41 });
    }

    #[test]
    fn nine_evaluates_the_manifest() {
        assert!(parse_bytes(b"{'version': '1.' + '0', 'name': NAME}", "9.0").is_ok());
        assert_eq!(odoo_reads(b"{'name': NAME}", "10.0"), Some(OdooReads::No));
        assert_eq!(odoo_reads(b"dict(name='x')", "9.0"), Some(OdooReads::AsExpression));
        assert_eq!(odoo_reads(b"dict(name='x')", "10.0"), Some(OdooReads::No));
        assert_eq!(odoo_reads(b"'doc'\n{}", "9.0"), Some(OdooReads::No));
        assert_eq!(odoo_reads(b"{'name': 'x',", "9.0"), Some(OdooReads::No));
        assert_eq!(odoo_reads(b"{'sequence': 0755}", "9.0"), Some(OdooReads::AsPython2));
        assert_eq!(odoo_reads(b"{'sequence': 0755}", "11.0"), Some(OdooReads::No));
    }

    #[test]
    fn keeps_the_entries_of_keys_nothing_loads_from() {
        let m = parse(
            r#"{"name": "x.xml", "data": ["d.xml"], "update_xml": ["u.xml"], "demo": ["e.xml"],
                "other_files": ["./data/x.xml"], "test": ("test/y.xml",)}"#,
        )
        .unwrap();
        assert_eq!(m.unread, ["data/x.xml", "test/y.xml"]);
    }
}
