use ruff_python_ast::{Expr, Stmt};
use ruff_python_parser::parse_module;

use crate::facts::Spanned;
use crate::python::{Locator, str_lit};
use crate::series::Series;

/// The manifest keys odooscan reads, evaluated as literals; the file is never executed.
#[derive(Debug, Default)]
pub struct Manifest {
    pub version: Option<String>,
    pub depends: Vec<Spanned<String>>,
    pub data: Vec<Spanned<String>>,
    pub demo: Vec<Spanned<String>>,
    pub category: Option<String>,
    pub installable: bool,
}

impl Manifest {
    pub fn series(&self) -> Option<Series> {
        self.version.as_deref().and_then(Series::from_module_version)
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
    let list = |e: &Expr| -> Vec<Spanned<String>> {
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
    };
    for item in &dict.items {
        let Some(key) = item.key.as_ref().and_then(str_lit) else {
            continue;
        };
        match key {
            "version" => m.version = str_lit(&item.value).map(str::to_owned),
            "depends" => m.depends = list(&item.value),
            "data" => m.data = list(&item.value),
            "demo" => m.demo = list(&item.value),
            "category" => m.category = str_lit(&item.value).map(str::to_owned),
            "installable" => m.installable = !matches!(&item.value, Expr::BooleanLiteral(b) if !b.value),
            _ => {}
        }
    }
    Some(m)
}
