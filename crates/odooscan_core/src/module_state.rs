//! State a Python file sets up at import, which Odoo shares between every database the process
//! serves: patches on what it imports from `odoo` (OD123), and containers that functions fill at
//! runtime (OD124). Also what it binds to `migrate`, which an upgrade script must define (OD515).

use ruff_python_ast::helpers::any_over_expr;
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, Stmt};

use crate::facts::{ImportPatch, Migrate, RawDiag};
use crate::python::{self, Locator, str_lit, tail_name};

/// Calls that build a dict, a list or a set.
const CONTAINER_CALLS: &[&str] = &["dict", "list", "set", "defaultdict", "OrderedDict"];

/// Methods that change a dict, a list or a set in place.
pub(crate) const MUTATORS: &[&str] = &[
    "add",
    "append",
    "clear",
    "difference_update",
    "discard",
    "extend",
    "insert",
    "intersection_update",
    "pop",
    "popitem",
    "remove",
    "reverse",
    "setdefault",
    "sort",
    "symmetric_difference_update",
    "update",
    "__setitem__",
    "__delitem__",
];

/// Packages of an addon that hold its model classes in Odoo's layout, which tell a model class
/// the index cannot look up.
const MODEL_PACKAGES: &[&str] = &["models", "wizard", "wizards", "report"];

/// The model classes of the ORM itself, under `odoo.models` or, from 19.0, `odoo.orm`.
const ORM_MODELS: &[&str] = &["BaseModel", "AbstractModel", "Model", "TransientModel", "CachedModel"];

/// Directories of upgrade scripts, which name XML ids of past versions.
pub(crate) const UPGRADE_DIRS: &[&str] = &["migrations", "upgrades"];

/// The patches OD123 reports, which `checks.rs` words with the index's answer, and the OD124
/// diagnostics.
pub(crate) fn check(body: &[Stmt], loc: &Locator) -> (Vec<ImportPatch>, Vec<RawDiag>) {
    let top = top_level(body);
    (patches(&top, loc), filled_caches(body, &top, loc))
}

/// The statements that run at import: the module body, into `if`, `try` and `with` blocks.
pub(crate) fn top_level(body: &[Stmt]) -> Vec<&Stmt> {
    let mut stmts = vec![];
    for stmt in body {
        stmts.push(stmt);
        match stmt {
            Stmt::If(i) => {
                stmts.extend(top_level(&i.body));
                for clause in &i.elif_else_clauses {
                    stmts.extend(top_level(&clause.body));
                }
            }
            Stmt::Try(t) => {
                for block in [&t.body, &t.orelse, &t.finalbody] {
                    stmts.extend(top_level(block));
                }
                for handler in &t.handlers {
                    let ast::ExceptHandler::ExceptHandler(h) = handler;
                    stmts.extend(top_level(&h.body));
                }
            }
            Stmt::With(w) => stmts.extend(top_level(&w.body)),
            _ => {}
        }
    }
    stmts
}

/// The names an import of `odoo`, or of a module under it, binds, each with the dotted path of
/// what it names.
pub(crate) fn odoo_names<'a>(top: &[&'a Stmt]) -> Vec<(&'a str, String)> {
    imported_names(top, "odoo")
}

/// The names the file binds at import to a list, tuple, set or dict literal of literals, and
/// binds to nothing else there.
pub(crate) fn collections<'a>(top: &[&'a Stmt]) -> Vec<&'a str> {
    let mut names = vec![];
    for stmt in top {
        let (targets, value): (Vec<&Expr>, _) = match stmt {
            Stmt::Assign(a) => (a.targets.iter().collect(), Some(&*a.value)),
            Stmt::AnnAssign(a) => (vec![&*a.target], a.value.as_deref()),
            _ => continue,
        };
        let literal = value.is_some_and(literals);
        for name in targets.into_iter().filter_map(Expr::as_name_expr) {
            names.retain(|n| *n != name.id.as_str());
            if literal {
                names.push(name.id.as_str());
            }
        }
    }
    names
}

/// Whether `expr` is a list, tuple, set or dict literal whose items are each a `scalar` or such
/// a literal.
fn literals(expr: &Expr) -> bool {
    items(expr).is_some_and(|items| items.into_iter().all(|i| i.is_some_and(|i| scalar(i) || literals(i))))
}

/// The items of a list, tuple or set literal, or the keys of a dict literal with `None` for a
/// `**` entry; `None` for any other expression.
pub(crate) fn items(expr: &Expr) -> Option<Vec<Option<&Expr>>> {
    match expr {
        Expr::List(ast::ExprList { elts, .. })
        | Expr::Tuple(ast::ExprTuple { elts, .. })
        | Expr::Set(ast::ExprSet { elts, .. }) => Some(elts.iter().map(Some).collect()),
        Expr::Dict(d) => Some(d.items.iter().map(|i| i.key.as_ref()).collect()),
        _ => None,
    }
}

/// Whether `expr` is a literal that holds no other value: a string but an f-string, bytes, a
/// number, a boolean or `None`.
pub(crate) fn scalar(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::NumberLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_)
    )
}

