//! The files Odoo runs of a module. Loading a module imports `odoo.addons.<name>`, which runs its
//! root `__init__.py` and only what that imports (18.0 `odoo/modules/module.py:370-384`, 20.0
//! `:536-541`, 9.0 `openerp/modules/module.py:316-331`), and the tests are the submodules `tests`
//! binds under a `test_` name (18.0 `odoo/tests/loader.py:57-67`, 9.0 `module.py:387-408`).

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, Stmt};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{Exports, FileImports, ImportFact, Spanned};
use crate::module_state;
use crate::project::Module;
use crate::python::{Locator, str_lit, tail_name};
use crate::python2;
use crate::series::Series;

/// Calls that import a module by a name or a path the code holds, or list the modules of a package
/// for code to import.
const LOADERS: &[&str] = &[
    "exec_module",
    "load_module",
    "load_source",
    "spec_from_file_location",
    "walk_packages",
    "iter_modules",
];

/// The imports of a parsed file.
pub(crate) fn file_imports(body: &[Stmt], loc: &Locator) -> FileImports {
    let mut collect = Collect {
        loc,
        list: vec![],
        dynamic: false,
    };
    collect.visit_body(body);
    FileImports {
        list: collect.list,
        exports: exports(body),
        dynamic: collect.dynamic,
    }
}

/// Every import of a file, in functions and classes too, since a call may run any of them.
struct Collect<'l> {
    loc: &'l Locator<'l>,
    list: Vec<ImportFact>,
    dynamic: bool,
}

impl<'a> Visitor<'a> for Collect<'_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Import(i) => self.list.push(ImportFact {
                level: 0,
                module: None,
                names: i.names.iter().map(|a| self.spanned(a)).collect(),
                from: false,
            }),
            Stmt::ImportFrom(i) => self.list.push(ImportFact {
                level: i.level,
                module: i.module.as_ref().map(ToString::to_string),
                names: i.names.iter().map(|a| self.spanned(a)).collect(),
                from: true,
            }),
            _ => {}
        }
        visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Call(call) = expr {
            self.call(call);
        }
        visitor::walk_expr(self, expr);
    }
}

impl Collect<'_> {
    /// The name an alias imports, which `as` leaves the parent package bound under too.
    fn spanned(&self, alias: &ast::Alias) -> Spanned<String> {
        Spanned {
            value: alias.name.to_string(),
            pos: self.loc.at(alias),
        }
    }

    /// An `import_module()` or `__import__()` of an absolute literal is an import of it; any other,
    /// or a loader, may run any file.
    fn call(&mut self, call: &ast::ExprCall) {
        let Some(name) = tail_name(&call.func) else { return };
        if LOADERS.contains(&name) {
            self.dynamic = true;
        }
        if !matches!(name, "import_module" | "__import__") {
            return;
        }
        let args = &call.arguments;
        let literal = match args.args.as_ref() {
            [only] if args.keywords.is_empty() => str_lit(only).filter(|n| !n.is_empty() && !n.starts_with('.')),
            _ => None,
        };
        match literal {
            Some(module) => self.list.push(ImportFact {
                level: 0,
                module: None,
                names: vec![Spanned {
                    value: module.to_owned(),
                    pos: self.loc.at(call),
                }],
                from: false,
            }),
            None => self.dynamic = true,
        }
    }
}

/// What the file binds to `__all__` at import.
fn exports(body: &[Stmt]) -> Exports {
    let mut exports = Exports::Absent;
    for stmt in module_state::top_level(body) {
        let (targets, value, extends) = match stmt {
            Stmt::Assign(a) => (a.targets.iter().collect::<Vec<_>>(), Some(&*a.value), false),
            Stmt::AnnAssign(a) => (vec![&*a.target], a.value.as_deref(), false),
            Stmt::AugAssign(a) => (vec![&*a.target], Some(&*a.value), true),
            _ => continue,
        };
        if !targets.iter().any(|t| is_all(t)) {
            continue;
        }
        let names = value.filter(|_| targets.len() == 1).and_then(literal_names);
        exports = match (exports, names) {
            (Exports::Literal(mut all), Some(more)) if extends => {
                all.extend(more);
                Exports::Literal(all)
            }
            (_, Some(names)) if !extends => Exports::Literal(names),
            _ => Exports::Unread,
        };
    }
    exports
}

