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

    /// The endpoints Odoo builds of `classes`, the served controllers of one database, that serve
    /// literal paths. Odoo builds one controller per class with `Controller` among its own bases,
    /// of the leaves of its subclasses among `classes`, and per method keeps each key of the last
    /// definition giving it (18.0 `odoo/http.py:778-789`, `:838-849`, 20.0
    /// `odoo/http/routing_map.py:243-254`, `:303-314`). The scan cannot order the leaves, so a key
    /// is read only where they agree once a definition another overrides is left out.
    pub fn endpoints<'t>(&'t self, classes: &[ClassAt<'t>]) -> Vec<Endpoint<'t>> {
        let held: FxHashSet<_> = classes.iter().map(|c| std::ptr::from_ref(c.class)).collect();
        let mut children = Children::default();
        for &c in classes {
            for base in self.bases(c).filter(|b| held.contains(&std::ptr::from_ref(b.class))) {
                children.entry(std::ptr::from_ref(base.class)).or_default().push(c);
            }
        }
        let tops = classes.iter().filter(|c| c.class.bases.contains(&ClassBase::Root));
        tops.flat_map(|&top| self.family_endpoints(&leaves(top, &children)))
            .collect()
    }

    /// The endpoints of the controller Odoo builds of `leaves`, for a family that spells a literal
    /// path in its classes or their ancestors.
    fn family_endpoints<'t>(&'t self, leaves: &[ClassAt<'t>]) -> Vec<Endpoint<'t>> {
        let routes = self.routes_above(leaves);
        if !routes.iter().any(|r| matches!(r.urls, RouteKey::Given(_))) {
            return vec![];
        }
        let mut methods: Vec<&str> = routes.iter().map(|r| r.method.as_str()).collect();
        methods.sort_unstable();
        methods.dedup();
        methods.into_iter().filter_map(|m| self.endpoint(leaves, m)).collect()
    }

    /// The routes of `leaves` and of the classes they inherit.
    fn routes_above<'t>(&'t self, leaves: &[ClassAt<'t>]) -> Vec<&'t RouteDef> {
        let mut seen = FxHashSet::default();
        let mut routes = vec![];
        let mut todo = leaves.to_vec();
        while let Some(c) = todo.pop() {
            if seen.insert(std::ptr::from_ref(c.class)) {
                routes.extend(&c.class.routes);
                todo.extend(self.bases(c));
            }
        }
        routes
    }

    /// The endpoint of `method` in the controller Odoo builds of `leaves`, none when the scan
    /// cannot tell its paths or no definition gives any.
    fn endpoint<'t>(&'t self, leaves: &[ClassAt<'t>], method: &str) -> Option<Endpoint<'t>> {
        let walks: Vec<_> = leaves.iter().map(|&leaf| self.walk(leaf, method)).collect();
        let (urls, origins) = self.decide(&walks, |r| &r.urls)?;
        let methods = self.decide(&walks, |r| &r.methods).map(|(m, _)| m.cloned().flatten());
        Some(Endpoint {
            urls: urls?,
            origins,
            methods,
        })
    }

    /// A key over the walks of the leaves, with the routes that give it: the last giving layer of
    /// each, less one a subclass of another overrides. None when one is a base the scan does not
    /// read or the rest disagree or are not literal, and no value when no route gives it.
    fn decide<'t, T: PartialEq>(
        &'t self,
        walks: &[Vec<Option<Origin<'t>>>],
        key: impl Fn(&RouteDef) -> &RouteKey<T>,
    ) -> Option<(Option<&'t T>, Vec<Origin<'t>>)> {
        let mut givers: Vec<Origin<'t>> = vec![];
        for walk in walks {
            match last_giver(walk, &key) {
                None => {}
                Some(None) => return None,
                Some(Some(o)) if givers.iter().any(|g| std::ptr::eq(g.route, o.route)) => {}
                Some(Some(o)) => givers.push(*o),
            }
        }
        let overridden = |g: &&Origin<'t>| givers.iter().any(|o| self.inherits(o.at, g.at.class));
        let kept: Vec<Origin<'t>> = givers.iter().filter(|g| !overridden(g)).copied().collect();
        let Some(first) = kept.first() else {
            return Some((None, kept));
        };
        let RouteKey::Given(value) = key(first.route) else {
            return None;
        };
        let agree = kept
            .iter()
            .all(|g| matches!(key(g.route), RouteKey::Given(v) if v == value));
        agree.then_some((Some(value), kept))
    }

    /// Whether `at` inherits `ancestor` through bases the table holds.
    fn inherits(&self, at: ClassAt<'_>, ancestor: &ControllerFact) -> bool {
        let mut seen = FxHashSet::default();
        let mut todo: Vec<_> = self.bases(at).collect();
        while let Some(c) = todo.pop() {
            if std::ptr::eq(c.class, ancestor) {
                return true;
            }
            if seen.insert(std::ptr::from_ref(c.class)) {
                todo.extend(self.bases(c));
            }
        }
        false
    }

    /// The definitions of `method` in `at` and its ancestors, as `Walk` lays them out.
    fn walk<'t>(&'t self, at: ClassAt<'t>, method: &str) -> Vec<Option<Origin<'t>>> {
        let mut walk = Walk {
            table: self,
            method,
            seen: FxHashSet::default(),
            layers: vec![],
        };
        walk.class(at);
        walk.layers
    }

    /// The classes the bases of `at` name that the table holds.
    fn bases<'t>(&'t self, at: ClassAt<'t>) -> impl Iterator<Item = ClassAt<'t>> {
        at.class.bases.iter().filter_map(move |b| self.base(at, b))
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

/// The classes of a scope by each class they name as a base.
type Children<'t> = FxHashMap<*const ControllerFact, Vec<ClassAt<'t>>>;

/// The leaves of the family of `top`: it and its descendants in `children`, those none descends from.
fn leaves<'t>(top: ClassAt<'t>, children: &Children<'t>) -> Vec<ClassAt<'t>> {
    let mut seen = FxHashSet::default();
    let mut leaves = vec![];
    let mut todo = vec![top];
    while let Some(c) = todo.pop() {
        if !seen.insert(std::ptr::from_ref(c.class)) {
            continue;
        }
        match children.get(&std::ptr::from_ref(c.class)) {
            Some(below) => todo.extend(below),
            None => leaves.push(c),
        }
    }
    leaves
}

/// A route of a class of the table.
#[derive(Clone, Copy, Debug)]
pub struct Origin<'t> {
    pub at: ClassAt<'t>,
    pub route: &'t RouteDef,
}

/// A method of a controller Odoo builds, with the literal paths it serves.
#[derive(Debug)]
pub struct Endpoint<'t> {
    pub urls: &'t [String],
    /// The routes that give `urls`, several when the leaves of the controller agree on them.
    pub origins: Vec<Origin<'t>>,
    /// The literal request methods, none inside when every method passes, none when the scan
    /// cannot tell them.
    pub methods: Option<Option<Vec<String>>>,
}

/// Where the file a scan reads sits among the controllers of the scan. `rel` also decides whether
/// the scan reads `migrate`, so a stage script read with the default scope never gives OD515.
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
        let at = ClassAt {
            addon: self.addon,
            rel,
            class: own,
        };
        let layers = &self.table.walk(at, method);
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
    layers: Vec<Option<Origin<'t>>>,
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
        self.layers.extend(own.map(|route| Some(Origin { at, route })));
    }

    fn base(&mut self, at: ClassAt<'t>, base: &ClassBase) {
        match self.table.base(at, base) {
            Some(class) => self.class(class),
            None if matches!(base, ClassBase::Root | ClassBase::Object) => {}
            None => self.layers.push(None),
        }
    }
}

/// The last layer that gives a key, none when no layer does. A base the scan does not read may
/// give any key.
fn last_giver<'l, 't, T>(
    layers: &'l [Option<Origin<'t>>],
    key: impl Fn(&RouteDef) -> &RouteKey<T>,
) -> Option<&'l Option<Origin<'t>>> {
    let absent = |layer: &Option<Origin<'t>>| layer.is_some_and(|o| matches!(key(o.route), RouteKey::Absent));
    layers.iter().rev().find(|layer| !absent(layer))
}

