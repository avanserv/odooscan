//! Rules that need facts from more than one file.

use std::path::Path;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{
    ActionFact, ActionValue, Annotation, ArchElement, ArchModifier, ClassFact, DefinitionSource, DomainFact, FieldFact,
    FunctionCall, JsFacts, JsName, ModelKind, OptionValue, OrmCall, Pos, RawDiag, RecordFact, Spanned, SqlColumn,
    SqlView, ViewFact, XmlRef,
};
use crate::index::{
    BASE_MODEL_METHODS, Export, FieldDef, FieldInfo, Index, JsSpec, RELATIONAL_TYPES, Scope, Status, ViewTree, full_id,
    js_module_path, located,
};
use crate::project::{Module, SourceFile};
use crate::rules::rule;
use crate::series::{self, Series};
use crate::xml::{LOCATOR, QWEB, UNKNOWN_CONTEXT};
use crate::{flows, manifest, modifier, module_state};

/// Directories of upgrade scripts, which name XML ids of past versions.
const UPGRADE_DIRS: &[&str] = &["migrations", "upgrades"];

/// Types a One2many's inverse may have: an Integer holds the id of a `res_model`/`res_id` pair.
const INVERSE_TYPES: &[&str] = &["Many2one", "Many2oneReference", "Integer"];

/// The longest identifier PostgreSQL keeps, which Odoo checks a relation table name against.
const PG_NAME_MAX: usize = 63;

/// Directories whose XML and CSV files are not meant to be listed in the manifest.
const UNLISTED_DIRS: &[&str] = &["tests", "i18n", "migrations", "upgrades", "doc", "docs", "examples"];

/// Names 17.0 lets a modifier read with a deprecation warning, and saas~17.1 no longer does.
const ACTIVE_NAMES: &[&str] = &["active_id", "active_ids", "active_model"];
const ACTIVE_NAMES_UNTIL: Series = Series { major: 17, minor: 1 };

/// View types whose arch elements list every field the view has. The others also take fields from
/// attributes, as a calendar does from `date_start`.
const VIEW_FIELD_KINDS: &[&str] = &["form", "tree", "kanban"];

/// View types Odoo generates for any model. `activity` is `BaseModel`'s in 12.0, which no scan
/// indexes, and `mail`'s after it.
const GENERATED_VIEWS: &[&str] = &["form", "search", "list", "tree", "pivot", "kanban", "graph", "activity"];

/// Fields Odoo's generated calendar takes its start from, after the model's `_date_name`.
const CALENDAR_STARTS: &[&str] = &["date", "date_start", "x_date", "x_date_start"];

/// Fields Odoo's generated calendar takes its end or its duration from.
const CALENDAR_STOPS: &[&str] = &[
    "date_stop",
    "date_end",
    "x_date_stop",
    "x_date_end",
    "date_delay",
    "planned_hours",
    "x_date_delay",
    "x_planned_hours",
];

/// 19.0 refuses to generate a calendar without a stop (`odoo/addons/base/models/ir_ui_view.py:2898`),
/// saas-19.1 does not (`:2909`).
const CALENDAR_STOP_OPTIONAL: Series = Series { major: 19, minor: 1 };

/// A name a modifier reads, with the view it sits in and the field it must find on `owner`.
struct ModifierName<'a> {
    model: &'a str,
    view: &'a ViewFact,
    kind: &'a str,
    attr: &'a str,
    name: &'a str,
    owner: String,
    field: &'a str,
}

pub struct Out<'m> {
    pub diags: Vec<(&'m Path, RawDiag)>,
}

impl<'m> Out<'m> {
    fn push(&mut self, path: &'m Path, code: &'static str, pos: Pos, message: impl Into<String>) {
        self.diags.push((path, RawDiag::new(code, pos, message)));
    }
}

pub fn check_module<'m>(index: &Index<'m>, mi: usize, series: Option<Series>) -> Vec<(&'m Path, RawDiag)> {
    let module = &index.modules[mi];
    let mut out = Out { diags: vec![] };
    manifest_files(module, &mut out);
    unloaded_files(module, series, &mut out);
    same_module_xmlids(module, series, &mut out);
    shadowed_members(module, series, &mut out);
    legacy_hook_rollbacks(module, series, &mut out);
    cache_key_names(module, series, &mut out);
    import_patches(index, module, &mut out);
    for (_, file) in es_modules(index, mi, series) {
        if let Some(e) = &file.facts.syntax_error {
            out.push(&file.path, "OD000", e.pos, e.value.clone());
        }
        private_rpc(&mut out, file);
    }
    flows::check(index, mi, &index.scope(mi), &mut out);
    if index.has_base {
        let scope = index.scope(mi);
        let ctx = Ctx {
            index,
            module,
            scope: &scope,
            dep_names: module.manifest.depends.iter().map(|d| d.value.as_str()).collect(),
            dynamic: module.python.iter().any(|f| f.facts.dynamic_models),
            partial: index.partial_scope(&scope),
            series,
        };
        ctx.dependencies(&mut out);
        ctx.python(&mut out);
        ctx.registry(&mut out);
        ctx.data(&mut out);
        for (fi, file) in es_modules(index, mi, series) {
            ctx.js(&mut out, file);
            ctx.js_patches(&mut out, mi, fi, file);
            for call in &file.facts.orm_calls {
                ctx.orm_call(&mut out, &file.path, call);
            }
            // A base whose subclasses set their own template may name one that is never rendered.
            for class in file
                .facts
                .classes
                .iter()
                .filter(|c| !index.template_base(mi, fi, &c.name))
            {
                if let Some(template) = &class.template {
                    ctx.template_ref(&mut out, &file.path, template);
                }
            }
        }
        ctx.templates(&mut out, mi);
    }
    out.diags
}

/// The module's scripts that a bundle loads as ES modules: annotated, or from 17.0 on, any script
/// under `static/src` not marked `ignore`. A script that is not one cannot use `import`.
fn es_modules<'a, 'm>(
    index: &'a Index<'m>,
    mi: usize,
    series: Option<Series>,
) -> impl Iterator<Item = (usize, &'m SourceFile<JsFacts>)> + 'a {
    let unannotated = series.is_some_and(|s| s >= Series { major: 17, minor: 0 });
    index.modules[mi].js.iter().enumerate().filter(move |(fi, f)| {
        index.js_loaded(mi, *fi)
            && match f.facts.annotation {
                Annotation::Module => true,
                Annotation::Ignore => false,
                Annotation::None => unannotated,
            }
    })
}

/// The server refuses to dispatch a method whose name starts with `_` to a model.
fn private_rpc<'m>(out: &mut Out<'m>, file: &'m SourceFile<JsFacts>) {
    let calls = file.facts.orm_calls.iter().filter_map(|c| c.method.as_ref());
    for method in calls.filter(|m| m.value.starts_with('_')) {
        let message = format!(
            "method '{}' is private, so the server refuses to call it over RPC",
            method.value
        );
        out.push(&file.path, "OD607", method.pos, message);
    }
}

/// The record attributes `env`, `ids` and `pool`, and the class attribute `_table_query`.
const MODEL_ATTRS: &[&str] = &["env", "ids", "pool", "_table_query"];

/// Members of `BaseModel` that only some series have, by the range of series that has them. 18.0
/// deprecates `check_access_rights` and `check_access_rule` for `check_access` and `has_access`
/// (`odoo/models.py:4503-4530`). The others are in 18.0 `odoo/models.py` or 20.0 `odoo/orm`, not
/// both. Every series was read for `_table_query`, which came in saas~13.5, and for
/// `_condition_to_sql`, which came in saas~17.1 and left `BaseModel` for `Field`, where a model
/// field does not shadow it, in saas~18.4. For the other names 19.0 was not read, so it matches
/// `>=18.0` but neither `>=20.0` nor `<=18.0`.
const MEMBER_SERIES: &[(&str, &[&str])] = &[
    (">=18.0", &["check_access", "has_access"]),
    (
        ">=20.0",
        &[
            "_access_domain",
            "_additional_allowed_keys_properties_definition",
            "_can_return_content",
            "_has_field_access",
            "_make_access_error_message",
            "_post_model_setup__",
            "_read_grouping_sets",
        ],
    ),
    (
        "<=18.0",
        &[
            "toggle_active",
            "check_access_rights",
            "check_access_rule",
            "_filter_access_rules",
            "check_field_access_rights",
            "_where_calc",
            "_setup_complete",
            "copy_translations",
            "_check_recursion",
            "_check_m2m_recursion",
            "_add_inherited_fields",
        ],
    ),
    (">=13.5,<20.0", &["_table_query"]),
    (">=17.1,<18.4", &["_condition_to_sql"]),
];

/// The names Odoo's cache module binds at run time, by the range of series that has them, read on
/// every branch from 10.0 to saas~19.4, so no row reaches before 10.0. Before 19.0 Odoo evaluates
/// an `ormcache` key with the globals of that module (18.0 `odoo/tools/cache.py:90,148`), so a key
/// may read them; from 19.0 it gives the key only `method` (19.0 `odoo/tools/cache.py:136-138`).
const CACHE_KEY_GLOBALS: &[(&str, &[&str])] = &[
    (
        ">=10.0,<19.0",
        &[
            "defaultdict",
            "decorator",
            "logging",
            "unsafe_eval",
            "_logger",
            "ormcache_counter",
            "STAT",
            "ormcache",
            "ormcache_context",
            "log_ormcache_stats",
            "get_cache_key_counter",
        ],
    ),
    (">=10.0,<13.1", &["formatargspec", "getargspec"]),
    (">=10.0,<16.4", &["ormcache_multi", "dummy_cache"]),
    (">=10.0,<18.2", &["cache"]),
    (">=11.0,<12.1", &["pycompat"]),
    (">=13.1,<19.0", &["signature"]),
    (">=13.4,<18.3", &["Counter"]),
    (">=16.2,<19.0", &["Parameter"]),
    (">=16.4,<18.1", &["warnings"]),
    (">=17.2,<19.0", &["time"]),
    (">=18.1,<19.0", &["annotations", "typing"]),
    // saas~18.1 drops the import of `warnings`, and saas~18.2 brings it back to deprecate `skiparg`.
    (">=18.2,<19.0", &["warnings"]),
    (
        ">=18.3,<19.0",
        &[
            "Mapping",
            "Collection",
            "chain",
            "signal",
            "sys",
            "threading",
            "OrmCacheStatsLogger",
            "get_cache_size",
        ],
    ),
    (">=19.0", &["method"]),
];

/// Methods of `BaseModel` that neither 18.0 nor 20.0 has, so the series that do are not known.
/// The other methods are in both, and when they came is not known either, so they count in every
/// series: `grouped`, `fetch` and `search_fetch` among them.
const UNPLACED_MEMBERS: &[&str] = &["copy_multi", "_read", "_name_search"];

/// Python finds a field before the member of `BaseModel` it is named like, so the model loses the
/// member. Odoo never checks for it.
fn shadowed_members<'m>(module: &'m Module, series: Option<Series>, out: &mut Out<'m>) {
    for file in &module.python {
        for field in file.facts.classes.iter().flat_map(|c| &c.fields) {
            if let Some(member) = shadowed_member(&field.name, series) {
                let message = format!(
                    "field '{}' replaces the {member} of BaseModel on this model, so callers get the field instead; rename it",
                    field.name
                );
                out.push(&file.path, "OD116", field.name_pos, message);
            }
        }
    }
}

/// From 16.0 Odoo no longer calls `ir.http._handle_exception()`, so its rollbacks are reported as
/// any other's.
fn legacy_hook_rollbacks<'m>(module: &'m Module, series: Option<Series>, out: &mut Out<'m>) {
    if series.is_some_and(|s| s >= Series { major: 16, minor: 0 }) {
        for file in &module.python {
            let found = file.facts.legacy_hook_rollbacks.iter().cloned();
            out.diags.extend(found.map(|d| (file.path.as_path(), d)));
        }
    }
}

/// OD120 on the names an `ormcache` key reads that Odoo's cache module does not bind either.
fn cache_key_names<'m>(module: &'m Module, series: Option<Series>, out: &mut Out<'m>) {
    for file in &module.python {
        let found = file
            .facts
            .cache_key_names
            .iter()
            .filter(|(name, _)| !cache_global(name, series));
        out.diags.extend(found.map(|(_, d)| (file.path.as_path(), d.clone())));
    }
}

/// OD123 on each patch at import, with the advice for a model class when the index, or failing it
/// the path, tells one.
fn import_patches<'m>(index: &Index<'m>, module: &'m Module, out: &mut Out<'m>) {
    for file in &module.python {
        let found = file.facts.import_patches.iter();
        out.diags.extend(found.map(|p| {
            let diag = module_state::diag(p, index.model_class(&p.path));
            (file.path.as_path(), diag)
        }));
    }
}

/// Whether a key may read `name` as a global of Odoo's cache module in `series`. A module of
/// unknown series may run on any, so it may read the names of every series.
fn cache_global(name: &str, series: Option<Series>) -> bool {
    CACHE_KEY_GLOBALS
        .iter()
        .any(|(spec, names)| names.contains(&name) && series.is_none_or(|s| series::matches(spec, s)))
}

/// The member of `BaseModel` in `series` that a field called `name` replaces, as the message
/// names it. A module of unknown series has only the members of every series.
fn shadowed_member(name: &str, series: Option<Series>) -> Option<String> {
    let range = MEMBER_SERIES.iter().find(|(_, names)| names.contains(&name));
    if range.is_some_and(|(spec, _)| !series.is_some_and(|s| series::matches(spec, s))) {
        return None;
    }
    if MODEL_ATTRS.contains(&name) {
        Some(format!("attribute {name}"))
    } else if BASE_MODEL_METHODS.contains(&name) && !UNPLACED_MEMBERS.contains(&name) {
        Some(format!("method {name}()"))
    } else {
        None
    }
}

fn manifest_files<'m>(module: &'m Module, out: &mut Out<'m>) {
    for file in module.manifest.data.iter().chain(&module.manifest.demo) {
        if !module.dir.join(&file.value).is_file() {
            out.push(
                &module.manifest_path,
                "OD501",
                file.pos,
                format!("'{}' does not exist", file.value),
            );
        }
    }
}

fn unloaded_files<'m>(module: &'m Module, series: Option<Series>, out: &mut Out<'m>) {
    let mentioned = |in_manifest: bool| -> FxHashSet<&str> {
        let files = module
            .python
            .iter()
            .filter(|f| (f.path == module.manifest_path) == in_manifest);
        files.flat_map(|f| &f.facts.file_mentions).map(String::as_str).collect()
    };
    let (in_code, in_manifest) = (mentioned(false), mentioned(true));
    for file in module.data.iter().filter(|f| f.rank.is_none() && f.facts.is_data) {
        let top = file.rel.split('/').next().unwrap_or_default();
        let base = file.rel.rsplit('/').next().unwrap_or_default();
        let ignored = module.manifest.ignored_key(&file.rel, series);
        // Chart templates and files named in Python code are loaded by that code.
        let code_loaded = in_code.contains(base) || file.rel.starts_with("data/template/");
        // A file listed under a legacy key was meant to load, wherever it sits. The manifest names
        // the files of keys such as `other_files` too, which the module loads itself.
        let unlisted = ignored.is_none()
            && (!file.rel.contains('/') || UNLISTED_DIRS.contains(&top) || in_manifest.contains(base));
        if !code_loaded && !unlisted {
            out.push(
                &file.path,
                "OD502",
                Pos { line: 1, col: 1 },
                unloaded(&file.rel, ignored, series),
            );
        }
    }
}

/// OD502's message for a data file Odoo never loads, listed under `ignored` or under no key.
fn unloaded(rel: &str, ignored: Option<&str>, series: Option<Series>) -> String {
    if let Some(key) = ignored {
        let to = if key == "demo_xml" { "demo" } else { "data" };
        return format!("'{rel}' is listed under {key}, which Odoo ignores in this series; move it to {to}");
    }
    let keys: Vec<_> = manifest::loaded_keys(series).collect();
    let keys = match keys.split_last() {
        Some((last, [])) => (*last).to_owned(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    };
    format!("'{rel}' is not in the manifest's {keys}, so Odoo never loads it")
}

fn same_module_xmlids<'m>(module: &'m Module, series: Option<Series>, out: &mut Out<'m>) {
    // base_data.sql creates base's bootstrap records before any data file loads.
    let bootstrapped = module.name == "base";
    let first_def = first_defs(module, out);
    for file in &module.data {
        let Some(rank) = file.rank else { continue };
        for r in &file.facts.refs {
            let id = full_id(&module.name, &r.id);
            if let Some(&(def_rank, def_line)) = first_def.get(&id)
                && (def_rank, def_line) > (rank, r.pos.line)
                && !bootstrapped
                && !registry_id(module, &id)
            {
                let place = load_place(module, series, def_rank, rank);
                out.push(
                    &file.path,
                    "OD308",
                    r.pos,
                    format!("'{}' is defined {place}, which loads after this reference", r.id),
                );
            }
        }
    }
}

/// Where each of the module's own XML ids is first defined, as (load rank, line).
fn first_defs<'m>(module: &'m Module, out: &mut Out<'m>) -> FxHashMap<String, (usize, u32)> {
    let mut first_def: FxHashMap<String, (usize, u32)> = FxHashMap::default();
    for file in &module.data {
        let mut in_file: FxHashSet<&str> = FxHashSet::default();
        for def in &file.facts.defs {
            if !def.assigned && !in_file.insert(&def.id) {
                out.push(
                    &file.path,
                    "OD303",
                    def.pos,
                    format!("XML id '{}' is already defined earlier in this file", def.id),
                );
            }
            // A dotted id naming another module overrides that module's record; it defines nothing here.
            let own = def.id.split_once('.').is_none_or(|(m, _)| m == module.name);
            if let Some(rank) = file.rank.filter(|_| own) {
                first_def
                    .entry(full_id(&module.name, &def.id))
                    .or_insert((rank, def.pos.line));
            }
        }
    }
    first_def
}

/// An Image field is a Binary one to Odoo, which compares field types by that name.
fn odoo_type(ftype: &str) -> &str {
    if ftype == "Image" { "Binary" } else { ftype }
}

/// The registry creates the `ir.model` and `ir.model.fields` ids of a module's models before any of
/// its data files load, so a record with such an id only updates it.
fn registry_id(module: &Module, id: &str) -> bool {
    let local = id.split_once('.').map_or(id, |(_, l)| l);
    module.python.iter().flat_map(|f| &f.facts.classes).any(|c| {
        let model = c.model.replace('.', "_");
        local == format!("model_{model}") || local.starts_with(&format!("field_{model}__"))
    })
}

