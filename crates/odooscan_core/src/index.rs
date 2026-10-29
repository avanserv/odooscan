use globset::{GlobBuilder, GlobSetBuilder};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{
    Annotation, ArchElement, ClassFact, FieldFact, FlowSummary, Heritage, JsClass, ModelKind, OptionValue, PyFacts,
    Receiver, ViewFact,
};
use crate::manifest::AutoInstall;
use crate::project::{Module, SourceFile};
use crate::series::Series;

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

/// The fields of `MAGIC_FIELDS` Odoo creates a column for when the model logs access: the ORM
/// computes the others.
const LOG_COLUMNS: &[&str] = &["create_uid", "create_date", "write_uid", "write_date"];

pub const RELATIONAL_TYPES: &[&str] = &["Many2one", "One2many", "Many2many"];

/// Types whose value is not a record of a fixed comodel, so a path cannot continue through them.
const OPAQUE_TYPES: &[&str] = &["Reference", "Many2oneReference", "Properties", "Json"];

/// Fields declared outside the class body, which the extractor cannot see.
const EXTRA_FIELDS: &[(&str, &str)] = &[("ir.rule", "global")];

/// Columns of fields Odoo does not store that `base` creates in `base_data.sql`, out of the reach
/// of the literals `added_columns` reads.
const SQL_MADE_COLUMNS: &[(&str, &str)] = &[("res.users", "password")];

/// BaseModel methods that fields commonly name as compute, search or group_expand.
const BASE_METHODS: &[&str] = &[
    "_compute_display_name",
    "_search_display_name",
    "_read_group_expand_full",
];

/// Methods of `BaseModel`, written from the ORM API reference: models inherit them from a class
/// outside every addons directory, so the index never sees them.
pub(crate) const BASE_MODEL_METHODS: &[&str] = &[
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

/// The flow summary of a function, with the file and module it is in.
#[derive(Clone, Copy)]
pub struct FlowRef<'m> {
    pub module: usize,
    pub file: &'m SourceFile<PyFacts>,
    pub summary: &'m FlowSummary,
}

/// By model, then method name.
type MethodFlows<'m> = FxHashMap<&'m str, FxHashMap<&'m str, Vec<FlowRef<'m>>>>;

/// Every node of an id: a module may redefine another module's view, or extend a template in place.
type Nodes<'m> = FxHashMap<String, Vec<ViewNode<'m>>>;

struct ViewNode<'m> {
    module: usize,
    inherit: Option<String>,
    /// The node defines the id, rather than extending in place what defines it.
    defines: bool,
    fact: &'m ViewFact,
}

/// Views by XML id, or Owl templates by name, with the ids that inherit from each.
pub struct Inheritance<'m> {
    nodes: Nodes<'m>,
    children: FxHashMap<String, Vec<String>>,
}

/// The elements of the views or templates in one inheritance tree: those of a module's scope, and
/// for the others the first module that adds each.
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
        attr: e.attr.clone(),
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
    views: &mut Nodes<'m>,
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
                    defines: true,
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

/// The first series that reads a list-form `auto_install` as its triggers (`saas~12.4`).
const LISTED_TRIGGERS: Series = Series { major: 12, minor: 4 };

/// `base`, and what Odoo always installs with it: each auto-install module whose triggers (see
/// `triggers` for how a series reads them) are all such modules and whose depends closure is indexed,
/// with that closure dragged in.
/// `default_series` stands in for a manifest version without a series, as it does for the checks.
/// The manifest `countries` key is ignored on purpose, because Odoo creates a database without reading it.
fn always_installed(modules: &[Module], by_name: &FxHashMap<&str, usize>, default_series: Option<Series>) -> Vec<bool> {
    let mut always: Vec<bool> = modules.iter().map(|m| m.name == "base").collect();
    loop {
        let installed = |name: &str| by_name.get(name).is_some_and(|&di| always[di]);
        let ready: Vec<usize> = (0..modules.len())
            .filter(|&mi| !always[mi])
            .filter(|&mi| triggers(&modules[mi], default_series).is_some_and(|t| t.into_iter().all(installed)))
            .collect();
        let mut grew = false;
        for mi in ready {
            // Odoo cannot load a module whose depends closure is incomplete, so nothing joins through it.
            let (closure, complete) = depends_closure(modules, by_name, [mi]);
            if complete {
                always.iter_mut().zip(closure).for_each(|(a, c)| *a |= c);
                grew = true;
            }
        }
        if !grew {
            return always;
        }
    }
}

