use std::borrow::Cow;

use ruff_python_ast::{self as ast, Expr, Number, Operator, Stmt, UnaryOp};
use ruff_python_parser::parse_module;
use ruff_text_size::{Ranged, TextSize};

use crate::facts::{Pos, Spanned};
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
    Listed(Vec<String>),
}

impl AutoInstall {
    /// A collection with an entry that is not a string literal names unknown triggers, so it reads as `Off`.
    fn read(value: &Expr) -> Self {
        let entries = match value {
            Expr::BooleanLiteral(b) if b.value => return Self::Depends,
            Expr::List(l) => &l.elts[..],
            Expr::Tuple(t) => &t.elts[..],
            Expr::Set(s) => &s.elts[..],
            _ => return Self::Off,
        };
        let names: Option<Vec<String>> = entries.iter().map(|e| str_lit(e).map(str::to_owned)).collect();
        names.map_or(Self::Off, Self::Listed)
    }
}

/// A bundle of the manifest's `assets`, at its key.
#[derive(Debug)]
pub struct Bundle {
    pub name: Spanned<String>,
    /// What `asset_globs` reads from its list.
    pub globs: Vec<String>,
}

/// The manifest keys odooscan reads, evaluated as literals; the file is never executed.
#[derive(Debug, Default)]
pub struct Manifest {
    pub version: Option<String>,
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
}

impl Manifest {
    pub fn series(&self) -> Option<Series> {
        self.version.as_deref().and_then(Series::from_module_version)
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
            "version" | "category" | "installable" => self.set_scalar(key, value),
            "depends" | "data" | "demo" | "init_xml" | "update_xml" | "demo_xml" => self.set_files(loc, key, value),
            "assets" => self.set_assets(loc, value),
            "qweb" => self.set_qweb(loc, at, value),
            "auto_install" | "countries" => self.set_install(loc, key, value),
            _ => self.unread.extend(names(loc, value).iter().map(|f| normalize(f))),
        }
    }

    fn set_scalar(&mut self, key: &str, value: &Expr) {
        match key {
            "version" => self.version = str_lit(value).map(str::to_owned),
            "category" => self.category = str_lit(value).map(str::to_owned),
            "installable" => self.installable = !matches!(value, Expr::BooleanLiteral(b) if !b.value),
            _ => {}
        }
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
                self.bundles.push(Bundle { name, globs });
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
            "auto_install" => self.auto_install = AutoInstall::read(value),
            "countries" => self.countries = !list(loc, value).is_empty(),
            _ => {}
        }
    }
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
    let manifest = dict.map(|d| entries(&loc, d));
    let series = series(manifest.as_ref().and_then(Manifest::series));
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
    match std::str::from_utf8(bytes) {
        Ok(src) => (Cow::Borrowed(src), None),
        Err(e) => {
            let valid = std::str::from_utf8(&bytes[..e.valid_up_to()]).unwrap_or_default();
            let pos = Locator::new(valid).pos(TextSize::of(valid));
            (Cow::Owned(bytes.iter().copied().map(char::from).collect()), Some(pos))
        }
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
        assert_eq!(read(r#"{"auto_install": True}"#), AutoInstall::Depends);
        assert_eq!(
            read(r#"{"auto_install": ["web"]}"#),
            AutoInstall::Listed(vec!["web".to_owned()])
        );
        assert_eq!(read(r#"{"auto_install": []}"#), AutoInstall::Listed(vec![]));
        assert_eq!(
            read(r#"{"auto_install": {"web"}}"#),
            AutoInstall::Listed(vec!["web".to_owned()])
        );
        assert_eq!(read(r#"{"auto_install": ["web", 1]}"#), AutoInstall::Off);
        assert_eq!(read(r#"{"auto_install": False}"#), AutoInstall::Off);
        assert_eq!(read(r#"{"name": "x"}"#), AutoInstall::Off);
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