fn is_all(target: &Expr) -> bool {
    match target {
        Expr::Name(n) => n.id.as_str() == "__all__",
        Expr::Tuple(t) => t.elts.iter().any(is_all),
        Expr::List(l) => l.elts.iter().any(is_all),
        Expr::Starred(s) => is_all(&s.value),
        _ => false,
    }
}

/// The strings of a list or tuple of string literals.
fn literal_names(expr: &Expr) -> Option<Vec<String>> {
    let elts = match expr {
        Expr::List(l) => &l.elts,
        Expr::Tuple(t) => &t.elts,
        _ => return None,
    };
    elts.iter().map(|e| str_lit(e).map(str::to_owned)).collect()
}

/// What an import names: `odoo.addons`, whose children are addons, or a module or package of an
/// addon by its segments under the addon's root.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Place {
    Addons,
    In(String, Vec<String>),
}

impl Place {
    fn join<'s>(self, segments: impl IntoIterator<Item = &'s str>) -> Self {
        let mut segments = segments.into_iter();
        match self {
            Self::Addons => match segments.next() {
                Some(addon) => Self::In(addon.to_owned(), segments.map(str::to_owned).collect()),
                None => Self::Addons,
            },
            Self::In(addon, mut path) => {
                path.extend(segments.map(str::to_owned));
                Self::In(addon, path)
            }
        }
    }
}

/// The files an import of a dotted path runs, each package on the way first, and the first segment
/// that names no module, as when it names an attribute.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Located {
    pub files: Vec<String>,
    pub missing: Option<usize>,
}

/// The Python files of an addon.
#[derive(Debug, Default)]
struct Addon {
    /// By path relative to the addon, none for a file that does not parse.
    files: FxHashMap<String, Option<FileImports>>,
    /// The directories that hold a Python file at any depth, which Python 3 imports as namespace
    /// packages when they have no `__init__.py`.
    dirs: FxHashSet<String>,
    /// The module runs on Python 2, or on a series the scan does not know, where `import x` may
    /// name a submodule of the importing package.
    implicit: bool,
}

/// The imports of every module of the scan.
#[derive(Debug, Default)]
pub struct Graph {
    addons: FxHashMap<String, Addon>,
    /// The files of each addon that a file of another addon imports.
    elsewhere: FxHashMap<String, FxHashSet<String>>,
}

impl Graph {
    /// The imports of the Python files of `modules`, each of its own series or `default_series`.
    pub fn of_modules(modules: &[Module], default_series: Option<Series>) -> Self {
        let addons = modules.iter().map(|m| {
            let series = m.series.or(default_series);
            let files = m.python.iter().map(|f| (f.rel.clone(), f.facts.imports.clone()));
            (m.name.clone(), addon(files, series))
        });
        Self::of(addons.collect())
    }

    fn of(addons: FxHashMap<String, Addon>) -> Self {
        let mut graph = Self {
            addons,
            elsewhere: FxHashMap::default(),
        };
        let mut elsewhere: FxHashMap<String, FxHashSet<String>> = FxHashMap::default();
        for (name, addon) in &graph.addons {
            for (rel, imports) in &addon.files {
                let list = imports.iter().flat_map(|i| &i.list);
                for (target, file) in list.flat_map(|fact| graph.targets(name, rel, fact)) {
                    if target != *name {
                        elsewhere.entry(target).or_default().insert(file);
                    }
                }
            }
        }
        graph.elsewhere = elsewhere;
        graph
    }

    /// The files of `addon` that a file of another addon imports.
    pub fn elsewhere(&self, addon: &str) -> impl Iterator<Item = &str> {
        self.elsewhere.get(addon).into_iter().flatten().map(String::as_str)
    }