fn load_place(module: &Module, series: Option<Series>, def_rank: usize, rank: usize) -> String {
    if def_rank == rank {
        return "further down this file".to_owned();
    }
    let file = module.manifest.loaded_files(series).nth(def_rank);
    format!("in {}", file.unwrap_or_default())
}

struct Ctx<'a, 'm> {
    index: &'a Index<'m>,
    module: &'m Module,
    scope: &'a Scope,
    dep_names: FxHashSet<&'m str>,
    /// The module builds model names at runtime, so its models and their ids cannot be listed.
    dynamic: bool,
    /// A module in scope depends on one that is not indexed, such as an Enterprise module.
    partial: bool,
    series: Option<Series>,
}

impl<'m> Ctx<'_, 'm> {
    fn model_ref(&self, out: &mut Out<'m>, path: &'m Path, model: &Spanned<String>, except: Option<&ClassFact>) {
        match self.index.model_status(&model.value, self.scope, except) {
            Status::Visible => {}
            Status::Hidden(m) => out.push(
                path,
                "OD202",
                model.pos,
                format!("model '{}' comes from module {m}; add it to depends", model.value),
            ),
            Status::Missing if self.dynamic => {}
            Status::Missing => out.push(
                path,
                "OD201",
                model.pos,
                format!("no indexed module defines model '{}'", model.value),
            ),
        }
    }

    fn dependencies(&self, out: &mut Out<'m>) {
        let me = self.index.by_name[self.module.name.as_str()];
        let manifest = &self.module.manifest_path;
        for dep in &self.module.manifest.depends {
            match self.index.by_name.get(dep.value.as_str()) {
                None => out.push(
                    manifest,
                    "OD503",
                    dep.pos,
                    format!("module '{}' is not on the addons path", dep.value),
                ),
                Some(&d) if self.index.reaches(d, me) => out.push(
                    manifest,
                    "OD504",
                    dep.pos,
                    format!(
                        "module '{}' depends back on {}, so Odoo installs neither",
                        dep.value, self.module.name
                    ),
                ),
                Some(_) => {}
            }
        }
    }

    fn path(&self, out: &mut Out<'m>, file: &'m Path, code: &'static str, model: &str, path: &Spanned<String>) {
        if let Err(e) = self.index.resolve_path(model, &path.value, self.scope) {
            out.push(file, code, path.pos, e.message(&path.value));
        }
    }

    fn python(&self, out: &mut Out<'m>) {
        for file in &self.module.python {
            let path = file.path.as_path();
            for class in &file.facts.classes {
                self.class(out, path, class);
            }
            if !UPGRADE_DIRS.contains(&file.rel.split('/').next().unwrap_or_default()) {
                self.sql_columns(out, path, &file.facts.sql_columns);
                for r in &file.facts.xml_refs {
                    self.py_ref(out, path, r, true);
                }
                for r in &file.facts.group_refs {
                    self.py_ref(out, path, r, false);
                }
                self.domains(out, path, &file.facts.domains);
            }
            // env['x'] for a model outside depends is usually guarded by `if 'x' in self.env`, and
            // a dotless name is as likely a key of some other mapping called env.
            for model in file.facts.env_models.iter().filter(|m| m.value.contains('.')) {
                if matches!(self.index.model_status(&model.value, self.scope, None), Status::Missing) {
                    out.push(
                        path,
                        "OD206",
                        model.pos,
                        format!("no indexed module defines model '{}'", model.value),
                    );
                }
            }
        }
    }

    fn js(&self, out: &mut Out<'m>, file: &'m SourceFile<JsFacts>) {
        let from = js_module_path(&self.module.name, &file.rel).unwrap_or_default();
        for specifier in &file.facts.specifiers {
            self.js_specifier(out, &file.path, &from, specifier);
        }
        for name in &file.facts.names {
            self.js_name(out, &file.path, &from, name);
        }
    }

    fn js_specifier(&self, out: &mut Out<'m>, path: &'m Path, from: &str, specifier: &Spanned<String>) {
        let module = |mi: usize| &self.index.modules[mi].name;
        let message = match self.index.js_resolve(from, &specifier.value) {
            JsSpec::Missing(mi) => format!("no file of module {} provides '{}'", module(mi), specifier.value),
            JsSpec::Found(t) if !self.scope.sees(t.module) => format!(
                "'{}' comes from module {}, which is not in depends",
                specifier.value,
                module(t.module)
            ),
            _ => return,
        };
        out.push(path, "OD601", specifier.pos, message);
    }

    fn js_name(&self, out: &mut Out<'m>, path: &'m Path, from: &str, name: &JsName) {
        let JsSpec::Found(target) = self.index.js_resolve(from, &name.specifier) else {
            return;
        };
        if self.index.js_export(target, &name.name.value, 0) != Export::Missing {
            return;
        }
        let message = match name.name.value.as_str() {
            "default" => format!("'{}' has no default export", name.specifier),
            n => format!("'{}' does not export '{n}'", name.specifier),
        };
        out.push(path, "OD601", name.name.pos, message);
    }

    /// A patch of a class from a module outside the index, such as an Enterprise one, may add
    /// the member a `super` reads.
    fn js_patches(&self, out: &mut Out<'m>, mi: usize, fi: usize, file: &'m SourceFile<JsFacts>) {
        if self.partial {
            return;
        }
        let here = self.index.js_target(mi, fi);
        for (pi, patch) in file.facts.patches.iter().enumerate() {
            let Some((t, class)) = self.index.js_class(here, &patch.target, 0) else {
                continue;
            };
            let Some(members) = self.index.js_members(t, class, (mi, fi, pi)) else {
                continue;
            };
            for read in patch.supers.iter().filter(|s| !members.contains(s.value.as_str())) {
                out.push(
                    &file.path,
                    "OD602",
                    read.pos,
                    format!(
                        "neither {} nor its parents have '{}', so super.{}() fails",
                        class.name, read.value, read.value
                    ),
                );
            }
        }
    }

    fn orm_call(&self, out: &mut Out<'m>, path: &'m Path, call: &OrmCall) {
        let Some(model) = &call.model else {
            return;
        };
        let message = match self.index.model_status(&model.value, self.scope, None) {
            Status::Visible => return self.orm_method(out, path, call, &model.value),
            Status::Hidden(m) => format!("model '{}' comes from module {m}, which is not in depends", model.value),
            Status::Missing if !self.dynamic => format!("no indexed module defines model '{}'", model.value),
            _ => return,
        };
        out.push(path, "OD603", model.pos, message);
    }

    /// A method from a module outside the index, such as an Enterprise one, may be the one called.
    /// A private method is OD607's, whether the model has it or not.
    fn orm_method(&self, out: &mut Out<'m>, path: &'m Path, call: &OrmCall, model: &str) {
        let Some(method) = call
            .method
            .as_ref()
            .filter(|m| !self.partial && !self.dynamic && !m.value.starts_with('_'))
        else {
            return;
        };
        let message = match self.index.rpc_method(model, &method.value, self.scope) {
            Status::Visible => return,
            Status::Hidden(m) => format!(
                "method '{}' of {model} comes from module {m}, which is not in depends",
                method.value
            ),
            Status::Missing => format!("model '{model}' has no method '{}'", method.value),
        };
        out.push(path, "OD604", method.pos, message);
    }

    /// Whether the template `name` names is registered in scope. A template of a module outside the
    /// index, such as an Enterprise one, may be the one named.
    fn template_ref(&self, out: &mut Out<'m>, path: &'m Path, name: &Spanned<String>) -> bool {
        if self.partial {
            return false;
        }
        let message = match self.index.template_status(&name.value, self.scope) {
            Status::Visible => return true,
            Status::Hidden(m) => format!(
                "template '{}' comes from module {m}, which is not in depends",
                name.value
            ),
            Status::Missing => format!("no indexed bundle defines template '{}'", name.value),
        };
        out.push(path, "OD605", name.pos, message);
        false
    }

    fn templates(&self, out: &mut Out<'m>, mi: usize) {
        let mut trees: FxHashMap<String, ViewTree<'m>> = FxHashMap::default();
        for (fi, file) in self.module.templates.iter().enumerate() {
            if !self.index.template_loaded(mi, fi) {
                continue;
            }
            for t in &file.facts.templates {
                for call in &t.arch.calls {
                    self.template_ref(out, &file.path, call);
                }
                if let Some(parent) = &t.inherit
                    && self.template_ref(out, &file.path, parent)
                {
                    self.template_locators(out, &file.path, &parent.value, &t.arch, &mut trees);
                }
            }
        }
    }

    /// Specs of a template must each find their element in the tree of templates it extends.
    fn template_locators(
        &self,
        out: &mut Out<'m>,
        path: &'m Path,
        parent: &str,
        arch: &ViewFact,
        trees: &mut FxHashMap<String, ViewTree<'m>>,
    ) {
        let Some(root) = self.index.templates.root(parent.to_owned()) else {
            return;
        };
        let tree = trees.entry(root.clone()).or_insert_with(|| {
            let sees = |m| self.scope.sees(m);
            self.index.templates.tree(root, self.index.modules, sees)
        });
        for locator in &arch.locators {
            let e = &locator.value;
            if tree.visible.contains(e) {
                continue;
            }
            let tag = match e.tag.as_str() {
                "*" => "an element".to_owned(),
                tag => format!("<{tag}>"),
            };
            let element = match e.attr.as_str() {
                "hasclass" => format!("{tag} with class '{}'", e.value),
                attr => format!("{tag} with {attr}=\"{}\"", e.value),
            };
            let message = match tree.hidden.get(e) {
                Some(m) => format!("{element} is added by module {m}, which is not in depends"),
                None => format!("no {element} in template {} or the templates that extend it", tree.root),
            };
            out.push(path, "OD606", locator.pos, message);
        }
    }

    fn class(&self, out: &mut Out<'m>, path: &'m Path, class: &ClassFact) {
        for parent in &class.inherit {
            self.model_ref(out, path, parent, Some(class));
        }
        for field in &class.fields {
            self.field(out, path, class, field);
        }
        let paths = class
            .paths
            .iter()
            .chain(class.depends.iter().flat_map(|(_, paths)| paths))
            .chain(&class.order_fields)
            .chain(self.rec_names_search(class));
        for p in paths.filter(|_| class.kind != ModelKind::Abstract) {
            self.path(out, path, "OD203", &class.model, p);
        }
        self.redefinition(out, path, class);
        self.super_calls(out, path, class);
        if class.new_model
            && class.kind != ModelKind::Abstract
            && class.registered()
            && !self.index.models_with_access.contains(class.model.as_str())
        {
            out.push(
                path,
                "OD401",
                class.pos,
                format!(
                    "model '{}' has no ir.model.access rule, so only the superuser can use it",
                    class.model
                ),
            );
        }
    }

    /// `_rec_names_search` is read from saas~15.3 on.
    fn rec_names_search<'c>(&self, class: &'c ClassFact) -> &'c [Spanned<String>] {
        if self.series.is_some_and(|s| s >= REC_NAMES_SEARCH) {
            &class.rec_names_search
        } else {
            &[]
        }
    }

    /// A new `_name` that a module in scope already defines builds a fresh model in its place.
    fn redefinition(&self, out: &mut Out<'m>, path: &'m Path, class: &ClassFact) {
        if !class.new_model {
            return;
        }
        let me = self.index.by_name[self.module.name.as_str()];
        let other = self.index.classes(&class.model).iter().find(|(mi, c)| {
            c.new_model
                && !std::ptr::eq(*c, class)
                && self.scope.sees(*mi)
                && (*mi != me || self.defined_before(c, class))
        });
        if let Some((mi, _)) = other {
            out.push(
                path,
                "OD209",
                class.pos,
                format!(
                    "module {} already defines model '{}', and this class replaces it; extend it with _inherit = '{}'",
                    self.index.modules[*mi].name, class.model, class.model
                ),
            );
        }
    }

    /// Parents the index cannot see: a plain Python base of this class, an `_inherit` element that
    /// is not a literal in any class of the model or of a model it inherits, a module that is not
    /// indexed, a model nobody indexes, and for a mixin, every model that inherits it.
    fn super_calls(&self, out: &mut Out<'m>, path: &'m Path, class: &ClassFact) {
        if class.super_calls.is_empty()
            || class.plain_bases
            || self.partial
            || class.kind == ModelKind::Abstract
            || !self.index.has_model(&class.model)
            || self
                .index
                .chain_classes(&class.model)
                .iter()
                .any(|(_, c)| c.inherit_unknown)
        {
            return;
        }
        for call in &class.super_calls {
            if !self.index.has_parent_method(&class.model, &call.value, class) {
                out.push(
                    path,
                    "OD211",
                    call.pos,
                    format!(
                        "no parent class of {} defines '{}', so this super() call fails",
                        class.model, call.value
                    ),
                );
            }
        }
    }

    /// Whether `first` comes before `then` in the same file of this module. The order of the files
    /// is that of the imports, which is not indexed.
    fn defined_before(&self, first: &ClassFact, then: &ClassFact) -> bool {
        let me = self.index.by_name[self.module.name.as_str()];
        self.in_one_file(me, first, then) && first.pos < then.pos
    }

    fn in_one_file(&self, mi: usize, a: &ClassFact, b: &ClassFact) -> bool {
        self.index.modules[mi].python.iter().any(|f| {
            let has = |c: &ClassFact| f.facts.classes.iter().any(|x| std::ptr::eq(x, c));
            has(a) && has(b)
        })
    }

    fn field(&self, out: &mut Out<'m>, path: &'m Path, class: &ClassFact, field: &FieldFact) {
        let index = self.index;
        if let Some(comodel) = &field.comodel {
            self.model_ref(out, path, comodel, None);
        }
        // A mixin's paths name fields of the models that inherit it.
        let mixin = class.kind == ModelKind::Abstract;
        if let Some(related) = field.related.as_ref().filter(|_| !mixin) {
            self.path(out, path, "OD203", &class.model, related);
            self.related_type(out, path, class, field, related);
        }
        self.model_field(out, path, class, field);
        if let (Some(comodel), Some(inverse)) = (&field.comodel, &field.inverse_name)
            && index.has_model(&comodel.value)
        {
            self.path(out, path, "OD203", &comodel.value, inverse);
            self.inverse(out, path, comodel, inverse);
        }
        if let Some(comodel) = &field.comodel {
            for f in &field.domain {
                self.path(out, path, "OD210", &comodel.value, f);
            }
        }
        self.missing_methods(out, path, class, field);
        if let Some(t) = self.other_type(class, field) {
            out.push(
                path,
                "OD205",
                field.pos,
                format!(
                    "'{}' is a {t} field elsewhere and is redefined here as {}",
                    field.name, field.ftype
                ),
            );
        }
    }

    /// Odoo replaces the compute and inverse of a related field with its own, and a parent the scan
    /// cannot see may hold methods only Python knows.
    fn missing_methods(&self, out: &mut Out<'m>, path: &'m Path, class: &ClassFact, field: &FieldFact) {
        if field.related.is_some() || self.index.hidden_parents(&class.model, self.scope) {
            return;
        }
        for method in &field.methods {
            if !self.index.has_method(&class.model, &method.value, self.scope) {
                let message = format!("{} has no method '{}'", class.model, method.value);
                out.push(path, "OD204", method.pos, message);
            }
        }
    }

    /// A Many2oneReference reads its record's model from the Char field `model_field` names, which
    /// in a mixin is a field of the models that inherit it. A related one keeps the `model_field`
    /// it sets; one it inherits from its target is not checked.
    fn model_field(&self, out: &mut Out<'m>, path: &'m Path, class: &ClassFact, field: &FieldFact) {
        if field.ftype != "Many2oneReference" || class.kind == ModelKind::Abstract {
            return;
        }
        if let Some(Spanned {
            value: OptionValue::Str(name),
            pos,
        }) = field.option("model_field")
        {
            let name = Spanned {
                value: name.clone(),
                pos: *pos,
            };
            self.path(out, path, "OD203", &class.model, &name);
        }
    }

    /// Odoo refuses to load a related field whose type is not its target's.
    fn related_type(
        &self,
        out: &mut Out<'m>,
        path: &'m Path,
        class: &ClassFact,
        field: &FieldFact,
        related: &Spanned<String>,
    ) {
        let Some(targets) = self.index.path_target(&class.model, &related.value, self.scope) else {
            return;
        };
        if targets.iter().all(|t| odoo_type(&t.ftype) != odoo_type(&field.ftype)) {
            out.push(
                path,
                "OD207",
                related.pos,
                format!(
                    "'{}' is a {} field related to '{}', which is a {} field",
                    field.name, field.ftype, related.value, targets[0].ftype
                ),
            );
        }
    }

    /// A One2many lists the comodel records whose inverse field points back at this record.
    fn inverse(&self, out: &mut Out<'m>, path: &'m Path, comodel: &Spanned<String>, inverse: &Spanned<String>) {
        let defs: Vec<&FieldFact> = self
            .index
            .field_defs(&comodel.value, &inverse.value, Some(self.scope))
            .into_iter()
            .map(|(_, _, f)| f)
            .collect();
        if let Some(first) = defs.first()
            && defs.iter().all(|f| !INVERSE_TYPES.contains(&f.ftype.as_str()))
        {
            let message = format!(
                "inverse '{}' on {} is a {} field, not a Many2one",
                inverse.value, comodel.value, first.ftype
            );
            out.push(path, "OD208", inverse.pos, message);
        }
    }

    /// A type another class of the same model in scope gives the field, when it differs from this
    /// one's. A new model, or one delegating through `_inherits`, may give a parent's field any type.
    fn other_type(&self, class: &ClassFact, field: &FieldFact) -> Option<&'m str> {
        self.index
            .field_defs(&class.model, &field.name, Some(self.scope))
            .into_iter()
            .filter(|(_, c, _)| !std::ptr::eq(*c, class) && c.model == class.model)
            .map(|(_, _, f)| f.ftype.as_str())
            .find(|t| *t != field.ftype)
    }

    fn data(&self, out: &mut Out<'m>) {
        let mut trees: FxHashMap<String, ViewTree<'m>> = FxHashMap::default();
        // Files the manifest does not load, such as chart template CSVs read by Python code, are skipped.
        for file in self.module.data.iter().filter(|f| f.rank.is_some()) {
            let path = file.path.as_path();
            for r in &file.facts.refs {
                self.xml_ref(out, path, r);
            }
            for view in &file.facts.views {
                self.view(out, path, view, &mut trees);
                self.locators(out, path, view, &mut trees);
                self.qweb_calls(out, path, view);
            }
            for record in &file.facts.records {
                self.record(out, path, record);
            }
            for call in &file.facts.functions {
                self.function_call(out, path, call);
            }
            self.domains(out, path, &file.facts.action_domains);
            self.actions(out, path, &file.facts.actions);
        }
    }

    fn actions(&self, out: &mut Out<'m>, path: &'m Path, actions: &'m [ActionFact]) {
        for action in actions {
            if let ActionValue::Literal(model) = &action.model {
                self.model_ref(out, path, model, None);
            }
            if let ActionValue::Literal(mode) = &action.view_mode {
                self.action_view_mode(out, path, action, mode);
            }
        }
    }

    /// Checks `mode` when Odoo keeps it. Each field holds the last value it was given, so a `view_mode`
    /// counts only if nothing replaces it, and an update takes its model from what the load left.
    /// Only `scope` loads ahead of this module: what a module depending on it replaces later does not
    /// mend a database without that module. A model that stays unknown is out of the scan or an `eval`.
    fn action_view_mode(&self, out: &mut Out<'m>, path: &'m Path, action: &'m ActionFact, mode: &Spanned<String>) {
        let own = [(self.index.by_name[self.module.name.as_str()], action)];
        let id = action.id.as_ref().map(|id| full_id(&self.module.name, id));
        let writes = id.and_then(|id| self.index.action_writes.get(&id));
        let writes = writes.map_or(&own[..], Vec::as_slice);
        let last = |value: fn(&'m ActionFact) -> &'m ActionValue| {
            let loaded = writes.iter().filter(|(mi, _)| self.scope.sees(*mi));
            loaded
                .rev()
                .map(|(_, a)| value(a))
                .find(|v| !matches!(v, ActionValue::Unset))
        };
        if !last(|a| &a.view_mode).is_some_and(|v| std::ptr::eq(v, &action.view_mode)) {
            return;
        }
        if let Some(ActionValue::Literal(model)) = last(|a| &a.model)
            && self.sees_whole_model(&model.value)
        {
            self.view_mode(out, path, &model.value, mode, writes);
        }
    }

    /// A view an action names counts from any indexed module, as one in `view_types` does.
    fn view_mode(
        &self,
        out: &mut Out<'m>,
        path: &'m Path,
        model: &str,
        mode: &Spanned<String>,
        writes: &[(usize, &ActionFact)],
    ) {
        let mut seen = FxHashSet::default();
        let kinds = mode.value.split(',').map(str::trim).filter(|k| !k.is_empty());
        for kind in kinds.filter(|k| seen.insert(*k)) {
            if writes.iter().any(|(_, a)| a.names_view(kind)) {
                continue;
            }
            if let Some(message) = self.view_gap(model, kind) {
                out.push(path, "OD318", mode.pos, message);
            }
        }
    }

    /// Why opening `model` as `kind` fails. A view counts from any indexed module, in `depends` or
    /// not, as a bridge module that depends on this one may be what adds it. A default counts in
    /// scope only: it comes with a module such as `web_gantt`, which nothing else installs.
    fn view_gap(&self, model: &str, kind: &str) -> Option<String> {
        if GENERATED_VIEWS.contains(&kind)
            || self.index.view_types.contains(&(model, kind))
            || self.index.default_view(model, kind, self.scope)
        {
            return None;
        }
        if kind == "calendar" {
            return self.calendar_gap(model);
        }
        Some(format!("{model} has no {kind} view, and nothing generates one"))
    }

    /// What Odoo misses to generate a calendar for `model`. A field only a module outside `depends`
    /// adds counts, as that module may be installed. A `_date_name` that is no literal may name any.
    fn calendar_gap(&self, model: &str) -> Option<String> {
        let mut starts = CALENDAR_STARTS.to_vec();
        for (_, class) in self.index.merged_classes(model, self.scope) {
            match class.attr("_date_name").map(|a| &a.value) {
                Some(OptionValue::Str(name)) => starts.push(name),
                Some(_) => return None,
                None => {}
            }
        }
        let has = |field: &&str| !matches!(self.index.field(model, field, self.scope), Err(None));
        if !starts.iter().any(has) {
            return Some(format!(
                "{model} has no calendar view, and no date field to generate one from"
            ));
        }
        let needs_stop = self.series.is_some_and(|s| s < CALENDAR_STOP_OPTIONAL);
        (needs_stop && !CALENDAR_STOPS.iter().any(has))
            .then(|| format!("{model} has no calendar view, and no stop or duration field to generate one from"))
    }

    fn xml_ref(&self, out: &mut Out<'m>, path: &'m Path, r: &XmlRef) {
        let index = self.index;
        let id = full_id(&self.module.name, &r.id);
        match index.xmlids.get(&id) {
            Some(defs) if defs.iter().any(|d| self.scope.sees(d.module)) => {}
            Some(defs) => {
                let m = &index.modules[defs[0].module].name;
                out.push(
                    path,
                    "OD302",
                    r.pos,
                    format!("'{id}' is defined by module {m}; add it to depends"),
                );
            }
            None => self.undefined_ref(out, path, r.pos, &id),
        }
    }

    /// An `ir.model` or `ir.model.fields` id of a model this module names at runtime.
    fn runtime_id(&self, owner: &str, local: &str) -> bool {
        self.dynamic && owner == self.module.name && (local.starts_with("model_") || local.starts_with("field_"))
    }

    fn undefined_ref(&self, out: &mut Out<'m>, path: &'m Path, pos: Pos, id: &str) {
        let (owner, local) = id.split_once('.').unwrap_or_default();
        if self.runtime_id(owner, local) {
            return;
        }
        match self.index.by_name.get(owner) {
            Some(&mi) if self.scope.sees(mi) => {
                out.push(path, "OD301", pos, format!("module {owner} defines no XML id '{id}'"))
            }
            Some(_) => out.push(
                path,
                "OD302",
                pos,
                format!("'{id}' belongs to module {owner}; add it to depends"),
            ),
            None if !self.dep_names.contains(owner) => {
                out.push(
                    path,
                    "OD302",
                    pos,
                    format!("'{id}' belongs to module {owner}, which is neither in depends nor on the addons path"),
                );
            }
            None => {}
        }
    }

    /// Python code may name ids of modules outside the index, such as Enterprise ones, behind a
    /// runtime check, so only ids of indexed modules are checked; `hidden` also reports those of
    /// modules missing from depends.
    fn py_ref(&self, out: &mut Out<'m>, path: &'m Path, r: &XmlRef, hidden: bool) {
        let owner = r.id.split_once('.').map_or("", |(m, _)| m);
        let Some(&mi) = self.index.by_name.get(owner) else {
            return;
        };
        if !self.scope.sees(mi) {
            if hidden {
                let message = format!("'{}' belongs to module {owner}; add it to depends", r.id);
                out.push(path, "OD302", r.pos, message);
            }
        } else if !self.index.xmlids.contains_key(&r.id) {
            out.push(
                path,
                "OD301",
                r.pos,
                format!("module {owner} defines no XML id '{}'", r.id),
            );
        }
    }

    fn view(&self, out: &mut Out<'m>, path: &'m Path, view: &ViewFact, trees: &mut FxHashMap<String, ViewTree<'m>>) {
        let model = match (&view.model, &view.inherit) {
            (Some(m), _) => {
                self.model_ref(out, path, m, None);
                Some(m.value.as_str())
            }
            (None, Some(parent)) => self.index.view_model(full_id(&self.module.name, parent)),
            (None, None) => None,
        };
        let Some(model) = model else { return };
        self.view_domains(out, path, model, view);
        self.buttons(out, path, model, view);
        self.modifiers(out, path, model, view, trees);
        for field in view
            .fields
            .iter()
            .filter(|f| !f.path.iter().any(|s| s == UNKNOWN_CONTEXT))
        {
            let Some(joined) = self.arch_path(model, &field.path) else {
                continue;
            };
            if let Err(e) = self.index.resolve_path(model, &joined, self.scope) {
                out.push(path, "OD304", field.pos, e.message(&joined));
            }
        }
    }

    fn domains(&self, out: &mut Out<'m>, path: &'m Path, domains: &[DomainFact]) {
        for d in domains {
            for f in &d.fields {
                self.path(out, path, "OD210", &d.model, f);
            }
        }
    }

    /// Domains in an arch filter the model at their position: the view's, or a subview's comodel.
    fn view_domains(&self, out: &mut Out<'m>, path: &'m Path, model: &str, view: &ViewFact) {
        for d in &view.domains {
            let target = self.arch_model(model, &d.path);
            for f in target.iter().flat_map(|t| d.fields.iter().map(move |f| (t, f))) {
                self.path(out, path, "OD210", f.0, f.1);
            }
        }
    }

    /// The model of an arch position: the view's, or the comodel of the subviews that enclose it.
    fn arch_model(&self, model: &str, segments: &[String]) -> Option<String> {
        if segments.iter().any(|s| s == UNKNOWN_CONTEXT) {
            return None;
        }
        match self.arch_path(model, segments)?.as_str() {
            "" => Some(model.to_owned()),
            joined => self.index.path_comodel(model, joined, self.scope),
        }
    }

    /// The type of the tree a view belongs to, unless it is QWeb, whose attributes are plain HTML. A
    /// root the index cannot read may be QWeb.
    fn arch_kind<'a>(&'a self, view: &'a ViewFact) -> Option<&'a str> {
        let kind = match view.inherit.as_deref() {
            Some(parent) => self.index.view_kind(full_id(&self.module.name, parent)),
            None => view.kind.as_deref(),
        };
        kind.filter(|k| *k != QWEB)
    }

    /// Odoo checks a button on the model at its arch position.
    fn buttons(&self, out: &mut Out<'m>, path: &'m Path, model: &str, view: &ViewFact) {
        if self.arch_kind(view).is_none() {
            return;
        }
        for button in &view.buttons {
            if let Some(target) = self.arch_model(model, &button.path) {
                self.button(out, path, &target, &button.method);
            }
        }
    }

    /// A leading `_` is private whatever the model defines, so it needs no lookup. `@api.private`
    /// counts on any definition, since Odoo walks the whole MRO.
    fn button(&self, out: &mut Out<'m>, path: &'m Path, model: &str, method: &Spanned<String>) {
        let name = &method.value;
        if name.starts_with('_') || self.index.private_method(model, name, self.scope) {
            let message = format!("method '{name}' of {model} is private, so Odoo refuses to call it from a button");
            out.push(path, "OD313", method.pos, message);
        } else if let Some(message) = self.method_gap(model, name) {
            out.push(path, "OD312", method.pos, message);
        }
    }

    fn function_call(&self, out: &mut Out<'m>, path: &'m Path, call: &FunctionCall) {
        if let Some(message) = self.method_gap(&call.model, &call.method.value) {
            out.push(path, "OD314", call.method.pos, message);
        }
    }

    /// Whether the scan sees every class that could add to `model`. A model that is not visible is
    /// OD201's or OD202's.
    fn sees_whole_model(&self, model: &str) -> bool {
        !self.partial
            && !self.dynamic
            && matches!(self.index.model_status(model, self.scope, None), Status::Visible)
            && !self.index.hidden_parents(model, self.scope)
            && !self.index.unindexed_parent(model, self.scope)
    }

    /// Why `model` lacks the method a data file calls, when the scan sees every class that could
    /// define it.
    fn method_gap(&self, model: &str, method: &str) -> Option<String> {
        if !self.sees_whole_model(model) {
            return None;
        }
        match self.index.data_method(model, method, self.scope) {
            Status::Visible => None,
            Status::Hidden(m) => Some(format!(
                "method '{method}' of {model} comes from module {m}, which is not in depends"
            )),
            Status::Missing => Some(format!("{model} has no method '{method}'")),
        }
    }

    /// Specs of a view must each find their element in the tree of views it extends, which is
    /// built once per root and module.
    fn locators(
        &self,
        out: &mut Out<'m>,
        path: &'m Path,
        view: &'m ViewFact,
        trees: &mut FxHashMap<String, ViewTree<'m>>,
    ) {
        let Some(parent) = view.inherit.as_deref().filter(|_| !view.locators.is_empty()) else {
            return;
        };
        let parent = full_id(&self.module.name, parent);
        // A parent outside depends is already reported by OD302.
        if !self.index.views.visible(&parent, |m| self.scope.sees(m)) {
            return;
        }
        let Some(tree) = self.view_tree(parent, trees) else {
            return;
        };
        for locator in &view.locators {
            let e = located(&locator.value, &self.module.name);
            if tree.visible.contains(&e) {
                continue;
            }
            let element = format!("<{} {}=\"{}\">", e.tag, e.attr, e.value);
            let message = match tree.hidden.get(&e) {
                Some(m) => format!("{element} is added by module {m}, which is not in depends"),
                None => format!("no {element} in view {} or the views that extend it", tree.root),
            };
            out.push(path, "OD310", locator.pos, message);
        }
    }

    /// The tree of views `id` belongs to, or None when the elements of some view on the way to its
    /// root are unknown.
    fn view_tree<'t>(&self, id: String, trees: &'t mut FxHashMap<String, ViewTree<'m>>) -> Option<&'t ViewTree<'m>> {
        let root = self.index.views.root(id)?;
        let tree = trees
            .entry(root.clone())
            .or_insert_with(|| self.index.views.tree(root, self.index.modules, |m| self.scope.sees(m)));
        Some(tree)
    }

    /// Odoo validates modifiers on every view but a QWeb one. The gate comes first so that a series
    /// without them, whose attributes mean something else, parses nothing.
    fn modifiers(
        &self,
        out: &mut Out<'m>,
        path: &'m Path,
        model: &str,
        view: &ViewFact,
        trees: &mut FxHashMap<String, ViewTree<'m>>,
    ) {
        if !rule("OD315").applies_to(self.series) {
            return;
        }
        let Some(kind) = self.arch_kind(view) else { return };
        for m in &view.modifiers {
            match modifier::names(&m.value.value) {
                Ok(names) => {
                    for name in &names {
                        let Some(name) = self.modifier_name(model, view, kind, m, name) else {
                            continue;
                        };
                        if let Some((code, message)) = self.modifier_finding(&name, trees) {
                            out.push(path, code, m.value.pos, message);
                        }
                    }
                }
                Err(why) => out.push(
                    path,
                    "OD315",
                    m.value.pos,
                    format!("invalid {} expression: {why}", m.attr),
                ),
            }
        }
    }

    /// Resolves a name to the field it reads, unless that field is out of the scan's reach. Each
    /// `parent.` leaves a subview; Odoo ignores what leaves the outermost one.
    fn modifier_name<'a>(
        &self,
        model: &'a str,
        view: &'a ViewFact,
        kind: &'a str,
        m: &'a ArchModifier,
        name: &'a str,
    ) -> Option<ModifierName<'a>> {
        self.arch_model(model, &m.path)?;
        let mut steps: Vec<String> = m.path.iter().filter(|s| !s.starts_with(LOCATOR)).cloned().collect();
        let mut field = name;
        while let Some(rest) = field.strip_prefix("parent.") {
            steps.pop()?;
            field = rest;
        }
        let owner = self.arch_model(model, &steps)?;
        let active = ACTIVE_NAMES.contains(&field) && self.series.is_some_and(|s| s < ACTIVE_NAMES_UNTIL);
        (field != "id" && !active && self.sees_whole_model(&owner)).then_some(ModifierName {
            model,
            view,
            kind,
            attr: m.attr,
            name,
            owner,
            field,
        })
    }

    /// A field outside `depends` stays silent: an `auto_install` module may bring it wherever the
    /// view is installed.
    fn modifier_finding(
        &self,
        n: &ModifierName,
        trees: &mut FxHashMap<String, ViewTree<'m>>,
    ) -> Option<(&'static str, String)> {
        match self.index.field(&n.owner, n.field, self.scope) {
            Ok(_) => self.absent_field(n, trees).map(|message| ("OD317", message)),
            Err(None) => self.unknown_name(n, trees).map(|message| ("OD316", message)),
            Err(Some(_)) => None,
        }
    }

    /// Only a dotted name after `parent.` gets here with a dot, and Odoo refuses those whatever the
    /// path would reach.
    fn unknown_name(&self, n: &ModifierName, trees: &mut FxHashMap<String, ViewTree<'m>>) -> Option<String> {
        if self.anchored_in_subview(n, trees)? {
            return None;
        }
        let why = if n.field.contains('.') {
            "Odoo refuses a composed field".to_owned()
        } else {
            format!("{} has no field '{}'", n.owner, n.field)
        };
        Some(format!("{} names '{}': {why}", n.attr, n.name))
    }

    /// Whether an extension's element may have landed in a subview of the tree, where the name
    /// would be a field of the comodel: a spec on a plain element can match inside one. None when
    /// the tree is unknown.
    fn anchored_in_subview(&self, n: &ModifierName, trees: &mut FxHashMap<String, ViewTree<'m>>) -> Option<bool> {
        let Some(parent) = n.view.inherit.as_deref() else {
            return Some(false);
        };
        let tree = self.view_tree(full_id(&self.module.name, parent), trees)?;
        Some(tree.subviews.iter().any(|subview| {
            self.arch_model(n.model, subview)
                .is_none_or(|comodel| !matches!(self.index.field(&comodel, n.field, self.scope), Err(None)))
        }))
    }

    /// Before saas~17.3 Odoo refuses a modifier on a field the view lacks. Any element of the tree
    /// counts, in a module outside the scope too, since the check cannot tell what is installed.
    fn absent_field(&self, n: &ModifierName, trees: &mut FxHashMap<String, ViewTree<'m>>) -> Option<String> {
        if !rule("OD317").applies_to(self.series) || !VIEW_FIELD_KINDS.contains(&n.kind) {
            return None;
        }
        let start = n.view.inherit.as_deref().or(n.view.id.as_deref())?;
        let tree = self.view_tree(full_id(&self.module.name, start), trees)?;
        let shown = ["field", "groupby"].into_iter().any(|tag| {
            let element = ArchElement {
                tag: tag.to_owned(),
                attr: "name".to_owned(),
                value: n.field.to_owned(),
            };
            tree.visible.contains(&element) || tree.hidden.contains_key(&element)
        });
        (!shown).then(|| format!("{} reads field '{}', which the view does not show", n.attr, n.field))
    }

    /// Whether the view renders on the server. The tree's root decides, since a kanban arch runs
    /// `t-call` in the client whatever the type of a view extending it.
    fn qweb_view(&self, view: &ViewFact) -> bool {
        let start = view.inherit.as_deref().or(view.id.as_deref());
        let root = start.and_then(|id| self.index.view_kind(full_id(&self.module.name, id)));
        root.or(view.kind.as_deref()) == Some(QWEB)
    }

    fn qweb_calls(&self, out: &mut Out<'m>, path: &'m Path, view: &ViewFact) {
        if !self.qweb_view(view) {
            return;
        }
        for call in &view.calls {
            // A module outside the index, such as an Enterprise one, may define the target.
            let owner = call.value.split_once('.').map(|(m, _)| m);
            if self.partial && !owner.is_some_and(|m| self.index.by_name.contains_key(m)) {
                continue;
            }
            let message = match self.index.qweb_status(&call.value, self.scope) {
                Status::Visible => continue,
                Status::Hidden(m) => format!(
                    "template '{}' comes from module {m}, which is not in depends",
                    call.value
                ),
                Status::Missing => format!("no indexed module defines template '{}'", call.value),
            };
            out.push(path, "OD311", call.pos, message);
        }
    }

    /// The dotted path of an arch field without its locators, or None when one of them is not on
    /// `model`: such a locator matched inside a subview we cannot see.
    fn arch_path(&self, model: &str, segments: &[String]) -> Option<String> {
        let mut steps: Vec<&str> = vec![];
        for segment in segments {
            match segment.strip_prefix(LOCATOR) {
                Some(locator) => {
                    let probe = steps.iter().copied().chain([locator]).collect::<Vec<_>>().join(".");
                    self.index.resolve_path(model, &probe, self.scope).ok()?;
                }
                None => steps.push(segment),
            }
        }
        Some(steps.join("."))
    }

    fn record(&self, out: &mut Out<'m>, path: &'m Path, record: &RecordFact) {
        if !matches!(
            self.index.model_status(&record.model.value, self.scope, None),
            Status::Visible
        ) {
            self.model_ref(out, path, &record.model, None);
            return;
        }
        for field in &record.fields {
            // `name@fr` sets the French translation of name.
            let fname = field.value.split('@').next().unwrap_or_default();
            match self.index.field(&record.model.value, fname, self.scope) {
                Ok(_) => {}
                Err(None) => out.push(
                    path,
                    "OD309",
                    field.pos,
                    format!("{} has no field '{fname}'", record.model.value),
                ),
                Err(Some(m)) => out.push(
                    path,
                    "OD309",
                    field.pos,
                    format!(
                        "field '{fname}' of {} comes from module {m}, which is not in depends",
                        record.model.value
                    ),
                ),
            }
        }
    }
}