/// The depends whose install auto-installs `module`, or `None` when nothing does. From `saas~12.4` a list
/// names the triggers, and one naming a module outside `depends` fails the manifest. Before `saas~12.4`,
/// database creation fails on a list, and `update_list` keeps only whether the value is truthy, so `must_install`
/// waits on every depend for a non-empty list, like `True`, and `[]` is off.
fn triggers(module: &Module, default_series: Option<Series>) -> Option<Vec<&str>> {
    let depends: Vec<&str> = module.manifest.depends.iter().map(|d| d.value.as_str()).collect();
    match &module.manifest.auto_install {
        AutoInstall::Off => None,
        AutoInstall::Depends => Some(depends),
        AutoInstall::Listed(names) if module.series.or(default_series).is_none_or(|s| s >= LISTED_TRIGGERS) => names
            .iter()
            .map(String::as_str)
            .map(|n| depends.contains(&n).then_some(n))
            .collect(),
        AutoInstall::Listed(names) => (!names.is_empty()).then_some(depends),
    }
}

/// The modules `seeds` reach through `depends`, and whether every depend on the way is indexed.
fn depends_closure(
    modules: &[Module],
    by_name: &FxHashMap<&str, usize>,
    seeds: impl IntoIterator<Item = usize>,
) -> (Vec<bool>, bool) {
    let mut reached = vec![false; modules.len()];
    let mut complete = true;
    let mut stack: Vec<usize> = seeds.into_iter().collect();
    while let Some(mi) = stack.pop() {
        if std::mem::replace(&mut reached[mi], true) {
            continue;
        }
        for d in &modules[mi].manifest.depends {
            match by_name.get(d.value.as_str()) {
                Some(&di) => stack.push(di),
                None => complete = false,
            }
        }
    }
    (reached, complete)
}

/// Files of a kind, as (module, index among the module's files of that kind).
type Loaded = FxHashSet<(usize, usize)>;

/// The scripts and the template files some bundle of an indexed manifest adds, as (module, file).
/// Odoo expands the globs against the addons directories, so a module may add another module's files.
fn loaded_static(modules: &[Module]) -> (Loaded, Loaded) {
    let mut builder = GlobSetBuilder::new();
    for glob in modules.iter().flat_map(|m| &m.manifest.assets) {
        if let Ok(g) = GlobBuilder::new(glob).literal_separator(true).build() {
            builder.add(g);
        }
    }
    let Ok(set) = builder.build() else {
        return Default::default();
    };
    let loaded = |files: fn(&Module) -> Vec<&str>| {
        let mut loaded = FxHashSet::default();
        for (mi, module) in modules.iter().enumerate() {
            for (fi, rel) in files(module).into_iter().enumerate() {
                if set.is_match(format!("{}/{rel}", module.name)) {
                    loaded.insert((mi, fi));
                }
            }
        }
        loaded
    };
    (
        loaded(|m| m.js.iter().map(|f| f.rel.as_str()).collect()),
        loaded(|m| m.templates.iter().map(|f| f.rel.as_str()).collect()),
    )
}

/// The templates of the loaded template files: a registered one under its name, and an extension
/// under its parent's, since it changes the parent in place.
fn template_nodes<'m>(modules: &'m [Module], loaded: &Loaded) -> Nodes<'m> {
    let mut nodes: Nodes<'m> = FxHashMap::default();
    // In module order, so the module a hidden element names does not vary between runs.
    let mut loaded: Vec<_> = loaded.iter().copied().collect();
    loaded.sort_unstable();
    for (mi, fi) in loaded {
        for t in &modules[mi].templates[fi].facts.templates {
            let (key, inherit) = match &t.name {
                Some(name) => (name, t.inherit.as_ref().map(|i| i.value.clone())),
                None => (&t.inherit.as_ref().expect("an extension has a parent").value, None),
            };
            nodes.entry(key.clone()).or_default().push(ViewNode {
                module: mi,
                inherit,
                defines: t.name.is_some(),
                fact: &t.arch,
            });
        }
    }
    nodes
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

/// The modules a module may use: itself, its transitive depends and the always-installed modules.
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
    /// Defined only by modules outside the scope. A model names the module that introduces it,
    /// anything else its first definer.
    Hidden(&'m str),
    Missing,
}

