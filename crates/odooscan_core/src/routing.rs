//! The routing Odoo gives a controller method: its own `@route` merged with those of the methods it
//! overrides, ancestors first (18.0 `odoo/http.py:831-849`, 20.0 `odoo/http/routing_map.py:296-314`).
//! An override that leaves out a key keeps the one it overrides, and the type stays that of the
//! first definition whatever an override says (18.0 `http.py:871-880`, 20.0 `routing_map.py:336-346`).

use ruff_python_ast::{self as ast, Expr, Stmt};
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};

use crate::facts::{ClassBase, ControllerFact, PyFacts, RouteDef, RouteKey};
use crate::index::class_file;
use crate::module_state;
use crate::python::{declares_model, keyword, str_lit, tail_name};

/// The controllers of the scan by addon, then by file relative to it.
#[derive(Debug, Default)]
pub struct Table(FxHashMap<String, FxHashMap<String, Vec<ControllerFact>>>);

static EMPTY: Table = Table(FxHashMap::with_hasher(FxBuildHasher));

impl Table {
    /// The controllers of each Python file, given with its addon and its path relative to it.
    pub fn of<'f>(files: impl Iterator<Item = (&'f str, &'f str, &'f PyFacts)>) -> Self {
        let mut table = Self::default();
        for (addon, rel, facts) in files.filter(|(_, _, f)| !f.controllers.is_empty()) {
            let classes = table.0.entry(addon.to_owned()).or_default();
            classes.insert(rel.to_owned(), facts.controllers.clone());
        }
        table
    }

    /// The controllers of a file, with its path as the table holds it.
    fn file<'t>(&'t self, addon: &str, rel: &str) -> Option<(&'t str, &'t [ControllerFact])> {
        let (rel, classes) = self.0.get(addon)?.get_key_value(rel)?;
        Some((rel.as_str(), classes))
    }

    /// The class `name` of a file, the last of that name, which Python binds the name to.
    fn class<'t>(&'t self, addon: &str, rel: &str, name: &str) -> Option<(&'t str, &'t ControllerFact)> {
        let (rel, classes) = self.file(addon, rel)?;
        Some((rel, classes.iter().rfind(|c| c.name == name)?))
    }

    /// The class a dotted `odoo.addons` path names, with its addon and file.
    fn resolve<'t>(&'t self, path: &str) -> Option<(&'t str, &'t str, &'t ControllerFact)> {
        let (addon, files, name) = class_file(path)?;
        let (addon, _) = self.0.get_key_value(addon)?;
        let found = files.iter().find_map(|rel| self.class(addon, rel, name))?;
        Some((addon.as_str(), found.0, found.1))
    }
}

/// Where the file a scan reads sits among the controllers of the scan.
#[derive(Clone, Copy)]
pub struct Scope<'t> {
    pub table: &'t Table,
    pub addon: &'t str,
    pub rel: &'t str,
}

impl Default for Scope<'_> {
    fn default() -> Self {
        Self {
            table: &EMPTY,
            addon: "",
            rel: "",
        }
    }
}

/// The routing Odoo merges for a method, each key none when a definition the scan does not read
/// may give it.
#[derive(Debug, PartialEq)]
pub struct Routing {
    pub urls: Option<Vec<String>>,
    /// The literal methods, none inside when every method passes.
    pub methods: Option<Option<Vec<String>>>,
    pub kind: Option<String>,
    pub auth: Option<String>,
    pub csrf: Option<bool>,
}

impl Scope<'_> {
    /// The merged routing of `method` of the file's `class`, none when the table does not hold the
    /// method's own route.
    pub fn routing(&self, class: &str, method: &str) -> Option<Routing> {
        let (rel, own) = self.table.class(self.addon, self.rel, class)?;
        if !own.routes.iter().any(|r| r.method == method) {
            return None;
        }
        let mut walk = Walk {
            table: self.table,
            method,
            seen: FxHashSet::default(),
            layers: vec![],
        };
        walk.class(self.addon, rel, own);
        let layers = &walk.layers;
        Some(Routing {
            urls: last_given(layers, vec![], |r| &r.urls),
            methods: last_given(layers, None, |r| &r.methods),
            kind: first_kind(layers),
            auth: last_given(layers, "user".to_owned(), |r| &r.auth),
            csrf: last_given(layers, true, |r| &r.csrf),
        })
    }
}

