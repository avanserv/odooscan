use globset::{GlobBuilder, GlobSetBuilder};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{Annotation, ArchElement, ClassFact, FieldFact, Heritage, JsClass, ViewFact};
use crate::project::Module;

/// Fields every model has without declaring them.
const MAGIC_FIELDS: &[&str] = &[
    "id",
    "display_name",
    "create_uid",
    "create_date",
    "write_uid",
    "write_date",
    "__last_update",
];

pub const RELATIONAL_TYPES: &[&str] = &["Many2one", "One2many", "Many2many"];

/// Types whose value is not a record of a fixed comodel, so a path cannot continue through them.
const OPAQUE_TYPES: &[&str] = &["Reference", "Many2oneReference", "Properties", "Json"];

/// Fields declared outside the class body, which the extractor cannot see.
const EXTRA_FIELDS: &[(&str, &str)] = &[("ir.rule", "global")];

/// BaseModel methods that fields commonly name as compute or search.
const BASE_METHODS: &[&str] = &["_compute_display_name", "_search_display_name"];

/// Methods of `BaseModel`, written from the ORM API reference: models inherit them from a class
/// outside every addons directory, so the index never sees them.
const BASE_MODEL_METHODS: &[&str] = &[
    // Records and environment
    "browse",
    "exists",
    "ensure_one",
    "sudo",
    "with_user",
    "with_company",
    "with_context",
    "with_env",
    "with_prefetch",
    "mapped",
    "filtered",
    "filtered_domain",
    "sorted",
    "grouped",
    "invalidate_recordset",
    "invalidate_model",
    "modified",
    "flush_recordset",
    "flush_model",
    // Create, read, update, delete
    "create",
    "write",
    "unlink",
    "read",
    "copy",
    "copy_data",
    "copy_multi",
    "default_get",
    "name_create",
    "_create",
    "_write",
    "_read",
    "fetch",
    "update",
    "toggle_active",
    "action_archive",
    "action_unarchive",
    // Search and grouping
    "search",
    "search_count",
    "search_fetch",
    "search_read",
    "name_search",
    "_search",
    "_name_search",
    "read_group",
    "_read_group",
    "_compute_display_name",
    "_search_display_name",
    // Fields, views and onchange
    "fields_get",
    "get_views",
    "get_view",
    "_get_view",
    "_get_view_cache_key",
    "onchange",
    // Access
    "check_access",
    "has_access",
    "check_access_rights",
    "check_access_rule",
    "_filter_access_rules",
    "_check_company",
    "check_field_access_rights",
    // Import, export and translations
    "load",
    "export_data",
    "_load_records",
    "_load_records_create",
    "_load_records_write",
    "get_field_translations",
    "update_field_translations",
    // Model setup and lifecycle
    "init",
    "_auto_init",
    "_table_query",
    "_register_hook",
    "_unregister_hook",
    "_validate_fields",
    "_check_recursion",
    "_check_m2m_recursion",
    "_parent_store_compute",
    "_compute_field_value",
    "_get_placeholder_filename",
    // Overridden by Odoo's own addons, so part of the API in practice
    "new",
    "get_base_url",
    "copy_translations",
    "_access_domain",
    "_add_inherited_fields",
    "_add_missing_default_values",
    "_add_precomputed_values",
    "_additional_allowed_keys_properties_definition",
    "_can_return_content",
    "_check_access",
    "_condition_to_sql",
    "_determine_fields_to_fetch",
    "_fetch_query",
    "_field_to_sql",
    "_flush",
    "_get_base_lang",
    "_has_field_access",
    "_init_column",
    "_make_access_error_message",
    "_order_field_to_sql",
    "_order_to_sql",
    "_post_model_setup__",
    "_prepare_create_values",
    "_read_format",
    "_read_group_groupby",
    "_read_group_orderby",
    "_read_group_postprocess_aggregate",
    "_read_group_select",
    "_read_grouping_sets",
    "_setup_complete",
    "_update_field_translations",
    "_valid_field_parameter",
    "_where_calc",
];

pub fn full_id(module: &str, id: &str) -> String {
    if id.contains('.') {
        id.to_owned()
    } else {
        format!("{module}.{id}")
    }
}

fn model_xmlid(model: &str) -> String {
    format!("model_{}", model.replace('.', "_"))
}