/// What the registry makes of an attribute of a field, as far as literals tell.
#[derive(Clone, Copy, PartialEq)]
enum Merged<'m> {
    Unset,
    Known(&'m OptionValue),
    Unknown,
}

/// The first series whose Many2one setup gives an empty `ondelete` its default, and refuses a
/// required field's 'set null', only after `_inherits_check` (`saas~14.4`). The check can then meet
/// a delegating field with no `ondelete`, and has replaced its 'set null' before the refusal.
const LATE_MANY2ONE_SETUP: Series = Series { major: 14, minor: 4 };

/// The first series whose Many2one setup fills an empty `ondelete` before `_inherits_check` reads
/// it (`saas~12.2`).
const EARLY_ONDELETE_DEFAULT: Series = Series { major: 12, minor: 2 };

/// The first series that refuses ondelete 'restrict' on a Many2one to one of
/// `NO_RESTRICT_COMODELS` (`saas~13.1`).
const REFUSED_IR_RESTRICT: Series = Series { major: 13, minor: 1 };

/// The comodels Odoo keeps ondelete 'restrict' from, since uninstalling a module deletes their
/// records.
const NO_RESTRICT_COMODELS: [&str; 7] = [
    "ir.model",
    "ir.model.data",
    "ir.model.fields",
    "ir.model.fields.selection",
    "ir.model.relation",
    "ir.model.constraint",
    "ir.module.module",
];

