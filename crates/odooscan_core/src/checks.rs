//! Rules that need facts from more than one file.

use std::path::Path;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{
    Annotation, ClassFact, DomainFact, FieldFact, JsFacts, JsName, ModelKind, OptionValue, OrmCall, Pos, RawDiag,
    RecordFact, Spanned, ViewFact, XmlRef,
};
use crate::flows;
use crate::index::{Export, FieldDef, Index, JsSpec, Scope, Status, ViewTree, full_id, js_module_path, located};
use crate::project::{Module, SourceFile};
use crate::series::Series;
use crate::xml::{LOCATOR, UNKNOWN_CONTEXT};

/// Directories of upgrade scripts, which name XML ids of past versions.
const UPGRADE_DIRS: &[&str] = &["migrations", "upgrades"];

/// Types a One2many's inverse may have: an Integer holds the id of a `res_model`/`res_id` pair.
const INVERSE_TYPES: &[&str] = &["Many2one", "Many2oneReference", "Integer"];

/// The longest identifier PostgreSQL keeps, which Odoo checks a relation table name against.
const PG_NAME_MAX: usize = 63;

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
        if !self.index.views.visible(&parent, |m| self.scope.sees(m)) {
            return;
        }
        let Some(root) = self.index.views.root(parent) else {
            return;
        };
        let tree = trees
            .entry(root.clone())
            .or_insert_with(|| self.index.views.tree(root, self.index.modules, |m| self.scope.sees(m)));
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