/// A key the last definition that gives it decides, `default` when none does.
fn last_given<T: Clone>(
    layers: &[Option<Origin<'_>>],
    default: T,
    key: impl Fn(&RouteDef) -> &RouteKey<T>,
) -> Option<T> {
    match last_giver(layers, &key) {
        None => Some(default),
        Some(Some(o)) => match key(o.route) {
            RouteKey::Given(value) => Some(value.clone()),
            RouteKey::Absent | RouteKey::Unknown => None,
        },
        Some(None) => None,
    }
}

/// The type of the first definition, `http` when it gives none.
fn first_kind(layers: &[Option<Origin<'_>>]) -> Option<String> {
    match &layers.first()?.as_ref()?.route.kind {
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
                pos: loc.at(&class.name),
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
    fn a_family_serves_each_method_on_the_paths_its_leaves_agree_on() {
        let ext = "from odoo import http\nfrom odoo.addons.shop.controllers.main import Shop\n\nclass Moved(Shop):\n    @http.route('/moved', methods=['GET'])\n    def a(self):\n        pass\n\nclass Aside(Shop):\n    @http.route('/c')\n    def c(self):\n        pass\n\n    @http.route('/a')\n    def renamed(self):\n        pass\n";
        let scan = table(&[SHOP, ("ext", "main.py", ext)]);
        assert_eq!(
            endpoints_of(&scan, |_| true),
            [
                ("a", vec!["/moved"], vec!["Moved"], only(&["GET"])),
                ("c", vec!["/c"], vec!["Aside"], Some(None)),
                ("renamed", vec!["/a"], vec!["Aside"], Some(None)),
            ]
        );
        // A subclass the database does not serve moves nothing.
        assert_eq!(
            endpoints_of(&scan, |c| c.addon == "shop"),
            [("a", vec!["/a", "/a/<int:x>"], vec!["Shop"], only(&["POST"]))]
        );
    }

    #[test]
    fn siblings_that_disagree_or_sit_on_an_unread_base_leave_the_paths_unknown() {
        let ext = |left: &str, right: &str, bases: &str| {
            format!(
                "from odoo import http\nfrom odoo.addons.shop.controllers.main import Shop\n\nclass Left(Shop):\n    @http.route({left})\n    def a(self):\n        pass\n\nclass Right({bases}):\n    @http.route({right})\n    def a(self):\n        pass\n"
            )
        };
        let check = |src: &str, expected: &[Summary]| {
            let scan = table(&[SHOP, ("ext", "main.py", src)]);
            assert_eq!(endpoints_of(&scan, |_| true), expected, "{src}");
        };
        let both = vec!["Left", "Right"];
        check(
            &ext("'/b'", "'/b'", "Shop"),
            &[("a", vec!["/b"], both.clone(), only(&["POST"]))],
        );
        check(&ext("'/b'", "'/c'", "Shop"), &[]);
        check(&ext("'/b'", "PATH", "Shop"), &[]);
        // Right gives no path, so Shop's comes through it, and Left overrides Shop.
        let left = ("a", vec!["/b"], vec!["Left"], only(&["GET"]));
        let unknown = ("a", vec!["/b"], vec!["Left"], None);
        check(&ext("'/b', methods=['GET']", "methods=None", "Shop"), &[unknown]);
        check(&ext("'/b'", "methods=['GET']", "Shop"), &[left]);
        check(&ext("'/b'", "methods=['GET']", "Mixin, Shop"), &[]);
        check(
            &ext("'/b', methods=VERBS", "'/b'", "Shop"),
            &[("a", vec!["/b"], both, None)],
        );
    }

    #[test]
    fn a_sibling_without_the_method_keeps_the_path_of_the_other() {
        let ext = "from odoo import http\nfrom odoo.addons.shop.controllers.main import Shop\n\nclass Left(Shop):\n    @http.route('/b')\n    def a(self):\n        pass\n\nclass Right(Shop):\n    def other(self):\n        pass\n";
        let scan = table(&[SHOP, ("ext", "main.py", ext)]);
        assert_eq!(
            endpoints_of(&scan, |_| true),
            [("a", vec!["/b"], vec!["Left"], only(&["POST"]))]
        );
    }

    #[test]
    fn each_top_is_a_controller_and_a_mixin_routes_only_through_one() {
        let src = "from odoo import http\n\nclass First(http.Controller):\n    @http.route('/t')\n    def t(self):\n        pass\n\nclass Second(http.Controller):\n    @http.route('/t')\n    def t(self):\n        pass\n\nclass Mixin(object):\n    @http.route('/m')\n    def m(self):\n        pass\n\nclass Plain(Mixin):\n    pass\n\nclass Served(Mixin, http.Controller):\n    pass\n";
        let scan = table(&[("shop", "main.py", src)]);
        assert_eq!(
            endpoints_of(&scan, |_| true),
            [
                ("m", vec!["/m"], vec!["Mixin"], Some(None)),
                ("t", vec!["/t"], vec!["First"], Some(None)),
                ("t", vec!["/t"], vec!["Second"], Some(None)),
            ]
        );
    }

    /// An endpoint as its method, paths, origin classes and request methods.
    type Summary<'t> = (&'t str, Vec<&'t str>, Vec<&'t str>, Option<Option<Vec<String>>>);

    /// The request methods `verbs` alone.
    fn only(verbs: &[&str]) -> Option<Option<Vec<String>>> {
        Some(Some(verbs.iter().map(|&v| v.to_owned()).collect()))
    }

    /// The endpoints of the classes of `table` that `keep` keeps, sorted.
    fn endpoints_of(table: &Table, keep: impl Fn(&ClassAt) -> bool) -> Vec<Summary<'_>> {
        let classes: Vec<_> = table
            .addons()
            .flat_map(|a| table.module(a))
            .filter(|c| keep(c))
            .collect();
        let mut summary: Vec<_> = table
            .endpoints(&classes)
            .into_iter()
            .map(|e| {
                let mut names: Vec<_> = e.origins.iter().map(|o| o.at.class.name.as_str()).collect();
                names.sort_unstable();
                (
                    e.origins[0].route.method.as_str(),
                    e.urls.iter().map(String::as_str).collect(),
                    names,
                    e.methods,
                )
            })
            .collect();
        summary.sort();
        summary
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