/// The first series whose field setup no longer merges a definition with the field of a model
/// class in the registry (`saas~12.5`).
const SETUP_MAGIC: Series = Series { major: 12, minor: 5 };

/// The first series whose model metaclass adds the magic fields to the class that introduces a
/// model, rather than the setup of every model (`saas~14.4`).
const METACLASS_MAGIC: Series = Series { major: 14, minor: 4 };

/// The first series without the `__last_update` field (`saas~16.1`).
const NO_LAST_UPDATE: Series = Series { major: 16, minor: 1 };

/// The first series whose `BaseModel` defines `id` and `display_name` (`saas~18.1`).
const BASE_MAGIC: Series = Series { major: 18, minor: 1 };

/// The first series that refuses an `_inherits` field that is not required with ondelete 'cascade'
/// or 'restrict', and a `delegate=True` field no entry names (`saas~18.3`).
const REFUSED_DELEGATION: Series = Series { major: 18, minor: 3 };

/// The first series that builds no company domain for a `check_company` field of a model without
/// a company field, logging that it cannot (`18.0`, `odoo/fields.py:3177`; `saas-17.4` builds it for
/// every model, `odoo/fields.py:3030`).
const COMPANY_FIELD_DOMAIN: Series = Series { major: 18, minor: 0 };

/// The first series whose name search reads `_rec_names_search` (`saas~15.3`, `odoo/models.py:1924`;
/// `saas-15.2` knows `_rec_name` alone, `odoo/models.py:1932`).
const REC_NAMES_SEARCH: Series = Series { major: 15, minor: 3 };

/// The first series that moves the acceptance of `tracking` to `mail.track.mixin`, so an abstract
/// model loses it (`saas~19.3`, `addons/mail/models/mail_track_mixin.py:34`; `saas-19.2` still
/// accepts it, `addons/mail/models/models.py:28`). No lower bound applies: `mail` accepts it from
/// `saas~13.4`, before OD219 starts, in `addons/mail/models/ir_model.py:11` until `saas-14.3`.
const MIXIN_TRACKING: Series = Series { major: 19, minor: 3 };

/// An `_inherits` entry of a class: its module, the class, the parent model and the field.
type Entry<'m> = (usize, &'m ClassFact, &'m str, &'m Spanned<String>);

/// The models the classes of one model list in `_inherit`, for `Ctx::entry_before_in`.
struct Listed<'m> {
    parents: Vec<Parent<'m>>,
}

/// An entry of `_inherit`: the class listing it, its position, the parent model and the classes
/// that model merges.
struct Parent<'m> {
    class: &'m ClassFact,
    at: usize,
    model: &'m str,
    merges: Vec<(usize, &'m ClassFact)>,
}

impl<'m> Listed<'m> {
    /// The one entry whose parent merges `class`. None when no entry or several merge it, as for a
    /// class reached through two parents, or one of `base` under two, since every model merges it.
    fn through(&self, class: &ClassFact) -> Option<&Parent<'m>> {
        let has = |classes: &[(usize, &ClassFact)]| classes.iter().any(|(_, c)| std::ptr::eq(*c, class));
        let mut hits = self.parents.iter().filter(|p| has(&p.merges));
        let first = hits.next()?;
        hits.next().is_none().then_some(first)
    }
}

/// A field of a model as the registry builds it in scope: the definitions it merges, from the last
/// one that changes the field's type.
struct Field<'m> {
    model: &'m str,
    defs: Vec<FieldDef<'m>>,
    /// `before[i][j]` when the registry certainly merges `defs[i]` before `defs[j]`.
    before: Vec<Vec<bool>>,
}