/// What the registry makes of an attribute of a field, as far as literals tell.
#[derive(Clone, Copy, PartialEq)]
enum Merged<'m> {
    Unset,
    Known(&'m OptionValue),
    Unknown,
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

    /// The value the last definitions that set the attribute agree on. One that unpacks `**kwargs`
    /// may set anything, and an expression only a run evaluates, so either makes it unknown.
    fn get(&self, name: &str) -> Merged<'m> {
        let mut merged = Merged::Unset;
        for i in self.last(|d| d.2.splat || d.2.option(name).is_some()) {
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
    /// none unless `store=True`.
    fn stored(&self) -> Option<bool> {
        match self.get("store") {
            Merged::Known(OptionValue::Bool(b)) => Some(*b),
            Merged::Unset => match (self.get("related"), self.get("compute")) {
                (Merged::Unset, Merged::Unset) => Some(true),
                (Merged::Unknown, _) | (_, Merged::Unknown) => None,
                _ => Some(false),
            },
            _ => None,
        }
    }

    fn position(&self, own: &FieldFact) -> Option<usize> {
        self.defs.iter().position(|d| std::ptr::eq(d.2, own))
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

/// Field definitions Odoo refuses when it sets up the registry. Each rule reads every definition
/// merged into the field, so none runs where a module in scope depends on one that is not indexed,
/// on a model whose introducing class is out of scope, or on one with a plain Python base class,
/// whose fields only Python knows.
impl<'m> Ctx<'_, 'm> {
    fn registry(&self, out: &mut Out<'m>) {
        if self.partial {
            return;
        }
        let checked = |c: &&ClassFact| {
            self.index.introduced(&c.model, self.scope) && !self.index.plain_bases(&c.model, self.scope)
        };
        let typed = |f: &&FieldFact| matches!(f.ftype.as_str(), "Monetary" | "Many2one" | "Many2many" | "Selection");
        for file in &self.module.python {
            for class in file.facts.classes.iter().filter(checked) {
                for own in class.fields.iter().filter(typed) {
                    self.registry_field(out, &file.path, &class.model, own);
                }
            }
        }
    }

    fn registry_field(&self, out: &mut Out<'m>, path: &'m Path, model: &'m str, own: &'m FieldFact) {
        let Some(field) = self.merged(model, &own.name) else {
            return;
        };
        let Some(at) = field.position(own) else {
            return;
        };
        match own.ftype.as_str() {
            "Monetary" => self.monetary(out, path, &field, own),
            "Many2one" => self.required_set_null(out, path, &field, own),
            "Many2many" if field.stored() == Some(true) => {
                self.long_relation(out, path, &field, own);
                self.relation_clash(out, path, &field, own);
            }
            "Selection" => self.selection_add(out, path, &field, at),
            _ => {}
        }
    }

    /// The field `name` of `model` as the registry builds it in scope. None when the order of a
    /// definition that changes its type against another one is unknown, or the type is not that of
    /// the last definitions.
    fn merged(&self, model: &'m str, name: &str) -> Option<Field<'m>> {
        let mut defs = self.index.merged_fields(model, self.scope);
        defs.retain(|d| d.2.name == name);
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

    /// Whether the registry certainly merges definition `a` of a field of `model` before `b`. The
    /// classes of `model` come after those of the models they inherit, unless one of them lists
    /// another model before its own in `_inherit`, which puts that model after the classes loaded
    /// before it. Among them, a dependency's come first, then in each module the class that
    /// introduces the model, then the others of one file in order; files load in the order of the
    /// imports, which is not indexed.
    fn before(&self, model: &str, a: &FieldDef<'m>, b: &FieldDef<'m>) -> bool {
        match (a.1.model == model, b.1.model == model) {
            (_, false) => false,
            (false, true) => !self.inherits_ahead(model),
            _ if a.0 != b.0 => self.index.reaches(b.0, a.0) && !self.index.reaches(a.0, b.0),
            _ if a.1.new_model != b.1.new_model => a.1.new_model,
            _ => self.in_one_file(a.0, a.1, b.1) && a.2.pos < b.2.pos,
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
        if field.is("required", &OptionValue::Bool(true))
            && field.is("ondelete", &OptionValue::Str("set null".to_owned()))
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
        let (relation, at, derived) = match field.get("relation") {
            Merged::Known(OptionValue::Str(r)) if self.reported_at(field, own, |f| f.option("relation").is_some()) => {
                (r.clone(), pos_of(own, "relation"), false)
            }
            Merged::Unset if self.introduces(field, own) => match self.implicit_relation(field) {
                Some(r) => (r, own.pos, true),
                None => return,
            },
            _ => return,
        };
        if relation.len() <= PG_NAME_MAX {
            return;
        }
        let fix = if derived {
            "; name a shorter one with relation="
        } else {
            ""
        };
        let message = format!(
            "relation table '{relation}' of '{}' is {} characters long, and PostgreSQL allows {PG_NAME_MAX}{fix}",
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
            value: OptionValue::Strs(added),
            pos,
        }) = own.option("selection_add")
        else {
            return;
        };
        if field.get("related") != Merged::Unset || !field.is("required", &OptionValue::Bool(true)) {
            return;
        }
        let Some(known) = earlier_values(field, at) else {
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
}

/// The values the field has before its definition `at`, or None when a definition computes them,
/// or one that sets them may come before or after it.
fn earlier_values<'m>(field: &Field<'m>, at: usize) -> Option<FxHashSet<&'m str>> {
    let mut values = FxHashSet::default();
    let mut based = false;
    for i in (0..field.defs.len()).filter(|&i| i != at && !field.before[at][i]) {
        let def: &'m FieldFact = field.defs[i].2;
        let sets = def.option("selection").is_some() || def.option("selection_add").is_some();
        if def.splat || (sets && !field.before[i][at]) {
            return None;
        }
        if sets {
            based |= selection_values(def, &mut values)?;
        }
    }
    based.then_some(values)
}

/// Adds the values of a definition's `selection` and `selection_add`, and tells whether it sets
/// `selection`. None when one is not a literal list.
fn selection_values<'m>(def: &'m FieldFact, values: &mut FxHashSet<&'m str>) -> Option<bool> {
    for name in ["selection", "selection_add"] {
        if let Some(option) = def.option(name) {
            let OptionValue::Strs(added) = &option.value else {
                return None;
            };
            values.extend(added.iter().map(String::as_str));
        }
    }
    Some(def.option("selection").is_some())
}