/// A field definition, with its module and class.
pub type FieldDef<'m> = (usize, &'m ClassFact, &'m FieldFact);

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
    pub views: Inheritance<'m>,
    pub templates: Inheritance<'m>,
    pub models_with_access: FxHashSet<String>,
    pub has_base: bool,
    js_paths: FxHashMap<String, JsTarget>,
    js_loaded: Loaded,
    templates_loaded: Loaded,
    always_installed: Vec<bool>,
    patched: Patched<'m>,
    /// Classes a subclass with a template of its own extends.
    template_bases: FxHashSet<ClassKey>,
    /// Flow summaries of model methods, by model and method name.
    method_flows: MethodFlows<'m>,
    /// The models whose table each table name may be, from any module.
    table_models: FxHashMap<String, Vec<&'m str>>,
}

impl<'m> Index<'m> {
    pub fn build(modules: &'m [Module], default_series: Option<Series>) -> Self {
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
        let (js_loaded, templates_loaded) = loaded_static(modules);
        let mut index = Self {
            modules,
            has_base: base.is_some(),
            xmlids,
            views: Inheritance::new(views),
            templates: Inheritance::new(template_nodes(modules, &templates_loaded)),
            models_with_access,
            js_paths: js_paths(modules),
            js_loaded,
            templates_loaded,
            always_installed: always_installed(modules, &by_name, default_series),
            by_name,
            patched: FxHashMap::default(),
            template_bases: FxHashSet::default(),
            method_flows: method_flows(modules),
            table_models: table_models(&models),
            models,
        };
        index.patched = index.patched_classes();
        index.template_bases = index.template_bases();
        index
    }

    fn template_bases(&self) -> FxHashSet<ClassKey> {
        let mut bases = FxHashSet::default();
        for (mi, module) in self.modules.iter().enumerate() {
            for (fi, file) in module.js.iter().enumerate() {
                for class in file.facts.classes.iter().filter(|c| c.template.is_some()) {
                    if let Heritage::Name(parent) = &class.extends
                        && let Some((t, base)) = self.js_class(self.js_target(mi, fi), parent, 0)
                    {
                        bases.insert((t.module, t.file, base.name.clone()));
                    }
                }
            }
        }
        bases
    }

    /// Whether every subclass of the class may replace its template, as for an abstract base.
    pub fn template_base(&self, module: usize, file: usize, class: &str) -> bool {
        self.template_bases.contains(&(module, file, class.to_owned()))
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
    fn always_installed(&self, module: usize) -> bool {
        self.always_installed[module]
    }

    pub fn js_loaded(&self, module: usize, file: usize) -> bool {
        self.js_loaded.contains(&(module, file))
    }

    pub fn template_loaded(&self, module: usize, file: usize) -> bool {
        self.templates_loaded.contains(&(module, file))
    }

    /// Whether some loaded template file registers `name` in scope.
    pub fn template_status(&self, name: &str, scope: &Scope) -> Status<'m> {
        let mut definers = self.templates.definers(name).peekable();
        let Some(&first) = definers.peek() else {
            return Status::Missing;
        };
        if definers.any(|m| scope.sees(m)) {
            Status::Visible
        } else {
            Status::Hidden(&self.modules[first].name)
        }
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

    /// The modules `module` may use. Every always-installed module is seeded with it, as Odoo
    /// installs those wherever `base` is; they are closed under `depends`, so they add no others.
    pub fn scope(&self, module: usize) -> Scope {
        let always = (0..self.modules.len()).filter(|&mi| self.always_installed(mi));
        let (visible, _) = depends_closure(self.modules, &self.by_name, std::iter::once(module).chain(always));
        Scope { visible }
    }

    /// The summaries of every definition of a model's method, overrides included.
    pub fn method_flows(&self, model: &str, method: &str) -> &[FlowRef<'m>] {
        self.method_flows
            .get(model)
            .and_then(|m| m.get(method))
            .map(Vec::as_slice)
            .unwrap_or_default()
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
        } else if let Some((first, _)) = others().next() {
            let mi = self.introducer(others()).unwrap_or(*first);
            Status::Hidden(&self.modules[mi].name)
        } else {
            Status::Missing
        }
    }

