//! Rules that need facts from more than one file.

use std::path::Path;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{ClassFact, FieldFact, ModelKind, Pos, RawDiag, RecordFact, Spanned, ViewFact, XmlRef};
use crate::index::{Index, Scope, Status, full_id};
use crate::project::Module;
use crate::xml::{LOCATOR, UNKNOWN_CONTEXT};

/// Directories of upgrade scripts, which name XML ids of past versions.
const UPGRADE_DIRS: &[&str] = &["migrations", "upgrades"];

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

pub fn check_module<'m>(index: &Index<'m>, mi: usize) -> Vec<(&'m Path, RawDiag)> {
    let module = &index.modules[mi];
    let mut out = Out { diags: vec![] };
    manifest_files(module, &mut out);
    same_module_xmlids(module, &mut out);
    if index.has_base {
        let scope = index.scope(mi);
        let ctx = Ctx {
            index,
            module,
            scope: &scope,
            dep_names: module.manifest.depends.iter().map(|d| d.value.as_str()).collect(),
        };
        ctx.dependencies(&mut out);
        ctx.python(&mut out);
        ctx.data(&mut out);
    }
    out.diags
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
            if !in_file.insert(&def.id) {
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
            Status::Missing => out.push(
                path,
                "OD201",
                model.pos,
                format!("no indexed module defines model '{}'", model.value),
            ),
        }
    }

    fn dependencies(&self, out: &mut Out<'m>) {
        for dep in &self.module.manifest.depends {
            if !self.index.by_name.contains_key(dep.value.as_str()) {
                out.push(
                    &self.module.manifest_path,
                    "OD503",
                    dep.pos,
                    format!("module '{}' is not on the addons path", dep.value),
                );
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

    fn class(&self, out: &mut Out<'m>, path: &'m Path, class: &ClassFact) {
        for parent in &class.inherit {
            self.model_ref(out, path, parent, Some(class));
        }
        for field in &class.fields {
            self.field(out, path, class, field);
        }
        for p in class.paths.iter().filter(|_| class.kind != ModelKind::Abstract) {
            self.path(out, path, "OD203", &class.model, p);
        }
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

    fn field(&self, out: &mut Out<'m>, path: &'m Path, class: &ClassFact, field: &FieldFact) {
        let index = self.index;
        if let Some(comodel) = &field.comodel {
            self.model_ref(out, path, comodel, None);
        }
        // A mixin's paths name fields of the models that inherit it.
        let mixin = class.kind == ModelKind::Abstract;
        if let Some(related) = field.related.as_ref().filter(|_| !mixin) {
            self.path(out, path, "OD203", &class.model, related);
        }
        if let (Some(comodel), Some(inverse)) = (&field.comodel, &field.inverse_name)
            && index.has_model(&comodel.value)
        {
            self.path(out, path, "OD203", &comodel.value, inverse);
        }
        for method in &field.methods {
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

    /// A type another class in scope gives the same field, when it differs from this one's.
    fn other_type(&self, class: &ClassFact, field: &FieldFact) -> Option<&'m str> {
        self.index
            .field_defs(&class.model, &field.name, Some(self.scope))
            .into_iter()
            .filter(|(_, c, _)| !std::ptr::eq(*c, class))
            .map(|(_, _, f)| f.ftype.as_str())
            .find(|t| *t != field.ftype)
    }

    fn data(&self, out: &mut Out<'m>) {
        // Files the manifest does not load, such as chart template CSVs read by Python code, are skipped.
        for file in self.module.data.iter().filter(|f| f.rank.is_some()) {
            let path = file.path.as_path();
            for r in &file.facts.refs {
                self.xml_ref(out, path, r);
            }
            for view in &file.facts.views {
                self.view(out, path, view);
            }
            for record in &file.facts.records {
                self.record(out, path, record);
            }
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

    fn undefined_ref(&self, out: &mut Out<'m>, path: &'m Path, pos: Pos, id: &str) {
        let owner = id.split_once('.').map_or("", |(m, _)| m);
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