impl<'m> Field<'m> {
    /// The definitions `pick` keeps that no other one it keeps certainly comes after.
    fn last(&self, pick: impl Fn(&FieldDef<'m>) -> bool) -> Vec<usize> {
        let picked: Vec<usize> = (0..self.defs.len()).filter(|&i| pick(&self.defs[i])).collect();
        let open = |i: &usize| !picked.iter().any(|&j| self.before[*i][j]);
        picked.iter().copied().filter(open).collect()
    }

    /// The definitions `pick` keeps that no other one it keeps certainly comes before.
    fn first(&self, pick: impl Fn(&FieldDef<'m>) -> bool) -> Vec<usize> {
        let picked: Vec<usize> = (0..self.defs.len()).filter(|&i| pick(&self.defs[i])).collect();
        let open = |i: &usize| !picked.iter().any(|&j| self.before[j][*i]);
        picked.iter().copied().filter(open).collect()
    }

    /// The value the last definitions that set the option agree on. A definition that unpacks
    /// `**kwargs` without naming the option may set it, and an option given as an expression that
    /// only a run can evaluate is unknown, so either makes the value unknown.
    fn get(&self, name: &str) -> Merged<'m> {
        self.get_where(name, |_| true)
    }

    /// The value of `get` among the definitions `keep` picks.
    fn get_where(&self, name: &str, keep: impl Fn(&FieldDef<'m>) -> bool) -> Merged<'m> {
        let mut merged = Merged::Unset;
        for i in self.last(|d| keep(d) && (d.2.splat || d.2.option(name).is_some())) {
            let def: &'m FieldFact = self.defs[i].2;
            let value = match def.option(name) {
                Some(o) if o.value != OptionValue::Other => &o.value,
                _ => return Merged::Unknown,
            };
            match merged {
                Merged::Known(v) if v != value => return Merged::Unknown,
                _ => merged = Merged::Known(value),
            }
        }
        merged
    }

    fn is(&self, name: &str, value: &OptionValue) -> bool {
        self.get(name) == Merged::Known(value)
    }

    /// Whether the field has a column, or for a Many2many a table: a related or computed one has
    /// none unless `store=True`. Odoo reads `related` and `compute` for their truth, so
    /// `compute=None` leaves the field stored.
    fn stored(&self) -> Option<bool> {
        match self.get("store") {
            Merged::Known(OptionValue::Bool(b)) => Some(*b),
            Merged::Unset => match (truth(self.get("related")), truth(self.get("compute"))) {
                (Some(false), Some(false)) => Some(true),
                (None, _) | (_, None) => None,
                _ => Some(false),
            },
            _ => None,
        }
    }

    fn position(&self, own: &FieldFact) -> Option<usize> {
        self.defs.iter().position(|d| std::ptr::eq(d.2, own))
    }
}

/// The fields of a model as the registry builds them in scope, by name, and their labels.
struct ModelFields<'m> {
    model: &'m str,
    fields: FxHashMap<&'m str, Field<'m>>,
    labels: Vec<Label<'m>>,
    /// The view `init()` creates for the model, when its text spells the names of the columns.
    view: Option<&'m SqlView>,
}

/// The label Odoo gives a field of a model, and the definition of this module that decides it with
/// its load key, if one does.
struct Label<'m> {
    text: String,
    name: &'m str,
    here: Option<(&'m FieldFact, (usize, Pos))>,
}

/// The fields Odoo adds to a model, with their labels: the first two, then `__last_update` in the
/// series that have it, then the rest only when the model logs access. See `Ctx::magic_labels`
/// for the models that get them.
const MAGIC_LABELS: &[(&str, &str)] = &[
    ("id", "ID"),
    ("display_name", "Display Name"),
    ("__last_update", "Last Modified on"),
    ("create_uid", "Created by"),
    ("create_date", "Created on"),
    ("write_uid", "Last Updated by"),
    ("write_date", "Last Updated on"),
];

/// The label Odoo derives from a field's name: without `_ids` or `_id`, underscores made spaces,
/// in Python's title case, which capitalizes each letter that follows no other letter.
fn derived_label(name: &str) -> String {
    let stem = name
        .strip_suffix("_ids")
        .or_else(|| name.strip_suffix("_id"))
        .unwrap_or(name);
    let mut label = String::with_capacity(stem.len());
    let mut after_letter = false;
    for c in stem.chars() {
        let cased = c.is_lowercase() || c.is_uppercase();
        match (c, cased, after_letter) {
            ('_', ..) => label.push(' '),
            (_, true, true) => label.extend(c.to_lowercase()),
            (_, true, false) => label.extend(c.to_uppercase()),
            _ => label.push(c),
        }
        after_letter = cased;
    }
    label
}

/// How Odoo adds the fields of `MAGIC_LABELS`, which decides the models that get them and what a
/// definition of the same name makes of them.
#[derive(Clone, Copy, Debug, PartialEq)]
enum MagicEra {
    /// Setup adds them to every model, and a definition merges with the field of the same name on
    /// the registry class of each model it inherits.
    RegistryMerge,
    /// Setup adds them to every model, but not one that a class of the model or of a model it
    /// inherits defines.
    Setup,
    /// The metaclass adds them to the class that introduces a model that is not abstract.
    Metaclass,
    /// `BaseModel` defines `id` and `display_name`, and the metaclass adds the others.
    Base,
}

/// The era of `series`. An unknown series takes the metaclass era, which every series from
/// saas~14.4 through 18.0 shares: it adds no field to an abstract model, so no label is reported
/// that the series of the module may lack.
fn magic_era(series: Option<Series>) -> MagicEra {
    match series {
        Some(s) if s < SETUP_MAGIC => MagicEra::RegistryMerge,
        Some(s) if s < METACLASS_MAGIC => MagicEra::Setup,
        Some(s) if s >= BASE_MAGIC => MagicEra::Base,
        _ => MagicEra::Metaclass,
    }
}

/// Whether Odoo adds `__last_update`. Not for an unknown series, so that no label is reported
/// against a field the newest series lack.
fn has_last_update(series: Option<Series>) -> bool {
    series.is_some_and(|s| s < NO_LAST_UPDATE)
}

fn unlogged(class: &ClassFact) -> bool {
    class
        .attr("_log_access")
        .is_some_and(|a| a.value == OptionValue::Bool(false))
}

/// What becomes of the field Odoo adds to a model under a name that indexed classes define too.
enum MagicBase<'a> {
    /// The definitions `After` picks merge into it, so its label stands unless one sets `string`.
    Kept(&'a str, After),
    /// Odoo does not add its own field, or from saas~18.1 merges `display_name` first, so the
    /// definitions decide the label.
    Replaced,
    /// Odoo adds its own field over every definition and merges none, so its label stands.
    Fixed(&'a str),
    /// A model it inherits defines the field too, and where Odoo merges that one is not modelled.
    Unknown,
}

/// The definitions that merge after the field Odoo adds.
#[derive(Clone, Copy)]
enum After {
    /// All of them: Odoo's field merges before every class.
    Every,
    /// Those of the classes of the model that do not introduce it: Odoo adds the field to the
    /// introducing class, which merges after the models it inherits.
    Extensions,
}

impl After {
    fn keeps(self, field: &Field, def: &FieldDef) -> bool {
        match self {
            After::Every => true,
            After::Extensions => def.1.model == field.model && !def.1.new_model,
        }
    }
}

/// Where a definition's attribute is, or the definition when it does not set it.
fn pos_of(own: &FieldFact, name: &str) -> Pos {
    own.option(name).map_or(own.pos, |o| o.pos)
}

/// Whether another stored Many2many without relation has the field's comodel and columns.
fn same_table(field: &Field, other: &Field) -> bool {
    let same = |name: &str| field.get(name) == other.get(name) && field.get(name) != Merged::Unknown;
    matches!(field.get("comodel_name"), Merged::Known(_))
        && same("comodel_name")
        && same("column1")
        && same("column2")
        && other.get("relation") == Merged::Unset
        && other.stored() == Some(true)
}

/// The values a definition leaves with ondelete 'set null': the default for each value it adds
/// that the field did not have, and those it gives 'set null'. Odoo only checks a definition that
/// adds a value. None when its ondelete is not a literal dict.
fn set_null_values<'m>(own: &'m FieldFact, added: &'m [String], known: &FxHashSet<&str>) -> Option<Vec<&'m str>> {
    let ondelete = match own.option("ondelete").map(|o| &o.value) {
        Some(OptionValue::Dict(d)) => d.as_slice(),
        None if !own.splat => &[],
        _ => return None,
    };
    let new: Vec<&String> = added.iter().filter(|v| !known.contains(v.as_str())).collect();
    if new.is_empty() {
        return Some(vec![]);
    }
    let unset = |v: &&&String| !ondelete.iter().any(|(k, _)| k == **v);
    let mut values: Vec<&str> = new.iter().filter(unset).map(|v| v.as_str()).collect();
    let explicit = ondelete.iter().filter(|(_, p)| p.as_deref() == Some("set null"));
    values.extend(explicit.map(|(k, _)| k.as_str()));
    Some(values)
}

/// Whether Odoo raises on a record that `method` leaves without a value of the field: it does for
/// a field it neither stores nor lets users write, as every definition merged into it sets, and
/// gives the others False. A related field gets Odoo's own compute method.
fn raises_unassigned(field: &Field, method: &str) -> bool {
    let readonly = match field.get("readonly") {
        Merged::Unset => field.get("inverse") == Merged::Unset,
        merged => merged == Merged::Known(&OptionValue::Bool(true)),
    };
    readonly
        && field.stored() == Some(false)
        && field.get("related") == Merged::Unset
        && field.is("compute", &OptionValue::Str(method.to_owned()))
}

/// The methods, by model, that a class of the module overrides with a `super()` call. The order
/// of the files that import them is not indexed, so any class may come after the one it extends.
fn super_overrides(module: &Module) -> FxHashSet<(&str, &str)> {
    let classes = module.python.iter().flat_map(|f| &f.facts.classes);
    let overrides = classes.flat_map(|c| c.extended.iter().map(|m| (c.model.as_str(), m.as_str())));
    overrides.collect()
}

/// OD223 at each compute method of the class that can leave a field unassigned, naming the fields
/// Odoo raises on. A method that a class of the module overrides with a `super()` call is skipped,
/// since the override may assign the rest.
fn unassigned_computes<'m>(
    out: &mut Out<'m>,
    path: &'m Path,
    class: &ClassFact,
    fields: &ModelFields<'m>,
    overrides: &FxHashSet<(&str, &str)>,
) {
    let extended = |(method, _): &&(Spanned<String>, _)| overrides.contains(&(&*class.model, &*method.value));
    for (method, names) in class.unassigned_computes.iter().filter(|c| !extended(c)) {
        let raises = |name: &&str| {
            fields
                .fields
                .get(name)
                .is_some_and(|f| raises_unassigned(f, &method.value))
        };
        let raising: Vec<&str> = names.iter().map(String::as_str).filter(raises).collect();
        if !raising.is_empty() {
            out.push(path, "OD223", method.pos, unassigned_message(&method.value, &raising));
        }
    }
}

fn unassigned_message(method: &str, fields: &[&str]) -> String {
    let (names, it) = match fields {
        [field] => (format!("'{field}'"), "it"),
        _ => (format!("'{}'", fields.join("', '")), "them"),
    };
    format!(
        "{method}() can leave {names} unassigned on a record, so reading {it} raises ValueError; assign {it} on every path through the loop, False when there is no value"
    )
}

/// Model and field definitions Odoo refuses, ignores or logs as wrong when it loads the registry,
/// or that fail once used. Each rule reads every definition merged into the model, so none runs
/// where a module in scope depends on a module that is not indexed. None runs on a model whose
/// class with `_name` is out of scope, or where a class of it, or of a model it inherits or
/// delegates to, has a plain Python base class or an `_inherit` element that is not a literal, a
/// parent whose fields the scan cannot see.
impl<'m> Ctx<'_, 'm> {
    fn registry(&self, out: &mut Out<'m>) {
        if self.partial {
            return;
        }
        let checked = |c: &&ClassFact| {
            self.index.introduced(&c.model, self.scope) && !self.index.hidden_parents(&c.model, self.scope)
        };
        let mut models: FxHashMap<&'m str, ModelFields<'m>> = FxHashMap::default();
        let mut reported_self_dependencies = FxHashSet::default();
        let overrides = super_overrides(self.module);
        for file in &self.module.python {
            for class in file.facts.classes.iter().filter(checked) {
                let model = class.model.as_str();
                let fields = models.entry(model).or_insert_with(|| self.model_fields(model));
                for own in &class.fields {
                    self.registry_field(out, &file.path, fields, own);
                }
                self.self_dependencies(out, &file.path, class, fields, &mut reported_self_dependencies);
                unassigned_computes(out, &file.path, class, fields, &overrides);
                self.parent_store(out, &file.path, class);
                self.view_not_created(out, &file.path, class);
                self.view_id(out, &file.path, class, fields);
                self.definitions(out, &file.path, class);
                for (_, entry) in &class.delegations {
                    self.delegation(out, &file.path, class, entry);
                }
            }
        }
    }

    fn registry_field(&self, out: &mut Out<'m>, path: &'m Path, fields: &ModelFields<'m>, own: &'m FieldFact) {
        let Some(field) = fields.fields.get(own.name.as_str()) else {
            return;
        };
        let Some(at) = field.position(own) else {
            return;
        };
        match own.ftype.as_str() {
            "Monetary" => self.monetary(out, path, field, own),
            "Many2one" => {
                self.required_set_null(out, path, field, own);
                self.delegate_field(out, path, field, own);
            }
            "Many2many" if field.stored() == Some(true) => {
                self.long_relation(out, path, field, own);
                self.relation_clash(out, path, field, own);
            }
            "Selection" => self.selection_add(out, path, field, at),
            _ => {}
        }
        self.tracking(out, path, field, own);
        self.check_company(out, path, field, own);
        self.duplicate_label(out, path, fields, own);
        self.view_field(out, path, fields, own);
    }

    /// The fields of `model` as the registry builds them in scope, merged once for every rule.
    fn model_fields(&self, model: &'m str) -> ModelFields<'m> {
        let mut by_name: FxHashMap<&'m str, Vec<FieldDef<'m>>> = FxHashMap::default();
        for def in self.index.merged_fields(model, self.scope) {
            by_name.entry(def.2.name.as_str()).or_default().push(def);
        }
        let names: FxHashSet<&str> = by_name.keys().copied().collect();
        let fields: FxHashMap<&'m str, Field<'m>> = by_name
            .into_iter()
            .filter_map(|(name, defs)| Some((name, self.merge(model, defs)?)))
            .collect();
        let magic = self.magic_labels(model);
        let magic_of = |name: &str| magic.iter().find(|(n, _)| *n == name).map(|(_, text)| *text);
        let mut labels: Vec<Label<'m>> = fields
            .iter()
            .filter_map(|(name, f)| self.label(name, f, magic_of(name)))
            .collect();
        labels.extend(
            magic
                .iter()
                .filter(|(name, _)| !names.contains(name))
                .map(|&(name, text)| Label {
                    text: text.to_owned(),
                    name,
                    here: None,
                }),
        );
        ModelFields {
            model,
            fields,
            labels,
            view: self.view_of(model),
        }
    }

    /// The field `name` of `model` as the registry builds it in scope.
    fn merged(&self, model: &'m str, name: &str) -> Option<Field<'m>> {
        let mut defs = self.index.merged_fields(model, self.scope);
        defs.retain(|d| d.2.name == name);
        self.merge(model, defs)
    }

    /// A field from its definitions in scope. None when the order of a definition that changes its
    /// type against another one is unknown, or the type is not that of the last definitions.
    fn merge(&self, model: &'m str, defs: Vec<FieldDef<'m>>) -> Option<Field<'m>> {
        let before = |a: &FieldDef<'m>, b: &FieldDef<'m>| self.before(model, a, b);
        let mut last = defs.iter().filter(|a| !defs.iter().any(|b| before(a, b)));
        let ftype = &last.next()?.2.ftype;
        if !last.all(|d| d.2.ftype == *ftype) {
            return None;
        }
        let (same, other): (Vec<&FieldDef<'m>>, Vec<_>) = defs.iter().partition(|d| d.2.ftype == *ftype);
        if same
            .iter()
            .any(|s| other.iter().any(|o| !before(o, s) && !before(s, o)))
        {
            return None;
        }
        // Odoo drops what the definitions before one of another type set.
        let kept = same.into_iter().filter(|s| other.iter().all(|o| before(o, s)));
        let defs: Vec<FieldDef<'m>> = kept.copied().collect();
        let before = defs
            .iter()
            .map(|a| defs.iter().map(|b| before(a, b)).collect())
            .collect();
        Some(Field { model, defs, before })
    }

    /// Whether the registry certainly merges definition `a` of a field of `model` before `b`.
    fn before(&self, model: &str, a: &FieldDef<'m>, b: &FieldDef<'m>) -> bool {
        self.merges_before(model, (a.0, a.1, a.2.pos), (b.0, b.1, b.2.pos))
    }

    /// Whether the registry certainly merges what module `a.0`'s class `a.1` gives `model` at `a.2`
    /// before what `b` gives. The classes of `model` come after those of the models they inherit,
    /// unless one of them lists another model before its own in `_inherit`, which puts that model
    /// after the classes loaded before it. Among them, a dependency's come first, then in each
    /// module the class that introduces the model, then the others of one file in order; files load
    /// in the order of the imports, which is not indexed.
    fn merges_before(&self, model: &str, a: (usize, &ClassFact, Pos), b: (usize, &ClassFact, Pos)) -> bool {
        match (a.1.model == model, b.1.model == model) {
            (_, false) => false,
            (false, true) => !self.inherits_ahead(model),
            _ if a.0 != b.0 => self.index.reaches(b.0, a.0) && !self.index.reaches(a.0, b.0),
            _ if a.1.new_model != b.1.new_model => a.1.new_model,
            _ => self.in_one_file(a.0, a.1, b.1) && a.2 < b.2,
        }
    }

    /// `merges_before` for the `_inherits` entries of `model`, which also orders two classes of
    /// models it inherits. Odoo merges the `_inherits` of each model one class lists in `_inherit`
    /// whole, in reverse, so the first listed parent's entries come last. Field definitions do not
    /// share this order: they merge over Python's method resolution order.
    fn entry_before(&self, model: &str, a: (usize, &ClassFact, Pos), b: (usize, &ClassFact, Pos)) -> bool {
        self.entry_before_in(model, a, b, &mut vec![model])
    }

    /// `entry_before`, having passed through the models in `seen`, `model` among them. Unknown
    /// between two classes of other models when either has no single parent, or the parents are
    /// listed by different classes of `model`, whose order in the registry's bases is not indexed.
    fn entry_before_in<'s>(
        &self,
        model: &'s str,
        a: (usize, &ClassFact, Pos),
        b: (usize, &ClassFact, Pos),
        seen: &mut Vec<&'s str>,
    ) -> bool
    where
        'm: 's,
    {
        if std::ptr::eq(a.1, b.1) {
            // One class keeps its own order whichever path merges it.
            return self.merges_before(&a.1.model, a, b);
        }
        if a.1.model == model || b.1.model == model {
            return self.merges_before(model, a, b);
        }
        let listed = self.listed(model);
        let (Some(pa), Some(pb)) = (listed.through(a.1), listed.through(b.1)) else {
            return false;
        };
        if pa.model == pb.model {
            // Defence only: on an `_inherit` cycle, `through` finds two paths first.
            if seen.contains(&pa.model) {
                return false;
            }
            seen.push(pa.model);
            return self.entry_before_in(pa.model, a, b, seen);
        }
        std::ptr::eq(pa.class, pb.class) && pa.at > pb.at
    }

    /// The models the in-scope classes of `model` list in `_inherit`, with what each merges.
    fn listed(&self, model: &str) -> Listed<'m> {
        let classes = self.index.classes(model).iter().filter(|(mi, _)| self.scope.sees(*mi));
        let parents = classes.flat_map(|&(_, class)| {
            let listed = class.inherit.iter().enumerate().filter(|(_, p)| p.value != model);
            listed.map(move |(at, p)| Parent {
                class,
                at,
                model: p.value.as_str(),
                merges: self.index.merged_classes(&p.value, self.scope),
            })
        });
        Listed {
            parents: parents.collect(),
        }
    }

    /// Whether a class of `model` in scope lists another model before `model` in `_inherit`.
    fn inherits_ahead(&self, model: &str) -> bool {
        let ahead = |c: &ClassFact| c.inherit.iter().position(|i| i.value == model).is_some_and(|p| p > 0);
        self.index
            .classes(model)
            .iter()
            .any(|(mi, c)| self.scope.sees(*mi) && ahead(c))
    }

    /// Where this module's definition loads, among those `before` leaves unordered: by the module's
    /// file list, which may not be the order of the imports, so it only picks where to report.
    fn load_key(&self, (_, class, field): &FieldDef<'m>) -> (usize, Pos) {
        let has = |c: &ClassFact| std::ptr::eq(c, *class);
        let file = self.module.python.iter().position(|f| f.facts.classes.iter().any(has));
        (file.unwrap_or_default(), field.pos)
    }

    /// Whether this module's definitions of the field that `sets` picks are reported at `own`: the
    /// last of them, so that the field is reported once per module.
    fn reported_at(&self, field: &Field<'m>, own: &FieldFact, sets: impl Fn(&FieldFact) -> bool) -> bool {
        let me = self.index.by_name[self.module.name.as_str()];
        let last = field.last(|d| d.0 == me && d.1.model == field.model && sets(d.2));
        last.into_iter()
            .max_by_key(|&i| self.load_key(&field.defs[i]))
            .is_some_and(|i| std::ptr::eq(field.defs[i].2, own))
    }

    /// The field's first definition: of a dependency when there is one, else of this module.
    fn first_def<'f>(&self, field: &'f Field<'m>) -> Option<&'f FieldDef<'m>> {
        let me = self.index.by_name[self.module.name.as_str()];
        let first = field.first(|_| true).into_iter().map(|i| &field.defs[i]);
        first.min_by_key(|d| (d.0 == me, self.load_key(d)))
    }

    /// Whether the field is new in this module and reported at `own`, its first definition here.
    fn introduces(&self, field: &Field<'m>, own: &FieldFact) -> bool {
        let me = self.index.by_name[self.module.name.as_str()];
        let first = field.first(|d| d.1.model == field.model).into_iter();
        field.defs.iter().all(|d| d.0 == me)
            && first
                .min_by_key(|&i| self.load_key(&field.defs[i]))
                .is_some_and(|i| std::ptr::eq(field.defs[i].2, own))
    }

    /// Odoo asserts that a Monetary's `currency_field`, by default `currency_id` or `x_currency_id`,
    /// is a field of the model; from 15.0 a related Monetary is held to it too.
    fn monetary(&self, out: &mut Out<'m>, path: &'m Path, field: &Field<'m>, own: &FieldFact) {
        let checked_related = self.series.is_some_and(|s| s >= Series { major: 15, minor: 0 });
        if (field.get("related") != Merged::Unset && !checked_related) || !self.reported_at(field, own, |_| true) {
            return;
        }
        let model = field.model;
        let missing = |name: &str| matches!(self.index.field(model, name, self.scope), Err(None));
        let name = &own.name;
        let message = match field.get("currency_field") {
            Merged::Known(OptionValue::Str(currency)) if missing(currency) => {
                format!("Monetary field '{name}' takes its currency from '{currency}', which {model} does not have")
            }
            Merged::Unset if missing("currency_id") && missing("x_currency_id") => format!(
                "Monetary field '{name}' has no currency: {model} has no currency_id, and the field names no currency_field"
            ),
            _ => return,
        };
        out.push(path, "OD212", pos_of(own, "currency_field"), message);
    }

    fn required_set_null(&self, out: &mut Out<'m>, path: &'m Path, field: &Field<'m>, own: &FieldFact) {
        let sets = |f: &FieldFact| f.option("required").is_some() || f.option("ondelete").is_some();
        if field.get("related") != Merged::Unset || !self.reported_at(field, own, sets) {
            return;
        }
        // The rewrite test walks the inherit chain, so it runs only on a field that needs it.
        if field.is("required", &OptionValue::Bool(true))
            && field.is("ondelete", &OptionValue::Str("set null".to_owned()))
            && !self.ondelete_rewritten(field, &own.name)
        {
            let at = match own.option("ondelete") {
                Some(o) => o.pos,
                None => pos_of(own, "required"),
            };
            let message = format!(
                "'{}' is required, so Odoo refuses ondelete='set null'; use 'restrict' or 'cascade'",
                own.name
            );
            out.push(path, "OD213", at, message);
        }
    }

    /// Whether Odoo may replace the field's `ondelete` with 'cascade' before it holds 'set null'
    /// against `required`, as it does when the field delegates by `delegate` or an `_inherits`
    /// entry. Until saas~14.3 it holds it first, in `setup_base`; from saas~14.4 it rewrites it in
    /// `_inherits_check`, until saas~18.3 refuses the field.
    fn ondelete_rewritten(&self, field: &Field<'m>, name: &str) -> bool {
        self.late_ondelete() && (may_delegate(field) || self.inherits_entry(field.model, name) != Some(false))
    }

    /// Whether `_inherits_check` reads a Many2one's `ondelete` before its setup gives the default:
    /// from saas~14.4, until saas~18.3 refuses such a field outright.
    fn late_ondelete(&self) -> bool {
        self.series
            .is_some_and(|s| LATE_MANY2ONE_SETUP <= s && s < REFUSED_DELEGATION)
    }

    /// Whether the field's setup fills an empty `ondelete`, by the truth of the value, with one
    /// Odoo accepts before `_inherits_check` reads it, so that `required=True` is all the field
    /// lacks: saas~12.2 until saas~14.3.
    fn ondelete_defaulted(&self, field: &Field<'m>) -> bool {
        self.series
            .is_some_and(|s| EARLY_ONDELETE_DEFAULT <= s && s < LATE_MANY2ONE_SETUP)
            && !self.default_refused(field)
    }

    /// Whether the 'restrict' a required field would get by default is refused for its comodel.
    /// A transient model's required field to such a comodel gets 'cascade' instead.
    fn default_refused(&self, field: &Field<'m>) -> bool {
        let refusing = |c: &&str| NO_RESTRICT_COMODELS.contains(c);
        self.series.is_some_and(|s| s >= REFUSED_IR_RESTRICT)
            && matches!(field.get("comodel_name"), Merged::Known(OptionValue::Str(c)) if refusing(&c.as_str()))
            && self
                .index
                .kind(field.model, self.scope)
                .is_some_and(|kind| kind != ModelKind::Transient)
    }

    /// A `delegate=True` field that no `_inherits` entry names. Before saas~18.3, Odoo forces it
    /// required with a warning, and from saas~14.4 fails on a field with no ondelete, reading it
    /// before giving it a default: OD228 reports that, and OD218 the rest. From saas~18.3 Odoo
    /// refuses such a field whatever its attributes, which neither rule reports.
    fn delegate_field(&self, out: &mut Out<'m>, path: &'m Path, field: &Field<'m>, own: &FieldFact) {
        const OPTIONS: [&str; 3] = ["delegate", "required", "ondelete"];
        let sets = |f: &FieldFact| OPTIONS.iter().any(|o| f.option(o).is_some());
        let refused = self.series.is_some_and(|s| s >= REFUSED_DELEGATION);
        if refused || !self.lone_delegate(field, &own.name) || !self.reported_at(field, own, sets) {
            return;
        }
        let Some(fix) = delegation_fix(field, self.ondelete_defaulted(field)) else {
            return;
        };
        let at = OPTIONS.iter().find_map(|o| own.option(o)).map_or(own.pos, |o| o.pos);
        let name = &own.name;
        if self.late_ondelete() && no_ondelete(field) {
            let message = format!(
                "'{name}' has delegate=True and no ondelete, so Odoo fails with AttributeError when it sets up the registry; give it {fix}"
            );
            out.push(path, "OD228", at, message);
        } else {
            let message = format!(
                "'{name}' has delegate=True, so it must be required with ondelete 'cascade' or 'restrict'; give it {fix}"
            );
            out.push(path, "OD218", at, message);
        }
    }

    /// Whether the field delegates with `delegate=True` alone: not related, and certainly named by
    /// no `_inherits` entry of its model.
    fn lone_delegate(&self, field: &Field<'m>, name: &str) -> bool {
        field.is("delegate", &OptionValue::Bool(true))
            && field.get("related") == Merged::Unset
            && self.inherits_entry(field.model, name) == Some(false)
    }

    /// Whether the `_inherits` of `model` names the field `name`, None when unknown: a class gives
    /// an `_inherits` or lists an `_inherit` that is not literal, whose unseen parent may give an
    /// entry, or the last classes giving a parent model disagree on whether its field is `name`.
    /// Odoo merges the `_inherits` of the classes the model merges, those of the models it
    /// inherits included, key by key, a later class replacing the field of a parent.
    fn inherits_entry(&self, model: &str, name: &str) -> Option<bool> {
        let classes = self.index.merged_classes(model, self.scope);
        if classes.iter().any(|(_, c)| c.inherits_unknown || c.inherit_unknown) {
            return None;
        }
        let entries: Vec<Entry<'m>> = classes
            .iter()
            .flat_map(|&(mi, c)| c.delegations.iter().map(move |(parent, f)| (mi, c, parent.as_str(), f)))
            .collect();
        let parents = entries.iter().filter(|e| e.3.value == name).map(|e| e.2);
        let named: Vec<Option<bool>> = parents.map(|p| self.names_parent(model, &entries, p, name)).collect();
        if named.contains(&Some(true)) {
            Some(true)
        } else {
            (!named.contains(&None)).then_some(false)
        }
    }