/// The names an import of `package`, or of a module under it, binds, each with the dotted path of
/// what it names.
pub(crate) fn imported_names<'a>(top: &[&'a Stmt], package: &'a str) -> Vec<(&'a str, String)> {
    let from_package = |module: &str| {
        module
            .strip_prefix(package)
            .is_some_and(|r| r.is_empty() || r.starts_with('.'))
    };
    let mut names = vec![];
    for stmt in top {
        match stmt {
            Stmt::Import(i) => names.extend(i.names.iter().filter(|a| from_package(&a.name)).map(
                |a| match &a.asname {
                    Some(alias) => (alias.as_str(), a.name.to_string()),
                    None => (package, package.to_owned()),
                },
            )),
            Stmt::ImportFrom(ast::StmtImportFrom {
                module: Some(module),
                names: aliases,
                ..
            }) if from_package(module) => names.extend(aliases.iter().map(|a| {
                let local = a.asname.as_ref().unwrap_or(&a.name).as_str();
                (local, format!("{module}.{}", a.name))
            })),
            _ => {}
        }
    }
    names
}

/// The name an attribute or item chain starts from.
pub(crate) fn root(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Name(n) => Some(n.id.as_str()),
        Expr::Attribute(a) => root(&a.value),
        Expr::Subscript(s) => root(&s.value),
        _ => None,
    }
}

/// `a.b.c` as written, or its root followed by `...` when an item or a call breaks the chain.
pub(crate) fn dotted(expr: &Expr) -> String {
    match expr {
        Expr::Name(n) => n.id.to_string(),
        Expr::Attribute(a) => format!("{}.{}", dotted(&a.value), a.attr),
        e => format!("{}...", root(e).unwrap_or_default()),
    }
}

/// The dotted path of what `expr` names, when it starts from one of the imported `names`.
pub(crate) fn imported_path(expr: &Expr, names: &[(&str, String)]) -> Option<String> {
    let (_, source) = names.iter().find(|(name, _)| root(expr) == Some(name))?;
    let rest = dotted(expr)
        .split_once('.')
        .map_or(String::new(), |(_, rest)| format!(".{rest}"));
    Some(format!("{source}{rest}"))
}

/// An attribute an import-time statement sets: on what object, and its name, `...` when the code
/// computes it.
fn set_attribute(stmt: &Stmt) -> Option<(&Expr, &str)> {
    let target = match stmt {
        Stmt::Assign(a) => a.targets.iter().find(|t| t.is_attribute_expr())?,
        Stmt::AugAssign(a) => &a.target,
        Stmt::Expr(e) => return setattr_target(&e.value),
        _ => return None,
    };
    let Expr::Attribute(a) = target else { return None };
    Some((&a.value, a.attr.as_str()))
}

/// The object `setattr(obj, "name", ...)` sets an attribute on, and the name.
fn setattr_target(expr: &Expr) -> Option<(&Expr, &str)> {
    let Expr::Call(call) = expr else { return None };
    if !matches!(&*call.func, Expr::Name(n) if n.id.as_str() == "setattr") {
        return None;
    }
    let object = call.arguments.args.first()?;
    Some((object, call.arguments.args.get(1).and_then(str_lit).unwrap_or("...")))
}

/// Whether the class a dotted path names is a model class by where it lives: in a model package of
/// an addon, or among the ORM's own model classes.
fn model_path(path: &str) -> bool {
    match path.split('.').collect::<Vec<_>>().as_slice() {
        ["odoo", "addons", _, package, _, ..] => MODEL_PACKAGES.contains(package),
        ["odoo", "models" | "orm", .., class] => ORM_MODELS.contains(class),
        _ => false,
    }
}

/// The fix for a patch: a model class is changed by inheriting the model, another class by a
/// subclass, and a module needs no patch. `model` is the index's answer for the class, which the
/// path stands in for when the index has none.
fn fix(patch: &ImportPatch, model: Option<bool>) -> String {
    let class = patch
        .path
        .rsplit('.')
        .next()
        .is_some_and(|last| last.starts_with(|c: char| c.is_ascii_uppercase()));
    if class && model.unwrap_or_else(|| model_path(&patch.path)) {
        "inherit the model in a class of this module and override it there, which applies only where the module is installed".to_owned()
    } else if class {
        format!(
            "subclass {} in this module and use the subclass where it needs the change",
            patch.object
        )
    } else {
        "keep the value in this module and import it from there".to_owned()
    }
}

/// OD123 on a patch, `model` as `fix` takes it.
pub(crate) fn diag(patch: &ImportPatch, model: Option<bool>) -> RawDiag {
    RawDiag::new(
        "OD123",
        patch.pos,
        format!(
            "setting {}.{} at import changes it in every database the server runs, whether this module is installed there or not; {}",
            patch.object,
            patch.attr,
            fix(patch, model)
        ),
    )
}

/// The attributes import-time statements set on what the file imports from `odoo`, but a
/// docstring and a module named under `odoo.addons`, which change no behaviour.
fn patches(top: &[&Stmt], loc: &Locator) -> Vec<ImportPatch> {
    let odoo = odoo_names(top);
    let mut found = vec![];
    for stmt in top {
        let Some((object, attr)) = set_attribute(stmt) else {
            continue;
        };
        let Some(path) = imported_path(object, &odoo) else {
            continue;
        };
        if attr == "__doc__" || path == "odoo.addons" {
            continue;
        }
        found.push(ImportPatch {
            pos: loc.at(*stmt),
            object: dotted(object),
            attr: attr.to_owned(),
            path,
        });
    }
    found
}