type Models<'m> = FxHashMap<&'m str, Vec<(usize, &'m ClassFact)>>;

type XmlIds = FxHashMap<String, Vec<XmlIdDef>>;

/// Every definition of a view id: a module may redefine another module's view.
type Views<'m> = FxHashMap<String, Vec<ViewNode<'m>>>;

struct ViewNode<'m> {
    module: usize,
    inherit: Option<String>,
    fact: &'m ViewFact,
}

/// The elements of the views in one inheritance tree: those of views in a module's scope, and for
/// the others the first module that adds each.
#[derive(Default)]
pub struct ViewTree<'m> {
    pub root: String,
    pub visible: FxHashSet<ArchElement>,
    pub hidden: FxHashMap<ArchElement, &'m str>,
}

/// `%(id)d` names an action by XML id, which Odoo resolves against the module of the view.
pub fn located(e: &ArchElement, module: &str) -> ArchElement {
    let value = match e.value.strip_prefix("%(").and_then(|v| v.strip_suffix(")d")) {
        Some(id) => format!("%({})d", full_id(module, id)),
        None => e.value.clone(),
    };
    ArchElement {
        tag: e.tag.clone(),
        attr: e.attr,
        value,
    }
}

/// The ids base generates for a module and for each level of its category.
fn module_ids(module: &Module, base: usize, xmlids: &mut XmlIds) {
    xmlids
        .entry(format!("base.module_{}", module.name))
        .or_default()
        .push(XmlIdDef { module: base });
    let Some(category) = &module.manifest.category else {
        return;
    };
    let parts: Vec<String> = category.split('/').map(|p| p.trim().to_lowercase()).collect();
    for n in 1..=parts.len() {
        let id = parts[..n].join("_").replace('&', "and").replace(' ', "_");
        xmlids
            .entry(format!("base.module_category_{id}"))
            .or_default()
            .push(XmlIdDef { module: base });
    }
}

fn class_ids<'m>(
    mi: usize,
    module: &'m Module,
    models: &mut Models<'m>,
    model_by_xmlid: &mut FxHashMap<String, &'m str>,
    xmlids: &mut XmlIds,
) {
    let name = module.name.as_str();
    for class in module.python.iter().flat_map(|f| &f.facts.classes) {
        models.entry(class.model.as_str()).or_default().push((mi, class));
        let prefix = model_xmlid(&class.model);
        // Odoo gives every module that defines or extends a model its own ir.model XML id.
        let id = format!("{name}.{prefix}");
        model_by_xmlid.insert(id.clone(), class.model.as_str());
        xmlids.entry(id).or_default().push(XmlIdDef { module: mi });
        let magic = MAGIC_FIELDS.iter().filter(|_| class.new_model).copied();
        for field in class.fields.iter().map(|f| f.name.as_str()).chain(magic) {
            let id = format!("{name}.field_{}__{field}", &prefix["model_".len()..]);
            xmlids.entry(id).or_default().push(XmlIdDef { module: mi });
        }
    }
}

fn data_ids<'m>(
    mi: usize,
    module: &'m Module,
    by_name: &FxHashMap<&str, usize>,
    xmlids: &mut XmlIds,
    views: &mut Views<'m>,
) {
    let name = module.name.as_str();
    for file in &module.data {
        for def in &file.facts.defs {
            // `<record id="other.x">` creates or updates an id that belongs to module other.
            let id = full_id(name, &def.id);
            let owner = id
                .split_once('.')
                .and_then(|(m, _)| by_name.get(m))
                .copied()
                .unwrap_or(mi);
            xmlids.entry(id).or_default().push(XmlIdDef { module: owner });
        }
        for view in &file.facts.views {
            if let Some(id) = &view.id {
                views.entry(full_id(name, id)).or_default().push(ViewNode {
                    module: mi,
                    inherit: view.inherit.as_deref().map(|i| full_id(name, i)),
                    fact: view,
                });
            }
        }
    }
}

/// Ids of records Odoo creates alongside others, with the module they belong to.
fn derived_ids(modules: &[Module], models: &Models) -> Vec<(String, usize)> {
    // A record of a model with _inherits also gets an id for each parent record Odoo creates,
    // e.g. <cron id>_ir_actions_server for an ir.cron.
    let mut derived = vec![];
    for (mi, module) in modules.iter().enumerate() {
        for def in module.data.iter().flat_map(|f| &f.facts.defs) {
            let Some(classes) = def.model.as_deref().and_then(|m| models.get(m)) else {
                continue;
            };
            for (parent, _) in classes.iter().flat_map(|(_, c)| &c.inherits) {
                derived.push((
                    format!("{}_{}", full_id(&module.name, &def.id), parent.replace('.', "_")),
                    mi,
                ));
            }
        }
    }
    derived
}