    /// Whether the last entries for `parent` name the field `name`, None when only some do.
    fn names_parent(&self, model: &str, entries: &[Entry<'m>], parent: &str, name: &str) -> Option<bool> {
        let at = |e: &Entry<'m>| (e.0, e.1, e.3.pos);
        let keyed: Vec<&Entry<'m>> = entries.iter().filter(|e| e.2 == parent).collect();
        let last = keyed
            .iter()
            .filter(|a| !keyed.iter().any(|b| self.entry_before(model, at(a), at(b))));
        let names: Vec<bool> = last.map(|e| e.3.value == name).collect();
        match (names.contains(&true), names.contains(&false)) {
            (true, false) => Some(true),
            (true, true) => None,
            (false, _) => Some(false),
        }
    }

    /// Odoo wants the Many2one an `_inherits` entry names required, with ondelete 'cascade' or
    /// 'restrict'. 18.0 warns and forces both, an unset ondelete included, which saas~12.2 until
    /// saas~14.3 fill first; saas~18.3 refuses the model. A missing field, or one of another type,
    /// is left alone, and an entry repeated in another class of the model in this module is
    /// reported at the first.
    fn delegation(&self, out: &mut Out<'m>, path: &'m Path, class: &'m ClassFact, entry: &Spanned<String>) {
        let names = |c: &ClassFact| c.delegations.iter().any(|(_, d)| d.value == entry.value);
        if !self.first_class(class, names) {
            return;
        }
        let Some(field) = self.merged(&class.model, &entry.value) else {
            return;
        };
        if field.defs[0].2.ftype != "Many2one" {
            return;
        }
        if let Some(fix) = delegation_fix(&field, self.ondelete_defaulted(&field)) {
            let message = format!(
                "_inherits delegates to '{}', which must be required with ondelete 'cascade' or 'restrict'; give it {fix}",
                entry.value
            );
            out.push(path, "OD218", entry.pos, message);
        }
    }

    /// Whether `class` is the first class of its model in this module that `picks`, by the module's
    /// file list and then by position, so that a model is reported once per module.
    fn first_class(&self, class: &ClassFact, picks: impl Fn(&ClassFact) -> bool) -> bool {
        let me = self.index.by_name[self.module.name.as_str()];
        let file = |c: &ClassFact| {
            let has = |x: &ClassFact| std::ptr::eq(x, c);
            self.module.python.iter().position(|f| f.facts.classes.iter().any(has))
        };
        self.index
            .classes(&class.model)
            .iter()
            .filter(|(mi, c)| *mi == me && picks(c))
            .min_by_key(|(_, c)| (file(c), c.pos))
            .is_some_and(|(_, c)| std::ptr::eq(*c, class))
    }

    /// Odoo logs an error when it creates the table of a model with `_parent_store` and no indexed
    /// `parent_path` field. A model without a table of its own, or with a class that sets either
    /// attribute otherwise, is left alone, and one set in several classes of this module is reported
    /// at the first.
    fn parent_store(&self, out: &mut Out<'m>, path: &'m Path, class: &'m ClassFact) {
        let sets = |c: &ClassFact| c.attr("_parent_store").is_some();
        let Some(at) = class
            .attr("_parent_store")
            .filter(|_| self.first_class(class, sets) && self.parent_store_table(&class.model))
        else {
            return;
        };
        let model = class.model.as_str();
        let message = match self.index.field(model, "parent_path", self.scope) {
            Err(_) => format!(
                "{model} sets _parent_store but has no parent_path field, which Odoo logs as an error; add parent_path = fields.Char(index=True)"
            ),
            Ok(_) => match self.merged(model, "parent_path").map(|f| f.get("index")) {
                Some(Merged::Unset | Merged::Known(OptionValue::Bool(false))) => format!(
                    "{model} sets _parent_store but its parent_path field has no index, which Odoo logs as an error; add index=True to it"
                ),
                _ => return,
            },
        };
        out.push(path, "OD217", at.pos, message);
    }

    /// Whether every class of `model` in scope leaves it a table with `_parent_store`: it has a
    /// table, and none sets `_parent_store` to anything but True.
    fn parent_store_table(&self, model: &str) -> bool {
        self.has_table(model) && self.every_class(model, |c| true_or_unset(c, "_parent_store"))
    }

    /// Whether every class of `model` in scope leaves it a table Odoo builds from its fields: none
    /// is abstract, or sets `_auto` to anything but True.
    fn has_table(&self, model: &str) -> bool {
        self.every_class(model, |c| c.kind != ModelKind::Abstract && true_or_unset(c, "_auto"))
    }

    fn every_class(&self, model: &str, keeps: impl Fn(&ClassFact) -> bool) -> bool {
        self.index
            .classes(model)
            .iter()
            .all(|(mi, c)| !self.scope.sees(*mi) || keeps(c))
    }

    fn some_class(&self, model: &str, picks: impl Fn(&ClassFact) -> bool) -> bool {
        !self.every_class(model, |c| !picks(c))
    }

    /// The table Odoo names a Many2many's relation after when it gives none: both tables, sorted.
    /// None when a table is unknown, or both are the same, which Odoo refuses on its own.
    fn implicit_relation(&self, field: &Field<'m>) -> Option<String> {
        let Merged::Known(OptionValue::Str(comodel)) = field.get("comodel_name") else {
            return None;
        };
        if !self.index.introduced(comodel, self.scope) {
            return None;
        }
        let mut tables = [
            self.index.table(field.model, self.scope)?,
            self.index.table(comodel, self.scope)?,
        ];
        tables.sort();
        (tables[0] != tables[1]).then(|| format!("{}_{}_rel", tables[0], tables[1]))
    }

    fn long_relation(&self, out: &mut Out<'m>, path: &'m Path, field: &Field<'m>, own: &FieldFact) {
        let (relation, at) = match field.get("relation") {
            Merged::Known(OptionValue::Str(r)) if self.reported_at(field, own, |f| f.option("relation").is_some()) => {
                (r.clone(), pos_of(own, "relation"))
            }
            Merged::Unset if self.introduces(field, own) => match self.implicit_relation(field) {
                Some(r) => (r, own.pos),
                None => return,
            },
            _ => return,
        };
        if relation.len() <= PG_NAME_MAX {
            return;
        }
        let message = format!(
            "relation table '{relation}' of '{}' is {} characters long, and PostgreSQL allows {PG_NAME_MAX}; name a shorter one with relation=",
            own.name,
            relation.len()
        );
        out.push(path, "OD215", at, message);
    }

    /// Two Many2many fields of a model to one comodel that leave the relation to Odoo get the same
    /// table and columns. The later field is reported, when it is new in this module; when files
    /// leave the order open, the later in the module's file list. A field to its own model is not:
    /// Odoo refuses it alone, and a `relation` on the other field would not help.
    fn relation_clash(&self, out: &mut Out<'m>, path: &'m Path, field: &Field<'m>, own: &'m FieldFact) {
        let Merged::Known(comodel) = field.get("comodel_name") else {
            return;
        };
        if field.get("relation") != Merged::Unset
            || !self.introduces(field, own)
            || self.implicit_relation(field).is_none()
        {
            return;
        }
        let Some(own_def) = field.defs.iter().find(|d| std::ptr::eq(d.2, own)) else {
            return;
        };
        let model = field.model;
        let mut seen = FxHashSet::default();
        let fields = self.index.merged_fields(model, self.scope);
        let mut candidates = fields.iter().map(|d| d.2).filter(|f| {
            f.ftype == "Many2many"
                && f.name != own.name
                && f.option("comodel_name").is_some_and(|c| c.value == *comodel)
                && seen.insert(&f.name)
        });
        if let Some(other) = candidates.find(|f| self.clashes(field, own_def, &f.name)) {
            let message = format!(
                "'{}' and '{}' are Many2many fields of {model} to the same comodel without relation, so Odoo gives both the same table; set relation on one",
                other.name, own.name
            );
            out.push(path, "OD214", own.pos, message);
        }
    }

    /// Whether the Many2many `name` of the field's model gets the same table and comes before `own`.
    fn clashes(&self, field: &Field<'m>, own: &FieldDef<'m>, name: &str) -> bool {
        let Some(other) = self.merged(field.model, name) else {
            return false;
        };
        other.defs[0].2.ftype == "Many2many"
            && same_table(field, &other)
            && self
                .first_def(&other)
                .is_some_and(|first| self.earlier(field.model, first, own))
    }

    /// Whether definition `a` loads before `b`, or comes first in the module's file list when their
    /// order is open; a dependency's comes first.
    fn earlier(&self, model: &str, a: &FieldDef<'m>, b: &FieldDef<'m>) -> bool {
        let me = self.index.by_name[self.module.name.as_str()];
        self.before(model, a, b) || (!self.before(model, b, a) && (a.0 != me || self.load_key(a) < self.load_key(b)))
    }

    /// Odoo gives each value `selection_add` adds to a Selection the ondelete policy 'set null'
    /// unless told otherwise, and refuses that policy on a required field.
    fn selection_add(&self, out: &mut Out<'m>, path: &'m Path, field: &Field<'m>, at: usize) {
        let own = field.defs[at].2;
        let Some(Spanned {
            value: OptionValue::Strs(added) | OptionValue::Tuple(added),
            pos,
        }) = own.option("selection_add")
        else {
            return;
        };
        if field.get("related") != Merged::Unset || !field.is("required", &OptionValue::Bool(true)) {
            return;
        }
        let tuples = self.series.is_some_and(|s| s >= Series { major: 19, minor: 0 });
        let Some(known) = earlier_values(field, at, tuples) else {
            return;
        };
        let Some(set_null) = set_null_values(own, added, &known).filter(|v| !v.is_empty()) else {
            return;
        };
        let (values, them) = if set_null.len() == 1 {
            ("value", "it")
        } else {
            ("values", "each")
        };
        let message = format!(
            "'{}' is required, so Odoo refuses ondelete 'set null' for the selection_add {values} '{}'; give {them} 'cascade', 'set default' or a function",
            own.name,
            set_null.join("', '")
        );
        out.push(path, "OD216", *pos, message);
    }

    /// Odoo hands each attribute a field does not know to the model's `_valid_field_parameter`, and
    /// logs a warning when it refuses it. `BaseModel` only accepts `related_sudo`; `mail.thread` and
    /// the other models that override the method may accept `tracking`, and before saas~19.3 so
    /// does `mail` for an abstract model.
    fn tracking(&self, out: &mut Out<'m>, path: &'m Path, field: &Field<'m>, own: &FieldFact) {
        let Some(at) = own.option("tracking") else {
            return;
        };
        let model = field.model;
        let abstract_accepted = self.series.is_none_or(|s| s < MIXIN_TRACKING)
            && self.index.kind(model, self.scope) == Some(ModelKind::Abstract);
        if abstract_accepted
            || self.index.defines_method(model, "_valid_field_parameter", false)
            || !self.reported_at(field, own, |f| f.option("tracking").is_some())
        {
            return;
        }
        let message = format!(
            "'{}' sets tracking, but {model} does not inherit mail.thread, so Odoo ignores it and logs a warning; inherit mail.thread or remove tracking",
            own.name
        );
        out.push(path, "OD219", at.pos, message);
    }

    /// Odoo logs a warning for each field of a model whose label another field already has. The
    /// definition of this module that decides a label is reported, against a label decided by
    /// another module or one this module loads earlier.
    fn duplicate_label(&self, out: &mut Out<'m>, path: &'m Path, fields: &ModelFields<'m>, own: &FieldFact) {
        let Some((label, key)) = fields.labels.iter().find_map(|l| match l.here {
            Some((def, key)) if std::ptr::eq(def, own) => Some((l, key)),
            _ => None,
        }) else {
            return;
        };
        let earlier = |o: &&Label<'m>| o.name != label.name && o.text == label.text && o.here.map(|h| h.1) < Some(key);
        let Some(other) = fields
            .labels
            .iter()
            .filter(earlier)
            .min_by_key(|o| (o.here.map(|h| h.1), o.name))
        else {
            return;
        };
        let message = format!(
            "'{}' has the label '{}' of field '{}' of {}, so Odoo logs a warning and users cannot tell them apart; give it another string",
            own.name, label.text, other.name, fields.model
        );
        out.push(path, "OD220", pos_of(own, "string"), message);
    }

    /// A field's label: a non-empty literal `string`, else `magic`, the label of the field Odoo
    /// adds under that name, else one derived from its name unless the field is related. It comes
    /// with the last definition of this module that decides it, if any. A `Fixed` magic field
    /// keeps its label even over a literal `string`, a `Kept` one reads `string` only from the
    /// definitions merged after it, and an `Unknown` one has none.
    fn label(&self, name: &'m str, field: &Field<'m>, magic: Option<&str>) -> Option<Label<'m>> {
        let (magic, after) = match magic.map(|text| self.magic_base(name, field, text)) {
            Some(MagicBase::Unknown) => return None,
            Some(MagicBase::Fixed(text)) => {
                return Some(Label {
                    text: text.to_owned(),
                    name,
                    here: None,
                });
            }
            Some(MagicBase::Kept(text, after)) => (Some(text), after),
            Some(MagicBase::Replaced) | None => (None, After::Every),
        };
        let keep = |d: &FieldDef<'m>| after.keeps(field, d);
        let (text, deciders) = match (field.get_where("string", keep), magic) {
            (Merged::Known(OptionValue::Str(s)), _) if !s.is_empty() => {
                (s.clone(), field.last(|d| keep(d) && d.2.option("string").is_some()))
            }
            (Merged::Unset, Some(text)) => (text.to_owned(), vec![]),
            (Merged::Unset, None) if field.get("related") == Merged::Unset => {
                (derived_label(name), field.first(|_| true))
            }
            _ => return None,
        };
        let here = self.decided_here(field, deciders);
        Some(Label { text, name, here })
    }

    /// The last of `deciders`, definitions of `field`, that this module gives the field's model,
    /// with its load key.
    fn decided_here(&self, field: &Field<'m>, deciders: Vec<usize>) -> Option<(&'m FieldFact, (usize, Pos))> {
        let me = self.index.by_name[self.module.name.as_str()];
        deciders
            .into_iter()
            .map(|i| &field.defs[i])
            .filter(|d| d.0 == me && d.1.model == field.model)
            .map(|d| (d.2, self.load_key(d)))
            .max_by_key(|(_, key)| *key)
    }