    /// The module of the class that introduces the model with `_name`, the first in load order.
    /// Independent introducers keep index order, since Odoo's tie order between them is unknown.
    fn introducer<'c>(&self, classes: impl Iterator<Item = &'c (usize, &'m ClassFact)>) -> Option<usize>
    where
        'm: 'c,
    {
        classes
            .filter(|(_, c)| c.new_model)
            .map(|(mi, _)| *mi)
            .reduce(|first, mi| if self.reaches(first, mi) { mi } else { first })
    }

    /// Every class of `model` and of the models it inherits from, `base` included. `delegated`
    /// adds the models it reaches through `_inherits`, whose fields it has but does not merge.
    fn chain(&self, model: &str, scope: Option<&Scope>, delegated: bool) -> Vec<(usize, &'m ClassFact)> {
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
                if delegated {
                    stack.extend(class.inherits.iter().map(|(parent, _)| parent.as_str()));
                }
            }
        }
        out
    }

    pub fn field_defs(&self, model: &str, field: &str, scope: Option<&Scope>) -> Vec<FieldDef<'m>> {
        self.chain(model, scope, true)
            .into_iter()
            .flat_map(|(mi, c)| c.fields.iter().filter(|f| f.name == field).map(move |f| (mi, c, f)))
            .collect()
    }

    /// Every class in scope of `model` and of the models it inherits by `_inherit`, `base` included:
    /// the classes the registry merges into the model.
    pub fn merged_classes(&self, model: &str, scope: &Scope) -> Vec<(usize, &'m ClassFact)> {
        self.chain(model, Some(scope), false)
    }

    /// Every in-scope field definition the registry merges into the fields of `model`: those of its
    /// classes and of the models it inherits by `_inherit`.
    pub fn merged_fields(&self, model: &str, scope: &Scope) -> Vec<FieldDef<'m>> {
        self.merged_classes(model, scope)
            .into_iter()
            .flat_map(|(mi, c)| c.fields.iter().map(move |f| (mi, c, f)))
            .collect()
    }

    /// Whether a class in scope of `model`, or of a model it inherits or delegates to, has a plain
    /// Python base class, whose fields and methods only Python knows.
    pub fn plain_bases(&self, model: &str, scope: &Scope) -> bool {
        self.chain(model, Some(scope), true).iter().any(|(_, c)| c.plain_bases)
    }

    /// Whether the class that introduces `model` with `_name` is in scope. When it is not, the
    /// definitions that give its fields their first attributes are missing from the index.
    pub fn introduced(&self, model: &str, scope: &Scope) -> bool {
        self.classes(model).iter().any(|(mi, c)| c.new_model && scope.sees(*mi))
    }

    /// The table of `model`: the `_table` its classes in scope set, or its name with dots made
    /// underscores. None when they set different ones, or one Python computes.
    pub fn table(&self, model: &str, scope: &Scope) -> Option<String> {
        let mut tables = self
            .classes(model)
            .iter()
            .filter(|(mi, _)| scope.sees(*mi))
            .filter_map(|(_, c)| c.table.as_ref());
        let Some(first) = tables.next() else {
            return Some(model.replace('.', "_"));
        };
        match first {
            OptionValue::Str(t) if tables.all(|other| other == first) => Some(t.clone()),
            _ => None,
        }
    }

    /// The models a non-abstract class introduces with `_name` whose table may be `table`, in any
    /// module.
    pub fn table_models(&self, table: &str) -> &[&'m str] {
        self.table_models.get(table).map(Vec::as_slice).unwrap_or_default()
    }

    /// Every class of `model` and of the models it inherits, `base` included, in any indexed module.
    pub fn chain_classes(&self, model: &str) -> Vec<(usize, &'m ClassFact)> {
        self.chain(model, None, false)
    }

    /// Whether an indexed module creates a view named `table`, in Python with a query whose literal
    /// head names it, or in an SQL file its manifest loads.
    pub fn creates_view(&self, table: &str) -> bool {
        self.modules.iter().any(|m| {
            m.sql_views.iter().any(|v| v == table)
                || m.python
                    .iter()
                    .any(|f| f.facts.created_views.iter().any(|v| v == table))
        })
    }

    /// Whether a module in `scope` creates a view whose name it computes outside the `init()` of a
    /// model, which may be any view.
    pub fn creates_any_view(&self, scope: &Scope) -> bool {
        self.modules
            .iter()
            .enumerate()
            .filter(|(mi, _)| scope.sees(*mi))
            .flat_map(|(_, m)| &m.python)
            .any(|f| f.facts.creates_any_view)
    }

    /// Whether a Many2many field of a module in `scope` keeps its rows in `table`, which it names
    /// with `relation`, or which Odoo names after the tables of its model and comodel.
    pub fn relation_table(&self, table: &str, scope: &Scope) -> bool {
        let named = |model: &str, field: &FieldFact| match field.option("relation").map(|r| &r.value) {
            Some(OptionValue::Str(relation)) => relation == table,
            Some(_) => false,
            None => field.comodel.as_ref().is_some_and(|comodel| {
                let mut tables = [self.table(model, scope), self.table(&comodel.value, scope)];
                tables.sort();
                matches!(tables, [Some(a), Some(b)] if format!("{a}_{b}_rel") == table)
            }),
        };
        self.models.values().flatten().any(|(mi, class)| {
            scope.sees(*mi)
                && class
                    .fields
                    .iter()
                    .any(|f| f.ftype == "Many2many" && named(&class.model, f))
        })
    }

    /// Whether a class of `model` or of a model it inherits, in any indexed module, defines `field`.
    pub fn defines_field(&self, model: &str, field: &str) -> bool {
        self.chain(model, None, false)
            .iter()
            .any(|(_, c)| c.fields.iter().any(|f| f.name == field))
    }

    /// Whether `field` is `id`, which every table of a model has, or a column of `model` that no
    /// class declares.
    pub fn implicit_column(&self, model: &str, field: &str) -> bool {
        field == "id" || EXTRA_FIELDS.contains(&(model, field))
    }

    /// Whether `field` is a column Odoo adds to the table of a model that logs access.
    pub fn log_column(&self, field: &str) -> bool {
        LOG_COLUMNS.contains(&field)
    }

    /// Whether the `table` of `model` has `column` though no field gives it: a module in scope adds
    /// it with a literal `ALTER TABLE` or `create_column()`, or `base` creates it in SQL.
    pub fn hand_made_column(&self, model: &str, table: &str, column: &str, scope: &Scope) -> bool {
        SQL_MADE_COLUMNS.contains(&(model, column))
            || self
                .modules
                .iter()
                .enumerate()
                .filter(|(mi, _)| scope.sees(*mi))
                .flat_map(|(_, m)| &m.python)
                .any(|f| f.facts.added_columns.iter().any(|(t, c)| t == table && c == column))
    }

    /// Whether `field` is one every model has, which the ORM computes when it has no column.
    pub fn magic_field(&self, field: &str) -> bool {
        MAGIC_FIELDS.contains(&field)
    }

    /// `Err(Some(module))` when the field exists only outside the scope.
    pub fn field(&self, model: &str, field: &str, scope: &Scope) -> Result<Option<FieldInfo<'m>>, Option<&'m str>> {
        if self.magic_field(field) || EXTRA_FIELDS.contains(&(model, field)) {
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
                .chain(model, None, true)
                .iter()
                .any(|(_, c)| !std::ptr::eq(*c, except) && c.methods.iter().any(|m| m == method))
    }

    pub fn has_method(&self, model: &str, method: &str, scope: &Scope) -> bool {
        BASE_METHODS.contains(&method)
            || self
                .chain(model, Some(scope), true)
                .iter()
                .any(|(_, c)| c.methods.iter().any(|m| m == method))
    }

    /// Whether a class of `model` or of a model it inherits, in any indexed module, defines
    /// `method`; the classes of `base` count only `with_base`.
    pub fn defines_method(&self, model: &str, method: &str, with_base: bool) -> bool {
        self.chain(model, None, false)
            .iter()
            .any(|(_, c)| (with_base || c.model != "base") && c.methods.iter().any(|m| m == method))
    }

    /// The kind of the class in scope that introduces `model` with `_name`.
    pub fn kind(&self, model: &str, scope: &Scope) -> Option<ModelKind> {
        let introducers = || {
            self.classes(model)
                .iter()
                .filter(|(mi, c)| c.new_model && scope.sees(*mi))
        };
        let mi = self.introducer(introducers())?;
        introducers().find(|(m, _)| *m == mi).map(|(_, c)| c.kind)
    }

    /// Whether a client may call `method` on `model`: a method of `BaseModel`, or of a class of the
    /// model or of a model it inherits, in scope or in a module that is always installed. The
    /// scoped walk skips a class outside the scope with its `_inherit`, so a mixin of an
    /// always-installed module that only such a class adds is found only by the unscoped walk;
    /// without the always-installed arm it would be `Hidden` under a module the caller has.
    pub fn rpc_method(&self, model: &str, method: &str, scope: &Scope) -> Status<'m> {
        if BASE_MODEL_METHODS.contains(&method) || self.has_method(model, method, scope) {
            return Status::Visible;
        }
        let defining: Vec<usize> = self
            .chain(model, None, true)
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
            let nodes = self.views.nodes.get(&id)?;
            if let Some(model) = nodes.iter().find_map(|n| n.fact.model.as_ref()) {
                return Some(&model.value);
            }
            id = nodes.iter().find_map(|n| n.inherit.clone())?;
        }
        None
    }
}