fn access_models(modules: &[Module], model_by_xmlid: &FxHashMap<String, &str>) -> FxHashSet<String> {
    let mut models_with_access = FxHashSet::default();
    for m in modules {
        for access in m.data.iter().flat_map(|f| &f.facts.access) {
            if access.by_name {
                models_with_access.insert(access.model.clone());
            } else if let Some(model) = model_by_xmlid.get(&full_id(&m.name, &access.model)) {
                models_with_access.insert((*model).to_owned());
            }
        }
    }
    models_with_access
}

/// The name Odoo registers a script under `static/src` as: `@module/` and its path from there.
pub fn js_module_path(module: &str, rel: &str) -> Option<String> {
    let path = rel.strip_prefix("static/src/")?.strip_suffix(".js")?;
    Some(format!("@{module}/{path}"))
}

/// Instance members of Owl's `Component`, which odooscan does not index: Owl is a library.
const OWL_COMPONENT: &[&str] = &["setup", "render"];

/// A class by the file that declares it and its name there.
type ClassKey = (usize, usize, String);

/// A prototype patch, as (module, file, index in the file).
type PatchId = (usize, usize, usize);

/// The members each prototype patch adds to a class.
type Patched<'m> = FxHashMap<ClassKey, Vec<(PatchId, &'m [String])>>;

/// The file a JS module name resolves to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct JsTarget {
    pub module: usize,
    pub file: usize,
    /// Registered by `odoo.define()` or marked `@odoo-module ignore`, so its exports are unknown.
    opaque: bool,
}

pub enum JsSpec {
    Found(JsTarget),
    /// The path is in an indexed module whose scripts are indexed, but no file provides it.
    Missing(usize),
    /// A library, or a module that is not indexed.
    Outside,
}

#[derive(PartialEq, Eq)]
pub enum Export {
    Found,
    Missing,
    /// Through an `export *` the index cannot follow.
    Unknown,
}

fn js_paths(modules: &[Module]) -> FxHashMap<String, JsTarget> {
    let mut paths = FxHashMap::default();
    for (mi, module) in modules.iter().enumerate() {
        for (fi, file) in module.js.iter().enumerate() {
            let target = JsTarget {
                module: mi,
                file: fi,
                opaque: file.facts.annotation == Annotation::Ignore,
            };
            if let Some(path) = js_module_path(&module.name, &file.rel) {
                if let Some(dir) = path.strip_suffix("/index") {
                    paths.entry(dir.to_owned()).or_insert(target);
                }
                paths.insert(path, target);
            }
            for name in &file.facts.defines {
                paths.entry(name.clone()).or_insert(JsTarget { opaque: true, ..target });
            }
        }
    }
    paths
}

/// `base`, and the modules that auto-install once their depends are, when those are all such modules.
fn always_installed(modules: &[Module], by_name: &FxHashMap<&str, usize>) -> Vec<bool> {
    let mut always: Vec<bool> = modules.iter().map(|m| m.name == "base").collect();
    loop {
        let next: Vec<usize> = (0..modules.len())
            .filter(|&mi| !always[mi] && modules[mi].manifest.auto_install)
            .filter(|&mi| {
                modules[mi]
                    .manifest
                    .depends
                    .iter()
                    .all(|d| by_name.get(d.value.as_str()).is_some_and(|&di| always[di]))
            })
            .collect();
        if next.is_empty() {
            return always;
        }
        for mi in next {
            always[mi] = true;
        }
    }
}

/// The scripts some bundle of an indexed manifest adds, as (module, file). Odoo expands the globs
/// against the addons directories, so a module may add another module's files.
fn loaded_js(modules: &[Module]) -> FxHashSet<(usize, usize)> {
    let mut builder = GlobSetBuilder::new();
    for glob in modules.iter().flat_map(|m| &m.manifest.assets) {
        if let Ok(g) = GlobBuilder::new(glob).literal_separator(true).build() {
            builder.add(g);
        }
    }
    let Ok(set) = builder.build() else {
        return FxHashSet::default();
    };
    let mut loaded = FxHashSet::default();
    for (mi, module) in modules.iter().enumerate() {
        for (fi, file) in module.js.iter().enumerate() {
            if set.is_match(format!("{}/{}", module.name, file.rel)) {
                loaded.insert((mi, fi));
            }
        }
    }
    loaded
}