    /// What becomes of the field Odoo adds as `name`, labelled `text`, given the definitions of
    /// `field`. Before saas~14.4 setup adds `id` over every definition, and from saas~12.5 no
    /// other field that a class defines. Python mangles `__last_update` in a class body, so what
    /// the index records under that name is another field and never changes Odoo's.
    fn magic_base<'a>(&self, name: &str, field: &Field, text: &'a str) -> MagicBase<'a> {
        let era = magic_era(self.series);
        let early = matches!(era, MagicEra::RegistryMerge | MagicEra::Setup);
        match era {
            _ if name == "__last_update" || (early && name == "id") => MagicBase::Fixed(text),
            MagicEra::Setup => MagicBase::Replaced,
            MagicEra::RegistryMerge => self.registry_magic(name, field, text),
            MagicEra::Metaclass | MagicEra::Base => self.introduced_magic(era, name, field, text),
        }
    }

    /// What becomes of the field Odoo adds as `name` before saas~12.5, where a definition merges
    /// with the field of the registry class of each model the model inherits: `base` has
    /// `display_name`, and a model that logs access has the log fields. Where Odoo merges a
    /// definition of another model's class is not modelled.
    fn registry_magic<'a>(&self, name: &str, field: &Field, text: &'a str) -> MagicBase<'a> {
        let model = field.model;
        let logs = |(_, c): &(usize, &ClassFact)| c.model != model && c.model != "base" && self.log_access(&c.model);
        if field.defs.iter().any(|d| d.1.model != model) {
            MagicBase::Unknown
        } else if name == "display_name" || self.index.merged_classes(model, self.scope).iter().any(logs) {
            MagicBase::Kept(text, After::Every)
        } else {
            MagicBase::Replaced
        }
    }

    /// What becomes of the field Odoo adds as `name`, labelled `text`, given the definitions of
    /// `field`, from saas~14.4. The field Odoo adds as `id` sets no `string`, and from saas~18.1
    /// `BaseModel` defines `id` and `display_name` before every class, so any definition's `string`
    /// stands. The metaclass adds any other field to the introducing class only when that class
    /// does not define it, and a model listed before the model's own in `_inherit` merges after it.
    fn introduced_magic<'a>(&self, era: MagicEra, name: &str, field: &Field, text: &'a str) -> MagicBase<'a> {
        let model = field.model;
        let introducer = |d: &FieldDef| d.1.model == model && d.1.new_model;
        if name == "id" {
            MagicBase::Kept(text, After::Every)
        } else if field.defs.iter().any(introducer) {
            // From saas~18.1 this covers `display_name` too: `BaseModel`'s merges first, and the
            // introducer's definition gives the same label unless it is related without `string`,
            // which then stays unknown; deciding it there reports a pair with a mixin's field.
            MagicBase::Replaced
        } else if name == "display_name" && era == MagicEra::Base {
            MagicBase::Kept(text, After::Every)
        } else if field.defs.iter().any(|d| d.1.model != model) && self.inherits_ahead(model) {
            MagicBase::Unknown
        } else {
            MagicBase::Kept(text, After::Extensions)
        }
    }

    /// The fields Odoo adds to `model`, with their labels. Before saas~14.4 every model gets `id`,
    /// `display_name` and `__last_update`, from saas~14.4 only a model that is not abstract, and
    /// from saas~18.1, where `BaseModel` defines the first two, every model gets those again.
    /// `__last_update` stops after 16.0 and is left out for an unknown series (`has_last_update`).
    fn magic_labels(&self, model: &str) -> Vec<(&'static str, &'static str)> {
        let era = magic_era(self.series);
        if era == MagicEra::Metaclass && self.index.kind(model, self.scope) == Some(ModelKind::Abstract) {
            return vec![];
        }
        let (last_update, logs) = (has_last_update(self.series), self.log_access(model));
        let added = |name: &str| match name {
            "id" | "display_name" => true,
            "__last_update" => last_update,
            _ => logs,
        };
        MAGIC_LABELS.iter().copied().filter(|(name, _)| added(name)).collect()
    }

    /// Whether Odoo adds the fields that log access to `model`: it has a table of its own, and no
    /// class of it in scope sets `_auto` or `_log_access` to anything but True.
    fn log_access(&self, model: &str) -> bool {
        let on = |c: &ClassFact, name: &str| c.attr(name).is_none_or(|a| a.value == OptionValue::Bool(true));
        self.index.kind(model, self.scope) != Some(ModelKind::Abstract)
            && self
                .index
                .classes(model)
                .iter()
                .all(|(mi, c)| !self.scope.sees(*mi) || (on(c, "_auto") && on(c, "_log_access")))
    }

    /// Odoo warns about a computed field whose dependency leads back to the field itself on
    /// another record, and makes it recursive. Each field is reported once per module, at the first
    /// such path. Abstract models get no dependencies, and from a transient model Odoo does not
    /// follow a path into a regular one.
    fn self_dependencies(
        &self,
        out: &mut Out<'m>,
        path: &'m Path,
        class: &ClassFact,
        fields: &ModelFields<'m>,
        reported: &mut FxHashSet<(&'m str, &'m str)>,
    ) {
        let kind = self.index.kind(&class.model, self.scope);
        if kind == Some(ModelKind::Abstract) {
            return;
        }
        let transient = kind == Some(ModelKind::Transient);
        for (method, paths) in &class.depends {
            for field in fields.fields.values().filter(|f| non_recursive_compute_of(f, method)) {
                let name = field.defs[0].2.name.as_str();
                if let Some(dep) = paths.iter().find(|p| self.leads_back(field, &p.value, transient))
                    && reported.insert((field.model, name))
                {
                    let message = format!(
                        "'{}' makes '{name}' depend on itself on other records, so Odoo logs a warning; declare '{name}' with recursive=True",
                        dep.value
                    );
                    out.push(path, "OD221", dep.pos, message);
                }
            }
        }
    }

    /// Whether a later segment of the dotted path names the field on its own model.
    fn leads_back(&self, field: &Field<'m>, path: &str, transient: bool) -> bool {
        let name = field.defs[0].2.name.as_str();
        let mut model = field.model.to_owned();
        for (i, segment) in path.split('.').enumerate() {
            if i > 0 && model == field.model && segment == name {
                return true;
            }
            let Ok(Some(FieldInfo {
                comodel: Some(next), ..
            })) = self.index.field(&model, segment, self.scope)
            else {
                return false;
            };
            if transient && self.index.kind(&next, self.scope) != Some(ModelKind::Transient) {
                return false;
            }
            model = next;
        }
        false
    }

    /// From saas~16.4 Odoo gives a relational field with `check_company` a domain on the comodel's
    /// `company_id`, through the comodel's `_check_company_domain`, which fails on a comodel without
    /// one. From 18.0 it builds that domain only for a model that has a company field, is
    /// `res.company`, or for a company-dependent field.
    fn check_company(&self, out: &mut Out<'m>, path: &'m Path, field: &Field<'m>, own: &FieldFact) {
        if !RELATIONAL_TYPES.contains(&own.ftype.as_str())
            || !field.is("check_company", &OptionValue::Bool(true))
            || field.get("related") != Merged::Unset
            || !self.reported_at(field, own, |f| f.option("check_company").is_some())
        {
            return;
        }
        let Merged::Known(OptionValue::Str(comodel)) = field.get("comodel_name") else {
            return;
        };
        if !self.lacks_company(comodel)
            || (self.series.is_some_and(|s| s >= COMPANY_FIELD_DOMAIN) && !self.company_domain(field))
        {
            return;
        }
        let message = format!(
            "'{}' has check_company=True, but {comodel} has no company_id field to check; remove check_company",
            own.name
        );
        out.push(path, "OD222", pos_of(own, "check_company"), message);
    }

    /// Whether the comodel is introduced in scope with no `company_id` anywhere, and nothing but
    /// `BaseModel` gives its `_check_company_domain`.
    fn lacks_company(&self, comodel: &str) -> bool {
        self.index.introduced(comodel, self.scope)
            && !self.index.hidden_parents(comodel, self.scope)
            && matches!(self.index.field(comodel, "company_id", self.scope), Err(None))
            && !self.index.defines_method(comodel, "_check_company_domain", true)
    }

    /// Whether Odoo, from 18.0, builds a company domain for the field rather than logging that it
    /// cannot.
    fn company_domain(&self, field: &Field<'m>) -> bool {
        field.model == "res.company"
            || field.is("company_dependent", &OptionValue::Bool(true))
            || ["company_id", "company_ids"]
                .iter()
                .any(|name| self.index.field(field.model, name, self.scope).is_ok())
    }
}

/// Whether the field is computed by `method` with dependencies it takes from the method, and is
/// not declared recursive.
fn non_recursive_compute_of(field: &Field, method: &str) -> bool {
    matches!(field.get("compute"), Merged::Known(OptionValue::Str(m)) if m == method)
        && field.get("related") == Merged::Unset
        && field.get("depends") == Merged::Unset
        && matches!(
            field.get("recursive"),
            Merged::Unset | Merged::Known(OptionValue::Bool(false))
        )
}

/// Whether the field is required, when a literal tells.
fn required(field: &Field) -> Option<bool> {
    match field.get("required") {
        Merged::Unset => Some(false),
        Merged::Known(OptionValue::Bool(b)) => Some(*b),
        _ => None,
    }
}

/// Whether the field gives no ondelete: unset, or a false literal other than `""`, which unlike the
/// others has the `.lower()` Odoo calls on the value.
fn no_ondelete(field: &Field) -> bool {
    matches!(
        field.get("ondelete"),
        Merged::Unset | Merged::Known(OptionValue::Falsy | OptionValue::Bool(false))
    )
}

/// Whether the field's ondelete keeps the records it points to from going away under it.
fn kept_on_delete(field: &Field) -> Option<bool> {
    if no_ondelete(field) {
        return Some(false);
    }
    match field.get("ondelete") {
        Merged::Known(OptionValue::Str(policy)) => {
            Some(matches!(policy.to_lowercase().as_str(), "cascade" | "restrict"))
        }
        _ => None,
    }
}

/// Whether the field may delegate by `delegate`, which Odoo reads for its truth: a literal tells,
/// and any other value may.
fn may_delegate(field: &Field) -> bool {
    truth(field.get("delegate")) != Some(false)
}

/// The truth Python gives an attribute, for one whose default is false; None when only a run
/// tells.
fn truth(merged: Merged) -> Option<bool> {
    match merged {
        Merged::Unset | Merged::Known(OptionValue::Falsy) => Some(false),
        Merged::Known(OptionValue::Bool(b)) => Some(*b),
        Merged::Known(OptionValue::Str(s)) => Some(!s.is_empty()),
        Merged::Known(OptionValue::Strs(v) | OptionValue::Tuple(v)) => Some(!v.is_empty()),
        Merged::Known(OptionValue::Dict(d)) => Some(!d.is_empty()),
        Merged::Known(OptionValue::Other) | Merged::Unknown => None,
    }
}

/// What a delegating field lacks of `required=True` and ondelete 'cascade' or 'restrict', or None
/// when it lacks neither. An attribute that is not a literal is not named, nor is an ondelete that
/// is false when `defaulted`: Odoo then picks one from `required`.
fn delegation_fix(field: &Field, defaulted: bool) -> Option<String> {
    let mut fix = vec![];
    if required(field) == Some(false) {
        fix.push("required=True");
    }
    if kept_on_delete(field) == Some(false) && !(defaulted && truth(field.get("ondelete")) == Some(false)) {
        fix.push("ondelete=\"cascade\"");
    }
    (!fix.is_empty()).then(|| fix.join(", "))
}

/// The values the field has before its definition `at` applies its `selection_add`, or None when a
/// definition computes them, or one that sets them may come before or after it or the last
/// `selection` list. Odoo replaces the values with each `selection` list it meets, the definition's
/// own included, and ignores the `selection_add` of a lone definition from 15.0. `tuples` tells
/// whether the series takes a tuple as a `selection` list.
fn earlier_values<'m>(field: &Field<'m>, at: usize, tuples: bool) -> Option<FxHashSet<&'m str>> {
    if let Some(own) = field.defs[at].2.option("selection") {
        let own = listed(&own.value, tuples)?;
        return (field.defs.len() > 1).then(|| own.iter().map(String::as_str).collect());
    }
    let setters = earlier_setters(field, at)?;
    let base = last_list(field, &setters)?;
    let mut values = FxHashSet::default();
    for i in setters.into_iter().filter(|&i| i == base || field.before[base][i]) {
        selection_values(field.defs[i].2, tuples, &mut values)?;
    }
    Some(values)
}

/// The definitions that set the field's values before its definition `at`, or None when one may
/// also come after it, or one unpacks `**kwargs`.
fn earlier_setters(field: &Field, at: usize) -> Option<Vec<usize>> {
    let mut setters = vec![];
    for i in (0..field.defs.len()).filter(|&i| i != at && !field.before[at][i]) {
        let def = field.defs[i].2;
        let sets = def.option("selection").is_some() || def.option("selection_add").is_some();
        if def.splat || (sets && !field.before[i][at]) {
            return None;
        }
        if sets {
            setters.push(i);
        }
    }
    Some(setters)
}

/// The setter whose `selection` list Odoo applies last, so that only it and the `selection_add` of
/// the setters after it count. None when no setter has a list, or no setter with a list is known
/// to come after every other setter.
fn last_list(field: &Field, setters: &[usize]) -> Option<usize> {
    let lists = |i: usize| field.defs[i].2.option("selection").is_some();
    let applied_last = |b: usize| {
        let placed = |s: usize| s == b || field.before[s][b] || (field.before[b][s] && !lists(s));
        setters.iter().all(|&s| placed(s))
    };
    setters.iter().copied().find(|&b| lists(b) && applied_last(b))
}

/// Adds the values of a definition's `selection` and `selection_add`. None when one is not a
/// literal list, or `selection` is a tuple and not `tuples`.
fn selection_values<'m>(def: &'m FieldFact, tuples: bool, values: &mut FxHashSet<&'m str>) -> Option<()> {
    for (name, tuples) in [("selection", tuples), ("selection_add", true)] {
        if let Some(option) = def.option(name) {
            values.extend(listed(&option.value, tuples)?.iter().map(String::as_str));
        }
    }
    Some(())
}

/// The values a literal list gives, or a literal tuple when `tuples`.
fn listed(value: &OptionValue, tuples: bool) -> Option<&[String]> {
    match value {
        OptionValue::Strs(values) => Some(values),
        OptionValue::Tuple(values) if tuples => Some(values),
        _ => None,
    }
}

fn sets_parent_store(class: &ClassFact) -> bool {
    class
        .attr("_parent_store")
        .is_some_and(|a| a.value != OptionValue::Bool(false))
}

fn true_or_unset(class: &ClassFact, attr: &str) -> bool {
    class.attr(attr).is_none_or(|a| a.value == OptionValue::Bool(true))
}

/// What the table of a model holds for a name, as far as its stored fields tell.
enum Column<'m> {
    /// A column, or a field whose column the rule cannot tell.
    Present,
    Missing,
    NotStored,
    /// A field every model has, which the ORM computes, such as `display_name`.
    Computed,
    /// A log column of a model that does not log access.
    Unlogged,
    X2many(&'m str),
}

/// Why the table of a model has no column of a name.
enum Lack<'m> {
    NotStored,
    Computed,
    Unlogged,
    X2many(&'m str),
    /// A field of the parent model it names first, which the model reaches through `_inherits` by
    /// the field it names second.
    Delegated(&'m str, &'m str),
    /// No field of the name; the `<name>_id` field the model stores, when it has one.
    Undefined(Option<String>),
}

/// Methods and attributes that create the table or the view of a model with `_auto = False`.
const VIEW_MAKERS: &[&str] = &["init", "_auto_init", "_table_query", "_table_sql"];