/// Whether a value is a new dict, list or set: a display, a comprehension, or a call that builds
/// one, an `LRU` cache among them.
fn is_container(value: &Expr) -> bool {
    match value {
        Expr::Dict(_) | Expr::List(_) | Expr::Set(_) | Expr::DictComp(_) | Expr::ListComp(_) | Expr::SetComp(_) => true,
        Expr::Call(c) => tail_name(&c.func).is_some_and(|name| CONTAINER_CALLS.contains(&name) || name.contains("LRU")),
        _ => false,
    }
}

/// The names import-time statements bind to a new container, each with its first such binding.
fn containers<'a>(top: &[&'a Stmt]) -> Vec<&'a ast::ExprName> {
    let mut found: Vec<&ast::ExprName> = vec![];
    for stmt in top {
        let (targets, value) = match stmt {
            Stmt::Assign(a) => (&a.targets[..], &*a.value),
            Stmt::AnnAssign(ast::StmtAnnAssign {
                target,
                value: Some(value),
                ..
            }) => (std::slice::from_ref(&**target), &**value),
            _ => continue,
        };
        if !is_container(value) {
            continue;
        }
        for name in targets.iter().filter_map(Expr::as_name_expr) {
            if !found.iter().any(|f| f.id == name.id) {
                found.push(name);
            }
        }
    }
    found
}

/// OD124 once per container that a function changes without binding its name, so that it changes
/// the module's, other than by an entry per database.
fn filled_caches(body: &[Stmt], top: &[&Stmt], loc: &Locator) -> Vec<RawDiag> {
    let candidates = containers(top);
    if candidates.is_empty() {
        return vec![];
    }
    let mut functions = Functions::default();
    functions.visit_body(body);
    candidates
        .into_iter()
        .filter(|name| {
            functions.0.iter().any(|f| {
                let uses = NameUses::scan(f, name.id.as_str());
                uses.writes && !uses.binds
            })
        })
        .map(|name| {
            RawDiag::new(
                "OD124",
                loc.at(name),
                format!(
                    "{} is filled at runtime but lives in the Python process, so every database the server runs shares it and each worker fills its own copy; cache on a model method with tools.ormcache, which Odoo keeps per database and clears in every worker",
                    name.id
                ),
            )
        })
        .collect()
}

/// A key that names the database, such as `cr.dbname` or `(env.registry.db_name, id)`: an entry per
/// database leaks nothing between them.
fn per_database(key: &Expr) -> bool {
    any_over_expr(key, |e| matches!(tail_name(e), Some("dbname" | "db_name")))
}

/// Every function of the file, methods and nested functions among them.
#[derive(Default)]
struct Functions<'a>(Vec<&'a ast::StmtFunctionDef>);

impl<'a> Visitor<'a> for Functions<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if let Stmt::FunctionDef(f) = stmt {
            self.0.push(f);
        }
        visitor::walk_stmt(self, stmt);
    }
}

/// Whether a function, the functions it nests aside, changes the container a name holds, and
/// whether it binds the name itself, so that the name is not the module's.
struct NameUses<'n> {
    name: &'n str,
    writes: bool,
    binds: bool,
}

impl<'n> NameUses<'n> {
    fn scan(func: &ast::StmtFunctionDef, name: &'n str) -> Self {
        let mut uses = Self {
            name,
            writes: false,
            binds: func.parameters.iter().any(|p| p.name().as_str() == name),
        };
        uses.visit_body(&func.body);
        uses
    }

    fn is_name(&self, expr: &Expr) -> bool {
        matches!(expr, Expr::Name(n) if n.id.as_str() == self.name)
    }

    /// A call of a mutating method on the name, other than one on the entry of the database.
    fn mutates(&self, call: &ast::ExprCall) -> bool {
        let Expr::Attribute(a) = &*call.func else { return false };
        MUTATORS.contains(&a.attr.as_str())
            && self.is_name(&a.value)
            && !call.arguments.args.first().is_some_and(per_database)
    }

    fn target(&mut self, target: &Expr) {
        match target {
            Expr::Subscript(s) => self.writes |= self.is_name(&s.value) && !per_database(&s.slice),
            Expr::Tuple(t) => t.elts.iter().for_each(|e| self.target(e)),
            Expr::List(l) => l.elts.iter().for_each(|e| self.target(e)),
            Expr::Starred(s) => self.target(&s.value),
            e => self.binds |= self.is_name(e),
        }
    }
}

impl<'a> Visitor<'a> for NameUses<'_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => return,
            Stmt::Assign(a) => a.targets.iter().for_each(|t| self.target(t)),
            Stmt::AnnAssign(a) => self.target(&a.target),
            Stmt::AugAssign(a) => self.target(&a.target),
            Stmt::Delete(d) => d.targets.iter().for_each(|t| self.target(t)),
            Stmt::For(f) => self.target(&f.target),
            Stmt::With(w) => w
                .items
                .iter()
                .filter_map(|i| i.optional_vars.as_deref())
                .for_each(|t| self.target(t)),
            Stmt::Import(ast::StmtImport { names, .. }) | Stmt::ImportFrom(ast::StmtImportFrom { names, .. }) => {
                self.binds |= names
                    .iter()
                    .any(|a| a.asname.as_ref().unwrap_or(&a.name).as_str() == self.name);
            }
            _ => {}
        }
        visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Named(n) => self.binds |= self.is_name(&n.target),
            Expr::Call(c) => self.writes |= self.mutates(c),
            _ => {}
        }
        visitor::walk_expr(self, expr);
    }
}