/// Appends the segments of a specifier to a module path's, or None when it climbs out of the
/// module, as `@web/../tests` does.
fn join_segments<'s>(mut parts: Vec<&'s str>, specifier: &'s str) -> Option<String> {
    for segment in specifier.split('/') {
        match segment {
            "." | "" => {}
            ".." if parts.len() > 1 => {
                parts.pop();
            }
            ".." => return None,
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

pub struct XmlIdDef {
    pub module: usize,
}

/// The modules a module may use: itself and its transitive depends, plus `base`.
pub struct Scope {
    visible: Vec<bool>,
}

impl Scope {
    pub fn sees(&self, module: usize) -> bool {
        self.visible[module]
    }
}

pub enum Status<'m> {
    Visible,
    /// Defined only by modules outside the scope; the first one is named.
    Hidden(&'m str),
    Missing,
}

pub struct FieldInfo<'m> {
    pub ftype: &'m str,
    pub comodel: Option<String>,
}

pub enum PathError<'m> {
    Missing {
        segment: String,
        model: String,
    },
    Hidden {
        segment: String,
        model: String,
        module: &'m str,
    },
    NotRelational {
        segment: String,
        model: String,
    },
}

pub struct Index<'m> {
    pub modules: &'m [Module],
    pub by_name: FxHashMap<&'m str, usize>,
    models: Models<'m>,
    pub xmlids: FxHashMap<String, Vec<XmlIdDef>>,
    views: Views<'m>,
    /// View id to the ids of the views that inherit from it.
    children: FxHashMap<String, Vec<String>>,
    pub models_with_access: FxHashSet<String>,
    pub has_base: bool,
    js_paths: FxHashMap<String, JsTarget>,
    js_loaded: FxHashSet<(usize, usize)>,
    always_installed: Vec<bool>,
    patched: Patched<'m>,
}

impl<'m> Index<'m> {
    pub fn build(modules: &'m [Module]) -> Self {
        let by_name: FxHashMap<&str, usize> = modules.iter().enumerate().map(|(i, m)| (m.name.as_str(), i)).collect();
        let mut models: Models<'m> = FxHashMap::default();
        let mut xmlids: XmlIds = FxHashMap::default();
        let mut model_by_xmlid: FxHashMap<String, &str> = FxHashMap::default();
        let mut views = FxHashMap::default();
        let base = by_name.get("base").copied();

        for (mi, module) in modules.iter().enumerate() {
            if let Some(b) = base {
                module_ids(module, b, &mut xmlids);
            }
            class_ids(mi, module, &mut models, &mut model_by_xmlid, &mut xmlids);
            data_ids(mi, module, &by_name, &mut xmlids, &mut views);
        }
        for (id, mi) in derived_ids(modules, &models) {
            xmlids.entry(id).or_default().push(XmlIdDef { module: mi });
        }
        let models_with_access = access_models(modules, &model_by_xmlid);
        let mut children: FxHashMap<String, Vec<String>> = FxHashMap::default();
        for (id, nodes) in &views {
            for parent in nodes.iter().filter_map(|n| n.inherit.clone()) {
                children.entry(parent).or_default().push(id.clone());
            }
        }
        let mut index = Self {
            modules,
            has_base: base.is_some(),
            models,
            xmlids,
            views,
            children,
            models_with_access,
            js_paths: js_paths(modules),
            js_loaded: loaded_js(modules),
            always_installed: always_installed(modules, &by_name),
            by_name,
            patched: FxHashMap::default(),
        };
        index.patched = index.patched_classes();
        index
    }

