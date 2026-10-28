//! Rules that need facts from more than one file.

use std::path::Path;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{
    Annotation, ClassFact, DomainFact, FieldFact, JsFacts, JsName, ModelKind, OrmCall, Pos, RawDiag, RecordFact,
    Spanned, ViewFact, XmlRef,
};
use crate::index::{Export, Index, JsSpec, Scope, Status, ViewTree, full_id, js_module_path, located};
use crate::project::{Module, SourceFile};
use crate::series::Series;
use crate::xml::{LOCATOR, UNKNOWN_CONTEXT};

/// Directories of upgrade scripts, which name XML ids of past versions.
const UPGRADE_DIRS: &[&str] = &["migrations", "upgrades"];

/// Types a One2many's inverse may have: an Integer holds the id of a `res_model`/`res_id` pair.
const INVERSE_TYPES: &[&str] = &["Many2one", "Many2oneReference", "Integer"];

/// Directories whose XML and CSV files are not meant to be listed in the manifest.
const UNLISTED_DIRS: &[&str] = &["tests", "i18n", "migrations", "upgrades", "doc", "docs", "examples"];

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
    same_module_xmlids(module, &mut out);
    for (_, file) in es_modules(index, mi, series) {
        if let Some(e) = &file.facts.syntax_error {
            out.push(&file.path, "OD000", e.pos, e.value.clone());
        }
    }
    if index.has_base {
        let scope = index.scope(mi);
        let ctx = Ctx {
            index,
            module,
            scope: &scope,
            dep_names: module.manifest.depends.iter().map(|d| d.value.as_str()).collect(),
            dynamic: module.python.iter().any(|f| f.facts.dynamic_models),
            partial: index.partial_scope(&scope),
        };
        ctx.dependencies(&mut out);
        ctx.python(&mut out);
        ctx.data(&mut out);
        for (fi, file) in es_modules(index, mi, series) {
            ctx.js(&mut out, file);
            ctx.js_patches(&mut out, mi, fi, file);
            for call in &file.facts.orm_calls {
                ctx.orm_call(&mut out, &file.path, call);
            }
        }
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
    let mentioned: FxHashSet<&str> = module
        .python
        .iter()
        .flat_map(|f| &f.facts.file_mentions)
        .map(String::as_str)
        .collect();
    for file in &module.data {
        let top = file.rel.split('/').next().unwrap_or_default();
        let base = file.rel.rsplit('/').next().unwrap_or_default();
        // Chart templates and files named in Python code are loaded by that code.
        let python_loaded = mentioned.contains(base) || file.rel.starts_with("data/template/");
        if file.rank.is_none()
            && file.facts.is_data
            && file.rel.contains('/')
            && !UNLISTED_DIRS.contains(&top)
            && !python_loaded
        {
            out.push(
                &file.path,
                "OD502",
                Pos { line: 1, col: 1 },
                format!(
                    "'{}' is not in the manifest's data or demo, so Odoo never loads it",
                    file.rel
                ),
            );
        }
    }
}

fn same_module_xmlids<'m>(module: &'m Module, out: &mut Out<'m>) {
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
                let place = load_place(module, def_rank, rank);
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

fn load_place(module: &Module, def_rank: usize, rank: usize) -> String {
    if def_rank == rank {
        return "further down this file".to_owned();
    }
    let file = module.manifest.data.iter().chain(&module.manifest.demo).nth(def_rank);
    format!("in {}", file.map_or("", |f| f.value.as_str()))
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
            JsSpec::Found(t) if !self.scope.sees(t.module) && !self.index.always_installed(t.module) => format!(
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
        let model = &call.model;
        let message = match self.index.model_status(&model.value, self.scope, None) {
            Status::Visible => return self.orm_method(out, path, call),
            Status::Hidden(m)
                if !self
                    .index
                    .by_name
                    .get(m)
                    .is_some_and(|&mi| self.index.always_installed(mi)) =>
            {
                format!("model '{}' comes from module {m}, which is not in depends", model.value)
            }
            Status::Missing if !self.dynamic => format!("no indexed module defines model '{}'", model.value),
            _ => return,
        };
        out.push(path, "OD603", model.pos, message);
    }

    /// A method from a module outside the index, such as an Enterprise one, may be the one called.
    fn orm_method(&self, out: &mut Out<'m>, path: &'m Path, call: &OrmCall) {
        let Some(method) = call.method.as_ref().filter(|_| !self.partial && !self.dynamic) else {
            return;
        };
        let model = &call.model.value;
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

    fn class(&self, out: &mut Out<'m>, path: &'m Path, class: &ClassFact) {
        for parent in &class.inherit {
            self.model_ref(out, path, parent, Some(class));
        }
        for field in &class.fields {
            self.field(out, path, class, field);
        }
        let paths = class.paths.iter().chain(&class.order_fields);
        for p in paths.filter(|_| class.kind != ModelKind::Abstract) {
            self.path(out, path, "OD203", &class.model, p);
        }
        self.redefinition(out, path, class);
        self.super_calls(out, path, class);
        if class.new_model
            && class.kind != ModelKind::Abstract
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

    /// Parents the index cannot see: a Python base class, a module that is not indexed, a model
    /// nobody indexes, and for a mixin, every model that inherits it.
    fn super_calls(&self, out: &mut Out<'m>, path: &'m Path, class: &ClassFact) {
        if class.plain_bases || self.partial || class.kind == ModelKind::Abstract || !self.index.has_model(&class.model)
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
        self.module.python.iter().any(|f| {
            let has = |c: &ClassFact| f.facts.classes.iter().any(|x| std::ptr::eq(x, c));
            has(first) && has(then) && first.pos < then.pos
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
        // Odoo replaces the compute and inverse of a related field with its own.
        for method in field.methods.iter().filter(|_| field.related.is_none()) {
            if !index.has_method(&class.model, &method.value, self.scope) {
                out.push(
                    path,
                    "OD204",
                    method.pos,
                    format!("{} has no method '{}'", class.model, method.value),
                );
            }
        }
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
                self.view(out, path, view);
                self.locators(out, path, view, &mut trees);
            }
            for record in &file.facts.records {
                self.record(out, path, record);
            }
            self.domains(out, path, &file.facts.action_domains);
        }
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

    fn view(&self, out: &mut Out<'m>, path: &'m Path, view: &ViewFact) {
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
        for d in view
            .domains
            .iter()
            .filter(|d| !d.path.iter().any(|s| s == UNKNOWN_CONTEXT))
        {
            let Some(joined) = self.arch_path(model, &d.path) else {
                continue;
            };
            let target = match joined.as_str() {
                "" => Some(model.to_owned()),
                joined => self.index.path_comodel(model, joined, self.scope),
            };
            for f in target.iter().flat_map(|t| d.fields.iter().map(move |f| (t, f))) {
                self.path(out, path, "OD210", f.0, f.1);
            }
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
        if !self.index.view_visible(&parent, self.scope) {
            return;
        }
        let Some(root) = self.index.view_root(parent) else {
            return;
        };
        let tree = trees
            .entry(root.clone())
            .or_insert_with(|| self.index.view_tree(root, self.scope));
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
        if !self.index.has_model(&record.model.value) {
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
