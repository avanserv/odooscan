use ruff_python_ast::{Expr, Stmt};
use ruff_python_parser::parse_module;

use crate::facts::Spanned;
use crate::python::{Locator, str_lit};
use crate::series::Series;

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
    /// The files of `init_xml` and `update_xml`, which Odoo loads as data before 19.0.
    pub old_data: Vec<String>,
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

    /// The SQL files Odoo runs when it loads the data of the module.
    pub fn sql_files(&self) -> impl Iterator<Item = &str> {
        let data = self.data.iter().map(|f| f.value.as_str());
        data.chain(self.old_data.iter().map(String::as_str))
            .filter(|f| f.ends_with(".sql"))
    }

    fn set(&mut self, loc: &Locator, key: &str, value: &Expr) {
        match key {
            "version" => self.version = str_lit(value).map(str::to_owned),
            "depends" => self.depends = list(loc, value),
            "data" => self.data = list(loc, value),
            "init_xml" | "update_xml" => self.old_data.extend(list(loc, value).into_iter().map(|f| f.value)),
            "demo" => self.demo = list(loc, value),
            "category" => self.category = str_lit(value).map(str::to_owned),
            "installable" => self.installable = !matches!(value, Expr::BooleanLiteral(b) if !b.value),
            "assets" => self.assets = asset_globs(value),
            "auto_install" => self.auto_install = AutoInstall::read(value),
            _ => {}
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
    fn lists_the_sql_files_of_every_data_key() {
        let m =
            parse(r#"{"data": ["a.xml", "a.sql"], "init_xml": ["b.sql"], "update_xml": ["c.sql", "c.csv"]}"#).unwrap();
        assert_eq!(m.sql_files().collect::<Vec<_>>(), ["a.sql", "b.sql", "c.sql"]);
    }
}
