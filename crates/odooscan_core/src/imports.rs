//! The files Odoo runs of a module. Loading a module imports `odoo.addons.<name>`, which runs its
//! root `__init__.py` and only what that imports (18.0 `odoo/modules/module.py:370-384`, 20.0
//! `:536-541`), and the tests are the submodules `tests` binds under a `test_` name (18.0
//! `odoo/tests/loader.py:57-67`, 20.0 `:56-66`). 9.0 does both under `openerp.addons`.

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, Stmt};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{Exports, FileImports, ImportFact, ImportGuard, Pos, Spanned};
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
        guard: ImportGuard::Unguarded,
        typing: false,
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
    guard: ImportGuard,
    /// Under `if TYPE_CHECKING:`, which never runs.
    typing: bool,
}

impl<'a> Visitor<'a> for Collect<'_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if self.guards(stmt) {
            return;
        }
        match stmt {
            Stmt::Import(_) | Stmt::ImportFrom(_) => self.import(stmt),
            Stmt::FunctionDef(f) => {
                self.function(f);
                return;
            }
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
            typing: self.typing,
            guard: self.guard,
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

    /// Visits a `try`, a `with` or an `if TYPE_CHECKING:`, guarding its body by what a failed
    /// import there may raise without stopping the module, and whether `stmt` was one.
    fn guards(&mut self, stmt: &'a Stmt) -> bool {
        match stmt {
            Stmt::Try(t) => self.guarded_try(t),
            Stmt::With(w) => {
                for item in &w.items {
                    self.visit_with_item(item);
                }
                self.guard(suppress_guard(w), &w.body);
            }
            Stmt::If(i) if type_checking(&i.test) => {
                self.visit_expr(&i.test);
                let outer = std::mem::replace(&mut self.typing, true);
                self.guard(ImportGuard::Any, &i.body);
                self.typing = outer;
                for clause in &i.elif_else_clauses {
                    self.visit_elif_else_clause(clause);
                }
            }
            _ => return false,
        }
        true
    }

    fn guarded_try(&mut self, t: &'a ast::StmtTry) {
        let kind = t.handlers.iter().map(handler_guard).max().unwrap_or_default();
        self.guard(kind, &t.body);
        for handler in &t.handlers {
            self.visit_except_handler(handler);
        }
        self.visit_body(&t.orelse);
        self.visit_body(&t.finalbody);
    }

    /// The body of a function runs when it is called, outside any `try` around the `def`, and
    /// never when the `def` sits under `if TYPE_CHECKING:`, which never defines it.
    fn function(&mut self, f: &'a ast::StmtFunctionDef) {
        for decorator in &f.decorator_list {
            self.visit_decorator(decorator);
        }
        self.visit_parameters(&f.parameters);
        let typed = if self.typing {
            ImportGuard::Any
        } else {
            ImportGuard::Unguarded
        };
        let outer = (self.deferred, self.guard);
        (self.deferred, self.guard) = (true, typed);
        self.visit_body(&f.body);
        (self.deferred, self.guard) = outer;
    }

    /// Visits `body` guarded by `kind` too, where the wider of the two guards wins.
    fn guard(&mut self, kind: ImportGuard, body: &'a [Stmt]) {
        let outer = self.guard;
        self.guard = outer.max(kind);
        self.visit_body(body);
        self.guard = outer;
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

/// Whether a handler catches what some failed import raises.
pub(crate) fn catches_import(handler: &ast::ExceptHandler) -> bool {
    handler_guard(handler) != ImportGuard::Unguarded
}

/// What a handler catches of a failed import. `ModuleNotFoundError` is the subclass of
/// `ImportError` a missing module raises, so a handler naming only it lets a missing name through.
fn handler_guard(handler: &ast::ExceptHandler) -> ImportGuard {
    let ast::ExceptHandler::ExceptHandler(h) = handler;
    if python::catches(h, "ImportError") {
        ImportGuard::Any
    } else if python::catches(h, "ModuleNotFoundError") {
        ImportGuard::MissingModule
    } else {
        ImportGuard::Unguarded
    }
}

/// What the `suppress()` calls of a `with` may swallow of a failed import.
fn suppress_guard(w: &ast::StmtWith) -> ImportGuard {
    let items = w.items.iter();
    let calls = items.filter_map(|i| method_body::suppress_call(&i.context_expr));
    let args = calls.flat_map(|c| c.arguments.args.iter());
    args.map(arg_guard).max().unwrap_or_default()
}

/// What a `suppress()` argument may swallow of a failed import. Only another exception class by
/// its name is known to swallow none: any other name, such as `OPTIONAL_ERRORS`, may hold a tuple
/// of classes, and a starred argument or a call any class.
fn arg_guard(arg: &Expr) -> ImportGuard {
    match arg {
        Expr::Tuple(types) => types.iter().map(arg_guard).max().unwrap_or_default(),
        _ if python::names_error(arg, "ImportError") => ImportGuard::Any,
        _ if python::names_error(arg, "ModuleNotFoundError") => ImportGuard::MissingModule,
        _ if tail_name(arg).is_none_or(|name| !exception_class(name)) => ImportGuard::Any,
        _ => ImportGuard::Unguarded,
    }
}

/// Whether `name` reads as an exception class: by the suffix Python and Odoo give them, or as one
/// of the built-in ones without it.
fn exception_class(name: &str) -> bool {
    ["Error", "Exception", "Warning"].iter().any(|s| name.ends_with(s))
        || [
            "GeneratorExit",
            "KeyboardInterrupt",
            "StopAsyncIteration",
            "StopIteration",
            "SystemExit",
        ]
        .contains(&name)
}

/// `TYPE_CHECKING` or `typing.TYPE_CHECKING`, true only for a type checker.
fn type_checking(test: &Expr) -> bool {
    tail_name(test) == Some("TYPE_CHECKING")
}

/// What the file binds to `__all__` at import, unread when `unmodelled` holds.
fn exports(body: &[Stmt]) -> Exports {
    if unmodelled(body) {
        return Exports::Unread;
    }
    block(Exports::Absent, body, false)
}

/// Whether the file binds `__all__` where `block` does not read it: a statement `module_state::binds`
/// sees binding it other than as an `Assign`, `AnnAssign`, `AugAssign` or `For` target, or code
/// `module_state::binds_unseen` finds. The whole file counts, since a function that binds it may
/// run at any later point.
fn unmodelled(body: &[Stmt]) -> bool {
    let folded = |stmt: &Stmt| {
        matches!(
            stmt,
            Stmt::Assign(_) | Stmt::AnnAssign(_) | Stmt::AugAssign(_) | Stmt::For(_)
        )
    };
    module_state::binds_unseen(body, "__all__")
        || module_state::bound(body)
            .into_iter()
            .any(|stmt| !folded(stmt) && module_state::binds(stmt, "__all__"))
}

/// `__all__` after `body` runs, where `may_skip` says some of it may not run, so that an
/// assignment there may leave the earlier list in place.
fn block(exports: Exports, body: &[Stmt], may_skip: bool) -> Exports {
    body.iter()
        .fold(exports, |exports, stmt| statement(exports, stmt, may_skip))
}

/// `__all__` after `stmt`: an `if`, or a `try` with an `except` clause, may not run a replacement,
/// while a `with` body and the body of a `try` with only `finally` run as their block does.
fn statement(exports: Exports, stmt: &Stmt, may_skip: bool) -> Exports {
    match stmt {
        Stmt::If(i) => branched(exports, i),
        Stmt::Try(t) => tried(exports, t, may_skip),
        Stmt::With(w) => block(exports, &w.body, may_skip),
        Stmt::For(_) | Stmt::While(_) | Stmt::Match(_) => module_state::bound(std::slice::from_ref(stmt))
            .into_iter()
            .fold(exports, looped),
        _ if may_skip => looped(exports, stmt),
        Stmt::Expr(e) => grown(exports, &e.value),
        _ => bound(exports, stmt),
    }
}

/// `__all__` after an `if`, where any one clause may run, or none without an `else`.
fn branched(exports: Exports, i: &ast::StmtIf) -> Exports {
    let clauses = i.elif_else_clauses.iter().map(|c| &c.body[..]);
    std::iter::once(&i.body[..])
        .chain(clauses)
        .fold(exports, |exports, body| block(exports, body, true))
}

/// `__all__` after a `try`, whose body may stop at any statement a handler catches and whose
/// handlers and `else` may not run. With no handler, a body that stops fails the import, so the
/// body and `finally` both read as at the `try`.
fn tried(exports: Exports, t: &ast::StmtTry, may_skip: bool) -> Exports {
    let ran = block(exports, &t.body, may_skip || !t.handlers.is_empty());
    let handlers = t.handlers.iter().map(|h| {
        let ast::ExceptHandler::ExceptHandler(h) = h;
        &h.body[..]
    });
    let skipped = handlers
        .chain(std::iter::once(&t.orelse[..]))
        .fold(ran, |exports, body| block(exports, body, true));
    block(skipped, &t.finalbody, may_skip)
}

/// `__all__` after `stmt` in a loop or `match` case, which may run any number of times or none,
/// so an assignment may leave the earlier list in place.
fn looped(exports: Exports, stmt: &Stmt) -> Exports {
    match stmt {
        Stmt::Expr(e) => grown(exports, &e.value),
        Stmt::AugAssign(_) => bound(exports, stmt),
        Stmt::Assign(a) if a.targets.iter().any(is_all) => assigned_maybe(exports, stmt),
        Stmt::AnnAssign(a) if is_all(&a.target) => assigned_maybe(exports, stmt),
        Stmt::For(f) if is_all(&f.target) => Exports::Unread,
        _ => exports,
    }
}

/// `__all__` after an assignment that may not run. With no earlier list, a star import takes
/// either no submodule or those the assignment lists, so the list holds every name it may. A list
/// bound where the fold does not see it is caught before, by `unmodelled`.
fn assigned_maybe(exports: Exports, stmt: &Stmt) -> Exports {
    match exports {
        Exports::Absent => bound(Exports::Absent, stmt),
        _ => Exports::Unread,
    }
}

/// `__all__` after `stmt`, when it assigns it or extends it with `+=`.
fn bound(exports: Exports, stmt: &Stmt) -> Exports {
    let (targets, value, extends) = match stmt {
        Stmt::Assign(a) => (a.targets.iter().collect::<Vec<_>>(), Some(&*a.value), false),
        Stmt::AnnAssign(a) => (vec![&*a.target], a.value.as_deref(), false),
        Stmt::AugAssign(a) => (vec![&*a.target], Some(&*a.value), true),
        _ => return exports,
    };
    if !targets.iter().any(|t| is_all(t)) {
        return exports;
    }
    let names = value.filter(|_| targets.len() == 1).and_then(literal_names);
    match (exports, names) {
        (Exports::Literal(mut all), Some(more)) if extends => {
            all.extend(more);
            Exports::Literal(all)
        }
        (_, Some(names)) if !extends => Exports::Literal(names),
        _ => Exports::Unread,
    }
}

/// `__all__` after `expr` runs, grown by every call it may make. A call that may not run, in a
/// comprehension, a branch of a conditional or `and`/`or`, or a lambda body, still counts: an extra
/// name only widens what a star import reaches, which can hide a finding but never invent one.
fn grown(exports: Exports, expr: &Expr) -> Exports {
    let mut calls = python::Calls::default();
    calls.visit_expr(expr);
    calls.calls.into_iter().fold(exports, grown_by)
}

/// `__all__` after `call`. `append` of a string and `extend` of literals grow a read list; any
/// other method of `__all__` may change it in any way.
fn grown_by(exports: Exports, call: &ast::ExprCall) -> Exports {
    let Expr::Attribute(method) = &*call.func else {
        return exports;
    };
    if !matches!(&*method.value, Expr::Name(n) if n.id.as_str() == "__all__") {
        return exports;
    }
    let more = match call.arguments.args.as_ref() {
        [only] if call.arguments.keywords.is_empty() => match method.attr.as_str() {
            "append" => str_lit(only).map(|name| vec![name.to_owned()]),
            "extend" => literal_names(only),
            _ => None,
        },
        _ => None,
    };
    match (exports, more) {
        (Exports::Literal(mut all), Some(more)) => {
            all.extend(more);
            Exports::Literal(all)
        }
        _ => Exports::Unread,
    }
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
    fn unread(&self) -> bool {
        self.lost || self.files.values().any(|f| f.as_ref().is_none_or(|i| i.dynamic))
    }

    /// The names module `path` binds, an empty list for a namespace package, and none when the
    /// scan cannot list them.
    fn defines(&self, path: &[String]) -> Option<&[String]> {
        let at = path.join("/");
        let file = if at.is_empty() {
            ROOT.to_owned()
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

impl Fault {
    /// Whether `guard` catches what the import raises: a missing module raises
    /// `ModuleNotFoundError`, and a name `from` finds neither bound nor a submodule plain
    /// `ImportError`, as `_handle_fromlist` swallows the failed submodule import first.
    pub fn guarded_by(&self, guard: ImportGuard) -> bool {
        match self {
            Self::Missing => guard >= ImportGuard::MissingModule,
            Self::Undefined(_) => guard == ImportGuard::Any,
        }
    }
}

/// The file Odoo imports a module by.
pub(crate) const ROOT: &str = "__init__.py";

/// Where a file first runs while its addon loads: the imports on the way from the root, each as
/// the importing statement and the file's place among those it runs.
pub type RunKey = Vec<(Pos, usize)>;

/// The imports of every module of the scan.
#[derive(Debug, Default)]
pub struct Graph {
    addons: FxHashMap<String, Addon>,
    /// The files of each addon that a file of another addon imports, each with that other addon.
    elsewhere: FxHashMap<String, FxHashSet<(String, String)>>,
    /// The addons with a file the scan cannot read the imports of, which may import any file of
    /// any addon.
    unread: FxHashSet<String>,
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
        let unread = addons
            .iter()
            .filter(|(_, addon)| addon.unread())
            .map(|(name, _)| name.clone());
        let mut graph = Self {
            unread: unread.collect(),
            elsewhere: FxHashMap::default(),
            addons,
        };
        let mut elsewhere: FxHashMap<String, FxHashSet<(String, String)>> = FxHashMap::default();
        for (name, addon) in &graph.addons {
            for (rel, imports) in &addon.files {
                let list = imports.iter().flat_map(|i| &i.list);
                for (target, file) in list.flat_map(|fact| graph.targets(name, rel, fact)) {
                    if target != *name {
                        elsewhere.entry(target).or_default().insert((file, name.clone()));
                    }
                }
            }
        }
        graph.elsewhere = elsewhere;
        graph
    }

    /// The files of `addon` that a file of another addon imports, as (file, importing addon).
    pub fn elsewhere(&self, addon: &str) -> impl Iterator<Item = (&str, &str)> {
        let pairs = self.elsewhere.get(addon).into_iter().flatten();
        pairs.map(|(file, importer)| (file.as_str(), importer.as_str()))
    }

    /// Whether a file of `addon` may import a file of another addon that `elsewhere` lacks: one
    /// that does not decode or parse, or imports by a computed name.
    pub fn unread(&self, addon: &str) -> bool {
        self.unread.contains(addon)
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

    /// When loading `addon` first runs each file its root `__init__.py` reaches, as keys that sort
    /// in that order: Python runs a module's imports depth first, each once, an import in a
    /// function only when called and one under `if TYPE_CHECKING:` never. None where `reached` or
    /// `run_from` is none, and for an addon the scan lost or does not know.
    pub fn run_order(&self, addon: &str) -> Option<FxHashMap<String, RunKey>> {
        let files = &self.addons.get(addon).filter(|a| !a.lost)?.files;
        self.reached(addon, &[ROOT])?;
        let mut order = FxHashMap::default();
        if files.contains_key(ROOT) {
            order.insert(ROOT.to_owned(), vec![]);
            self.run_from(addon, ROOT, &mut order)?;
        }
        Some(order)
    }

    /// Gives each file `rel` of `addon` runs first the key of `rel` and where the import runs it.
    /// None past a star import of a package whose `__all__` the scan does not read, as `star` then
    /// runs every submodule in an order Python need not follow.
    fn run_from(&self, addon: &str, rel: &str, order: &mut FxHashMap<String, RunKey>) -> Option<()> {
        let Some(Some(imports)) = self.addons[addon].files.get(rel) else {
            return Some(());
        };
        let mut facts: Vec<&ImportFact> = imports.list.iter().filter(|f| !f.deferred && !f.typing).collect();
        facts.sort_by_key(|f| f.pos);
        for fact in facts {
            if self.stars_unread(addon, rel, fact) {
                return None;
            }
            let targets = self.targets(addon, rel, fact).into_iter().enumerate();
            for (i, (_, file)) in targets.filter(|(_, (to, _))| to == addon) {
                if order.contains_key(&file) {
                    continue;
                }
                let mut key = order[rel].clone();
                key.push((fact.pos, i));
                order.insert(file.clone(), key);
                self.run_from(addon, &file, order)?;
            }
        }
        Some(())
    }

    /// Whether `fact` of file `rel` star-imports a package of `addon` whose `__all__` the scan
    /// does not read.
    fn stars_unread(&self, addon: &str, rel: &str, fact: &ImportFact) -> bool {
        if !fact.from || !fact.names.iter().any(|n| n.value == "*") {
            return false;
        }
        match self.place(addon, rel, fact.level, fact.module.as_deref()) {
            Some(Place::In(to, path)) => to == addon && *self.package_exports(&to, &path) == Exports::Unread,
            _ => false,
        }
    }

    /// The modules of addons `fact` of file `rel` imports by their path, each with what the addon
    /// lacks of it. The scan reads no fault where the addon may hold anything: outside the graph,
    /// with a file that does not decode or parse on the way, under `base.maintenance`, which Odoo
    /// maps to `odoo.upgrade`, or under a `tests/` it lacks, as a fetched core drops it.
    pub fn addon_imports(&self, rel: &str, fact: &ImportFact) -> Vec<AddonImport> {
        let named = named_modules(rel, fact).into_iter();
        named.map(|n| self.addon_import(n)).collect()
    }

    fn addon_import(&self, named: Named) -> AddonImport {
        AddonImport {
            faults: self.faults(&named.addon, &named.path, named.names),
            addon: named.addon,
            text: named.text,
            pos: named.pos,
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
            .extend(files.files.get_key_value(ROOT).map(|(k, _)| k.clone()));
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

    /// What the `__init__.py` of package `path` of `addon` binds to `__all__`, unread when it does
    /// not parse.
    fn package_exports(&self, addon: &str, path: &[String]) -> &Exports {
        let dir = path.join("/");
        let init = if dir.is_empty() {
            ROOT.to_owned()
        } else {
            format!("{dir}/__init__.py")
        };
        match self.addons.get(addon).and_then(|files| files.files.get(&init)) {
            Some(Some(imports)) => &imports.exports,
            Some(None) => &Exports::Unread,
            None => &Exports::Absent,
        }
    }

    /// The submodules a star import of package `path` of `addon` runs: those its `__all__` lists,
    /// none without one, and every one when the scan does not read it.
    fn star(&self, addon: &str, path: &[String]) -> Vec<String> {
        let Some(files) = self.addons.get(addon) else {
            return vec![];
        };
        let dir = path.join("/");
        match self.package_exports(addon, path) {
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

/// The addons `fact` of file `rel` imports a module of by its path, its own included.
pub fn imported_addons(rel: &str, fact: &ImportFact) -> Vec<String> {
    named_modules(rel, fact).into_iter().map(|n| n.addon).collect()
}

/// A module of an addon an import names by its path, with the names a `from` imports of it.
struct Named<'f> {
    addon: String,
    path: Vec<String>,
    /// The module as the import writes it, its leading dots included.
    text: String,
    pos: Pos,
    names: &'f [Spanned<String>],
}

/// The modules of addons `fact` of file `rel` names by their path.
fn named_modules<'f>(rel: &str, fact: &'f ImportFact) -> Vec<Named<'f>> {
    if !fact.from {
        return fact.names.iter().filter_map(|name| plain_named(rel, name)).collect();
    }
    let module = fact.module.as_deref();
    let written = format!("{}{}", ".".repeat(fact.level as usize), module.unwrap_or_default());
    match addon_place(rel, fact.level, module) {
        Some(Place::In(addon, path)) => vec![Named {
            addon,
            path,
            text: written,
            pos: fact.pos,
            names: &fact.names,
        }],
        Some(Place::Addons) => {
            let sep = if module.is_some() { "." } else { "" };
            let names = fact.names.iter().filter(|n| n.value != "*");
            let named = names.map(|name| Named {
                addon: name.value.clone(),
                path: vec![],
                text: format!("{written}{sep}{}", name.value),
                pos: name.pos,
                names: &[],
            });
            named.collect()
        }
        None => vec![],
    }
}

/// The module of an addon a plain `import` of `name` in file `rel` names, if any.
fn plain_named<'f>(rel: &str, name: &Spanned<String>) -> Option<Named<'f>> {
    let Some(Place::In(addon, path)) = addon_place(rel, 0, Some(&name.value)) else {
        return None;
    };
    Some(Named {
        addon,
        path,
        text: name.value.clone(),
        pos: name.pos,
        names: &[],
    })
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
        let flags: Vec<(String, bool, ImportGuard)> = imports(src)
            .list
            .into_iter()
            .map(|f| (f.names[0].value.clone(), f.deferred, f.guard))
            .collect();
        let (no, module, any) = (ImportGuard::Unguarded, ImportGuard::MissingModule, ImportGuard::Any);
        let expected = [
            ("a", false, no),
            ("b", false, any),
            ("c", true, no),
            ("d", false, no),
            ("e", false, no),
            ("g", false, module),
            ("h", false, no),
            ("i", false, any),
            ("j", false, any),
            ("k", false, no),
            ("l", true, any),
            ("o", true, no),
        ];
        let expected: Vec<_> = expected.iter().map(|(n, d, g)| ((*n).to_owned(), *d, *g)).collect();
        assert_eq!(flags, expected);
    }

    /// The guard of the first import of `src`.
    fn guarded(src: &str) -> ImportGuard {
        imports(src).list[0].guard
    }

    #[test]
    fn a_suppress_guards_only_when_it_may_swallow_an_import_error() {
        let (no, module, any) = (ImportGuard::Unguarded, ImportGuard::MissingModule, ImportGuard::Any);
        for (args, expected) in [
            ("KeyError", no),
            ("()", no),
            ("", no),
            ("ImportError", any),
            ("KeyError, ModuleNotFoundError", module),
            ("(KeyError, ImportError)", any),
            ("ModuleNotFoundError, ImportError", any),
            ("Exception", any),
            ("builtins.BaseException", any),
            ("*errors", any),
            ("UserError, binascii.Error, KeyboardInterrupt", no),
            ("OPTIONAL_ERRORS", any),
            ("(KeyError, self.errors)", any),
            ("errors()", any),
            ("ModuleNotFoundError, errors()", any),
        ] {
            let src = format!("with suppress({args}):\n    import a\n");
            assert_eq!(guarded(&src), expected, "{args}");
        }
        let among = "with open(p) as f, contextlib.suppress(ImportError):\n    import a\n";
        assert_eq!(guarded(among), any);
    }

    #[test]
    fn a_try_guards_by_its_widest_handler() {
        let (no, module, any) = (ImportGuard::Unguarded, ImportGuard::MissingModule, ImportGuard::Any);
        for (handlers, expected) in [
            ("except KeyError:\n    pass\n", no),
            ("except ModuleNotFoundError:\n    pass\n", module),
            (
                "except KeyError:\n    pass\nexcept ModuleNotFoundError:\n    pass\n",
                module,
            ),
            (
                "except ModuleNotFoundError:\n    pass\nexcept ImportError:\n    pass\n",
                any,
            ),
            ("except (ModuleNotFoundError, ImportError):\n    pass\n", any),
            ("except (ModuleNotFoundError, KeyError):\n    pass\n", module),
            ("except:\n    pass\n", any),
            ("except Exception:\n    pass\n", any),
            ("except BaseException:\n    pass\n", any),
            ("finally:\n    pass\n", no),
        ] {
            let src = format!("try:\n    import a\n{handlers}");
            assert_eq!(guarded(&src), expected, "{handlers}");
        }
    }

    #[test]
    fn the_wider_of_nested_guards_wins() {
        let (module, any) = (ImportGuard::MissingModule, ImportGuard::Any);
        let narrow_in_wide =
            "try:\n    with suppress(ModuleNotFoundError):\n        import a\nexcept ImportError:\n    pass\n";
        assert_eq!(guarded(narrow_in_wide), any);
        let wide_in_narrow = "try:\n    with suppress(ImportError):\n        import a\n    import b\nexcept ModuleNotFoundError:\n    pass\n";
        let guards: Vec<_> = imports(wide_in_narrow).list.iter().map(|f| f.guard).collect();
        assert_eq!(guards, [any, module]);
        let unguarded_in_narrow =
            "with suppress(ModuleNotFoundError):\n    try:\n        import a\n    except KeyError:\n        pass\n";
        assert_eq!(guarded(unguarded_in_narrow), module);
    }

    #[test]
    fn a_missing_module_is_guarded_by_any_guard_and_a_missing_name_only_by_a_full_one() {
        let name = Fault::Undefined(Spanned {
            value: "x".to_owned(),
            pos: Pos::default(),
        });
        for (guard, missing, undefined) in [
            (ImportGuard::Unguarded, false, false),
            (ImportGuard::MissingModule, true, false),
            (ImportGuard::Any, true, true),
        ] {
            assert_eq!(Fault::Missing.guarded_by(guard), missing, "{guard:?}");
            assert_eq!(name.guarded_by(guard), undefined, "{guard:?}");
        }
    }

    #[test]
    fn a_def_under_type_checking_is_guarded_and_one_in_its_else_is_not() {
        let src = "if TYPE_CHECKING:\n    def f():\n        import a\n    class K:\n        def m(self):\n            import b\n\
                   else:\n    def g():\n        import c\n";
        let flags: Vec<(bool, ImportGuard)> = imports(src).list.iter().map(|f| (f.deferred, f.guard)).collect();
        let (no, any) = (ImportGuard::Unguarded, ImportGuard::Any);
        assert_eq!(flags, [(true, any), (true, any), (true, no)]);
    }

    #[test]
    fn only_an_import_the_type_checking_branch_holds_is_typing() {
        for (src, expected) in [
            ("if typing.TYPE_CHECKING:\n    import a\n", true),
            (
                "if TYPE_CHECKING:\n    if x:\n        try:\n            import a\n        except KeyError:\n            pass\n",
                true,
            ),
            ("if TYPE_CHECKING:\n    def f():\n        import a\n", true),
            ("if TYPE_CHECKING:\n    pass\nelse:\n    import a\n", false),
            ("if TYPE_CHECKING:\n    pass\nelif x:\n    import a\n", false),
            ("if not TYPE_CHECKING:\n    import a\n", false),
        ] {
            assert_eq!(imports(src).list[0].typing, expected, "{src}");
        }
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
    fn all_grows_by_a_literal_append_or_extend() {
        let read = |src: &str| imports(src).exports;
        assert_eq!(
            read("__all__ = ['a']\n__all__.append('b')\n__all__.extend(['c', 'd'])\n__all__.extend(('e',))\n"),
            Exports::Literal(owned(&["a", "b", "c", "d", "e"]))
        );
        assert_eq!(
            read("__all__ = ['a']\nif x:\n    __all__.append('b')\n"),
            Exports::Literal(owned(&["a", "b"]))
        );
        assert_eq!(read("names.append('b')\n"), Exports::Absent);
        for unread in [
            "__all__.append('a')\n",
            "__all__ = names\n__all__.append('a')\n",
            "__all__ = ['a']\n__all__.append(name)\n",
            "__all__ = ['a']\n__all__.extend(names)\n",
            "__all__ = ['a']\n__all__.extend(['b', c])\n",
            "__all__ = ['a']\n__all__.insert(0, 'b')\n",
            "__all__ = ['a']\n__all__.remove('a')\n",
            "__all__ = ['a']\n__all__.append('b', 'c')\n",
        ] {
            assert_eq!(read(unread), Exports::Unread, "{unread}");
        }
    }

    #[test]
    fn all_grown_in_a_loop_or_case_is_read() {
        let read = |src: &str| imports(src).exports;
        for place in [
            "for _ in x:\n        ",
            "for _ in x:\n    pass\nelse:\n        ",
            "while x:\n        ",
            "for _ in x:\n    if y:\n        ",
            "match x:\n    case 1:\n        ",
        ] {
            for grows in ["__all__.append('b')", "__all__.extend(['b'])", "__all__ += ['b']"] {
                let src = format!("__all__ = ['a']\n{place}{grows}\n");
                assert_eq!(read(&src), Exports::Literal(owned(&["a", "b"])), "{src}");
            }
        }
        assert_eq!(
            read("__all__ = ['a']\nfor _ in x:\n    def f():\n        __all__.append('b')\n"),
            Exports::Literal(owned(&["a"]))
        );
        for unread in [
            "for n in ('a',):\n    __all__.append(n)\n",
            "for _ in x:\n    __all__ = ['b']\n",
            "for _ in x:\n    __all__: list = ['b']\n",
            "match x:\n    case 1:\n        __all__ = ['b']\n",
            "for __all__ in x:\n    pass\n",
            "while x:\n    __all__.remove('a')\n",
        ] {
            let src = format!("__all__ = ['a']\n{unread}");
            assert_eq!(read(&src), Exports::Unread, "{unread}");
        }
    }

    #[test]
    fn all_grown_inside_an_expression_is_read() {
        let read = |src: &str| imports(src).exports;
        for grows in [
            "[__all__.append('b') for _ in x]",
            "{__all__.append('b') for _ in x}",
            "list(__all__.append('b') for _ in x)",
            "__all__.append('b') if x else None",
            "x and __all__.extend(['b'])",
            "x or __all__.append('b')",
            "if x:\n    [__all__.append('b') for _ in x]",
            "(lambda: __all__.append('b'))()",
            "for y in x:\n    [__all__.append('b') for _ in y]",
        ] {
            let src = format!("__all__ = ['a']\n{grows}\n");
            assert_eq!(read(&src), Exports::Literal(owned(&["a", "b"])), "{src}");
        }
        for unread in [
            "[__all__.append(n) for n in names]",
            "x and __all__.insert(0, 'b')",
            "print(__all__.remove('a'))",
        ] {
            let src = format!("__all__ = ['a']\n{unread}\n");
            assert_eq!(read(&src), Exports::Unread, "{src}");
        }
        assert_eq!(
            read("__all__ = ['a']\n[n.append('b') for n in x]\n"),
            Exports::Literal(owned(&["a"]))
        );
    }

    #[test]
    fn all_assigned_in_a_branch_is_unread() {
        let read = |src: &str| imports(src).exports;
        let caught = "except KeyError:\n    pass\n";
        for (place, rest) in [
            ("if x:\n    ", ""),
            ("if x:\n    pass\nelif y:\n    ", ""),
            ("if x:\n    pass\nelse:\n    ", ""),
            ("try:\n    ", caught),
            ("try:\n    pass\nexcept KeyError:\n    ", ""),
            ("try:\n    pass\nexcept KeyError:\n    pass\nelse:\n    ", ""),
            ("if x:\n    with y:\n        ", ""),
            ("with y:\n    if x:\n        ", ""),
            ("if x:\n    try:\n        pass\n    finally:\n        ", ""),
        ] {
            for assigns in ["__all__ = ['b']", "__all__: list = ['b']"] {
                let src = format!("__all__ = ['a']\n{place}{assigns}\n{rest}");
                assert_eq!(read(&src), Exports::Unread, "{src}");
            }
            for grows in ["__all__.append('b')", "__all__.extend(['b'])", "__all__ += ['b']"] {
                let src = format!("__all__ = ['a']\n{place}{grows}\n{rest}");
                assert_eq!(read(&src), Exports::Literal(owned(&["a", "b"])), "{src}");
            }
        }
        for place in ["try:\n    pass\nfinally:\n    ", "with y:\n    "] {
            let src = format!("__all__ = ['a']\n{place}__all__ = ['b']\n");
            assert_eq!(read(&src), Exports::Literal(owned(&["b"])), "{src}");
        }
        assert_eq!(
            read("__all__ = ['a']\nif x:\n    __all__ = ['b']\n__all__ = ['c']\n"),
            Exports::Literal(owned(&["c"]))
        );
    }

    #[test]
    fn all_assigned_maybe_with_no_list_is_the_list() {
        let read = |src: &str| imports(src).exports;
        let caught = "except KeyError:\n    pass\n";
        for (place, rest) in [
            ("if x:\n    ", ""),
            ("if x:\n    pass\nelif y:\n    ", ""),
            ("if x:\n    pass\nelse:\n    ", ""),
            ("try:\n    ", caught),
            ("try:\n    pass\nexcept KeyError:\n    ", ""),
            ("try:\n    pass\nexcept KeyError:\n    pass\nelse:\n    ", ""),
            ("for _ in x:\n    ", ""),
            ("match x:\n    case 1:\n        ", ""),
            ("if x:\n    try:\n        pass\n    finally:\n        ", ""),
        ] {
            for assigns in ["__all__ = ['b']", "__all__: list = ['b']"] {
                let src = format!("{place}{assigns}\n{rest}");
                assert_eq!(read(&src), Exports::Literal(owned(&["b"])), "{src}");
            }
            let src = format!("{place}__all__ = names\n{rest}");
            assert_eq!(read(&src), Exports::Unread, "{src}");
        }
    }

    #[test]
    fn all_bound_unseen_is_unread() {
        let read = |src: &str| imports(src).exports;
        for bound in [
            "from .m import __all__\n",
            "def f():\n    global __all__\n    __all__ = ['a']\nf()\n",
            "(__all__ := ['a'])\n",
            "@register(__all__ := ['a'])\ndef f():\n    pass\n",
            "def f(x=(__all__ := ['a'])):\n    pass\n",
            "globals()['__all__'] = ['a']\n",
            "try:\n    pass\nexcept E as __all__:\n    pass\n",
            "match x:\n    case [*__all__]:\n        pass\n",
            "class __all__:\n    pass\n",
            "type __all__ = list[str]\n",
        ] {
            assert_eq!(read(bound), Exports::Unread, "{bound}");
            let src = format!("{bound}if x:\n    __all__ = ['b']\n");
            assert_eq!(read(&src), Exports::Unread, "{src}");
        }
        for src in [
            "import pkg as __all__\n",
            "with f() as __all__:\n    pass\n",
            "def __all__():\n    pass\n",
            "del __all__\n",
            "[(__all__ := ['a']) for _ in x]\n",
            "sys.modules[__name__].__all__ = ['a']\n",
            "setattr(sys.modules[__name__], '__all__', ['a'])\n",
            "globals().update(__all__=['a'])\n",
        ] {
            assert_eq!(read(src), Exports::Unread, "{src}");
        }
        for src in [
            "x = eval(s)\n",
            "globals().update(d)\n",
            "def f():\n    __all__ = ['a']\n",
            "def f():\n    (__all__ := ['a'])\n",
        ] {
            assert_eq!(read(src), Exports::Absent, "{src}");
        }
        for src in ["__all__ = ['a']\nx = eval(s)\n", "from x import *\n__all__ = ['a']\n"] {
            assert_eq!(read(src), Exports::Literal(owned(&["a"])), "{src}");
        }
    }

    #[test]
    fn a_try_with_only_finally_runs_its_body() {
        let read = |src: &str| imports(src).exports;
        let only_finally = "try:\n    __all__ = ['b']\nfinally:\n    pass\n";
        assert_eq!(
            read(&format!("__all__ = ['a']\n{only_finally}")),
            Exports::Literal(owned(&["b"]))
        );
        let nested = only_finally.replace('\n', "\n    ");
        for (head, tail) in [("if x:\n    ", ""), ("try:\n    ", "except KeyError:\n    pass\n")] {
            let src = format!("__all__ = ['a']\n{head}{}\n{tail}", nested.trim_end());
            assert_eq!(read(&src), Exports::Unread, "{src}");
        }
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
                .map(|file| (file, "shop"))
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
    fn an_addon_is_unread_with_a_file_that_is_lost_unparsed_or_dynamic() {
        let mut addons: FxHashMap<String, Addon> = [
            ("clean", Some("from . import models\n")),
            ("unparsed", None),
            ("dynamic", Some("import importlib\nimportlib.import_module(name)\n")),
            ("lost", Some("")),
        ]
        .into_iter()
        .map(|(name, src)| {
            let files = [("__init__.py".to_owned(), src.map(imports))].into_iter();
            (name.to_owned(), addon(files, Series::parse("18.0")))
        })
        .collect();
        addons.get_mut("lost").unwrap().lost = true;
        let g = Graph::of(addons);
        let unread: Vec<_> = ["clean", "unparsed", "dynamic", "lost", "gone"]
            .into_iter()
            .map(|name| g.unread(name))
            .collect();
        assert_eq!(unread, [false, true, true, true, false]);
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
    fn an_import_by_path_names_its_addons() {
        let src = "from odoo.addons.lib.models import Book\nfrom odoo.addons import a, b\n\
                   import odoo.addons.shop.models, os\nfrom ...far.util import x\nfrom .. import near\n";
        let named: Vec<Vec<String>> = imports(src)
            .list
            .iter()
            .map(|f| imported_addons("models/x.py", f))
            .collect();
        let owned = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
        let rows = [
            owned(&["lib"]),
            owned(&["a", "b"]),
            owned(&["shop"]),
            owned(&["far"]),
            owned(&[]),
        ];
        assert_eq!(named, rows);
        assert!(
            imports("import os\n")
                .list
                .iter()
                .all(|f| imported_addons("x.py", f).is_empty())
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

    /// The files `addon` runs, in the order of their keys.
    fn run_order(graph: &Graph, addon: &str) -> Option<Vec<String>> {
        let mut order: Vec<_> = graph.run_order(addon)?.into_iter().collect();
        order.sort_by(|a, b| a.1.cmp(&b.1));
        Some(order.into_iter().map(|(file, _)| file).collect())
    }

    #[test]
    fn the_run_order_is_depth_first_by_import_and_skips_function_imports() {
        let g = graph(&[(
            "shop",
            false,
            &[
                ("__init__.py", Some("from .models import sale\nfrom . import b, a\n")),
                ("models/__init__.py", Some("from . import stock\n")),
                ("models/sale.py", Some("from . import tax\n")),
                ("models/stock.py", Some("")),
                ("models/tax.py", Some("")),
                ("a.py", Some("def load():\n    from . import late\n")),
                ("b.py", Some("from . import a\n")),
                ("late.py", Some("")),
            ],
        )]);
        assert_eq!(
            run_order(&g, "shop").unwrap(),
            owned(&[
                "__init__.py",
                "models/__init__.py",
                "models/stock.py",
                "models/sale.py",
                "models/tax.py",
                "b.py",
                "a.py"
            ])
        );
    }

    #[test]
    fn the_run_order_skips_type_checking_imports_but_not_guarded_ones() {
        let g = graph(&[(
            "shop",
            false,
            &[
                ("__init__.py", Some("from . import a, c, b\n")),
                (
                    "a.py",
                    Some("from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from . import b\n"),
                ),
                ("b.py", Some("")),
                (
                    "c.py",
                    Some("try:\n    from . import d\nexcept ImportError:\n    pass\n"),
                ),
                ("d.py", Some("")),
            ],
        )]);
        assert_eq!(
            run_order(&g, "shop").unwrap(),
            owned(&["__init__.py", "a.py", "c.py", "d.py", "b.py"])
        );
    }

    #[test]
    fn the_run_order_is_unknown_past_an_unparsed_or_lost_file() {
        let files: &[(&str, Option<&str>)] = &[("__init__.py", Some("from . import a\n")), ("a.py", None)];
        assert!(graph(&[("shop", false, files)]).run_order("shop").is_none());
        let mut g = graph(&[("shop", false, &[("__init__.py", Some(""))])]);
        assert_eq!(run_order(&g, "shop").unwrap(), owned(&["__init__.py"]));
        g.addons.get_mut("shop").unwrap().lost = true;
        assert!(g.run_order("shop").is_none());
        assert!(g.run_order("gone").is_none());
    }

    #[test]
    fn the_run_order_is_unknown_past_a_star_import_of_an_unread_all() {
        let files = |base: &'static str| {
            vec![
                ("__init__.py", Some("from .pkg import *\n")),
                ("pkg/__init__.py", Some(base)),
                ("pkg/base.py", Some("__all__ = ['z_parent', 'a_child']\n")),
                ("pkg/z_parent.py", Some("")),
                ("pkg/a_child.py", Some("")),
            ]
        };
        let listed = graph(&[("shop", false, files("__all__ = ['z_parent', 'a_child']\n").as_slice())]);
        assert_eq!(
            run_order(&listed, "shop").unwrap(),
            owned(&["__init__.py", "pkg/__init__.py", "pkg/z_parent.py", "pkg/a_child.py"])
        );
        let unread = graph(&[("shop", false, files("from .base import __all__\n").as_slice())]);
        assert!(unread.run_order("shop").is_none());
        assert!(unread.reached("shop", &[ROOT]).is_some());
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
