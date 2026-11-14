//! The files Odoo runs of a module. Loading a module imports `odoo.addons.<name>`, which runs its
//! root `__init__.py` and only what that imports (18.0 `odoo/modules/module.py:370-384`, 20.0
//! `:536-541`, 9.0 `openerp/modules/module.py:316-331`), and the tests are the submodules `tests`
//! binds under a `test_` name (18.0 `odoo/tests/loader.py:57-67`, 9.0 `module.py:387-408`).

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, Stmt};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{Exports, FileImports, ImportFact, Pos, Spanned};
use crate::project::Module;
use crate::python::{Locator, str_lit, tail_name};
use crate::series::Series;
use crate::{method_body, module_state, python, python2};

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
        deferred: false,
        guarded: false,
    };
    collect.visit_body(body);
    FileImports {
        list: collect.list,
        exports: exports(body),
        dynamic: collect.dynamic,
        defines: module_state::defined_names(body),
    }
}

/// Every import of a file, in functions and classes too, since a call may run any of them.
struct Collect<'l> {
    loc: &'l Locator<'l>,
    list: Vec<ImportFact>,
    dynamic: bool,
    deferred: bool,
    guarded: bool,
}

impl<'a> Visitor<'a> for Collect<'_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Import(_) | Stmt::ImportFrom(_) => self.import(stmt),
            Stmt::FunctionDef(f) => {
                self.function(f);
                return;
            }
            _ if self.guards(stmt) => return,
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

impl<'a> Collect<'_> {
    fn fact(&self, pos: Pos, level: u32, module: Option<String>, names: &[ast::Alias], from: bool) -> ImportFact {
        ImportFact {
            pos,
            level,
            module,
            names: names.iter().map(|a| self.spanned(a)).collect(),
            from,
            deferred: self.deferred,
            guarded: self.guarded,
        }
    }

    fn import(&mut self, stmt: &Stmt) {
        let fact = match stmt {
            Stmt::Import(i) => self.fact(self.loc.at(i), 0, None, &i.names, false),
            Stmt::ImportFrom(i) => {
                let module = i.module.as_ref();
                let pos = module.map_or_else(|| self.loc.at(i), |m| self.loc.at(m));
                self.fact(pos, i.level, module.map(ToString::to_string), &i.names, true)
            }
            _ => return,
        };
        self.list.push(fact);
    }

    /// Visits a statement whose body an import may fail in without stopping the module, guarding
    /// that body, and whether it was one.
    fn guards(&mut self, stmt: &'a Stmt) -> bool {
        match stmt {
            Stmt::Try(t) if t.handlers.iter().any(catches_import) => self.guarded_try(t),
            Stmt::With(w) if method_body::suppresses(w) => {
                for item in &w.items {
                    self.visit_with_item(item);
                }
                self.guard(&w.body);
            }
            Stmt::If(i) if type_checking(&i.test) => {
                self.visit_expr(&i.test);
                self.guard(&i.body);
                for clause in &i.elif_else_clauses {
                    self.visit_elif_else_clause(clause);
                }
            }
            _ => return false,
        }
        true
    }

    fn guarded_try(&mut self, t: &'a ast::StmtTry) {
        self.guard(&t.body);
        for handler in &t.handlers {
            self.visit_except_handler(handler);
        }
        self.visit_body(&t.orelse);
        self.visit_body(&t.finalbody);
    }

    /// The body of a function runs when it is called, outside any `try` around the `def`.
    fn function(&mut self, f: &'a ast::StmtFunctionDef) {
        for decorator in &f.decorator_list {
            self.visit_decorator(decorator);
        }
        self.visit_parameters(&f.parameters);
        let outer = (self.deferred, self.guarded);
        (self.deferred, self.guarded) = (true, false);
        self.visit_body(&f.body);
        (self.deferred, self.guarded) = outer;
    }

    fn guard(&mut self, body: &'a [Stmt]) {
        let outer = std::mem::replace(&mut self.guarded, true);
        self.visit_body(body);
        self.guarded = outer;
    }

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
            Some(module) => {
                let pos = self.loc.at(call);
                let name = Spanned {
                    value: module.to_owned(),
                    pos,
                };
                self.list.push(ImportFact {
                    names: vec![name],
                    ..self.fact(pos, 0, None, &[], false)
                });
            }
            None => self.dynamic = true,
        }
    }
}