    /// The files of `addon` that importing `starts` runs, those that exist among them included,
    /// none when one of them does not parse or may import any file.
    pub fn reached(&self, addon: &str, starts: &[&str]) -> Option<FxHashSet<String>> {
        let files = &self.addons.get(addon)?.files;
        let starts = starts.iter().filter(|s| files.contains_key(**s));
        let mut seen: FxHashSet<String> = starts.map(|s| (*s).to_owned()).collect();
        let mut todo: Vec<String> = seen.iter().cloned().collect();
        while let Some(rel) = todo.pop() {
            let imports = files.get(&rel)?.as_ref().filter(|i| !i.dynamic)?;
            let targets = imports.list.iter().flat_map(|fact| self.targets(addon, &rel, fact));
            for (_, file) in targets.filter(|(target, _)| target == addon) {
                if seen.insert(file.clone()) {
                    todo.push(file);
                }
            }
        }
        Some(seen)
    }

    /// The files an import of file `rel` of `addon` runs, each with its addon.
    fn targets(&self, addon: &str, rel: &str, fact: &ImportFact) -> Vec<(String, String)> {
        let places = if fact.from {
            self.named_places(addon, rel, fact)
        } else {
            let names = fact.names.iter();
            names
                .filter_map(|n| self.place(addon, rel, 0, Some(&n.value)))
                .collect()
        };
        let mut found = vec![];
        for place in places {
            if let Place::In(to, path) = place {
                let files = self.locate(&to, &path).files;
                found.extend(files.into_iter().map(|f| (to.clone(), f)));
            }
        }
        found
    }

    /// What `from P import n` names: `P`, and `P.n` for each name that may be a submodule.
    fn named_places(&self, addon: &str, rel: &str, fact: &ImportFact) -> Vec<Place> {
        let Some(place) = self.place(addon, rel, fact.level, fact.module.as_deref()) else {
            return vec![];
        };
        let names = fact.names.iter().map(|n| n.value.as_str());
        let Place::In(to, path) = &place else {
            return names
                .filter(|n| *n != "*")
                .map(|n| Place::In(n.to_owned(), vec![]))
                .collect();
        };
        if self.locate(to, path).missing.is_some() {
            return vec![place];
        }
        let mut places = vec![];
        for name in names {
            let children = if name == "*" {
                self.star(to, path)
            } else {
                vec![name.to_owned()]
            };
            places.extend(children.iter().map(|c| place.clone().join([c.as_str()])));
        }
        places.push(place);
        places
    }

    /// What an import of `module`, with `level` leading dots, from file `rel` of `addon` names.
    fn place(&self, addon: &str, rel: &str, level: u32, module: Option<&str>) -> Option<Place> {
        let segments = module.into_iter().flat_map(|m| m.split('.'));
        let package: Vec<&str> = rel.rsplit_once('/').map_or(vec![], |(dir, _)| dir.split('/').collect());
        let own = Place::In(addon.to_owned(), vec![]);
        if level > 0 {
            let up = level as usize - 1;
            let base = match package.len().checked_sub(up) {
                Some(kept) => own.join(package[..kept].iter().copied()),
                None if up == package.len() + 1 => Place::Addons,
                None => return None,
            };
            return Some(base.join(segments));
        }
        let module = module?;
        let first = module.split('.').next().unwrap_or_default();
        let local: Vec<String> = package.iter().chain([&first]).map(|s| (*s).to_owned()).collect();
        let implicit = self.addons.get(addon).is_some_and(|a| a.implicit);
        if implicit && self.locate(addon, &local).missing.is_none() {
            return Some(own.join(package).join(segments));
        }
        match module.split('.').collect::<Vec<_>>().as_slice() {
            ["odoo" | "openerp", "addons", rest @ ..] => Some(Place::Addons.join(rest.iter().copied())),
            _ => None,
        }
    }

    /// The files importing `path` of `addon` runs, its root `__init__.py` first: a package's
    /// `__init__.py`, else a module, else a directory Python 3 reads as a namespace package.
    pub fn locate(&self, addon: &str, path: &[String]) -> Located {
        let Some(files) = self.addons.get(addon) else {
            return Located {
                files: vec![],
                missing: Some(0),
            };
        };
        let mut located = Located::default();
        located
            .files
            .extend(files.files.get_key_value("__init__.py").map(|(k, _)| k.clone()));
        let mut dir = String::new();
        for (i, segment) in path.iter().enumerate() {
            let at = if dir.is_empty() {
                segment.clone()
            } else {
                format!("{dir}/{segment}")
            };
            let init = format!("{at}/__init__.py");
            let module = format!("{at}.py");
            if files.files.contains_key(&init) {
                located.files.push(init);
            } else if files.files.contains_key(&module) {
                located.files.push(module);
                located.missing = (i + 1 < path.len()).then_some(i + 1);
                return located;
            } else if !files.dirs.contains(&at) {
                located.missing = Some(i);
                return located;
            }
            dir = at;
        }
        located
    }

