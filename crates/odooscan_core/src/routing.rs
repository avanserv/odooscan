//! The routing Odoo gives a controller method: its own `@route` merged with those of the methods it
//! overrides, ancestors first (18.0 `odoo/http.py:831-849`, 20.0 `odoo/http/routing_map.py:296-314`).
//! An override that leaves out a key keeps the one it overrides, and the type stays that of the
//! first definition whatever an override says (18.0 `http.py:871-880`, 20.0 `routing_map.py:336-346`).

use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::{Ranged, TextSize};
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};

use crate::facts::{ClassBase, ControllerFact, PyFacts, RouteDef, RouteKey, Spanned};
use crate::index::class_file;
use crate::module_state;
use crate::project::Module;
use crate::python::{BoundNames, Locator, declares_model, keyword, str_lit, tail_name};

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

    /// The controllers of the Python files of `modules`.
    pub fn of_modules(modules: &[Module]) -> Self {
        let files = modules
            .iter()
            .flat_map(|m| m.python.iter().map(|f| (m.name.as_str(), f.rel.as_str(), &f.facts)));
        Self::of(files)
    }

    /// The controllers of `addon`.
    pub fn module<'t>(&'t self, addon: &str) -> impl Iterator<Item = ClassAt<'t>> {
        let files = self.0.get_key_value(addon).into_iter();
        files.flat_map(|(addon, files)| {
            files
                .iter()
                .flat_map(move |(rel, classes)| classes.iter().map(move |class| ClassAt { addon, rel, class }))
        })
    }

    /// The addons that have controllers.
    pub fn addons(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    /// The class a base of `at` names, none for a base the table does not hold or that names none.
    fn base<'t>(&'t self, at: ClassAt<'t>, base: &ClassBase) -> Option<ClassAt<'t>> {
        let (addon, rel, class) = match base {
            ClassBase::Local(i) => {
                let (rel, classes) = self.file(at.addon, at.rel)?;
                (at.addon, rel, classes.get(*i)?)
            }
            ClassBase::Addon(path) => self.resolve(path)?,
            ClassBase::Root | ClassBase::Object | ClassBase::Unread => return None,
        };
        Some(ClassAt { addon, rel, class })
    }

    /// The classes `at` is made of, itself included, none when a base is one the table does not
    /// hold, whose own bases may be anything, or when none is Odoo's `Controller`, so that Odoo
    /// builds no controller of it.
    pub fn lineage(&self, at: ClassAt<'_>) -> Option<Lineage> {
        let mut lineage = Lineage::default();
        let mut todo = vec![at];
        while let Some(c) = todo.pop() {
            if !lineage.classes.insert(std::ptr::from_ref(c.class)) {
                continue;
            }
            for base in &c.class.bases {
                match base {
                    ClassBase::Root => _ = lineage.tops.insert(std::ptr::from_ref(c.class)),
                    ClassBase::Object => {}
                    base => todo.push(self.base(c, base)?),
                }
            }
        }
        (!lineage.tops.is_empty()).then_some(lineage)
    }

    /// The nearest ancestor of `at` that routes `method`, when `at` is a controller: a held
    /// base reaches Odoo's `Controller`.
    pub fn routed_ancestor<'t>(&'t self, at: ClassAt<'t>, method: &str) -> Option<ClassAt<'t>> {
        let mut seen = FxHashSet::default();
        let mut todo = vec![at];
        let (mut routed, mut rooted) = (None, false);
        while let Some(c) = todo.pop() {
            if !seen.insert(std::ptr::from_ref(c.class)) {
                continue;
            }
            let routes = c.class.routes.iter().any(|r| r.method == method);
            if routed.is_none() && routes && !std::ptr::eq(c.class, at.class) {
                routed = Some(c);
            }
            rooted |= c.class.bases.contains(&ClassBase::Root);
            // Pushed in reverse, so that the first base and its own bases are walked first.
            todo.extend(c.class.bases.iter().rev().filter_map(|b| self.base(c, b)));
        }
        routed.filter(|_| rooted)
    }

    /// The controllers of a file, with its path as the table holds it.
    fn file<'t>(&'t self, addon: &str, rel: &str) -> Option<(&'t str, &'t [ControllerFact])> {
        let (rel, classes) = self.0.get(addon)?.get_key_value(rel)?;
        Some((rel.as_str(), classes))
    }

    /// The class of a file that starts at `start`.
    fn at<'t>(&'t self, addon: &str, rel: &str, start: TextSize) -> Option<(&'t str, &'t ControllerFact)> {
        let (rel, classes) = self.file(addon, rel)?;
        Some((rel, classes.iter().find(|c| c.start == start)?))
    }

    /// The class `name` of a file, the last of that name, which an import of the name binds.
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