impl<'m> Inheritance<'m> {
    fn new(nodes: Nodes<'m>) -> Self {
        let mut children: FxHashMap<String, Vec<String>> = FxHashMap::default();
        for (id, nodes) in &nodes {
            for parent in nodes.iter().filter_map(|n| n.inherit.clone()) {
                children.entry(parent).or_default().push(id.clone());
            }
        }
        Self { nodes, children }
    }

    /// The modules whose nodes define `id`.
    pub fn definers(&self, id: &str) -> impl Iterator<Item = usize> {
        let nodes = self.nodes.get(id).map(Vec::as_slice).unwrap_or_default();
        nodes.iter().filter(|n| n.defines).map(|n| n.module)
    }

    /// The root of the inheritance tree above `id`, or None when an id on the way is not defined
    /// or its arch was not read, so the tree's elements would be incomplete.
    pub fn root(&self, mut id: String) -> Option<String> {
        for _ in 0..32 {
            let nodes = self.nodes.get(&id)?;
            if !nodes.iter().any(|n| n.defines && n.fact.has_arch) {
                return None;
            }
            match nodes.iter().find_map(|n| n.inherit.clone()) {
                Some(parent) => id = parent,
                None => return Some(id),
            }
        }
        None
    }

    pub fn visible(&self, id: &str, sees: impl Fn(usize) -> bool) -> bool {
        self.nodes
            .get(id)
            .is_some_and(|nodes| nodes.iter().any(|n| sees(n.module)))
    }