/// The definitions of a method in a class and its ancestors, ancestors first, depth-first. A base
/// the table does not hold stands as none, since it may define the method too.
struct Walk<'t, 'm> {
    table: &'t Table,
    method: &'m str,
    /// The classes walked, by address, since a file may define two classes of one name.
    seen: FxHashSet<*const ControllerFact>,
    layers: Vec<Option<&'t RouteDef>>,
}

impl<'t> Walk<'t, '_> {
    fn class(&mut self, addon: &'t str, rel: &'t str, class: &'t ControllerFact) {
        if !self.seen.insert(std::ptr::from_ref(class)) {
            return;
        }
        // The first base comes last in the reversed method resolution order.
        for base in class.bases.iter().rev() {
            self.base(addon, rel, base);
        }
        let own = class.routes.iter().find(|r| r.method == self.method);
        self.layers.extend(own.map(Some));
    }

    fn base(&mut self, addon: &'t str, rel: &'t str, base: &'t ClassBase) {
        let found = match base {
            ClassBase::Root => return,
            ClassBase::Unread => None,
            ClassBase::Local(at) => self
                .table
                .file(addon, rel)
                .and_then(|(rel, classes)| Some((addon, rel, classes.get(*at)?))),
            ClassBase::Addon(path) => self.table.resolve(path),
        };
        match found {
            Some((addon, rel, class)) => self.class(addon, rel, class),
            None => self.layers.push(None),
        }
    }
}

/// A key the last definition that gives it decides, `default` when none does.
fn last_given<T: Clone>(
    layers: &[Option<&RouteDef>],
    default: T,
    key: impl Fn(&RouteDef) -> &RouteKey<T>,
) -> Option<T> {
    let mut last = layers.iter().rev().map(|layer| layer.map(&key));
    match last.find(|given| !matches!(given, Some(RouteKey::Absent))) {
        None => Some(default),
        Some(Some(RouteKey::Given(value))) => Some(value.clone()),
        Some(_) => None,
    }
}

/// The type of the first definition, `http` when it gives none.
fn first_kind(layers: &[Option<&RouteDef>]) -> Option<String> {
    match &layers.first()?.as_ref()?.kind {
        RouteKey::Absent => Some("http".to_owned()),
        RouteKey::Given(kind) => Some(kind.clone()),
        RouteKey::Unknown => None,
    }
}

/// The classes at the top of a file that are no models and have a base, with the routes of their
/// methods.
pub(crate) fn controllers(body: &[Stmt]) -> Vec<ControllerFact> {
    let top = module_state::top_level(body);
    let imported = module_state::imported_names(&top, "odoo");
    // Each class above, with its place among the controllers when it is one.
    let mut above: Vec<(&str, Option<usize>)> = vec![];
    let mut controllers = vec![];
    for class in top.iter().filter_map(|stmt| stmt.as_class_def_stmt()) {
        let controller = (!class.bases().is_empty() && !declares_model(class)).then_some(controllers.len());
        if controller.is_some() {
            controllers.push(ControllerFact {
                name: class.name.to_string(),
                bases: class.bases().iter().map(|b| base(b, &imported, &above)).collect(),
                routes: last_defs(&class.body).into_iter().filter_map(route_def).collect(),
            });
        }
        above.push((class.name.as_str(), controller));
    }
    controllers
}