/// The folder of a script Odoo runs at a stage of an upgrade, as `(dir, folder)`: a `pre-`, `post-`
/// or `end-` `.py` file of a folder under `UPGRADE_DIRS` but `tests` (18.0
/// `odoo/modules/migration.py:108-140`, `:184-191`). Odoo runs no other file of the module that way.
pub(crate) fn stage_script(rel: &str) -> Option<(&str, &str)> {
    let mut parts = rel.split('/');
    let (dir, folder, name) = (parts.next()?, parts.next()?, parts.next()?);
    let staged = ["pre-", "post-", "end-"].iter().any(|s| name.starts_with(s)) && name.ends_with(".py");
    (parts.next().is_none() && UPGRADE_DIRS.contains(&dir) && folder != "tests" && staged).then_some((dir, folder))
}

/// The names Odoo takes for the two parameters of `migrate` (18.0
/// `odoo/modules/migration.py:221-224`).
const MIGRATE_PARAMS: [[&str; 2]; 2] = [["cr", "_cr"], ["version", "_version"]];

/// Calls through which code may bind any name of the module unseen.
const NAME_WRITERS: &[&str] = &["globals", "locals", "exec", "eval", "vars"];

/// Attributes that hold the names of a module: of `sys`, a frame and a function.
const NAME_HOLDERS: &[&str] = &["modules", "f_globals", "f_locals", "__globals__"];

/// What the file binds to `migrate` at import. A star import, a module `__getattr__` or code that
/// writes the module's names may bind it unseen, a `def` in any block (`if`, a loop, a `match`
/// case, `try`, `with`) may run any number of times or none, and a decorator may change the
/// signature Odoo reads, so only a lone undecorated `def` at module level is judged.
pub(crate) fn migrate(body: &[Stmt], loc: &Locator) -> Migrate {
    let top = bound(body);
    let mut bindings = top.iter().copied().filter(|s| binds(s, "migrate"));
    let (first, more) = (bindings.next(), bindings.next().is_some());
    if more || top.iter().any(|s| binds(s, "*") || binds(s, "__getattr__")) {
        return Migrate::Unread;
    }
    let judged = match first {
        None => Migrate::Missing,
        Some(stmt @ Stmt::FunctionDef(def))
            if def.decorator_list.is_empty()
                && body.iter().any(|s| std::ptr::eq(s, stmt))
                && !odoo_takes(&def.parameters) =>
        {
            Migrate::Refused {
                pos: loc.at(def),
                params: python::signature(&def.parameters, loc, true).spelled(""),
            }
        }
        Some(_) => return Migrate::Unread,
    };
    // Only a file judged so far is worth the walk of its whole tree.
    if writes_names(body, "migrate") {
        Migrate::Unread
    } else {
        judged
    }
}

/// Whether `params` are two positional parameters of the names Odoo takes, defaults or not.
fn odoo_takes(params: &ast::Parameters) -> bool {
    let names: Vec<&str> = params
        .posonlyargs
        .iter()
        .chain(&params.args)
        .map(|p| p.parameter.name.as_str())
        .collect();
    params.vararg.is_none()
        && params.kwonlyargs.is_empty()
        && params.kwarg.is_none()
        && names.len() == 2
        && MIGRATE_PARAMS
            .iter()
            .zip(&names)
            .all(|(taken, name)| taken.contains(name))
}

/// Whether `stmt` binds or unbinds `name` in the scope it runs in, `*` for a star import.
fn binds(stmt: &Stmt, name: &str) -> bool {
    match stmt {
        Stmt::FunctionDef(f) => f.name.as_str() == name,
        Stmt::ClassDef(c) => c.name.as_str() == name,
        Stmt::Import(i) => i.names.iter().any(|a| match &a.asname {
            Some(alias) => alias.as_str() == name,
            None => a.name.split('.').next() == Some(name),
        }),
        Stmt::ImportFrom(i) => i
            .names
            .iter()
            .any(|a| a.asname.as_ref().unwrap_or(&a.name).as_str() == name),
        Stmt::Try(t) => t.handlers.iter().any(|h| {
            let ast::ExceptHandler::ExceptHandler(h) = h;
            h.name.as_ref().is_some_and(|n| n.as_str() == name)
        }),
        Stmt::Match(m) => m.cases.iter().any(|c| captures(&c.pattern).contains(&name)),
        _ => targets(stmt).into_iter().any(|t| target_binds(t, name)),
    }
}

/// The names a `match` pattern captures.
fn captures(pattern: &ast::Pattern) -> Vec<&str> {
    #[derive(Default)]
    struct Captures<'a>(Vec<&'a str>);
    impl<'a> Visitor<'a> for Captures<'a> {
        fn visit_pattern(&mut self, pattern: &'a ast::Pattern) {
            self.0.extend(python::capture(pattern).map(ast::Identifier::as_str));
            visitor::walk_pattern(self, pattern);
        }
    }
    let mut captures = Captures::default();
    captures.visit_pattern(pattern);
    captures.0
}