    /// The submodules a star import of package `path` of `addon` runs: those its `__all__` lists,
    /// none without one, and every one when the scan does not read it.
    fn star(&self, addon: &str, path: &[String]) -> Vec<String> {
        let Some(files) = self.addons.get(addon) else {
            return vec![];
        };
        let dir = path.join("/");
        let init = if dir.is_empty() {
            "__init__.py".to_owned()
        } else {
            format!("{dir}/__init__.py")
        };
        let exports = match files.files.get(&init) {
            Some(Some(imports)) => &imports.exports,
            Some(None) => &Exports::Unread,
            None => &Exports::Absent,
        };
        match exports {
            Exports::Absent => vec![],
            Exports::Literal(names) => names.clone(),
            Exports::Unread => {
                let prefix = if dir.is_empty() {
                    String::new()
                } else {
                    format!("{dir}/")
                };
                let children = files.files.keys().chain(&files.dirs).filter_map(|rel| {
                    let rest = rel.strip_prefix(&prefix)?;
                    let child = rest.split('/').next()?;
                    Some(child.strip_suffix(".py").unwrap_or(child).to_owned())
                });
                let mut children: Vec<_> = children.filter(|c| c != "__init__").collect();
                children.sort_unstable();
                children.dedup();
                children
            }
        }
    }
}