/// A class of the table, with its addon and its file.
#[derive(Clone, Copy, Debug)]
pub struct ClassAt<'t> {
    pub addon: &'t str,
    pub rel: &'t str,
    pub class: &'t ControllerFact,
}

/// The classes a controller class is made of, by address.
#[derive(Debug, Default)]
pub struct Lineage {
    classes: FxHashSet<*const ControllerFact>,
    /// Those with Odoo's `Controller` among their own bases, each of which Odoo builds one
    /// controller of, merged with all of its subclasses.
    tops: FxHashSet<*const ControllerFact>,
}

impl Lineage {
    pub fn holds(&self, class: &ControllerFact) -> bool {
        self.classes.contains(&std::ptr::from_ref(class))
    }

    /// Whether Odoo merges both classes into one controller.
    pub fn shares_top(&self, other: &Self) -> bool {
        !self.tops.is_disjoint(&other.tops)
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
    /// The merged routing of `method` of the file's class that starts at `start`, none when the
    /// table does not hold the method's own route.
    pub fn routing(&self, start: TextSize, method: &str) -> Option<Routing> {
        let (rel, own) = self.table.at(self.addon, self.rel, start)?;
        if !own.routes.iter().any(|r| r.method == method) {
            return None;
        }
        let mut walk = Walk {
            table: self.table,
            method,
            seen: FxHashSet::default(),
            layers: vec![],
        };
        walk.class(ClassAt {
            addon: self.addon,
            rel,
            class: own,
        });
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
    fn class(&mut self, at: ClassAt<'t>) {
        if !self.seen.insert(std::ptr::from_ref(at.class)) {
            return;
        }
        // The first base comes last in the reversed method resolution order.
        for base in at.class.bases.iter().rev() {
            self.base(at, base);
        }
        let own = at.class.routes.iter().find(|r| r.method == self.method);
        self.layers.extend(own.map(Some));
    }

    fn base(&mut self, at: ClassAt<'t>, base: &ClassBase) {
        match self.table.base(at, base) {
            Some(class) => self.class(class),
            None if matches!(base, ClassBase::Root | ClassBase::Object) => {}
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
/// methods and the methods they keep undecorated.
pub(crate) fn controllers(body: &[Stmt], loc: &Locator) -> Vec<ControllerFact> {
    // The names bound above, in source order, so that a base reads the nearest binding.
    let mut bound: Vec<(&str, Binding)> = vec![];
    let mut controllers = vec![];
    for stmt in module_state::top_level(body) {
        let Stmt::ClassDef(class) = stmt else {
            bound.extend(import_bindings(stmt));
            continue;
        };
        let controller = (!class.bases().is_empty() && !declares_model(class)).then_some(controllers.len());
        if controller.is_some() {
            let defs = last_defs(&class.body);
            let plain = defs.iter().filter(|f| f.decorator_list.is_empty()).map(|f| Spanned {
                value: f.name.to_string(),
                pos: loc.at(&f.name),
            });
            controllers.push(ControllerFact {
                name: class.name.to_string(),
                start: class.start(),
                bases: class.bases().iter().map(|b| base(b, &bound)).collect(),
                plain: plain.collect(),
                routes: defs.into_iter().filter_map(|f| route_def(f, loc)).collect(),
            });
        }
        bound.push((class.name.as_str(), Binding::Class(controller)));
    }
    controllers
}

/// What a name at the top of a file is bound to, for a class below that names it as a base.
enum Binding {
    /// An import from `odoo`, with the dotted path of what it names.
    Odoo(String),
    /// Any other import, a relative one included, whose classes the scan does not look up.
    Import,
    /// A class of the file, with its place among the controllers when it is one.
    Class(Option<usize>),
}

/// The names an import binds, in order.
fn import_bindings(stmt: &Stmt) -> Vec<(&str, Binding)> {
    let (aliases, relative) = match stmt {
        Stmt::Import(i) => (&i.names, false),
        Stmt::ImportFrom(i) => (&i.names, i.level > 0),
        _ => return vec![],
    };
    let odoo = if relative {
        vec![]
    } else {
        module_state::imported_names(&[stmt], "odoo")
    };
    let mut names = BoundNames::default();
    names.aliases(aliases);
    let binding = |name: &str| match odoo.iter().rfind(|(local, _)| *local == name) {
        Some((_, path)) => Binding::Odoo(path.clone()),
        None => Binding::Import,
    };
    names.0.into_iter().map(|name| (name, binding(name))).collect()
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

/// A base as the last binding of its name above the class gives it: a class of `odoo.addons`,
/// Odoo's `Controller`, or a controller of the file.
fn base(expr: &Expr, bound: &[(&str, Binding)]) -> ClassBase {
    let Some(name) = module_state::root(expr) else {
        return ClassBase::Unread;
    };
    match bound.iter().rfind(|(n, _)| *n == name).map(|(_, b)| b) {
        Some(Binding::Odoo(source)) => {
            let path = module_state::imported_path(expr, &[(name, source.clone())]).unwrap_or_default();
            odoo_base(path)
        }
        Some(Binding::Class(Some(at))) if expr.is_name_expr() => ClassBase::Local(*at),
        None if name == "object" && expr.is_name_expr() => ClassBase::Object,
        _ => ClassBase::Unread,
    }
}

/// A base an import from `odoo` names, by its dotted path.
fn odoo_base(path: String) -> ClassBase {
    if matches!(
        path.as_str(),
        "odoo.http.Controller" | "odoo.http.routing_map.Controller"
    ) {
        ClassBase::Root
    } else if path.starts_with("odoo.addons.") {
        ClassBase::Addon(path)
    } else {
        ClassBase::Unread
    }
}

/// The route of a method, from its outermost `@route`, the one whose routing Odoo keeps.
fn route_def(func: &ast::StmtFunctionDef, loc: &Locator) -> Option<RouteDef> {
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
        pos: loc.at(&func.name),
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

    fn routing(table: &Table, file: (&str, &str), class: &str, method: &str) -> Option<Routing> {
        nth_routing(table, file, (class, 0), method)
    }

    /// The routing of `method` of the `nth` class named `class` in the file.
    fn nth_routing(
        table: &Table,
        (addon, rel): (&str, &str),
        (class, nth): (&str, usize),
        method: &str,
    ) -> Option<Routing> {
        let (_, classes) = table.file(addon, rel)?;
        let start = classes.iter().filter(|c| c.name == class).nth(nth)?.start;
        Scope { table, addon, rel }.routing(start, method)
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
    fn a_subclass_of_a_same_named_class_reads_the_last_def_of_each_method() {
        let src = "from odoo import http\n\nclass Shop(http.Controller):\n    @http.route('/first', auth='public')\n    def a(self):\n        pass\n\n    @http.route('/second')\n    def a(self):\n        pass\n\nclass Shop(Shop):\n    @http.route()\n    def a(self):\n        pass\n\n    @http.route('/b', auth='public')\n    def b(self):\n        pass\n\n    def b(self):\n        pass\n";
        let scan = table(&[("shop", "main.py", src)]);
        let a = nth_routing(&scan, ("shop", "main.py"), ("Shop", 1), "a").unwrap();
        assert_eq!(
            (a.urls, a.auth.as_deref()),
            (Some(vec!["/second".to_owned()]), Some("user"))
        );
        assert_eq!(nth_routing(&scan, ("shop", "main.py"), ("Shop", 1), "b"), None);
    }

    #[test]
    fn two_classes_of_one_name_each_read_their_own_routing() {
        let class = |auth: &str| {
            format!(
                "class Main(http.Controller):\n    @http.route('/a', auth='{auth}')\n    def a(self):\n        pass\n"
            )
        };
        let src = format!("from odoo import http\n\n{}\n{}", class("public"), class("user"));
        let scan = table(&[("shop", "main.py", &src)]);
        let auth = |nth| {
            nth_routing(&scan, ("shop", "main.py"), ("Main", nth), "a")
                .unwrap()
                .auth
        };
        assert_eq!((auth(0).as_deref(), auth(1).as_deref()), (Some("public"), Some("user")));
    }

    #[test]
    fn a_baseless_class_named_like_a_controller_merges_no_routing() {
        let src = "from odoo import http\n\nclass Shop(http.Controller):\n    @http.route('/a', auth='public')\n    def a(self):\n        pass\n\nclass Shop:\n    @http.route('/b')\n    def a(self):\n        pass\n";
        let scan = table(&[("shop", "main.py", src)]);
        let scope = Scope {
            table: &scan,
            addon: "shop",
            rel: "main.py",
        };
        let baseless = TextSize::try_from(src.rfind("class Shop:").unwrap()).unwrap();
        assert_eq!(scope.routing(baseless, "a"), None);
        let controller = routing(&scan, ("shop", "main.py"), "Shop", "a").unwrap();
        assert_eq!(controller.auth.as_deref(), Some("public"));
    }

    #[test]
    fn a_class_rebinding_an_imported_name_is_the_base_of_the_classes_below() {
        let ext = "from odoo import http\nfrom odoo.addons.shop.controllers.main import Shop\n\nclass Shop(Shop):\n    @http.route(auth='user')\n    def a(self):\n        pass\n\nclass Leaf(Shop):\n    @http.route()\n    def a(self):\n        pass\n";
        let scan = table(&[SHOP, ("ext", "main.py", ext)]);
        let leaf = routing(&scan, ("ext", "main.py"), "Leaf", "a").unwrap();
        assert_eq!(
            (leaf.auth.as_deref(), leaf.kind.as_deref(), leaf.csrf),
            (Some("user"), Some("json"), Some(false))
        );
    }

    #[test]
    fn a_base_reads_only_the_nearest_binding_above_it() {
        let bases = |src: &str| {
            analyze(src, None)
                .controllers
                .iter()
                .map(|c| c.bases.clone())
                .collect::<Vec<_>>()
        };
        let below = "from odoo import http\n\nclass Ext(Shop):\n    pass\n\nfrom odoo.addons.shop.controllers.main import Shop\n";
        assert_eq!(bases(below), [[ClassBase::Unread]]);
        for import in [
            "from .main import Shop",
            "from shop_lib import Shop",
            "import shop_lib as Shop",
        ] {
            let src = format!(
                "from odoo import http\n\nclass Shop(http.Controller):\n    pass\n\n{import}\n\nclass Ext(Shop):\n    pass\n"
            );
            assert_eq!(
                bases(&src),
                [vec![ClassBase::Root], vec![ClassBase::Unread]],
                "{import}"
            );
        }
        let nearer = "from odoo import http\n\nclass Shop(http.Controller):\n    pass\n\nfrom odoo.addons.shop.controllers.main import Shop\n\nclass Ext(Shop):\n    pass\n";
        let addon = ClassBase::Addon("odoo.addons.shop.controllers.main.Shop".to_owned());
        assert_eq!(bases(nearer)[1], [addon]);
    }

    #[test]
    fn an_object_base_is_no_controller_root() {
        let src = "from odoo import http\n\nclass Mixin(object):\n    pass\n\nclass Main(Mixin, http.Controller):\n    pass\n";
        let bases: Vec<_> = analyze(src, None).controllers.into_iter().map(|c| c.bases).collect();
        assert_eq!(
            bases,
            [vec![ClassBase::Object], vec![ClassBase::Local(0), ClassBase::Root]]
        );
    }

    #[test]
    fn a_class_keeps_its_undecorated_methods_and_its_routes_at_the_def_name() {
        let src = "from odoo import http\n\nclass Main(http.Controller):\n    @http.route('/a')\n    def a(self):\n        pass\n\n    def a(self):\n        pass\n\n    @staticmethod\n    def b():\n        pass\n\n    def c(self):\n        pass\n\n    @http.route('/d')\n    def d(self):\n        pass\n";
        let main = analyze(src, None).controllers.remove(0);
        let plain: Vec<_> = main
            .plain
            .iter()
            .map(|p| (p.value.as_str(), p.pos.line, p.pos.col))
            .collect();
        assert_eq!(plain, [("a", 8, 9), ("c", 15, 9)]);
        let routes: Vec<_> = main
            .routes
            .iter()
            .map(|r| (r.method.as_str(), r.pos.line, r.pos.col))
            .collect();
        assert_eq!(routes, [("d", 19, 9)]);
    }

    /// The `nth` class named `class` of a file of the table.
    fn class_at<'t>(table: &'t Table, (addon, rel): (&str, &str), (class, nth): (&str, usize)) -> ClassAt<'t> {
        let at = table
            .module(addon)
            .filter(|c| c.rel == rel && c.class.name == class)
            .nth(nth);
        at.unwrap_or_else(|| panic!("{addon}/{rel} has no class {class}"))
    }

    #[test]
    fn a_lineage_holds_the_ancestors_and_tells_merged_controllers() {
        let ext = "from odoo import http\nfrom odoo.addons.shop.controllers.main import Shop\n\nclass Left(Shop):\n    pass\n\nclass Right(Shop):\n    pass\n\nclass Mixin(object):\n    pass\n\nclass Own(Mixin, http.Controller):\n    pass\n\nclass Other(Mixin, http.Controller):\n    pass\n";
        let scan = table(&[SHOP, ("ext", "main.py", ext)]);
        let at = |class| class_at(&scan, ("ext", "main.py"), (class, 0));
        let shop = class_at(&scan, ("shop", "controllers/main.py"), ("Shop", 0));
        let [left, right, own, other] = ["Left", "Right", "Own", "Other"].map(|c| scan.lineage(at(c)).unwrap());
        assert!(left.holds(shop.class) && !left.holds(at("Right").class));
        assert!(left.shares_top(&right));
        // Each class with `Controller` among its own bases is a controller of its own.
        assert!(own.holds(at("Mixin").class) && !own.shares_top(&other));
        assert!(!left.shares_top(&own));
        assert!(scan.lineage(at("Mixin")).is_none());
    }

    #[test]
    fn a_lineage_is_unknown_past_a_base_the_table_does_not_hold() {
        let ext = "from odoo import http\nfrom odoo.addons.gone.controllers.main import Gone\nfrom .main import Near\n\nclass Lost(Gone):\n    pass\n\nclass Relative(Near, http.Controller):\n    pass\n\nclass Leaf(Lost):\n    pass\n";
        let scan = table(&[("ext", "main.py", ext)]);
        for class in ["Lost", "Relative", "Leaf"] {
            assert!(
                scan.lineage(class_at(&scan, ("ext", "main.py"), (class, 0))).is_none(),
                "{class}"
            );
        }
    }

    #[test]
    fn the_routed_ancestor_is_found_across_modules_for_a_controller() {
        let mid = (
            "shop_mid",
            "controllers/mid.py",
            "from odoo.addons.shop.controllers import main\n\nclass Mid(main.Shop):\n    def a(self):\n        pass\n",
        );
        let leaf = "from odoo.addons.shop_mid.controllers.mid import Mid\nfrom odoo import http\n\nclass Leaf(Mid):\n    def a(self):\n        pass\n\nclass Mixin(object):\n    @http.route('/m')\n    def m(self):\n        pass\n\nclass Plain(Mixin):\n    def m(self):\n        pass\n\nclass Served(Mixin, http.Controller):\n    def m(self):\n        pass\n";
        let scan = table(&[SHOP, mid, ("shop_leaf", "main.py", leaf)]);
        let at = |class| class_at(&scan, ("shop_leaf", "main.py"), (class, 0));
        let found = scan.routed_ancestor(at("Leaf"), "a").unwrap();
        assert_eq!((found.addon, found.class.name.as_str()), ("shop", "Shop"));
        assert!(scan.routed_ancestor(at("Leaf"), "b").is_none());
        let shop = class_at(&scan, ("shop", "controllers/main.py"), ("Shop", 0));
        assert!(scan.routed_ancestor(shop, "a").is_none());
        // A mixin's route counts only in a class Odoo builds a controller of.
        assert!(scan.routed_ancestor(at("Plain"), "m").is_none());
        assert_eq!(scan.routed_ancestor(at("Served"), "m").unwrap().class.name, "Mixin");
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