    /// The elements of every node in the tree under `root`.
    pub fn tree(&self, root: String, modules: &'m [Module], sees: impl Fn(usize) -> bool) -> ViewTree<'m> {
        let mut tree = ViewTree::default();
        let mut seen = FxHashSet::default();
        let mut stack = vec![root.as_str()];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            for node in self.nodes.get(id).map(Vec::as_slice).unwrap_or_default() {
                let module = modules[node.module].name.as_str();
                let elements = node.fact.elements.iter().map(|e| located(e, module));
                if sees(node.module) {
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

/// The tables of the models a non-abstract class introduces with `_name`: each `_table` their
/// classes set, or else the name with dots made underscores. A model with a `_table` that is no
/// literal has none.
fn table_models<'m>(models: &Models<'m>) -> FxHashMap<String, Vec<&'m str>> {
    let mut by_table: FxHashMap<String, Vec<&'m str>> = FxHashMap::default();
    for (&model, classes) in models {
        if !classes
            .iter()
            .any(|(_, c)| c.new_model && c.kind != ModelKind::Abstract)
        {
            continue;
        }
        let set: Option<Vec<&str>> = classes
            .iter()
            .filter_map(|(_, c)| c.table.as_ref())
            .map(|t| match t {
                OptionValue::Str(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        let Some(mut tables) = set else {
            continue;
        };
        let default = model.replace('.', "_");
        if tables.is_empty() {
            tables.push(&default);
        }
        tables.sort_unstable();
        tables.dedup();
        for table in tables {
            by_table.entry(table.to_owned()).or_default().push(model);
        }
    }
    by_table
}

fn method_flows(modules: &[Module]) -> MethodFlows<'_> {
    let mut flows: MethodFlows = FxHashMap::default();
    for (module, m) in modules.iter().enumerate() {
        for file in &m.python {
            for summary in &file.facts.flows {
                if let Some(Receiver::Model(model)) = &summary.owner {
                    let at = FlowRef { module, file, summary };
                    let methods = flows.entry(model.as_str()).or_default();
                    methods.entry(summary.method.as_str()).or_default().push(at);
                }
            }
        }
    }
    flows
}