/// The targets `stmt` assigns or deletes.
fn targets(stmt: &Stmt) -> Vec<&Expr> {
    match stmt {
        Stmt::Assign(a) => a.targets.iter().collect(),
        Stmt::AnnAssign(a) if a.value.is_some() => vec![&*a.target],
        Stmt::AugAssign(a) => vec![&*a.target],
        Stmt::TypeAlias(a) => vec![&*a.name],
        Stmt::For(f) => vec![&*f.target],
        Stmt::With(w) => w.items.iter().filter_map(|i| i.optional_vars.as_deref()).collect(),
        Stmt::Delete(d) => d.targets.iter().collect(),
        _ => vec![],
    }
}

/// Whether the assignment target `target` binds `name`, alone or unpacked.
fn target_binds(target: &Expr, name: &str) -> bool {
    match target {
        Expr::Tuple(ast::ExprTuple { elts, .. }) | Expr::List(ast::ExprList { elts, .. }) => {
            elts.iter().any(|e| target_binds(e, name))
        }
        Expr::Starred(s) => target_binds(&s.value, name),
        e => is_named(e, name),
    }
}

fn is_named(expr: &Expr, name: &str) -> bool {
    expr.as_name_expr().is_some_and(|n| n.id.as_str() == name)
}

/// Whether the file may bind `name` in the module where no statement shows it: through a `global`
/// declaration, an assignment expression or code that may write any name, as `Unseen` reads them.
fn writes_names(body: &[Stmt], name: &str) -> bool {
    let mut unseen = Unseen::default();
    unseen.visit_body(body);
    unseen.writes || unseen.names.iter().any(|n| n == name)
}

/// The names the file binds at import, in the module body or a block of it, a `match` pattern
/// included, or that a function or an assignment expression may bind in it. None when it may bind
/// names no statement shows: through a star import, a module `__getattr__` or code that may write
/// any name, as `Unseen` reads them.
pub(crate) fn defined_names(body: &[Stmt]) -> Option<Vec<String>> {
    let mut names = vec![];
    for stmt in bound(body) {
        stmt_names(stmt, &mut names)?;
    }
    let mut unseen = Unseen::default();
    unseen.visit_body(body);
    if unseen.writes || names.iter().any(|n| n == "__getattr__") {
        return None;
    }
    names.extend(unseen.names);
    Some(names)
}

/// The names `stmt` binds where it runs, or None for a star import.
fn stmt_names(stmt: &Stmt, names: &mut Vec<String>) -> Option<()> {
    match stmt {
        Stmt::FunctionDef(f) => names.push(f.name.to_string()),
        Stmt::ClassDef(c) => names.push(c.name.to_string()),
        Stmt::Import(i) => names.extend(i.names.iter().map(|a| match &a.asname {
            Some(alias) => alias.to_string(),
            None => a.name.split('.').next().unwrap_or_default().to_owned(),
        })),
        Stmt::ImportFrom(i) if i.names.iter().any(|a| a.name.as_str() == "*") => return None,
        Stmt::ImportFrom(i) => {
            names.extend(i.names.iter().map(|a| a.asname.as_ref().unwrap_or(&a.name).to_string()));
        }
        Stmt::Try(t) => names.extend(t.handlers.iter().filter_map(|h| {
            let ast::ExceptHandler::ExceptHandler(h) = h;
            h.name.as_ref().map(ToString::to_string)
        })),
        Stmt::Match(m) => {
            let cases = m.cases.iter().flat_map(|c| captures(&c.pattern));
            names.extend(cases.map(str::to_owned));
        }
        _ => targets(stmt).into_iter().for_each(|t| target_names(t, names)),
    }
    Some(())
}

/// `top_level`, into the bodies of loops and `match` cases too.
pub(crate) fn bound(body: &[Stmt]) -> Vec<&Stmt> {
    let mut stmts = vec![];
    for stmt in top_level(body) {
        stmts.push(stmt);
        let blocks: Vec<&[Stmt]> = match stmt {
            Stmt::For(f) => vec![&f.body, &f.orelse],
            Stmt::While(w) => vec![&w.body, &w.orelse],
            Stmt::Match(m) => m.cases.iter().map(|c| &c.body[..]).collect(),
            _ => continue,
        };
        stmts.extend(blocks.into_iter().flat_map(bound));
    }
    stmts
}

/// Each name a target binds, alone or unpacked.
fn target_names(target: &Expr, names: &mut Vec<String>) {
    match target {
        Expr::Tuple(ast::ExprTuple { elts, .. }) | Expr::List(ast::ExprList { elts, .. }) => {
            elts.iter().for_each(|e| target_names(e, names));
        }
        Expr::Starred(s) => target_names(&s.value, names),
        Expr::Name(n) => names.push(n.id.to_string()),
        _ => {}
    }
}

/// The names a `global` declaration or an assignment expression anywhere in the file may bind in the
/// module, and whether code may write any: a call of a `NAME_WRITERS` builtin, by its name, through
/// `builtins` or imported from it, or a `NAME_HOLDERS` attribute, or `modules` imported from `sys`.
#[derive(Default)]
struct Unseen {
    names: Vec<String>,
    writes: bool,
}