/// An addon of `series` with its Python files.
fn addon(files: impl Iterator<Item = (String, Option<FileImports>)>, series: Option<Series>) -> Addon {
    let mut addon = Addon {
        implicit: series.is_none() || python2::runs_on(series),
        ..Addon::default()
    };
    for (rel, imports) in files {
        let mut dir = rel.as_str();
        while let Some((parent, _)) = dir.rsplit_once('/') {
            addon.dirs.insert(parent.to_owned());
            dir = parent;
        }
        addon.files.insert(rel, imports);
    }
    addon
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::python::analyze;

    fn imports(src: &str) -> FileImports {
        analyze(src, None).imports.unwrap()
    }

    /// The facts of each import as `(level, module, names, from)`.
    fn facts(src: &str) -> Vec<(u32, Option<String>, Vec<String>, bool)> {
        let list = imports(src).list.into_iter();
        list.map(|f| {
            (
                f.level,
                f.module,
                f.names.into_iter().map(|n| n.value).collect(),
                f.from,
            )
        })
        .collect()
    }

    fn owned(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    #[test]
    fn each_import_form_is_one_fact() {
        let src = "import a.b, c as d\nfrom . import x as y, z\nfrom ..m import *\nfrom odoo.addons.shop import models\n\ndef f():\n    from .late import g\n\nclass K:\n    import h\n\ntry:\n    from . import opt\nexcept ImportError:\n    pass\n";
        assert_eq!(
            facts(src),
            [
                (0, None, owned(&["a.b", "c"]), false),
                (1, None, owned(&["x", "z"]), true),
                (2, Some("m".to_owned()), owned(&["*"]), true),
                (0, Some("odoo.addons.shop".to_owned()), owned(&["models"]), true),
                (1, Some("late".to_owned()), owned(&["g"]), true),
                (0, None, owned(&["h"]), false),
                (1, None, owned(&["opt"]), true),
            ]
        );
        let pos = imports(src).list[1].names[1].pos;
        assert_eq!((pos.line, pos.col), (2, 23));
    }

    #[test]
    fn a_literal_import_module_is_an_import_and_any_other_is_dynamic() {
        let literal = "import importlib\nimportlib.import_module('odoo.addons.shop.models')\n__import__('lib')\n";
        let found = imports(literal);
        assert!(!found.dynamic);
        assert_eq!(
            facts(literal)[1..],
            [
                (0, None, owned(&["odoo.addons.shop.models"]), false),
                (0, None, owned(&["lib"]), false),
            ]
        );
        for call in [
            "importlib.import_module(name)",
            "importlib.import_module('.x', __package__)",
            "__import__('x', fromlist=['y'])",
            "spec.loader.exec_module(module)",
            "imp.load_source('x', path)",
            "importlib.util.spec_from_file_location('x', path)",
            "pkgutil.walk_packages(path)",
            "pkgutil.iter_modules(path)",
            "loader.load_module('x')",
        ] {
            assert!(imports(&format!("def f():\n    {call}\n")).dynamic, "{call}");
        }
    }

    #[test]
    fn all_is_read_when_it_lists_literals() {
        let read = |src: &str| imports(src).exports;
        assert_eq!(read("x = 1\n"), Exports::Absent);
        assert_eq!(read("__all__ = ['a', 'b']\n"), Exports::Literal(owned(&["a", "b"])));
        assert_eq!(
            read("__all__ = ('a',)\n__all__ += ['b']\n"),
            Exports::Literal(owned(&["a", "b"]))
        );
        assert_eq!(read("__all__ = names\n"), Exports::Unread);
        assert_eq!(read("__all__ = ['a', b]\n"), Exports::Unread);
        assert_eq!(read("__all__ += ['a']\n"), Exports::Unread);
        assert_eq!(read("__all__, x = ['a'], 1\n"), Exports::Unread);
    }

    #[test]
    fn a_file_that_does_not_parse_has_no_imports() {
        assert!(analyze("from . import (\n", None).imports.is_none());
    }

    /// Files as paths and sources, `None` for one that does not parse.
    type Files<'s> = &'s [(&'s str, Option<&'s str>)];

    /// A graph of addons, each a name, whether it reads implicit relative imports, and its files.
    fn graph(addons: &[(&str, bool, Files)]) -> Graph {
        let addons = addons.iter().map(|(name, implicit, files)| {
            let files = files.iter().map(|(rel, src)| ((*rel).to_owned(), src.map(imports)));
            let series = if *implicit { None } else { Series::parse("18.0") };
            ((*name).to_owned(), addon(files, series))
        });
        Graph::of(addons.collect())
    }

    fn reached(graph: &Graph, addon: &str, starts: &[&str]) -> Option<Vec<String>> {
        let mut found: Vec<_> = graph.reached(addon, starts)?.into_iter().collect();
        found.sort();
        Some(found)
    }

    #[test]
    fn locate_runs_each_package_then_the_module() {
        let g = graph(&[(
            "shop",
            false,
            &[
                ("__init__.py", Some("")),
                ("models/__init__.py", Some("")),
                ("models/sale.py", Some("")),
                ("wizard/pick.py", Some("")),
            ],
        )]);
        let locate = |path: &[&str]| g.locate("shop", &owned(path));
        assert_eq!(
            locate(&["models", "sale"]),
            Located {
                files: owned(&["__init__.py", "models/__init__.py", "models/sale.py"]),
                missing: None
            }
        );
        assert_eq!(
            locate(&["wizard", "pick"]).files,
            owned(&["__init__.py", "wizard/pick.py"])
        );
        assert_eq!(locate(&["models", "Sale"]).missing, Some(1));
        assert_eq!(locate(&["models", "sale", "Sale"]).missing, Some(2));
        assert_eq!(g.locate("gone", &[]).missing, Some(0));
    }

    #[test]
    fn relative_levels_climb_packages_and_past_the_root_name_an_addon() {
        let g = graph(&[
            (
                "shop",
                false,
                &[
                    ("__init__.py", Some("from . import models\n")),
                    ("models/__init__.py", Some("from . import sale\nfrom .. import tools\n")),
                    ("models/sale.py", Some("from ...stock.models import move\n")),
                    ("tools.py", Some("")),
                    ("unused.py", Some("")),
                ],
            ),
            (
                "stock",
                false,
                &[
                    ("__init__.py", Some("")),
                    ("models/__init__.py", Some("")),
                    ("models/move.py", Some("")),
                    ("models/quant.py", Some("")),
                ],
            ),
        ]);
        assert_eq!(
            reached(&g, "shop", &["__init__.py"]).unwrap(),
            owned(&["__init__.py", "models/__init__.py", "models/sale.py", "tools.py"])
        );
        let elsewhere: FxHashSet<_> = g.elsewhere("stock").collect();
        assert_eq!(
            elsewhere,
            ["__init__.py", "models/__init__.py", "models/move.py"]
                .into_iter()
                .collect()
        );
        assert_eq!(g.elsewhere("shop").count(), 0);
    }

    #[test]
    fn an_absolute_import_of_the_addon_itself_reaches_its_files() {
        let g = graph(&[(
            "shop",
            false,
            &[
                (
                    "__init__.py",
                    Some(
                        "import odoo.addons.shop.a\nfrom openerp.addons.shop import b\nfrom odoo.addons import shop\nimport models\n",
                    ),
                ),
                ("a.py", Some("")),
                ("b.py", Some("")),
                ("models.py", Some("")),
            ],
        )]);
        assert_eq!(
            reached(&g, "shop", &["__init__.py"]).unwrap(),
            owned(&["__init__.py", "a.py", "b.py"])
        );
    }

    #[test]
    fn a_plain_import_is_relative_on_python_2_or_an_unknown_series() {
        let files: &[(&str, Option<&str>)] = &[
            ("__init__.py", Some("import models\nfrom wizard import pick\n")),
            ("models/__init__.py", Some("import sale\n")),
            ("models/sale.py", Some("")),
            ("wizard/__init__.py", Some("")),
            ("wizard/pick.py", Some("")),
        ];
        let g = graph(&[("shop", true, files)]);
        assert_eq!(
            reached(&g, "shop", &["__init__.py"]).unwrap(),
            owned(&[
                "__init__.py",
                "models/__init__.py",
                "models/sale.py",
                "wizard/__init__.py",
                "wizard/pick.py"
            ])
        );
        let g = graph(&[("shop", false, files)]);
        assert_eq!(reached(&g, "shop", &["__init__.py"]).unwrap(), owned(&["__init__.py"]));
    }

    #[test]
    fn a_star_import_runs_what_all_lists_or_every_child_when_unread() {
        let star = |all: &str| {
            let init = format!("{all}\n");
            let g = graph(&[(
                "shop",
                false,
                &[
                    ("__init__.py", Some("from .models import *\n")),
                    ("models/__init__.py", Some(&init)),
                    ("models/a.py", Some("")),
                    ("models/b.py", Some("")),
                    ("models/sub/c.py", Some("")),
                ],
            )]);
            reached(&g, "shop", &["__init__.py"]).unwrap()
        };
        assert_eq!(star(""), owned(&["__init__.py", "models/__init__.py"]));
        assert_eq!(
            star("__all__ = ['a']"),
            owned(&["__init__.py", "models/__init__.py", "models/a.py"])
        );
        assert_eq!(
            star("__all__ = names"),
            owned(&["__init__.py", "models/__init__.py", "models/a.py", "models/b.py"])
        );
    }

    #[test]
    fn the_reach_is_unknown_past_a_dynamic_or_unparsed_file() {
        let g = graph(&[(
            "shop",
            false,
            &[
                ("__init__.py", Some("from . import a\n")),
                ("a.py", Some("from . import b\n")),
                ("b.py", None),
                ("c.py", Some("import importlib\nimportlib.import_module(name)\n")),
            ],
        )]);
        assert!(g.reached("shop", &["__init__.py"]).is_none());
        assert!(g.reached("shop", &["c.py"]).is_none());
        assert!(g.reached("gone", &["__init__.py"]).is_none());
    }

    #[test]
    fn importing_a_submodule_binds_it_whatever_name_it_takes() {
        let g = graph(&[(
            "shop",
            false,
            &[
                ("__init__.py", Some("")),
                ("tests/__init__.py", Some("from . import test_a as a\n")),
                ("tests/test_a.py", Some("")),
                ("tests/test_b.py", Some("")),
            ],
        )]);
        assert_eq!(
            reached(&g, "shop", &["__init__.py", "tests/__init__.py", "tests/gone.py"]).unwrap(),
            owned(&["__init__.py", "tests/__init__.py", "tests/test_a.py"])
        );
    }
}