/// The methods a class body keeps: the last `def` of each name.
fn last_defs(body: &[Stmt]) -> Vec<&ast::StmtFunctionDef> {
    let defs: Vec<_> = body.iter().filter_map(Stmt::as_function_def_stmt).collect();
    let later = |i: usize, name: &str| defs[i + 1..].iter().any(|f| f.name.as_str() == name);
    let kept = defs.iter().enumerate().filter(|(i, f)| !later(*i, f.name.as_str()));
    kept.map(|(_, f)| *f).collect()
}

/// Whether a file has a route in a class with a base the scan may hold, whose routing it merges:
/// even a route giving every key takes its type from the route it overrides.
pub(crate) fn inherits_routes(controllers: &[ControllerFact]) -> bool {
    controllers.iter().any(|c| {
        let read = c
            .bases
            .iter()
            .any(|b| matches!(b, ClassBase::Addon(_) | ClassBase::Local(_)));
        read && !c.routes.is_empty()
    })
}

/// A base as an import names it: a class of `odoo.addons`, Odoo's `Controller`, or one defined
/// `above` in the file. An import binds the name a class of the same name extends.
fn base(expr: &Expr, imported: &[(&str, String)], above: &[(&str, Option<usize>)]) -> ClassBase {
    if let Some(path) = module_state::imported_path(expr, imported) {
        return if matches!(
            path.as_str(),
            "odoo.http.Controller" | "odoo.http.routing_map.Controller"
        ) {
            ClassBase::Root
        } else if path.starts_with("odoo.addons.") {
            ClassBase::Addon(path)
        } else {
            ClassBase::Unread
        };
    }
    match expr {
        Expr::Name(n) if n.id.as_str() == "object" => ClassBase::Root,
        Expr::Name(n) => match above.iter().rfind(|(name, _)| *name == n.id.as_str()) {
            Some((_, Some(at))) => ClassBase::Local(*at),
            _ => ClassBase::Unread,
        },
        _ => ClassBase::Unread,
    }
}

/// The route of a method, from its outermost `@route`, the one whose routing Odoo keeps.
fn route_def(func: &ast::StmtFunctionDef) -> Option<RouteDef> {
    let call = func.decorator_list.iter().find_map(|d| match &d.expression {
        Expr::Call(call) if tail_name(&call.func) == Some("route") => Some(call),
        _ => None,
    })?;
    let url = call
        .arguments
        .args
        .first()
        .or_else(|| keyword(call, "route"))
        .filter(|e| !falsy(e));
    Some(RouteDef {
        method: func.name.to_string(),
        urls: url.map_or(RouteKey::Absent, |e| given(strings(e))),
        methods: read(call, "methods", |e| match e {
            Expr::NoneLiteral(_) => Some(None),
            e => list(e).map(|elts| Some(elts.iter().filter_map(str_lit).map(str::to_owned).collect())),
        }),
        kind: read(call, "type", |e| str_lit(e).map(str::to_owned)),
        auth: read(call, "auth", |e| str_lit(e).map(str::to_owned)),
        csrf: read(call, "csrf", |e| e.as_boolean_literal_expr().map(|b| b.value)),
    })
}

/// Whether a route argument is one Odoo ignores, setting the URLs only from a truthy one (18.0
/// `odoo/http.py:740`, 20.0 `odoo/http/routing_map.py:198`).
fn falsy(expr: &Expr) -> bool {
    match expr {
        Expr::NoneLiteral(_) => true,
        e => str_lit(e).map_or_else(|| list(e).is_some_and(<[Expr]>::is_empty), str::is_empty),
    }
}

fn read<T>(call: &ast::ExprCall, name: &str, value: impl Fn(&Expr) -> Option<T>) -> RouteKey<T> {
    keyword(call, name).map_or(RouteKey::Absent, |e| given(value(e)))
}

fn given<T>(value: Option<T>) -> RouteKey<T> {
    value.map_or(RouteKey::Unknown, RouteKey::Given)
}

fn list(expr: &Expr) -> Option<&[Expr]> {
    match expr {
        Expr::List(l) => Some(&l.elts),
        Expr::Tuple(t) => Some(&t.elts),
        _ => None,
    }
}

