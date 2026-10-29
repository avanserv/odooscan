//! State a Python file sets up at import, which Odoo shares between every database the process
//! serves: patches on what it imports from `odoo` (OD123), and containers that functions fill at
//! runtime (OD124).

use ruff_python_ast::helpers::any_over_expr;
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, Stmt};

use crate::facts::{ImportPatch, RawDiag};
use crate::python::{Locator, str_lit, tail_name};

/// Calls that build a dict, a list or a set.
const CONTAINER_CALLS: &[&str] = &["dict", "list", "set", "defaultdict", "OrderedDict"];

/// Methods that change a dict, a list or a set in place.
const MUTATORS: &[&str] = &[
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

/// The patches OD123 reports, which `checks.rs` words with the index's answer, and the OD124
/// diagnostics.
pub(crate) fn check(body: &[Stmt], loc: &Locator) -> (Vec<ImportPatch>, Vec<RawDiag>) {
    let top = top_level(body);
    (patches(&top, loc), filled_caches(body, &top, loc))
}

/// The statements that run at import: the module body, into `if` and `try` blocks.
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
            _ => {}
        }
    }
    stmts
}

/// The names an import of `odoo`, or of a module under it, binds, each with the dotted path of
/// what it names.
pub(crate) fn odoo_names<'a>(top: &[&'a Stmt]) -> Vec<(&'a str, String)> {
    let from_odoo = |module: &str| module == "odoo" || module.starts_with("odoo.");
    let mut names = vec![];
    for stmt in top {
        match stmt {
            Stmt::Import(i) => names.extend(i.names.iter().filter(|a| from_odoo(&a.name)).map(|a| match &a.asname {
                Some(alias) => (alias.as_str(), a.name.to_string()),
                None => ("odoo", "odoo".to_owned()),
            })),
            Stmt::ImportFrom(ast::StmtImportFrom {
                module: Some(module),
                names: aliases,
                ..
            }) if from_odoo(module) => names.extend(aliases.iter().map(|a| {
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
fn dotted(expr: &Expr) -> String {
    match expr {
        Expr::Name(n) => n.id.to_string(),
        Expr::Attribute(a) => format!("{}.{}", dotted(&a.value), a.attr),
        e => format!("{}...", root(e).unwrap_or_default()),
    }
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
        let Some((_, source)) = odoo.iter().find(|(name, _)| root(object) == Some(name)) else {
            continue;
        };
        let path = format!(
            "{source}{}",
            dotted(object)
                .split_once('.')
                .map_or(String::new(), |(_, rest)| format!(".{rest}"))
        );
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
}
