use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{ClassFact, FieldFact};
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

const RUNTIME_XMLIDS: &[&str] = &[
    "stock.stock_location_stock",
    "stock.stock_location_company",
    "stock.picking_type_in",
    "stock.picking_type_out",
    "stock.picking_type_internal",
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
    views: &mut FxHashMap<String, (Option<&'m str>, Option<String>)>,
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
                let inherit = view.inherit.as_deref().map(|i| full_id(name, i));
                views.insert(
                    full_id(name, id),
                    (view.model.as_ref().map(|m| m.value.as_str()), inherit),
                );
            }
        }
    }
}

/// Ids of records Odoo creates alongside others or from Python code, with the module they belong to.
fn derived_ids(modules: &[Module], by_name: &FxHashMap<&str, usize>, models: &Models) -> Vec<(String, usize)> {
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
    // Records Python code creates with an XML id, such as the locations of the first warehouse.
    for id in RUNTIME_XMLIDS {
        if let Some(&mi) = by_name.get(id.split_once('.').map_or("", |(m, _)| m)) {
            derived.push(((*id).to_owned(), mi));
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
    views: FxHashMap<String, (Option<&'m str>, Option<String>)>,
    pub models_with_access: FxHashSet<String>,
    pub has_base: bool,
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
        for (id, mi) in derived_ids(modules, &by_name, &models) {
            xmlids.entry(id).or_default().push(XmlIdDef { module: mi });
        }
        let models_with_access = access_models(modules, &model_by_xmlid);
        Self {
            modules,
            has_base: base.is_some(),
            by_name,
            models,
            xmlids,
            views,
            models_with_access,
        }
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

    pub fn has_method(&self, model: &str, method: &str, scope: &Scope) -> bool {
        BASE_METHODS.contains(&method)
            || self
                .chain(model, Some(scope))
                .iter()
                .any(|(_, c)| c.methods.iter().any(|m| m == method))
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
            let (model, inherit) = self.views.get(&id)?;
            if let Some(model) = model {
                return Some(model);
            }
            id = inherit.clone()?;
        }
        None
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