/// A URL literal, or a list of them; one that is not a literal leaves the URLs unread.
fn strings(expr: &Expr) -> Option<Vec<String>> {
    match list(expr) {
        Some(elts) => elts.iter().map(|e| str_lit(e).map(str::to_owned)).collect(),
        None => str_lit(expr).map(|url| vec![url.to_owned()]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::python::analyze;

    /// The table of `files`, each an addon, a path in it and a source.
    fn table(files: &[(&str, &str, &str)]) -> Table {
        let facts: Vec<PyFacts> = files.iter().map(|(_, _, src)| analyze(src, None)).collect();
        Table::of(files.iter().zip(&facts).map(|((addon, rel, _), f)| (*addon, *rel, f)))
    }

    fn routing(table: &Table, (addon, rel): (&str, &str), class: &str, method: &str) -> Option<Routing> {
        Scope { table, addon, rel }.routing(class, method)
    }

    const SHOP: (&str, &str, &str) = (
        "shop",
        "controllers/main.py",
        "from odoo import http\n\nclass Shop(http.Controller):\n    @http.route(['/a', '/a/<int:x>'], type='json', auth='public', methods=['POST'], csrf=False)\n    def a(self):\n        pass\n",
    );

    #[test]
    fn a_three_class_chain_across_files_merges_ancestors_first() {
        let middle = (
            "shop_mid",
            "controllers/mid.py",
            "from odoo.addons.shop.controllers import main\n\nclass Mid(main.Shop):\n    @http.route(auth='user', type='http')\n    def a(self):\n        pass\n",
        );
        let leaf = (
            "shop_leaf",
            "controllers/__init__.py",
            "from odoo.addons.shop_mid.controllers.mid import Mid\n\nclass Leaf(Mid):\n    @http.route('/b', csrf=True)\n    def a(self):\n        pass\n",
        );
        let scan = table(&[SHOP, middle, leaf]);
        let merged = Routing {
            urls: Some(vec!["/b".to_owned()]),
            methods: Some(Some(vec!["POST".to_owned()])),
            kind: Some("json".to_owned()),
            auth: Some("user".to_owned()),
            csrf: Some(true),
        };
        assert_eq!(
            routing(&scan, ("shop_leaf", "controllers/__init__.py"), "Leaf", "a"),
            Some(merged)
        );
        let mid = routing(&scan, ("shop_mid", "controllers/mid.py"), "Mid", "a").unwrap();
        assert_eq!(mid.urls, Some(vec!["/a".to_owned(), "/a/<int:x>".to_owned()]));
        assert_eq!(mid.kind.as_deref(), Some("json"));
    }

    #[test]
    fn a_base_the_scan_does_not_read_leaves_the_keys_after_it_unknown() {
        let src = |bases: &str| {
            format!(
                "from odoo.addons.shop.controllers.main import Shop\n\nclass Ext({bases}):\n    @http.route(auth='none')\n    def a(self):\n        pass\n"
            )
        };
        let after = src("Shop, Mixin");
        let before = src("Mixin, Shop");
        let gone = src("Gone");
        let scan = table(&[
            SHOP,
            ("ext", "a.py", &after),
            ("ext", "b.py", &before),
            ("ext", "c.py", &gone),
        ]);
        // The first base comes last, so Shop's keys override what Mixin may give.
        let after = routing(&scan, ("ext", "a.py"), "Ext", "a").unwrap();
        assert_eq!((after.kind.as_deref(), after.csrf), (None, Some(false)));
        let before = routing(&scan, ("ext", "b.py"), "Ext", "a").unwrap();
        assert_eq!(
            (before.urls, before.csrf, before.auth.as_deref()),
            (None, None, Some("none"))
        );
        let gone = routing(&scan, ("ext", "c.py"), "Ext", "a").unwrap();
        assert_eq!((gone.urls, gone.methods, gone.kind), (None, None, None));
        assert_eq!(routing(&scan, ("ext", "c.py"), "Ext", "b"), None);
        assert_eq!(routing(&scan, ("ext", "d.py"), "Ext", "a"), None);
    }

    #[test]
    fn the_type_is_that_of_the_first_definition() {
        let ext = "from odoo.addons.shop.controllers.main import Shop\nfrom odoo import http\n\nclass Base(http.Controller):\n    @http.route('/c')\n    def c(self):\n        pass\n\nclass Ext(Shop, Base):\n    @http.route(type='http')\n    def a(self):\n        pass\n\n    @http.route(type='json')\n    def c(self):\n        pass\n";
        let scan = table(&[SHOP, ("ext", "main.py", ext)]);
        let kind = |method| routing(&scan, ("ext", "main.py"), "Ext", method).unwrap().kind;
        assert_eq!(kind("a").as_deref(), Some("json"));
        assert_eq!(kind("c").as_deref(), Some("http"));
        let unread = "class Ext(Mixin):\n    @http.route('/a', type='http')\n    def a(self):\n        pass\n";
        let unread = table(&[("ext", "main.py", unread)]);
        assert_eq!(routing(&unread, ("ext", "main.py"), "Ext", "a").unwrap().kind, None);
    }

    #[test]
    fn a_route_inherits_when_its_class_has_a_base_the_scan_may_hold() {
        let inherits = |src: &str| inherits_routes(&analyze(src, None).controllers);
        let route = |bases: &str, args: &str| {
            format!(
                "from odoo import http\nfrom odoo.addons.shop.controllers.main import Shop\n\nclass Base(http.Controller):\n    pass\n\nclass Ext({bases}):\n    @http.route({args})\n    def a(self):\n        pass\n"
            )
        };
        let all = "'/a', methods=None, type='http', auth='user', csrf=True";
        assert!(inherits(&route("Shop", "'/a'")));
        assert!(inherits(&route("Base", "")));
        assert!(inherits(&route("Shop", all)));
        assert!(!inherits(&route("http.Controller", "")));
        assert!(!inherits(&route("Mixin", "")));
        assert!(!inherits(
            "from odoo.addons.shop.controllers.main import Shop\n\nclass Ext(Shop):\n    def a(self):\n        pass\n"
        ));
    }

    #[test]
    fn python_keeps_the_last_class_and_the_last_def_of_a_name() {
        let src = "from odoo import http\n\nclass Shop(http.Controller):\n    @http.route('/first', auth='public')\n    def a(self):\n        pass\n\n    @http.route('/second')\n    def a(self):\n        pass\n\nclass Shop(Shop):\n    @http.route()\n    def a(self):\n        pass\n\n    @http.route('/b', auth='public')\n    def b(self):\n        pass\n\n    def b(self):\n        pass\n";
        let scan = table(&[("shop", "main.py", src)]);
        let a = routing(&scan, ("shop", "main.py"), "Shop", "a").unwrap();
        assert_eq!(
            (a.urls, a.auth.as_deref()),
            (Some(vec!["/second".to_owned()]), Some("user"))
        );
        assert_eq!(routing(&scan, ("shop", "main.py"), "Shop", "b"), None);
    }

    #[test]
    fn an_empty_or_none_route_keeps_the_urls_it_overrides() {
        for args in ["[]", "None", "route=None", "''", "route=()"] {
            let ext = format!(
                "from odoo.addons.shop.controllers.main import Shop\n\nclass Ext(Shop):\n    @http.route({args})\n    def a(self):\n        pass\n"
            );
            let scan = table(&[SHOP, ("ext", "main.py", &ext)]);
            let urls = routing(&scan, ("ext", "main.py"), "Ext", "a").unwrap().urls;
            assert_eq!(urls, Some(vec!["/a".to_owned(), "/a/<int:x>".to_owned()]), "{args}");
        }
    }
}