impl<'a> Visitor<'a> for Unseen {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Global(g) => self.names.extend(g.names.iter().map(ToString::to_string)),
            Stmt::ImportFrom(i) => {
                let held = |writers: &[&str]| i.names.iter().any(|a| writers.contains(&a.name.as_str()));
                self.writes |= match i.module.as_ref().map(ast::Identifier::as_str) {
                    Some("sys") if i.level == 0 => held(&["modules"]),
                    Some("builtins") if i.level == 0 => held(NAME_WRITERS),
                    _ => false,
                };
            }
            _ => {}
        }
        visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Name(n) => self.writes |= NAME_WRITERS.contains(&n.id.as_str()),
            Expr::Attribute(a) => {
                let attr = a.attr.as_str();
                self.writes |=
                    NAME_HOLDERS.contains(&attr) || (NAME_WRITERS.contains(&attr) && is_named(&a.value, "builtins"));
            }
            Expr::Named(n) => target_names(&n.target, &mut self.names),
            _ => {}
        }
        visitor::walk_expr(self, expr);
    }
}

#[cfg(test)]
mod tests {
    use ruff_python_parser::parse_module;

    use super::*;
    use crate::facts::Pos;

    /// OD123 as the path words it, then OD124.
    fn diags(src: &str) -> Vec<RawDiag> {
        let parsed = parse_module(src).expect("valid Python");
        let (patches, caches) = check(&parsed.syntax().body, &Locator::new(src));
        patches.iter().map(|p| diag(p, None)).chain(caches).collect()
    }

    fn codes(src: &str) -> Vec<(&'static str, u32)> {
        diags(src).into_iter().map(|d| (d.code, d.pos.line)).collect()
    }

    #[test]
    fn reports_attributes_set_at_import_on_what_odoo_gives() {
        let src = "import odoo.http as web\nfrom odoo.tools import misc\nfrom . import local\nimport json\n\
                   web.x = 1\nmisc.y += 'a'\nlocal.z = 2\njson.w = 3\nsetattr(misc, 'v', 4)\n\
                   while True:\n    misc.u = 5\n";
        assert_eq!(codes(src), [("OD123", 5), ("OD123", 6), ("OD123", 9)]);
    }

    #[test]
    fn reads_a_with_block_at_import() {
        let src = "from odoo.tools import misc\nwith guard():\n    misc.x = 1\n    X = {}\n\
                   def fill():\n    X['k'] = 1\n";
        assert_eq!(codes(src), [("OD123", 3), ("OD124", 4)]);
    }

    #[test]
    fn a_docstring_or_a_module_named_under_odoo_addons_changes_no_behaviour() {
        let src = "import odoo\nfrom odoo import addons, fields\n\
                   fields.Field.__doc__ += 'x'\nsetattr(fields.Char, '__doc__', 'y')\n\
                   odoo.addons.x = sys.modules[__name__]\naddons.y = sys.modules[__name__]\n\
                   odoo.addons.base.z = 1\n";
        assert_eq!(codes(src), [("OD123", 7)]);
    }

    #[test]
    fn tells_a_model_class_another_class_and_a_module_apart() {
        let src = "from odoo import models, fields\nfrom odoo.tools import misc\n\
                   from odoo.addons.sale.models.sale_order import SaleOrder as Order\n\
                   models.BaseModel.x = 1\nOrder.x = 1\nfields.Field.x = 1\nmisc.x = 1\nmodels.MetaModel.x = 1\n";
        let fixes: Vec<String> = diags(src)
            .into_iter()
            .map(|d| d.message.split("; ").nth(1).unwrap_or_default().to_owned())
            .collect();
        assert!(fixes[0].starts_with("inherit the model"), "{}", fixes[0]);
        assert!(fixes[1].starts_with("inherit the model"), "{}", fixes[1]);
        assert!(fixes[2].starts_with("subclass fields.Field"), "{}", fixes[2]);
        assert!(fixes[3].starts_with("keep the value"), "{}", fixes[3]);
        assert!(fixes[4].starts_with("subclass models.MetaModel"), "{}", fixes[4]);
    }

    #[test]
    fn tells_a_model_class_by_its_package_or_as_the_orm_s_own() {
        for path in [
            "odoo.addons.sale.models.sale_order.SaleOrder",
            "odoo.addons.sale.wizard.sale_make_invoice.SaleAdvance",
            "odoo.addons.sale.wizards.SaleAdvance",
            "odoo.addons.sale.report.sale_report.SaleReport",
            "odoo.models.BaseModel",
            "odoo.models.AbstractModel",
            "odoo.models.Model",
            "odoo.models.TransientModel",
            "odoo.orm.models.BaseModel",
            "odoo.models.CachedModel",
            "odoo.orm.models_cached.CachedModel",
        ] {
            assert!(model_path(path), "{path}");
        }
        for path in [
            "odoo.models.MetaModel",
            "odoo.addons.sale.controllers.portal.CustomerPortal",
            "odoo.addons.sale.SaleOrder",
            "odoo.addons.models.SaleOrder",
            "odoo.fields.Model",
        ] {
            assert!(!model_path(path), "{path}");
        }
    }

    #[test]
    fn the_index_answer_overrides_the_path() {
        let patch = |path: &str| ImportPatch {
            pos: Pos::default(),
            object: "Label".to_owned(),
            attr: "x".to_owned(),
            path: path.to_owned(),
        };
        let wizard = patch("odoo.addons.base.wizard.x_label.Label");
        let controller = patch("odoo.addons.base.controllers.main.Label");
        assert!(fix(&wizard, None).starts_with("inherit the model"));
        assert!(fix(&wizard, Some(false)).starts_with("subclass Label"));
        assert!(fix(&controller, None).starts_with("subclass Label"));
        assert!(fix(&controller, Some(true)).starts_with("inherit the model"));
        assert!(fix(&patch("odoo.addons.base.models.res_partner"), Some(true)).starts_with("keep the value"));
    }