/// OD224 on a qualified column a literal query names on the table of a model, which no stored
/// field of the model provides, or on a view whose columns its `init()` spells, which selects no
/// column of that name. The rule does not run for a module whose scope is partial, nor in upgrade
/// scripts, and reads only tables `column_model` or `view_model` accepts.
impl<'m> Ctx<'_, 'm> {
    fn sql_columns(&self, out: &mut Out<'m>, path: &'m Path, columns: &[SqlColumn]) {
        if self.partial {
            return;
        }
        for column in columns {
            if let Some(message) = self.unknown_column(column) {
                out.push(path, "OD224", column.pos, message);
            }
        }
    }

    /// Whether `model` is the one model whose table may be `table`, introduced in scope, which
    /// takes that table there and has no parent the scan cannot see.
    fn owns_table(&self, model: &str, table: &str) -> bool {
        self.index.table_models(table) == [model]
            && self.index.table(model, self.scope).as_deref() == Some(table)
            && self.index.introduced(model, self.scope)
            && !self.index.hidden_parents(model, self.scope)
    }

    /// The model whose stored fields make up `table`: the one `owns_table` accepts, when Odoo builds
    /// the table from its fields rather than an SQL view or a table query.
    fn column_model(&self, table: &str) -> Option<&'m str> {
        let &[model] = self.index.table_models(table) else {
            return None;
        };
        let readable = self.owns_table(model, table) && self.has_table(model) && !self.table_query(model);
        readable.then_some(model)
    }

    /// The model whose view is `table`, the one `owns_table` accepts, with the view `view_of` reads.
    fn view_model(&self, table: &str) -> Option<(&'m str, &'m SqlView)> {
        let &[model] = self.index.table_models(table) else {
            return None;
        };
        let view = self.owns_table(model, table).then(|| self.view_of(model))??;
        Some((model, view))
    }

    /// Whether a class of `model` in scope gives it a `_table_query` or, from 20.0, a `_table_sql`.
    fn table_query(&self, model: &str) -> bool {
        self.some_class(model, |c| {
            c.methods.iter().any(|m| m == "_table_query" || m == "_table_sql")
        })
    }

    /// The message on a column no field gives, unless a module adds it by hand.
    fn unknown_column(&self, column: &SqlColumn) -> Option<String> {
        let head = format!("{}: {} has no such column", column.text, column.table);
        if let Some((model, view)) = self.view_model(&column.table) {
            let names = view.names.as_deref().unwrap_or_default();
            return (!names.contains(&column.column)).then(|| {
                format!(
                    "{head}, since the view init() creates for {model} selects none; name a column the view selects"
                )
            });
        }
        let model = self.column_model(&column.table)?;
        let name = column.column.as_str();
        let reason = match self.lack(model, &column.table, name)? {
            Lack::NotStored => {
                format!("{model}.{name} is not stored; set store=True on the field, or read it through the ORM")
            }
            Lack::Computed => format!("{model}.{name} is computed by the ORM, not stored; read it through the ORM"),
            Lack::Unlogged => format!(
                "{model}.{name} is not created, as {model} sets _log_access to False; drop it from the query, or log access on the model"
            ),
            Lack::X2many("One2many") => {
                format!("{model}.{name} is a One2many; join the table of its comodel on its inverse field")
            }
            Lack::X2many(ftype) => format!("{model}.{name} is a {ftype}; join its relation table"),
            Lack::Delegated(parent, via) => format!(
                "{name} is a field of {parent}, which {model} reaches through _inherits; join {} on {via}",
                self.parent_table(parent)
            ),
            Lack::Undefined(suggested) => format!("{model} has no field {name}; {}", fix_name(model, suggested)),
        };
        Some(format!("{head}, since {reason}"))
    }

    fn parent_table(&self, parent: &str) -> String {
        self.index
            .table(parent, self.scope)
            .unwrap_or_else(|| parent.replace('.', "_"))
    }

    /// Why the `table` of `model` has no column `name`, None when it has one or may have one: a
    /// module adds it by hand, or one outside the scope defines the field.
    fn lack(&self, model: &'m str, table: &str, name: &str) -> Option<Lack<'m>> {
        if self.index.hand_made_column(model, table, name, self.scope) {
            return None;
        }
        match self.column(model, name) {
            Column::Present => None,
            Column::NotStored => Some(Lack::NotStored),
            Column::Computed => Some(Lack::Computed),
            Column::Unlogged => Some(Lack::Unlogged),
            Column::X2many(ftype) => Some(Lack::X2many(ftype)),
            Column::Missing => self.undefined_column(model, name),
        }
    }

    /// What a table lacks for a name no field of `model` in scope has, unless a module outside the
    /// scope defines it.
    fn undefined_column(&self, model: &'m str, name: &str) -> Option<Lack<'m>> {
        if self.index.defines_field(model, name) {
            return None;
        }
        if let Some((parent, via)) = self.delegated_field(model, name) {
            return Some(Lack::Delegated(parent, via));
        }
        let suggested = format!("{name}_id");
        let stored =
            matches!(self.column(model, &suggested), Column::Present) && self.index.defines_field(model, &suggested);
        Some(Lack::Undefined(stored.then_some(suggested)))
    }

    /// What the table of `model` holds for `name`. Of the fields Odoo adds without a definition,
    /// `id`, the log columns where `logs_access` says so and `parent_path` under `_parent_store`
    /// are present, and `display_name`, and `__last_update` up to 16.0, are not stored. Present too
    /// are fields whose column depends on the series or on `attachment`, and an undefined name the
    /// merged `_inherits` may name, which Odoo creates or refuses the model over.
    fn column(&self, model: &'m str, name: &str) -> Column<'m> {
        if self.index.implicit_column(model, name)
            || self.index.log_column(name) && self.logs_access(model)
            || name == "parent_path" && self.some_class(model, sets_parent_store)
        {
            return Column::Present;
        }
        let mut defs = self.index.merged_fields(model, self.scope);
        defs.retain(|d| d.2.name == name);
        if defs.is_empty() {
            return match self.undefined(name) {
                Column::Missing if self.inherits_entry(model, name) != Some(false) => Column::Present,
                lack => lack,
            };
        }
        self.merge(model, defs)
            .map_or(Column::Present, |field| stored_column(&field))
    }

    /// What a name no field of the model defines is: a log column the model leaves out, a field the
    /// ORM computes, or nothing, which `__last_update` is after 16.0 or in an unknown series.
    fn undefined(&self, name: &str) -> Column<'m> {
        if self.index.log_column(name) {
            Column::Unlogged
        } else if self.index.magic_field(name) && (name != "__last_update" || has_last_update(self.series)) {
            Column::Computed
        } else {
            Column::Missing
        }
    }

    /// Whether Odoo gives the table of `model` its log columns. From saas~14.4 the metaclass adds
    /// them only to the class that introduces the model, so they are there unless every such class
    /// in scope sets `_log_access` to False. Before, classes merge into one whose last setting wins,
    /// and Odoo never drops a column, so they are missing only when the module introducing the
    /// model sets False and no class sets anything else. An unknown series misses them only when
    /// both rules agree, so no era reports a column its series has.
    fn logs_access(&self, model: &str) -> bool {
        if self.series.is_none() {
            return self.introduced_logs_access(model) || self.merged_logs_access(model);
        }
        match magic_era(self.series) {
            MagicEra::RegistryMerge | MagicEra::Setup => self.merged_logs_access(model),
            MagicEra::Metaclass | MagicEra::Base => self.introduced_logs_access(model),
        }
    }

    fn introduced_logs_access(&self, model: &str) -> bool {
        !self.some_class(model, |c| c.new_model) || self.some_class(model, |c| c.new_model && !unlogged(c))
    }

    fn merged_logs_access(&self, model: &str) -> bool {
        let classes = self.index.classes(model);
        let introducing = |mi: usize| classes.iter().any(|(m, c)| *m == mi && c.new_model);
        let unlogged_at_introduction = classes
            .iter()
            .any(|(mi, c)| self.scope.sees(*mi) && introducing(*mi) && unlogged(c));
        !unlogged_at_introduction || self.some_class(model, |c| c.attr("_log_access").is_some() && !unlogged(c))
    }

    /// The model `model` delegates to that has the field `name`, with the field that delegates to
    /// it, among the `_inherits` entries and `delegate=True` fields of the classes it merges: the
    /// first whose field the merged `_inherits` may still name, else the first.
    fn delegated_field(&self, model: &str, name: &str) -> Option<(&'m str, &'m str)> {
        let classes = self.index.merged_classes(model, self.scope);
        let pairs: Vec<(&'m str, &'m str)> = classes
            .iter()
            .flat_map(|(_, c)| &c.inherits)
            .filter(|(parent, _)| !self.index.field_defs(parent, name, None).is_empty())
            .map(|(parent, via)| (parent.as_str(), via.as_str()))
            .collect();
        let holds = |&&(_, via): &&(&str, &str)| self.inherits_entry(model, via) != Some(false);
        pairs.iter().find(holds).or(pairs.first()).copied()
    }
}

/// The fix for a name no field of `model` has: its `_id` field, or else any stored field.
fn fix_name(model: &str, suggested: Option<String>) -> String {
    suggested.map_or_else(
        || format!("name the column of a stored field of {model}"),
        |field| format!("use {field}"),
    )
}

/// OD225 and OD226 on models built on an SQL view, and OD227 on the columns of constraint and index
/// definitions. They run where `registry` does.
impl<'m> Ctx<'_, 'm> {
    /// OD226 at the `_auto = False` of the class that introduces a model with a literal table that
    /// `view_made` finds nothing to create, unless the class is kept out of the registry.
    fn view_not_created(&self, out: &mut Out<'m>, path: &'m Path, class: &ClassFact) {
        let auto = class.attr("_auto").filter(|a| a.value == OptionValue::Bool(false));
        let Some(auto) = auto.filter(|_| class.new_model && class.registered()) else {
            return;
        };
        let model = class.model.as_str();
        let Some(table) = self.index.table(model, self.scope) else {
            return;
        };
        if self.view_made(model, &table) {
            return;
        }
        out.push(
            path,
            "OD226",
            auto.pos,
            format!(
                "_auto = False leaves {model} without a table, and nothing creates the view {table}; create it in init(), or drop _auto = False"
            ),
        );
    }

    /// Whether something may give `model` the relation `table`: a class of it or of a model it
    /// inherits, in any indexed module, defines `init()` or `_auto_init()`, sets a table query,
    /// creates a view whose whole name is `self._table` or has a parent the scan cannot see; another
    /// model, or a Many2many in scope, has that table; an indexed module creates a view of that name
    /// in Python or SQL; or this module, not a dependency, creates a view it names at runtime.
    /// `table_models` leaves out an abstract model.
    fn view_made(&self, model: &str, table: &str) -> bool {
        let creates = |c: &ClassFact| {
            c.hidden_parents() || c.own_view || c.methods.iter().any(|m| VIEW_MAKERS.contains(&m.as_str()))
        };
        self.index.chain_classes(model).iter().any(|(_, c)| creates(c))
            || self.index.table_models(table) != [model]
            || self.index.relation_table(table, self.scope)
            || self.index.creates_view(table)
            || self.module.python.iter().any(|f| f.facts.creates_any_view)
    }

    /// The view the `init()` of `model` creates, when its text spells the names of the columns. The
    /// model has no table nor table query, and the one class of it or of a model it inherits, in any
    /// indexed module, that defines `init()` is one of `model` in scope, which creates one view, on
    /// the table of the model or on a placeholder; none defines `_auto_init()`.
    fn view_of(&self, model: &'m str) -> Option<&'m SqlView> {
        if !self.viewed(model) {
            return None;
        }
        let chain = self.index.chain_classes(model);
        let defining = |method: &str| -> Vec<(usize, &'m ClassFact)> {
            chain
                .iter()
                .filter(|(_, c)| c.methods.iter().any(|m| m == method))
                .copied()
                .collect()
        };
        let [(module, class)] = defining("init")[..] else {
            return None;
        };
        let [view] = class.views.as_slice() else {
            return None;
        };
        let table = self.index.table(model, self.scope)?;
        let own = class.model == model && self.scope.sees(module) && defining("_auto_init").is_empty();
        let on_table = view.table.as_ref().is_none_or(|t| *t == table);
        (own && on_table && view.names.is_some()).then_some(view)
    }

    /// Whether `model` reads from a view: no class of it in scope is abstract, one sets
    /// `_auto = False`, and none gives it a table query.
    fn viewed(&self, model: &str) -> bool {
        let auto_false = |c: &ClassFact| c.attr("_auto").is_some_and(|a| a.value == OptionValue::Bool(false));
        self.every_class(model, |c| c.kind != ModelKind::Abstract)
            && self.some_class(model, auto_false)
            && !self.table_query(model)
    }

    /// OD225 on a field with a column, at its first definition, when the view of its model selects
    /// no column of its name.
    fn view_field(&self, out: &mut Out<'m>, path: &'m Path, fields: &ModelFields<'m>, own: &'m FieldFact) {
        let (Some(view), Some(field)) = (fields.view, fields.fields.get(own.name.as_str())) else {
            return;
        };
        let names = view.names.as_deref().unwrap_or_default();
        let first = self.first_def(field).is_some_and(|d| std::ptr::eq(d.2, own));
        if names.contains(&own.name) || !first || !has_certain_column(field) {
            return;
        }
        let (model, name) = (fields.model, own.name.as_str());
        out.push(
            path,
            "OD225",
            own.name_pos,
            format!(
                "{model}.{name} is stored, but the view {} selects no column {name} for it; select one AS {name} in init()",
                self.view_table(model, view)
            ),
        );
    }

    /// OD225 at the view `class` creates for its model, when the view selects no `id`, or no column
    /// for a field with a column that another module defines first, which `view_field` reports
    /// nowhere in this one.
    fn view_id(&self, out: &mut Out<'m>, path: &'m Path, class: &ClassFact, fields: &ModelFields<'m>) {
        let Some(view) = fields
            .view
            .filter(|v| class.views.iter().any(|own| std::ptr::eq(own, *v)))
        else {
            return;
        };
        let (model, table) = (fields.model, self.view_table(fields.model, view));
        let names = view.names.as_deref().unwrap_or_default();
        if !names.iter().any(|n| n == "id") {
            let message = format!(
                "the view {table} of {model} selects no id column, which every read of its records needs; select one AS id"
            );
            out.push(path, "OD225", view.pos, message);
        }
        for (name, module) in self.foreign_fields(fields, names) {
            let message = format!(
                "the view {table} of {model} selects no column {name}, which the stored field {model}.{name} of {module} needs; select one AS {name}"
            );
            out.push(path, "OD225", view.pos, message);
        }
    }

    /// The fields with a column of a model with a view that the view selects no column for and
    /// another module defines first, by name, with that module.
    fn foreign_fields(&self, fields: &ModelFields<'m>, names: &[String]) -> Vec<(&'m str, &'m str)> {
        let me = self.index.by_name[self.module.name.as_str()];
        let mut found: Vec<(&str, &str)> = fields
            .fields
            .iter()
            .filter(|(name, field)| !names.iter().any(|n| n == *name) && has_certain_column(field))
            .filter_map(|(&name, field)| {
                let (module, _, _) = self.first_def(field)?;
                (*module != me).then(|| (name, self.index.modules[*module].name.as_str()))
            })
            .collect();
        found.sort_unstable();
        found
    }

    fn view_table(&self, model: &str, view: &SqlView) -> String {
        view.table
            .clone()
            .or_else(|| self.index.table(model, self.scope))
            .unwrap_or_else(|| model.replace('.', "_"))
    }

    /// OD227 on each column a constraint or an index definition of `class` names that the table of
    /// its model lacks. Odoo reads `_sql_constraints` before 19.0 and the table objects from 19.0,
    /// and applies neither to a model without a table of its fields.
    fn definitions(&self, out: &mut Out<'m>, path: &'m Path, class: &'m ClassFact) {
        let model = class.model.as_str();
        if class.definitions.is_empty() || !self.has_table(model) || self.table_query(model) {
            return;
        }
        let Some(table) = self.index.table(model, self.scope) else {
            return;
        };
        for (_, column) in class
            .definitions
            .iter()
            .filter(|(source, _)| applies(*source, self.series))
        {
            if let Some(reason) = self.definition_reason(model, &table, &column.value) {
                let message = format!("{}: {table} has no such column, since {reason}", column.value);
                out.push(path, "OD227", column.pos, message);
            }
        }
    }

    /// Why a definition cannot name `name` on the table of `model`, with the fix.
    fn definition_reason(&self, model: &'m str, table: &str, name: &str) -> Option<String> {
        Some(match self.lack(model, table, name)? {
            Lack::NotStored => {
                format!("{model}.{name} is not stored; set store=True on the field, or drop it from the definition")
            }
            Lack::Computed => format!("{model}.{name} is computed by the ORM, not stored; drop it from the definition"),
            Lack::Unlogged => format!(
                "{model}.{name} is not created, as {model} sets _log_access to False; drop it from the definition, or log access on the model"
            ),
            Lack::X2many(ftype) => {
                format!("{model}.{name} is a {ftype}, which has no column; drop it from the definition")
            }
            Lack::Delegated(parent, _) => format!(
                "{name} is a field of {parent}, which {model} reaches through _inherits; declare the definition on {parent}"
            ),
            Lack::Undefined(suggested) => format!("{model} has no field {name}; {}", fix_name(model, suggested)),
        })
    }
}

/// Whether Odoo applies the definitions of `source` in `series`, or may when it is unknown.
fn applies(source: DefinitionSource, series: Option<Series>) -> bool {
    let modern = series.map(|s| s >= Series { major: 19, minor: 0 });
    match source {
        DefinitionSource::SqlConstraints => modern != Some(true),
        DefinitionSource::TableObject => modern != Some(false),
    }
}

fn company_dependent(field: &Field) -> bool {
    !matches!(
        field.get("company_dependent"),
        Merged::Unset | Merged::Known(OptionValue::Bool(false))
    )
}

/// Whether a merged field has a column: x2many fields and fields not stored have none, and the
/// column of a Binary or a company-dependent field depends on `attachment` or on the series.
fn stored_column<'m>(field: &Field<'m>) -> Column<'m> {
    let ftype = field.defs.first().map_or("", |d| d.2.ftype.as_str());
    match ftype {
        "One2many" | "Many2many" => Column::X2many(ftype),
        "Binary" | "Image" => Column::Present,
        _ if company_dependent(field) => Column::Present,
        _ if field.stored() == Some(false) => Column::NotStored,
        _ => Column::Present,
    }
}

/// Whether a merged field certainly has a column: it is stored, and its column does not depend on
/// `attachment` or the series, as `stored_column` says.
fn has_certain_column(field: &Field) -> bool {
    let ftype = field.defs.first().map_or("", |d| d.2.ftype.as_str());
    let typed = !matches!(ftype, "One2many" | "Many2many" | "Binary" | "Image");
    typed && !company_dependent(field) && field.stored() == Some(true)
}

#[cfg(test)]
mod tests {
    use super::{MagicEra, applies, cache_global, derived_label, has_last_update, magic_era, shadowed_member};
    use crate::facts::DefinitionSource;
    use crate::series::Series;

    #[test]
    fn applies_sql_constraints_before_19_and_table_objects_from_19() {
        let applied = |source, series: &str| applies(source, Series::parse(series));
        assert!(
            applied(DefinitionSource::SqlConstraints, "18.0") && !applied(DefinitionSource::SqlConstraints, "19.0")
        );
        assert!(!applied(DefinitionSource::TableObject, "18.0") && applied(DefinitionSource::TableObject, "19.0"));
        assert!(applies(DefinitionSource::SqlConstraints, None) && applies(DefinitionSource::TableObject, None));
    }

    #[test]
    fn derives_labels_as_python_title_case() {
        assert_eq!(derived_label("partner_id"), "Partner");
        assert_eq!(derived_label("tag_ids"), "Tag");
        assert_eq!(derived_label("parent_id_ids"), "Parent Id");
        assert_eq!(derived_label("x_studio_9a_date"), "X Studio 9A Date");
        assert_eq!(derived_label("is_VAT"), "Is Vat");
        assert_eq!(derived_label("id"), "Id");
    }

    #[test]
    fn adds_magic_fields_by_series() {
        let era = |series: &str| magic_era(Series::parse(series));
        assert_eq!(era("12.0"), MagicEra::RegistryMerge);
        assert_eq!(era("saas~12.4"), MagicEra::RegistryMerge);
        assert_eq!(era("saas~12.5"), MagicEra::Setup);
        assert_eq!(era("saas~14.3"), MagicEra::Setup);
        assert_eq!(era("saas~14.4"), MagicEra::Metaclass);
        assert_eq!(era("18.0"), MagicEra::Metaclass);
        assert_eq!(era("saas~18.1"), MagicEra::Base);
        assert_eq!(magic_era(None), MagicEra::Metaclass);
        assert!(has_last_update(Series::parse("16.0")) && !has_last_update(Series::parse("saas~16.1")));
        assert!(!has_last_update(None));
    }

    /// Each bound of `CACHE_KEY_GLOBALS` with the series on either side of it.
    #[test]
    fn cache_keys_read_the_globals_of_the_series() {
        let global = |name: &str, series: &str| cache_global(name, Series::parse(series));
        assert!(!global("time", "saas~17.1") && global("time", "saas~17.2") && !global("time", "19.0"));
        assert!(global("warnings", "18.0") && !global("warnings", "saas~18.1") && global("warnings", "saas~18.2"));
        assert!(!global("warnings", "saas~16.3") && global("warnings", "saas~16.4") && !global("warnings", "19.0"));
        assert!(!global("Counter", "saas~13.3") && global("Counter", "saas~13.4"));
        assert!(global("Counter", "saas~18.2") && !global("Counter", "saas~18.3"));
        assert!(!global("method", "saas~18.4") && global("method", "19.0") && global("method", "20.0"));
        assert!(global("ormcache", "saas~18.4") && !global("ormcache", "19.0"));
        assert!(global("getargspec", "13.0") && !global("getargspec", "saas~13.1"));
        assert!(!global("signature", "13.0") && global("signature", "saas~13.1") && !global("signature", "19.0"));
        assert!(!global("pycompat", "10.0") && global("pycompat", "11.0"));
        assert!(global("pycompat", "12.0") && !global("pycompat", "saas~12.1"));
        assert!(global("dummy_cache", "saas~16.3") && !global("dummy_cache", "saas~16.4"));
        assert!(global("cache", "saas~18.1") && !global("cache", "saas~18.2"));
        assert!(!global("Parameter", "saas~16.1") && global("Parameter", "saas~16.2"));
        assert!(global("Parameter", "saas~18.4") && !global("Parameter", "19.0"));
        assert!(!global("typing", "18.0") && global("typing", "saas~18.1") && !global("typing", "19.0"));
        assert!(!global("threading", "saas~18.2") && global("threading", "saas~18.3"));
        assert!(global("threading", "saas~18.4") && !global("threading", "19.0"));
        assert!(!global("_logger", "9.0") && global("_logger", "10.0") && !global("getargspec", "9.0"));
        assert!(cache_global("time", None) && cache_global("method", None) && !cache_global("lang", None));
    }

    #[test]
    fn shadows_only_the_members_the_series_has() {
        let shadows = |name: &str, series: &str| shadowed_member(name, Series::parse(series)).is_some();
        assert!(shadows("copy", "12.0") && shadows("env", "20.0") && shadows("grouped", "16.0"));
        assert!(!shadows("has_access", "17.0") && shadows("has_access", "18.0"));
        assert!(!shadows("_read_grouping_sets", "19.0") && shadows("_read_grouping_sets", "20.0"));
        assert!(shadows("toggle_active", "18.0") && !shadows("toggle_active", "20.0"));
        assert!(!shadows("_table_query", "saas~13.4") && shadows("_table_query", "saas~13.5"));
        assert!(shadows("_table_query", "19.0") && shadows("_table_query", "saas~19.4"));
        assert!(!shadows("_table_query", "20.0"));
        assert!(!shadows("_condition_to_sql", "17.0") && shadows("_condition_to_sql", "saas~17.1"));
        assert!(shadows("_condition_to_sql", "saas~18.3") && !shadows("_condition_to_sql", "saas~18.4"));
        assert!(!shadows("_condition_to_sql", "19.0") && !shadows("_condition_to_sql", "20.0"));
        assert!(!shadows("_name_search", "16.0") && !shadows("copied", "18.0"));
        assert!(shadowed_member("has_access", None).is_none() && shadowed_member("copy", None).is_some());
        let eighteen = Series::parse("18.0");
        assert_eq!(
            shadowed_member("_table_query", eighteen).as_deref(),
            Some("attribute _table_query")
        );
        assert_eq!(shadowed_member("copy", eighteen).as_deref(), Some("method copy()"));
    }
}