/// Whether a handler catches what a failed import raises.
fn catches_import(handler: &ast::ExceptHandler) -> bool {
    let ast::ExceptHandler::ExceptHandler(h) = handler;
    ["ImportError", "ModuleNotFoundError"]
        .into_iter()
        .any(|error| python::catches(h, error))
}

/// `TYPE_CHECKING` or `typing.TYPE_CHECKING`, true only for a type checker.
fn type_checking(test: &Expr) -> bool {
    tail_name(test) == Some("TYPE_CHECKING")
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
    /// A file of the module does not decode, so it may hold any module or name.
    lost: bool,
}

impl Addon {
    /// The names module `path` binds, an empty list for a namespace package, and none when the
    /// scan cannot list them.
    fn defines(&self, path: &[String]) -> Option<&[String]> {
        let at = path.join("/");
        let file = if at.is_empty() {
            "__init__.py".to_owned()
        } else if self.files.contains_key(&format!("{at}/__init__.py")) {
            format!("{at}/__init__.py")
        } else {
            format!("{at}.py")
        };
        match self.files.get(&file) {
            Some(imports) => imports.as_ref()?.defines.as_deref(),
            None => Some(&[]),
        }
    }
}

/// An import of a module of an addon by its path under `odoo.addons`, its own addon included, or
/// by a relative path past the importer's root.
#[derive(Debug, PartialEq, Eq)]
pub struct AddonImport {
    pub addon: String,
    /// The module as the import writes it, its leading dots included.
    pub text: String,
    pub pos: Pos,
    pub faults: Vec<Fault>,
}

/// What an import names that its addon does not have.
#[derive(Debug, PartialEq, Eq)]
pub enum Fault {
    /// No module or package of the addon has the path.
    Missing,
    /// The module neither binds the name nor has a submodule of it.
    Undefined(Spanned<String>),
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
            let addon = Addon {
                lost: m.lost_text,
                ..addon(files, series)
            };
            (m.name.clone(), addon)
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

    /// The modules of addons `fact` of file `rel` imports by their path, each with what the addon
    /// lacks of it. The scan reads no fault where the addon may hold anything: outside the graph,
    /// with a file that does not decode or parse on the way, under `base.maintenance`, which Odoo
    /// maps to `odoo.upgrade`, or under a `tests/` it lacks, as a fetched core drops it.
    pub fn addon_imports(&self, rel: &str, fact: &ImportFact) -> Vec<AddonImport> {
        let mut found = vec![];
        if !fact.from {
            for name in &fact.names {
                if let Some(Place::In(to, path)) = addon_place(rel, 0, Some(&name.value)) {
                    found.push(self.addon_import(to, path, name.value.clone(), name.pos, &[]));
                }
            }
            return found;
        }
        let module = fact.module.as_deref();
        let written = format!("{}{}", ".".repeat(fact.level as usize), module.unwrap_or_default());
        match addon_place(rel, fact.level, module) {
            Some(Place::In(to, path)) => found.push(self.addon_import(to, path, written, fact.pos, &fact.names)),
            Some(Place::Addons) => {
                let sep = if module.is_some() { "." } else { "" };
                for name in fact.names.iter().filter(|n| n.value != "*") {
                    let text = format!("{written}{sep}{}", name.value);
                    found.push(self.addon_import(name.value.clone(), vec![], text, name.pos, &[]));
                }
            }
            None => {}
        }
        found
    }

    fn addon_import(
        &self,
        to: String,
        path: Vec<String>,
        text: String,
        pos: Pos,
        names: &[Spanned<String>],
    ) -> AddonImport {
        AddonImport {
            faults: self.faults(&to, &path, names),
            addon: to,
            text,
            pos,
        }
    }

    /// What addon `to` lacks of `path` and of the `names` imported from it.
    fn faults(&self, to: &str, path: &[String], names: &[Spanned<String>]) -> Vec<Fault> {
        let Some(target) = self.addons.get(to).filter(|t| !self.blind(to, t, path)) else {
            return vec![];
        };
        if self.locate(to, path).missing.is_some() {
            return vec![Fault::Missing];
        }
        let Some(defines) = target.defines(path) else {
            return vec![];
        };
        let undefined = names.iter().filter(|n| {
            let name = n.value.as_str();
            let dunder = name.starts_with("__") && name.ends_with("__");
            name != "*" && !dunder && !defines.contains(&n.value) && !self.child(to, path, name)
        });
        undefined.map(|n| Fault::Undefined(n.clone())).collect()
    }