    fn patched_classes(&self) -> Patched<'m> {
        let mut patched: Patched<'m> = FxHashMap::default();
        for (mi, module) in self.modules.iter().enumerate() {
            for (fi, file) in module.js.iter().enumerate() {
                let here = self.js_target(mi, fi);
                for (pi, patch) in file.facts.patches.iter().enumerate() {
                    if let Some((t, class)) = self.js_class(here, &patch.target, 0) {
                        let key = (t.module, t.file, class.name.clone());
                        patched
                            .entry(key)
                            .or_default()
                            .push(((mi, fi, pi), patch.members.as_slice()));
                    }
                }
            }
        }
        patched
    }

    pub fn js_target(&self, module: usize, file: usize) -> JsTarget {
        JsTarget {
            module,
            file,
            opaque: false,
        }
    }

    fn js_from(&self, t: JsTarget) -> String {
        let module = &self.modules[t.module];
        js_module_path(&module.name, &module.js[t.file].rel).unwrap_or_default()
    }

    /// The file and binding behind the name `exported` of the file `t`.
    fn js_binding(&self, t: JsTarget, exported: &str, depth: u8) -> Option<(JsTarget, String)> {
        let facts = &self.modules[t.module].js[t.file].facts;
        if t.opaque || depth > 8 {
            return None;
        }
        if let Some((_, local)) = facts.export_locals.iter().find(|(e, _)| e == exported) {
            return Some((t, local.clone()));
        }
        let from = self.js_from(t);
        let next = |specifier: &str, name: &str| match self.js_resolve(&from, specifier) {
            JsSpec::Found(n) => self.js_binding(n, name, depth + 1),
            _ => None,
        };
        if let Some((_, specifier, imported)) = facts.reexports.iter().find(|(e, ..)| e == exported) {
            return next(specifier, imported);
        }
        facts
            .star_exports
            .iter()
            .find_map(|specifier| next(specifier, exported))
    }

    /// The class the binding `local` of the file `t` holds, following imports.
    pub fn js_class(&self, t: JsTarget, local: &str, depth: u8) -> Option<(JsTarget, &'m JsClass)> {
        let facts = &self.modules[t.module].js[t.file].facts;
        if depth > 8 {
            return None;
        }
        if let Some(class) = facts.classes.iter().find(|c| c.name == local) {
            return Some((t, class));
        }
        let import = facts.names.iter().find(|n| n.local.as_deref() == Some(local))?;
        let JsSpec::Found(n) = self.js_resolve(&self.js_from(t), &import.specifier) else {
            return None;
        };
        let (t, local) = self.js_binding(n, &import.name.value, depth + 1)?;
        self.js_class(t, &local, depth + 1)
    }

    /// Whether the file `t` imports `local` as Owl's `Component`.
    fn owl_component(&self, t: JsTarget, local: &str) -> bool {
        self.modules[t.module].js[t.file]
            .facts
            .names
            .iter()
            .any(|n| n.local.as_deref() == Some(local) && n.specifier == "@odoo/owl" && n.name.value == "Component")
    }

    /// The members of a class, its parents, and every prototype patch of them but `except`, or
    /// None when a parent cannot be resolved.
    pub fn js_members(&self, t: JsTarget, class: &'m JsClass, except: PatchId) -> Option<FxHashSet<&'m str>> {
        let mut members = FxHashSet::default();
        let (mut t, mut class) = (t, class);
        for _ in 0..32 {
            members.extend(class.members.iter().map(String::as_str));
            let key = (t.module, t.file, class.name.clone());
            for (_, added) in self
                .patched
                .get(&key)
                .into_iter()
                .flatten()
                .filter(|(p, _)| *p != except)
            {
                members.extend(added.iter().map(String::as_str));
            }
            let parent = match &class.extends {
                Heritage::None => return Some(members),
                Heritage::Other => return None,
                Heritage::Name(parent) => parent,
            };
            if self.owl_component(t, parent) {
                members.extend(OWL_COMPONENT);
                return Some(members);
            }
            (t, class) = self.js_class(t, parent, 0)?;
        }
        None
    }

    /// Whether Odoo installs `module` with `base`, as it does `web`, so any module may use it.
    pub fn always_installed(&self, module: usize) -> bool {
        self.always_installed[module]
    }

    pub fn js_loaded(&self, module: usize, file: usize) -> bool {
        self.js_loaded.contains(&(module, file))
    }

    /// The file a specifier imported by the module `from` names.
    pub fn js_resolve(&self, from: &str, specifier: &str) -> JsSpec {
        let path = if specifier.starts_with('.') {
            let mut dir: Vec<&str> = from.split('/').collect();
            dir.pop();
            join_segments(dir, specifier)
        } else {
            join_segments(vec![], specifier).filter(|_| specifier.starts_with('@'))
        };
        let Some(path) = path.map(|p| p.strip_suffix(".js").map(str::to_owned).unwrap_or(p)) else {
            return JsSpec::Outside;
        };
        if let Some(target) = self.js_paths.get(&path) {
            return JsSpec::Found(*target);
        }
        let addon = path[1..].split('/').next().unwrap_or_default();
        match self.by_name.get(addon) {
            Some(&mi) if !self.modules[mi].js.is_empty() => JsSpec::Missing(mi),
            _ => JsSpec::Outside,
        }
    }

    /// Whether the file `target` exports `name`, following `export *`.
    pub fn js_export(&self, target: JsTarget, name: &str, depth: u8) -> Export {
        let module = &self.modules[target.module];
        let file = &module.js[target.file];
        if target.opaque || depth > 8 {
            return Export::Unknown;
        }
        if file.facts.exports.iter().any(|e| e == name) {
            return Export::Found;
        }
        let from = js_module_path(&module.name, &file.rel).unwrap_or_default();
        let mut found = Export::Missing;
        for specifier in file.facts.star_exports.iter().filter(|_| name != "default") {
            let next = match self.js_resolve(&from, specifier) {
                JsSpec::Found(t) => self.js_export(t, name, depth + 1),
                _ => Export::Unknown,
            };
            match next {
                Export::Found => return Export::Found,
                Export::Unknown => found = Export::Unknown,
                Export::Missing => {}
            }
        }
        found
    }

    pub fn scope(&self, module: usize) -> Scope {
        let mut visible = vec![false; self.modules.len()];
        let mut stack = vec![module];
        stack.extend(self.by_name.get("base"));
        while let Some(m) = stack.pop() {
            if std::mem::replace(&mut visible[m], true) {
                continue;
            }
            stack.extend(
                self.modules[m]
                    .manifest
                    .depends
                    .iter()
                    .filter_map(|d| self.by_name.get(d.value.as_str())),
            );
        }
        Scope { visible }
    }

    /// Every class that defines or extends `model`, with its module.
    pub fn classes(&self, model: &str) -> &[(usize, &'m ClassFact)] {
        self.models.get(model).map(Vec::as_slice).unwrap_or_default()
    }

    /// Whether `from` depends on `to`, directly or through other modules.
    pub fn reaches(&self, from: usize, to: usize) -> bool {
        let mut seen = vec![false; self.modules.len()];
        let mut stack = vec![from];
        while let Some(m) = stack.pop() {
            if m == to {
                return true;
            }
            if std::mem::replace(&mut seen[m], true) {
                continue;
            }
            stack.extend(
                self.modules[m]
                    .manifest
                    .depends
                    .iter()
                    .filter_map(|d| self.by_name.get(d.value.as_str())),
            );
        }
        false
    }

    /// Whether a module in `scope` depends on a module that is not indexed.
    pub fn partial_scope(&self, scope: &Scope) -> bool {
        self.modules
            .iter()
            .enumerate()
            .filter(|(mi, _)| scope.sees(*mi))
            .flat_map(|(_, m)| &m.manifest.depends)
            .any(|d| !self.by_name.contains_key(d.value.as_str()))
    }

    pub fn has_model(&self, model: &str) -> bool {
        self.models.contains_key(model)
    }

    /// Whether `model` is defined in scope, ignoring the class `except` (the one asking).
    pub fn model_status(&self, model: &str, scope: &Scope, except: Option<&ClassFact>) -> Status<'m> {
        let classes = self.models.get(model).map(Vec::as_slice).unwrap_or_default();
        let others = || {
            classes
                .iter()
                .filter(|(_, c)| !except.is_some_and(|e| std::ptr::eq(*c, e)))
        };
        if others().any(|(mi, _)| scope.sees(*mi)) {
            Status::Visible
        } else if let Some((mi, _)) = others().next() {
            Status::Hidden(&self.modules[*mi].name)
        } else {
            Status::Missing
        }
    }

    /// Every class of `model` and of the models it inherits from, `base` included.
    fn chain(&self, model: &str, scope: Option<&Scope>) -> Vec<(usize, &'m ClassFact)> {
        let mut out = vec![];
        let mut seen = FxHashSet::default();
        let mut stack = vec![model, "base"];
        while let Some(m) = stack.pop() {
            if !seen.insert(m) {
                continue;
            }
            for &(mi, class) in self.models.get(m).map(Vec::as_slice).unwrap_or_default() {
                if scope.is_some_and(|s| !s.sees(mi)) {
                    continue;
                }
                out.push((mi, class));
                stack.extend(class.inherit.iter().map(|i| i.value.as_str()).filter(|i| *i != m));
                stack.extend(class.inherits.iter().map(|(parent, _)| parent.as_str()));
            }
        }
        out
    }

    pub fn field_defs(
        &self,
        model: &str,
        field: &str,
        scope: Option<&Scope>,
    ) -> Vec<(usize, &'m ClassFact, &'m FieldFact)> {
        self.chain(model, scope)
            .into_iter()
            .flat_map(|(mi, c)| c.fields.iter().filter(|f| f.name == field).map(move |f| (mi, c, f)))
            .collect()
    }

    /// `Err(Some(module))` when the field exists only outside the scope.
    pub fn field(&self, model: &str, field: &str, scope: &Scope) -> Result<Option<FieldInfo<'m>>, Option<&'m str>> {
        if MAGIC_FIELDS.contains(&field) || EXTRA_FIELDS.contains(&(model, field)) {
            return Ok(None);
        }
        let defs = self.field_defs(model, field, Some(scope));
        let Some((_, _, first)) = defs.first() else {
            return Err(self
                .field_defs(model, field, None)
                .first()
                .map(|(mi, _, _)| self.modules[*mi].name.as_str()));
        };
        let comodel = defs
            .iter()
            .find_map(|(_, _, f)| f.comodel.as_ref().map(|c| c.value.clone()))
            .or_else(|| {
                let (_, class, related) = defs.iter().find(|(_, _, f)| f.related.is_some())?;
                self.related_comodel(&class.model, &related.related.as_ref()?.value, scope, 0)
            });
        Ok(Some(FieldInfo {
            ftype: &first.ftype,
            comodel,
        }))
    }

    /// Every in-scope definition of the field a dotted path ends on, or None when the path does not
    /// resolve or ends on a field every model has.
    pub fn path_target(&self, model: &str, path: &str, scope: &Scope) -> Option<Vec<&'m FieldFact>> {
        let (head, last) = match path.rsplit_once('.') {
            Some((head, last)) => (self.related_comodel(model, head, scope, 0)?, last),
            None => (model.to_owned(), path),
        };
        let defs: Vec<_> = self
            .field_defs(&head, last, Some(scope))
            .into_iter()
            .map(|(_, _, f)| f)
            .collect();
        (!defs.is_empty()).then_some(defs)
    }

    /// The comodel a dotted path of relational fields ends on.
    pub fn path_comodel(&self, model: &str, path: &str, scope: &Scope) -> Option<String> {
        self.related_comodel(model, path, scope, 0)
    }

    fn related_comodel(&self, model: &str, path: &str, scope: &Scope, depth: u8) -> Option<String> {
        if depth > 8 {
            return None;
        }
        let mut model = model.to_owned();
        for segment in path.split('.') {
            let info = self.field(&model, segment, scope).ok()??;
            model = info.comodel?;
        }
        Some(model)
    }

    /// Whether a class of `model` or of a model it inherits, other than `except`, or `BaseModel`
    /// defines `method`. Modules outside the scope count: an override of a method from a module
    /// that is not in depends is called only by that module.
    pub fn has_parent_method(&self, model: &str, method: &str, except: &ClassFact) -> bool {
        BASE_MODEL_METHODS.contains(&method)
            || self
                .chain(model, None)
                .iter()
                .any(|(_, c)| !std::ptr::eq(*c, except) && c.methods.iter().any(|m| m == method))
    }

    pub fn has_method(&self, model: &str, method: &str, scope: &Scope) -> bool {
        BASE_METHODS.contains(&method)
            || self
                .chain(model, Some(scope))
                .iter()
                .any(|(_, c)| c.methods.iter().any(|m| m == method))
    }

    /// Whether a client may call `method` on `model`: a method of `BaseModel`, or of a class of the
    /// model or of a model it inherits, in scope or in a module that is always installed.
    pub fn rpc_method(&self, model: &str, method: &str, scope: &Scope) -> Status<'m> {
        if BASE_MODEL_METHODS.contains(&method) || self.has_method(model, method, scope) {
            return Status::Visible;
        }
        let defining: Vec<usize> = self
            .chain(model, None)
            .iter()
            .filter(|(_, c)| c.methods.iter().any(|m| m == method))
            .map(|(mi, _)| *mi)
            .collect();
        match defining.first() {
            None => Status::Missing,
            Some(_) if defining.iter().any(|&mi| self.always_installed(mi)) => Status::Visible,
            Some(&mi) => Status::Hidden(&self.modules[mi].name),
        }
    }

    /// Follows a dotted field path from `model`. Paths through models nobody defines are accepted.
    pub fn resolve_path(&self, model: &str, path: &str, scope: &Scope) -> Result<(), PathError<'m>> {
        let mut model = model.to_owned();
        let segments: Vec<&str> = path.split('.').collect();
        for (i, segment) in segments.iter().enumerate() {
            if !self.has_model(&model) {
                return Ok(());
            }
            let Some(info) = self.step(&model, segment, scope)? else {
                return Ok(());
            };
            if i + 1 == segments.len() || OPAQUE_TYPES.contains(&info.ftype) {
                return Ok(());
            }
            match info.comodel {
                Some(comodel) => model = comodel,
                None if RELATIONAL_TYPES.contains(&info.ftype) => return Ok(()),
                None => {
                    return Err(PathError::NotRelational {
                        segment: (*segment).to_owned(),
                        model,
                    });
                }
            }
        }
        Ok(())
    }

    fn step(&self, model: &str, segment: &str, scope: &Scope) -> Result<Option<FieldInfo<'m>>, PathError<'m>> {
        self.field(model, segment, scope).map_err(|module| match module {
            None => PathError::Missing {
                segment: segment.to_owned(),
                model: model.to_owned(),
            },
            Some(module) => PathError::Hidden {
                segment: segment.to_owned(),
                model: model.to_owned(),
                module,
            },
        })
    }

    /// The model a view applies to, following `inherit_id` when the view does not name one.
    pub fn view_model(&self, mut id: String) -> Option<&'m str> {
        for _ in 0..32 {
            let nodes = self.views.get(&id)?;
            if let Some(model) = nodes.iter().find_map(|n| n.fact.model.as_ref()) {
                return Some(&model.value);
            }
            id = nodes.iter().find_map(|n| n.inherit.clone())?;
        }
        None
    }

    /// The root of the inheritance tree above view `id`, or None when a view on the way is not
    /// indexed or its arch was not read, so the tree's elements would be incomplete.
    pub fn view_root(&self, mut id: String) -> Option<String> {
        for _ in 0..32 {
            let nodes = self.views.get(&id)?;
            if !nodes.iter().any(|n| n.fact.has_arch) {
                return None;
            }
            match nodes.iter().find_map(|n| n.inherit.clone()) {
                Some(parent) => id = parent,
                None => return Some(id),
            }
        }
        None
    }

    pub fn view_visible(&self, id: &str, scope: &Scope) -> bool {
        self.views
            .get(id)
            .is_some_and(|nodes| nodes.iter().any(|n| scope.sees(n.module)))
    }

    /// The elements of every view in the tree under `root`.
    pub fn view_tree(&self, root: String, scope: &Scope) -> ViewTree<'m> {
        let mut tree = ViewTree::default();
        let mut seen = FxHashSet::default();
        let mut stack = vec![root.as_str()];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            for node in self.views.get(id).map(Vec::as_slice).unwrap_or_default() {
                let module = self.modules[node.module].name.as_str();
                let elements = node.fact.elements.iter().map(|e| located(e, module));
                if scope.sees(node.module) {
                    tree.visible.extend(elements);
                } else {
                    for e in elements {
                        tree.hidden.entry(e).or_insert(module);
                    }
                }
            }
            stack.extend(self.children.get(id).into_iter().flatten().map(String::as_str));
        }
        tree.root = root;
        tree
    }
}

impl PathError<'_> {
    pub fn message(&self, path: &str) -> String {
        match self {
            PathError::Missing { segment, model } => format!("'{path}': {model} has no field '{segment}'"),
            PathError::Hidden { segment, model, module } => {
                format!("'{path}': field '{segment}' of {model} comes from module {module}, which is not in depends")
            }
            PathError::NotRelational { segment, model } => {
                format!("'{path}': {model}.{segment} is not a relational field, so the path cannot continue")
            }
        }
    }
}