    #[test]
    fn tells_containers_from_other_values() {
        let container = |src: &str| {
            is_container(
                parse_module(src).unwrap().syntax().body[0]
                    .as_expr_stmt()
                    .unwrap()
                    .value
                    .as_ref(),
            )
        };
        for src in [
            "{}",
            "[]",
            "{1}",
            "{k: 1 for k in a}",
            "[k for k in a]",
            "dict()",
            "collections.OrderedDict()",
            "LRU(8)",
        ] {
            assert!(container(src), "{src}");
        }
        for src in ["()", "frozenset()", "'a'", "build()", "MappingProxyType({})"] {
            assert!(!container(src), "{src}");
        }
    }

    #[test]
    fn reports_a_container_once_unless_each_writer_binds_it_or_keys_by_database() {
        let src = "X = {}\nX = []\nY = {}\nZ = {}\nW = []\n\
                   def a():\n    X['k'] = 1\n    X.clear()\n\
                   def b(Y):\n    Y['k'] = 1\n\
                   def c(cr):\n    Z[cr.dbname] = 1\n    Z.pop(cr.dbname)\n\
                   def d():\n    for W in ():\n        W.append(1)\n\
                   def e(env):\n    Z[(env.cr.dbname, 1)] = 1\n    Z[f'{env.registry.db_name}-1'] = 1\n";
        assert_eq!(codes(src), [("OD124", 1)]);
        assert_eq!(codes("Z = {}\ndef e(cr):\n    Z[(cr.name, 1)] = 1\n"), [("OD124", 1)]);
    }

    fn migrate_of(src: &str) -> Migrate {
        let parsed = parse_module(src).expect("valid Python");
        migrate(&parsed.syntax().body, &Locator::new(src))
    }

    #[test]
    fn judges_a_lone_undecorated_migrate_of_the_module_body() {
        for (src, params) in [
            (
                "import logging\n\ndef migrate(env, version):\n    pass\n",
                "(env, version)",
            ),
            (
                "def migrate(cr, installed_version):\n    pass\n",
                "(cr, installed_version)",
            ),
            (
                "def migrate(cr, version, extra=None):\n    pass\n",
                "(cr, version, extra=None)",
            ),
            (
                "def migrate(\n    cr,\n    version,\n    *args,\n):\n    pass\n",
                "(cr, version, *args)",
            ),
            ("def migrate(cr, *, version):\n    pass\n", "(cr, *, version)"),
            ("def migrate(cr):\n    pass\n", "(cr)"),
            (
                "def migrate(env,  # cursor\n            version):\n    pass\n",
                "(env, version)",
            ),
            (
                "def migrate(cr: Cursor, version: str, extra=None):\n    pass\n",
                "(cr, version, extra=None)",
            ),
            (
                "def migrate(cr, /, version, extra):\n    pass\n",
                "(cr, /, version, extra)",
            ),
            ("def migrate(cr, version, **kw):\n    pass\n", "(cr, version, **kw)"),
        ] {
            let Migrate::Refused { pos, params: written } = migrate_of(src) else {
                panic!("{src}");
            };
            assert_eq!(
                (pos.line, pos.col, written.as_str()),
                (
                    src.lines().position(|l| l.starts_with("def")).unwrap() as u32 + 1,
                    1,
                    params
                )
            );
        }
        for src in [
            "def migrate(cr, version):\n    pass\n",
            "def migrate(_cr, _version):\n    pass\n",
            "def migrate(cr, version=None, /):\n    pass\n",
            "def migrate(cr, version):\n    if (n := 1):\n        setattr(cr, 'x', n)\n",
        ] {
            assert_eq!(migrate_of(src), Migrate::Unread, "{src}");
        }
        assert_eq!(
            migrate_of("import logging\n\ndef helper(cr):\n    pass\n"),
            Migrate::Missing
        );
        assert_eq!(migrate_of("migrate: int\n"), Migrate::Missing);
    }

    #[test]
    fn leaves_a_migrate_bound_any_other_way_unjudged() {
        for src in [
            "@decorate\ndef migrate(env, version):\n    pass\n",
            "if True:\n    def migrate(env, version):\n        pass\n",
            "try:\n    def migrate(env, version):\n        pass\nexcept E:\n    pass\n",
            "with build():\n    def migrate(env, version):\n        pass\n",
            "def migrate(env):\n    pass\nmigrate = wrap(migrate)\n",
            "from .helpers import migrate\n",
            "from .helpers import run as migrate\n",
            "import migrate\n",
            "from .helpers import *\n",
            "from .helpers import *\ndef migrate(env, version):\n    pass\n",
            "migrate = build()\n",
            "migrate, other = build()\n",
            "class migrate:\n    pass\n",
            "with build() as migrate:\n    pass\n",
            "for migrate in ():\n    pass\n",
            "def __getattr__(name):\n    pass\n",
            "globals()['migrate'] = build()\n",
            "sys.modules[__name__].migrate = build()\n",
            "def bind():\n    global migrate\n    migrate = build()\nbind()\n",
            "if (migrate := build()):\n    pass\n",
            "try:\n    pass\nexcept E as migrate:\n    pass\n",
            "match build():\n    case migrate:\n        pass\n",
            "match build():\n    case [*migrate]:\n        pass\n",
            "match build():\n    case {**migrate}:\n        pass\n",
        ] {
            assert_eq!(migrate_of(src), Migrate::Unread, "{src}");
        }
        for params in ["(cr, version)", "(env, version)"] {
            for block in ["for _ in ():\n", "for _ in ():\n    pass\nelse:\n", "while build():\n"] {
                let src = format!("{block}    def migrate{params}:\n        pass\n");
                assert_eq!(migrate_of(&src), Migrate::Unread, "{src}");
            }
            let src = format!("match build():\n    case 1:\n        def migrate{params}:\n            pass\n");
            assert_eq!(migrate_of(&src), Migrate::Unread, "{src}");
        }
        let def = "def migrate(env, version):\n    pass\n";
        for writer in WRITERS {
            let src = format!("{def}{writer}");
            assert_eq!(migrate_of(&src), Migrate::Unread, "{src}");
        }
    }