    /// Whether the scan cannot tell what `path` of addon `to` holds.
    fn blind(&self, to: &str, target: &Addon, path: &[String]) -> bool {
        let head = path.first().map(String::as_str);
        let unlisted =
            (to == "base" && head == Some("maintenance")) || (head == Some("tests") && !target.dirs.contains("tests"));
        let unread = || {
            let files = self.locate(to, path).files;
            files.iter().any(|f| target.files.get(f).is_some_and(Option::is_none))
        };
        target.lost || unlisted || unread()
    }

    /// Whether `name` is a module under `path` of addon `to`.
    fn child(&self, to: &str, path: &[String], name: &str) -> bool {
        let mut sub = path.to_vec();
        sub.push(name.to_owned());
        self.locate(to, &sub).missing.is_none()
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

/// What an import names in `odoo.addons`, by its absolute path under `odoo.addons` or
/// `openerp.addons` or by one dot more than file `rel` has packages above it.
fn addon_place(rel: &str, level: u32, module: Option<&str>) -> Option<Place> {
    let segments = module.into_iter().flat_map(|m| m.split('.'));
    if level > 0 {
        let past_root = level as usize == rel.matches('/').count() + 2;
        return past_root.then(|| Place::Addons.join(segments));
    }
    match segments.collect::<Vec<_>>().as_slice() {
        ["odoo" | "openerp", "addons", rest @ ..] => Some(Place::Addons.join(rest.iter().copied())),
        _ => None,
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
    fn an_import_is_deferred_in_a_function_and_guarded_where_its_failure_changes_nothing() {
        let src = "import a\ntry:\n    import b\n    def f():\n        import c\nexcept (ImportError, KeyError):\n    import d\nelse:\n    import e\n\
                   try:\n    import g\nexcept ModuleNotFoundError:\n    pass\ntry:\n    import h\nexcept KeyError:\n    pass\n\
                   with contextlib.suppress(ImportError):\n    import i\nif typing.TYPE_CHECKING:\n    import j\nelse:\n    import k\n\
                   class K:\n    def m(self):\n        try:\n            import l\n        except ImportError:\n            pass\n\
                   def n():\n    importlib.import_module('o')\n";
        let flags: Vec<(String, bool, bool)> = imports(src)
            .list
            .into_iter()
            .map(|f| (f.names[0].value.clone(), f.deferred, f.guarded))
            .collect();
        let expected = [
            ("a", false, false),
            ("b", false, true),
            ("c", true, false),
            ("d", false, false),
            ("e", false, false),
            ("g", false, true),
            ("h", false, false),
            ("i", false, true),
            ("j", false, true),
            ("k", false, false),
            ("l", true, true),
            ("o", true, false),
        ];
        let expected: Vec<_> = expected.iter().map(|(n, d, g)| ((*n).to_owned(), *d, *g)).collect();
        assert_eq!(flags, expected);
    }

    #[test]
    fn a_from_import_is_at_its_module_and_any_other_at_the_statement() {
        let list = imports("x = 1; from  odoo.addons.shop import a\nfrom . import b\nimport c\n").list;
        let at: Vec<_> = list.iter().map(|f| (f.pos.line, f.pos.col)).collect();
        assert_eq!(at, [(1, 14), (2, 1), (3, 1)]);
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

    /// The modules and faults of each addon import of file `rel` of `addon`, as
    /// `(addon, text, line, faults)` with each fault as `-` for a missing path or the name.
    fn addon_imports(g: &Graph, rel: &str, src: &str) -> Vec<(String, String, u32, Vec<String>)> {
        let facts = imports(src).list;
        let found = facts.iter().flat_map(|f| g.addon_imports(rel, f));
        found
            .map(|i| {
                let faults = i.faults.iter().map(|f| match f {
                    Fault::Missing => "-".to_owned(),
                    Fault::Undefined(n) => n.value.clone(),
                });
                (i.addon, i.text, i.pos.line, faults.collect())
            })
            .collect()
    }

    fn lib() -> Graph {
        graph(&[
            (
                "lib",
                false,
                &[
                    ("__init__.py", Some("from . import models\nVERSION = 1\n")),
                    (
                        "models/__init__.py",
                        Some("from . import book\nfrom .book import Book\n"),
                    ),
                    ("models/book.py", Some("class Book:\n    pass\n")),
                    ("models/extra.py", Some("")),
                    ("open.py", Some("from .models.book import *\n")),
                    ("broken.py", None),
                    ("space/util.py", Some("")),
                ],
            ),
            ("lost", false, &[("__init__.py", Some(""))]),
            ("base", false, &[("__init__.py", Some(""))]),
        ])
    }

    #[test]
    fn an_addon_import_names_its_addon_and_what_it_lacks() {
        let g = lib();
        let src = "from odoo.addons.lib.models import Book, extra, Gone, __path__\n\
                   import odoo.addons.lib.models.book\n\
                   import odoo.addons.lib.wizard\n\
                   from odoo.addons.lib.models.book import Book\n\
                   from odoo.addons.lib.models.book.Book import x\n\
                   from openerp.addons import lib, other\n\
                   from odoo.addons.lib import VERSION, space\n\
                   from odoo.addons.lib.space import util, nothing\n\
                   from odoo.addons.lib.open import Anything\n\
                   from odoo.addons.lib.broken import Anything\n\
                   from odoo.addons.other.models import x\n\
                   from odoo.addons.base.maintenance.migrations import util\n\
                   from odoo.addons.lib.tests.common import Case\n\
                   from odoo import addons\nfrom . import local\nimport odoo.addons\n";
        let owned = |faults: &[&str]| faults.iter().map(|f| (*f).to_owned()).collect::<Vec<_>>();
        let row = |addon: &str, text: &str, line: u32, faults: &[&str]| {
            (addon.to_owned(), text.to_owned(), line, owned(faults))
        };
        assert_eq!(
            addon_imports(&g, "models/x.py", src),
            [
                row("lib", "odoo.addons.lib.models", 1, &["Gone"]),
                row("lib", "odoo.addons.lib.models.book", 2, &[]),
                row("lib", "odoo.addons.lib.wizard", 3, &["-"]),
                row("lib", "odoo.addons.lib.models.book", 4, &[]),
                row("lib", "odoo.addons.lib.models.book.Book", 5, &["-"]),
                row("lib", "openerp.addons.lib", 6, &[]),
                row("other", "openerp.addons.other", 6, &[]),
                row("lib", "odoo.addons.lib", 7, &[]),
                row("lib", "odoo.addons.lib.space", 8, &["nothing"]),
                row("lib", "odoo.addons.lib.open", 9, &[]),
                row("lib", "odoo.addons.lib.broken", 10, &[]),
                row("other", "odoo.addons.other.models", 11, &[]),
                row("base", "odoo.addons.base.maintenance.migrations", 12, &[]),
                row("lib", "odoo.addons.lib.tests.common", 13, &[]),
            ]
        );
    }

    #[test]
    fn a_relative_import_names_an_addon_only_past_the_root() {
        let g = lib();
        let src = "from ...lib.models import Gone\nfrom ... import lib\nfrom .. import sibling\nfrom .... import far\n";
        assert_eq!(
            addon_imports(&g, "models/x.py", src),
            [
                ("lib".to_owned(), "...lib.models".to_owned(), 1, vec!["Gone".to_owned()]),
                ("lib".to_owned(), "...lib".to_owned(), 2, vec![]),
            ]
        );
        let at_root = addon_imports(&g, "x.py", "from ..lib import wizard\n");
        assert_eq!(
            at_root,
            [("lib".to_owned(), "..lib".to_owned(), 1, vec!["wizard".to_owned()])]
        );
    }

    #[test]
    fn an_addon_with_a_file_that_does_not_decode_lacks_nothing() {
        let mut g = lib();
        g.addons.get_mut("lost").unwrap().lost = true;
        let found = addon_imports(
            &g,
            "x.py",
            "from odoo.addons.lost.models import x\nimport odoo.addons.lib.gone\n",
        );
        let faults: Vec<_> = found.into_iter().map(|(addon, .., f)| (addon, f.len())).collect();
        assert_eq!(faults, [("lost".to_owned(), 0), ("lib".to_owned(), 1)]);
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
