use ruff_python_ast::{Expr, Stmt};
use ruff_python_parser::parse_module;

use crate::facts::Spanned;
use crate::python::{Locator, str_lit};
use crate::series::Series;

/// The first series that loads a module's data from `data` alone (`saas~15.2`).
const DATA_ONLY: Series = Series { major: 15, minor: 2 };

/// The first series that loads the files of `init_xml` with those of `data` again (`19.0`).
const INIT_XML_BACK: Series = Series { major: 19, minor: 0 };

/// The first series that loads the files of `data` alone again (`20.0`).
const INIT_XML_GONE: Series = Series { major: 20, minor: 0 };

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

/// The manifest keys odooscan reads, evaluated as literals; the file is never executed.
#[derive(Debug, Default)]
pub struct Manifest {
    pub version: Option<String>,
    pub depends: Vec<Spanned<String>>,
    pub data: Vec<Spanned<String>>,
    pub init_xml: Vec<String>,
    pub update_xml: Vec<String>,
    pub demo: Vec<Spanned<String>>,
    pub category: Option<String>,
    pub installable: bool,
    pub auto_install: AutoInstall,
    /// Globs the `assets` bundles add, relative to the addons directory.
    pub assets: Vec<String>,
}

impl Manifest {
    pub fn series(&self) -> Option<Series> {
        self.version.as_deref().and_then(Series::from_module_version)
    }

    /// The SQL files Odoo runs when it loads the data of the module in `series`: those of `data`,
    /// and of the legacy keys the series still reads. An unknown series reads every key.
    pub fn sql_files(&self, series: Option<Series>) -> impl Iterator<Item = &str> {
        let (init_xml, update_xml) = match series {
            Some(s) if s >= DATA_ONLY => ((INIT_XML_BACK..INIT_XML_GONE).contains(&s), false),
            _ => (true, true),
        };
        let data = self.data.iter().map(|f| f.value.as_str());
        let init_xml = self.init_xml.iter().filter(move |_| init_xml).map(String::as_str);
        let update_xml = self.update_xml.iter().filter(move |_| update_xml).map(String::as_str);
        data.chain(init_xml).chain(update_xml).filter(|f| f.ends_with(".sql"))
    }

    fn set(&mut self, loc: &Locator, key: &str, value: &Expr) {
        match key {
            "version" => self.version = str_lit(value).map(str::to_owned),
            "depends" => self.depends = list(loc, value),
            "data" => self.data = list(loc, value),
            "init_xml" | "update_xml" => self.set_legacy(key, list(loc, value).into_iter().map(|f| f.value).collect()),
            "demo" => self.demo = list(loc, value),
            "category" => self.category = str_lit(value).map(str::to_owned),
            "installable" => self.installable = !matches!(value, Expr::BooleanLiteral(b) if !b.value),
            "assets" => self.assets = asset_globs(value),
            "auto_install" => self.auto_install = AutoInstall::read(value),
            _ => {}
        }
    }

    fn set_legacy(&mut self, key: &str, files: Vec<String>) {
        if key == "init_xml" {
            self.init_xml = files;
        } else {
            self.update_xml = files;
        }
    }
}

pub fn parse(src: &str) -> Option<Manifest> {
    let loc = Locator::new(src);
    let parsed = parse_module(src).ok()?;
    let dict = parsed.syntax().body.iter().find_map(|s| match s {
        Stmt::Expr(e) => e.value.as_dict_expr(),
        _ => None,
    })?;
    let mut m = Manifest {
        installable: true,
        ..Manifest::default()
    };
    for item in &dict.items {
        let Some(key) = item.key.as_ref().and_then(str_lit) else {
            continue;
        };
        m.set(&loc, key, &item.value);
    }
    Some(m)
}

fn list(loc: &Locator, e: &Expr) -> Vec<Spanned<String>> {
    let elts = match e {
        Expr::List(l) => &l.elts[..],
        Expr::Tuple(t) => &t.elts[..],
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

/// The globs of every bundle, whether a plain entry or the last item of an `(op, ..., glob)` one.
/// `include` names a bundle and `remove` takes files out, so neither adds anything.
fn asset_globs(e: &Expr) -> Vec<String> {
    let Expr::Dict(bundles) = e else {
        return vec![];
    };
    let entries = bundles.items.iter().flat_map(|item| match &item.value {
        Expr::List(l) => &l.elts[..],
        _ => &[],
    });
    entries
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
        assert_eq!(read(r#"{"auto_install": ["web", OTHER]}"#), AutoInstall::Off);
        assert_eq!(read(r#"{"auto_install": False}"#), AutoInstall::Off);
        assert_eq!(read(r#"{"name": "x"}"#), AutoInstall::Off);
    }

    #[test]
    fn lists_the_sql_files_of_the_data_keys_a_series_reads() {
        let m =
            parse(r#"{"data": ["a.xml", "a.sql"], "init_xml": ["b.sql"], "update_xml": ["c.sql", "c.csv"]}"#).unwrap();
        let files = |series: Option<&str>| m.sql_files(series.and_then(Series::parse)).collect::<Vec<_>>();
        assert_eq!(files(Some("14.0")), ["a.sql", "b.sql", "c.sql"]);
        assert_eq!(files(Some("saas~15.1")), ["a.sql", "b.sql", "c.sql"]);
        assert_eq!(files(Some("saas~15.2")), ["a.sql"]);
        assert_eq!(files(Some("17.0")), ["a.sql"]);
        assert_eq!(files(Some("19.0")), ["a.sql", "b.sql"]);
        assert_eq!(files(Some("20.0")), ["a.sql"]);
        assert_eq!(files(None), ["a.sql", "b.sql", "c.sql"]);
    }
}