    /// A line of each form through which code may write any name of the module.
    const WRITERS: &[&str] = &[
        "globals()['migrate'] = build()\n",
        "locals()['migrate'] = build()\n",
        "vars()['migrate'] = build()\n",
        "exec('migrate = build()')\n",
        "eval('(migrate := build())')\n",
        "import builtins\nbuiltins.exec('migrate = build()')\n",
        "from builtins import exec as run\nrun('migrate = build()')\n",
        "from builtins import vars\n",
        "sys.modules[__name__].migrate = build()\n",
        "from sys import modules\nmodules[__name__].migrate = build()\n",
        "from sys import modules as loaded\nloaded[__name__].migrate = build()\n",
        "sys._getframe().f_globals['migrate'] = build()\n",
        "sys._getframe().f_locals['migrate'] = build()\n",
        "def helper():\n    pass\nhelper.__globals__['migrate'] = build()\n",
    ];

    fn defined(src: &str) -> Option<Vec<String>> {
        let parsed = parse_module(src).expect("valid Python");
        let mut names = defined_names(&parsed.syntax().body)?;
        names.sort();
        Some(names)
    }

    #[test]
    fn lists_the_names_a_file_binds_at_import() {
        let src = "import a.b, c as d\nfrom .e import f, g as h\nclass K:\n    inner = 1\ndef run():\n    local = 1\n    global shared\n\
                   x, (y, *z) = 1, (2, 3)\nn: int\nm: int = 1\nif c:\n    p = 1\nelse:\n    with o() as q:\n        pass\n\
                   try:\n    r = 1\nexcept E as s:\n    pass\nfor t in ():\n    u = 1\nwhile c:\n    v = 1\n\
                   match c:\n    case 1:\n        w = 1\n    case Point(x=px): pass\n    case [ab, *rest]: pass\n\
                   match c:\n    case {\"k\": kv, **others}: pass\n    case int() as whole: pass\n\
                   print(walrus := 1)\nobj.attr = 1\n";
        let expected = [
            "K", "a", "ab", "d", "f", "h", "kv", "m", "others", "p", "px", "q", "r", "rest", "run", "s", "shared", "t",
            "u", "v", "w", "walrus", "whole", "x", "y", "z",
        ];
        assert_eq!(defined(src), Some(expected.iter().map(|n| (*n).to_owned()).collect()));
    }

    #[test]
    fn lists_no_names_where_the_file_may_bind_unseen_ones() {
        for src in [
            "from .models import *\n",
            "def __getattr__(name):\n    pass\n",
            "__getattr__ = lookup\n",
            "globals()['x'] = 1\n",
            "def f():\n    exec('x = 1')\n",
            "sys.modules[__name__].x = 1\n",
        ]
        .iter()
        .chain(WRITERS)
        {
            assert_eq!(defined(src), None, "{src}");
        }
    }

    #[test]
    fn takes_the_stage_scripts_of_an_upgrade_folder() {
        assert_eq!(stage_script("migrations/1.0/pre-x.py"), Some(("migrations", "1.0")));
        assert_eq!(stage_script("upgrades/1.0/end-x.py"), Some(("upgrades", "1.0")));
        for rel in [
            "migrations/1.0/x.py",
            "migrations/tests/post-x.py",
            "migrations/1.0/a/post-x.py",
            "migrations/post-x.py",
            "models/1.0/post-x.py",
            "migrations/1.0/post-x.txt",
        ] {
            assert_eq!(stage_script(rel), None, "{rel}");
        }
    }

    #[test]
    fn reads_migrate_only_of_a_stage_script_that_parses() {
        let read = |src: &str, rel: &str| {
            let scope = crate::routing::Scope {
                rel,
                ..crate::routing::Scope::default()
            };
            let (known, taint) = (Default::default(), Default::default());
            crate::python::analyze_with(src, crate::series::Series::parse("18.0"), &known, &taint, scope).migrate
        };
        let helper = "def helper():\n    pass\n";
        assert_eq!(read(helper, "models/x.py"), Migrate::Unread);
        assert_eq!(read(helper, "migrations/1.0/post-x.py"), Migrate::Missing);
        assert_eq!(read("def helper(:\n", "migrations/1.0/post-x.py"), Migrate::Unread);
    }
}
