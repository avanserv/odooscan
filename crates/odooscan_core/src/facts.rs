//! What one pass over a file learns: the facts the index needs and the diagnostics local rules raise.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Debug)]
pub struct Spanned<T> {
    pub value: T,
    pub pos: Pos,
}

/// A point on the path of a data flow. `path` is none for a point in the finding's own file.
#[derive(Clone, Debug)]
pub struct Step {
    pub path: Option<PathBuf>,
    pub pos: Pos,
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct RawDiag {
    pub code: &'static str,
    pub pos: Pos,
    pub message: String,
    /// The path a data flow takes from its source to the finding.
    pub flow: Vec<Step>,
}

impl RawDiag {
    pub fn new(code: &'static str, pos: Pos, message: impl Into<String>) -> Self {
        Self {
            code,
            pos,
            message: message.into(),
            flow: vec![],
        }
    }

    pub fn with_flow(mut self, flow: Vec<Step>) -> Self {
        self.flow = flow;
        self
    }
}

/// Byte offset to 1-based line and column, for parsers that only give offsets.
pub struct Lines {
    starts: Vec<usize>,
}

impl Lines {
    pub fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Self { starts }
    }

    pub fn pos(&self, offset: usize) -> Pos {
        let line = self.starts.partition_point(|&s| s <= offset);
        Pos {
            line: line as u32,
            col: (offset - self.starts[line - 1] + 1) as u32,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelKind {
    Model,
    Transient,
    Abstract,
}

/// The value a field definition gives an attribute, as far as a literal tells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OptionValue {
    Str(String),
    Bool(bool),
    /// A list of strings, or of tuples whose first item is a string, as selection values.
    Strs(Vec<String>),
    /// A tuple of the same, which Odoo before 19.0 does not take as a `selection` list, so a
    /// `selection_add` next to it fails.
    Tuple(Vec<String>),
    /// A dict with string keys; a value other than a string literal is none.
    Dict(Vec<(String, Option<String>)>),
    /// `None` or a zero number, which Python takes as false.
    Falsy,
    /// An expression whose value only a run tells.
    Other,
}

#[derive(Debug)]
pub struct FieldFact {
    pub name: String,
    /// Where the class body assigns the field to its name.
    pub name_pos: Pos,
    pub ftype: String,
    pub pos: Pos,
    pub comodel: Option<Spanned<String>>,
    pub inverse_name: Option<Spanned<String>>,
    pub related: Option<Spanned<String>>,
    /// Methods named by a string given to compute, inverse, search, selection or group_expand.
    pub methods: Vec<Spanned<String>>,
    /// Fields the literal `domain` of a relational field names on its comodel.
    pub domain: Vec<Spanned<String>>,
    /// Attributes given by keyword, or by position under the name the field type gives it.
    pub options: Vec<(String, Spanned<OptionValue>)>,
    /// The label of each value of literal `selection` and `selection_add` lists, none where only a
    /// run tells it.
    pub labels: Vec<Option<String>>,
    /// The call unpacks `*args` or `**kwargs`, so it may set any attribute it does not name.
    pub splat: bool,
}

impl FieldFact {
    pub fn option(&self, name: &str) -> Option<&Spanned<OptionValue>> {
        self.options.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
}

/// The fields a literal domain names, and the model it filters.
#[derive(Debug)]
pub struct DomainFact {
    pub model: String,
    pub fields: Vec<Spanned<String>>,
}

#[derive(Debug)]
pub struct ClassFact {
    pub pos: Pos,
    /// The Python name of the class.
    pub class_name: String,
    /// `_name`, else the name Odoo gives the class in the module's series, from `_inherit` and the
    /// class name.
    pub model: String,
    /// The model is not among the class's `_inherit`, so the class defines it.
    pub new_model: bool,
    pub kind: ModelKind,
    pub inherit: Vec<Spanned<String>>,
    /// `_inherit` is neither `_name`, a string, nor a list or tuple of strings only, so which models
    /// it lists, and in which order, is unknown.
    pub inherit_unknown: bool,
    /// `_inherits` and `delegate=True` fields: parent model to delegating field.
    pub inherits: Vec<(String, String)>,
    /// The `_inherits` entries whose key and field are literals: parent model to delegating field,
    /// at the entry.
    pub delegations: Vec<(String, Spanned<String>)>,
    /// `_inherits` is not a dict, or has an entry that is not two string literals, so which fields
    /// it names is unknown.
    pub inherits_unknown: bool,
    /// A Many2one may delegate to a parent `inherits` lacks: its comodel or its `delegate` is not a
    /// literal. Apart from `inherits_unknown`, which says the `_inherits` entries are unknown, as
    /// such a field still has no entry there.
    pub delegate_unknown: bool,
    pub fields: Vec<FieldFact>,
    /// Methods the class defines, in its body or under an `if`, `try` or `with` block, the names it
    /// binds to a value that may be a method, as Python finds those as it finds a method, and,
    /// in its body, `_check_company_domain`, `_table_query` and `_table_sql` when it assigns them
    /// anything but a field.
    pub methods: Vec<String>,
    /// Methods of the class decorated with `@api.private`.
    pub private_methods: Vec<String>,
    /// The undotted names given to `@api.onchange` and `@api.constrains`.
    pub paths: Vec<Spanned<String>>,
    /// The field paths each method decorated with `@api.depends` names, by method.
    pub depends: Vec<(String, Vec<Spanned<String>>)>,
    /// The fields `_order` sorts by and `_parent_name` and `_fold_name` name, which must exist on the
    /// model when Odoo reads them.
    pub order_fields: Vec<Spanned<String>>,
    /// The fields `_rec_name` and `_active_name` name, which Odoo asserts as it sets up the model.
    pub setup_fields: Vec<Spanned<String>>,
    /// The field paths of `_rec_names_search`.
    pub rec_names_search: Vec<Spanned<String>>,
    /// Methods the class's methods call through `super()`, leaving out a call inside a `hasattr`
    /// test on that same `super()` and name.
    pub super_calls: Vec<Spanned<String>>,
    /// Methods of the class that call the method they override through `super()`.
    pub extended: Vec<String>,
    /// A base class is a plain Python class, whose methods only Python knows: any base but
    /// `Model`, `TransientModel`, `AbstractModel` and `CachedModel`.
    pub plain_bases: bool,
    /// The value of `_table`, when the class sets it.
    pub table: Option<OptionValue>,
    /// `_parent_store`, `_auto`, `_log_access`, `_register`, `_date_name` and `_defaults`, when the
    /// class sets them.
    pub attrs: Vec<(String, Spanned<OptionValue>)>,
    /// Compute methods, at their name, with the fields of the class they compute and can leave
    /// unassigned on a record.
    pub unassigned_computes: Vec<(Spanned<String>, Vec<String>)>,
    /// The views `init()` creates with a literal `CREATE VIEW`.
    pub views: Vec<SqlView>,
    /// A method of the class creates a view whose whole name is `self._table`, the model's own.
    pub own_view: bool,
    /// The columns each literal definition of `_sql_constraints`, or of a call of `Constraint`,
    /// `Index` or `UniqueIndex` such as `models.Index(...)`, names, by where Odoo reads it.
    pub definitions: Vec<(DefinitionSource, Spanned<String>)>,
}

/// A view a literal `CREATE VIEW` creates.
#[derive(Debug)]
pub struct SqlView {
    /// The view's name, None for a placeholder such as `%s`.
    pub table: Option<String>,
    /// The names of its columns, None when the text does not spell each one.
    pub names: Option<Vec<String>>,
    pub pos: Pos,
}

/// Where a constraint or an index definition comes from, which decides the series Odoo reads it in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefinitionSource {
    /// An entry of `_sql_constraints`, which Odoo reads before 19.0.
    SqlConstraints,
    /// A `Constraint`, `Index` or `UniqueIndex` attribute, such as `models.Index(...)`, from 19.0.
    TableObject,
}

impl ClassFact {
    pub fn attr(&self, name: &str) -> Option<&Spanned<OptionValue>> {
        self.attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    /// Whether Odoo adds the class to the registry. Odoo tests the truth of `_register` in the
    /// class's own body only, so a subclass that leaves it unset is registered; only `False`,
    /// `None` and zero count here as false.
    pub fn registered(&self) -> bool {
        !self
            .attr("_register")
            .is_some_and(|r| matches!(r.value, OptionValue::Bool(false) | OptionValue::Falsy))
    }

    /// Whether the class has a parent the scan cannot see, which may give it fields and methods: a
    /// plain Python base class, or an `_inherit` element that is not a literal.
    pub fn hidden_parents(&self) -> bool {
        self.plain_bases || self.inherit_unknown
    }
}

/// The methods a call can run: those of a model, or of the same Python class in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Receiver {
    Model(String),
    Class(String),
}

#[derive(Debug, PartialEq, Eq)]
pub struct CallSite {
    pub receiver: Receiver,
    pub method: String,
    pub pos: Pos,
}

/// Where a tainted value comes from within a function: a source, a parameter (by position, without
/// `self`), or what the call of that index in `FlowSummary::calls` returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowOrigin {
    Source,
    Param(usize),
    Return(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgSlot {
    Pos(usize),
    Name(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlowTarget {
    /// The call a rule reports, by the rule's code.
    Sink(&'static str),
    Arg(usize, ArgSlot),
    Return,
}

#[derive(Debug)]
pub struct FlowEdge {
    pub origin: FlowOrigin,
    pub target: FlowTarget,
    /// The user still picks the keys of the value at the target, if they picked those of the origin.
    pub keys: bool,
    /// The origin, each local in between, and for a sink the call that receives the value.
    pub steps: Vec<Spanned<String>>,
}

/// How data moves through one function, for the flows that cross into the methods it calls.
#[derive(Debug)]
pub struct FlowSummary {
    /// What `self` is in the function: a model, or a Python class of the file.
    pub owner: Option<Receiver>,
    pub method: String,
    /// Parameter names in order, without `self`.
    pub params: Vec<String>,
    pub calls: Vec<CallSite>,
    pub edges: Vec<FlowEdge>,
}

/// An attribute an import-time statement sets on what the file imports from `odoo`.
#[derive(Debug)]
pub struct ImportPatch {
    pub pos: Pos,
    /// The object as written, `name...` when an item or a call breaks the chain.
    pub object: String,
    /// `...` when the code computes it.
    pub attr: String,
    /// The object's dotted path from `odoo`, such as `odoo.addons.sale.models.sale_order.SaleOrder`.
    pub path: String,
}

/// A `table.column` or `alias.column` reference of a literal query, on the table the query binds.
#[derive(Debug)]
pub struct SqlColumn {
    pub table: String,
    pub column: String,
    /// The reference as written.
    pub text: String,
    pub pos: Pos,
    /// The query runs in `init()` or `_auto_init()`, ahead of its module's data.
    pub before_data: bool,
}

#[derive(Debug, Default)]
pub struct PyFacts {
    pub classes: Vec<ClassFact>,
    /// A class names its model with an expression, so models and their ids are built at runtime,
    /// or lacks `_name` in a module of unknown series and is named apart before and from saas~18.1.
    pub dynamic_models: bool,
    /// Literal model names in `env['x']` outside code guarded by a test that the model exists.
    pub env_models: Vec<Spanned<String>>,
    /// Literal domains given to `search()` and the like on `self` or `env['x']`.
    pub domains: Vec<DomainFact>,
    /// XML ids named by `env.ref()`.
    pub xml_refs: Vec<XmlRef>,
    /// Groups named by `has_group()` and the `groups` of fields, which may come from modules
    /// that are not installed.
    pub group_refs: Vec<XmlRef>,
    /// Base names of `.xml` and `.csv` files named in string literals, which Python code may load.
    pub file_mentions: Vec<String>,
    /// Functions whose parameters or calls carry data a caller or callee may have from a user.
    pub flows: Vec<FlowSummary>,
    /// The qualified columns of the queries `SQL()` spells out, and of those an Odoo cursor runs.
    pub sql_columns: Vec<SqlColumn>,
    /// The table and the column each literal `ALTER TABLE ... ADD COLUMN` or `create_column()`
    /// adds by hand, as modules do for a field Odoo does not store.
    pub added_columns: Vec<(String, String)>,
    /// The names of the views `CREATE VIEW` queries create, wherever they run, when the literal
    /// head of the query spells them.
    pub created_views: Vec<String>,
    /// A `CREATE VIEW` outside the `init()` or `_auto_init()` of a class names its view at runtime
    /// with something other than `self._table` whole in a model class, so it may create any view.
    pub creates_any_view: bool,
    pub diags: Vec<RawDiag>,
    /// OD121 on the rollbacks of `ir.http._handle_exception()`, Odoo's hook for a failed request
    /// only before 16.0, which the series of the module decides.
    pub legacy_hook_rollbacks: Vec<RawDiag>,
    /// OD120 on each name an `ormcache` key reads that the method does not bind, keyed by the name:
    /// `checks.rs` drops those that Odoo's cache module defines in the module's series.
    pub cache_key_names: Vec<(String, RawDiag)>,
    /// What OD123 reports, whose advice depends on whether the index knows the object as a model
    /// class.
    pub import_patches: Vec<ImportPatch>,
}

#[derive(Debug)]
pub struct XmlDef {
    pub id: String,
    pub model: Option<String>,
    pub pos: Pos,
    /// A `<template>` or `<asset>`, whose model is another one in a `theme_` module.
    pub themed: bool,
    /// The id is given to an existing record, which a `<record>` with that id may then update.
    pub assigned: bool,
}

#[derive(Debug)]
pub struct XmlRef {
    pub id: String,
    pub pos: Pos,
}

/// A `<field>` in a view arch. `path` holds the x2many fields whose subviews enclose it.
#[derive(Debug)]
pub struct ArchField {
    pub path: Vec<String>,
    pub pos: Pos,
}

/// Fields a literal domain or `group_by` in an arch names, on the model of the arch position `path`
/// (the x2many fields whose subviews enclose it, as in `ArchField`).
#[derive(Debug)]
pub struct ArchDomain {
    pub path: Vec<String>,
    pub fields: Vec<Spanned<String>>,
}

/// An arch element as an inheriting view locates it: its tag and its `name`, `id` or `t-call`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArchElement {
    pub tag: String,
    pub attr: String,
    pub value: String,
}

/// A `<button type="object">` in a view arch, which calls `method` on the model of the arch position
/// `path` (the x2many fields whose subviews enclose it, as in `ArchField`).
#[derive(Debug)]
pub struct ArchButton {
    pub path: Vec<String>,
    pub method: Spanned<String>,
}

/// An expression of a view modifier, or the text an `<attribute>` spec gives one, at its arch
/// position `path` (the x2many fields whose subviews enclose it, as in `ArchField`).
#[derive(Debug)]
pub struct ArchModifier {
    pub path: Vec<String>,
    /// The subviews its own arch shows around it, innermost last: those the arch opens and those an
    /// xpath names on its way in.
    pub subviews: Vec<ArchSubview>,
    pub attr: &'static str,
    pub value: Spanned<String>,
    /// The element an `add` joins this value to, unless the spec locates none or a `remove` may
    /// empty what it joins.
    pub joins: Option<ArchElement>,
}

/// An element an arch adds, by each name that locates it, with the modifiers it gives a non-empty
/// value.
#[derive(Debug)]
pub struct ArchValued {
    pub elements: Vec<ArchElement>,
    pub attrs: Vec<&'static str>,
}

/// A modifier an `<attribute>` spec may leave empty, on the element the spec names. A spec that names
/// none may reach any element.
#[derive(Debug)]
pub struct ArchEmptied {
    pub attr: &'static str,
    pub target: Option<ArchElement>,
}

/// A subview at its arch position: opened inline, on a `<groupby>` or inside a field a spec locates,
/// which keeps the locator in its `path`, or only named by an xpath, whose arch lacks its content.
#[derive(Clone, Debug)]
pub struct ArchSubview {
    pub path: Vec<String>,
    pub tag: String,
    /// The fields it loads without showing them in a `<field>`.
    pub attribute_fields: Vec<String>,
}

#[derive(Debug, Default)]
pub struct ViewFact {
    pub id: Option<String>,
    pub model: Option<Spanned<String>>,
    pub inherit: Option<String>,
    pub fields: Vec<ArchField>,
    /// The view's arch was read, so `elements` is complete.
    pub has_arch: bool,
    /// Elements the arch adds, which views extending it can locate.
    pub elements: Vec<ArchElement>,
    /// The elements the arch's own tags add, with their modifiers.
    pub valued: Vec<ArchValued>,
    /// What the arch's `<attribute>` specs may leave empty.
    pub emptied: Vec<ArchEmptied>,
    /// Elements the arch's specs must find in the parent views.
    pub locators: Vec<Spanned<ArchElement>>,
    pub domains: Vec<ArchDomain>,
    pub buttons: Vec<ArchButton>,
    pub modifiers: Vec<ArchModifier>,
    pub subviews: Vec<ArchSubview>,
    /// Literal targets of `t-call`, in the arch or set by an `<attribute>` spec.
    pub calls: Vec<Spanned<String>>,
    /// The view's type, as its `type` field or the root of its arch says. A root that Odoo does not
    /// take as a type, such as an inherited view's `<xpath>`, says nothing.
    pub kind: Option<String>,
    /// Whether the view can be the default of its model and type, which only a `primary` one is.
    /// None when only a run can tell.
    pub primary: Option<bool>,
    /// `inherit_id` holds a parent `inherit` does not read, so what the view takes from it, such as
    /// its type, is out of reach.
    pub unread_parent: bool,
}

#[derive(Debug)]
pub struct RecordFact {
    pub model: Spanned<String>,
    pub fields: Vec<Spanned<String>>,
}

/// What one `<field>` or CSV cell writes, as far as its text tells without a run.
#[derive(Debug)]
pub enum WrittenValue {
    /// XML ids of records of the field's comodel.
    Ids(Vec<Spanned<String>>),
    Text(Spanned<String>),
    /// The name of the `ir.model` a `search` finds.
    Model(Spanned<String>),
    Opaque,
}

#[derive(Debug)]
pub struct FieldWrite {
    pub name: String,
    pub value: WrittenValue,
    /// It adds to what the field holds rather than replacing it: x2many commands that keep the
    /// other rows, or records nested in the field.
    pub incremental: bool,
}

/// One write of a data file to a record: a `<record>` or a CSV row.
#[derive(Debug)]
pub struct RecordWrite {
    pub id: Option<String>,
    pub model: String,
    pub pos: Pos,
    /// The value each field ends on: the last `<field>` of a name replaces those before.
    pub fields: Vec<FieldWrite>,
    /// The model and the field whose `<field>` encloses the record.
    pub nested_in: Option<(String, String)>,
    pub csv: bool,
    /// The fields a `default_*` key names in the `context` of the record or of the roots around it.
    pub context_defaults: Vec<String>,
    /// `forcecreate` is false, so the author expects the record to exist.
    pub no_create: bool,
}

/// A row that grants access: a row of `ir.access` with no group restricts instead.
#[derive(Debug)]
pub struct AccessRow {
    pub id: Option<String>,
    /// An `ir.model` XML id, or a model name when `by_name` (the `ir.access` CSV format).
    pub model: String,
    pub by_name: bool,
    pub pos: Pos,
    /// A CSV row with no group gives every user a permission other than read.
    pub global_write: bool,
}

/// A `<function>` of a data file, which Odoo calls when it loads the file.
#[derive(Debug)]
pub struct FunctionCall {
    pub model: String,
    pub method: Spanned<String>,
    /// The model and the field of a literal `ir.default.set()`.
    pub default_of: Option<(String, String)>,
}

/// What one element writes to a field of a window action or a record rule. A write that is no
/// literal still replaces what an earlier one gave, so it differs from no write at all.
#[derive(Debug, Default)]
pub enum ActionValue {
    #[default]
    Unset,
    Literal(Spanned<String>),
    Opaque,
}

/// One write to a window action: a `<record>`, an `<act_window>`, or a row of its `view_ids` in a
/// `<record>` of its own, whose `id` here is that of the action.
#[derive(Debug, Default)]
pub struct ActionFact {
    pub id: Option<String>,
    pub model: ActionValue,
    pub view_mode: ActionValue,
    /// The types it names a view for, through `view_ids` or a `<kind>_view_ref` context key.
    pub view_kinds: Vec<String>,
    /// It names a view without saying the type: a `view_id`, or `view_ids` that an `eval` hides.
    pub view_of_any_kind: bool,
}

impl ActionFact {
    pub fn names_view(&self, kind: &str) -> bool {
        self.view_of_any_kind || self.view_kinds.iter().any(|k| k == kind)
    }
}

/// One write to an `ir.rule`, or to an `ir.access` with a domain, by a `<record>` or a CSV row.
#[derive(Debug)]
pub struct RuleFact {
    pub id: Option<String>,
    /// What it gives `model_id`: an `ir.model` XML id, or a model name when `by_name`.
    pub model: ActionValue,
    pub by_name: bool,
    pub domain: ActionValue,
    /// The field paths `domain` names, which a domain only a run can read leaves empty.
    pub fields: Vec<Spanned<String>>,
    /// What it gives `active`, where a value only a run tells counts as false.
    pub active: Option<bool>,
}

/// One write to an `ir.actions.server` or an `ir.cron`, by a `<record>`.
#[derive(Debug)]
pub struct ServerFact {
    pub id: Option<String>,
    pub cron: bool,
    /// What it gives `model_id`: an `ir.model` XML id, or a model name when `by_name`.
    pub model: ActionValue,
    pub by_name: bool,
    /// Whether what it gives `state` is `code`, where a value only a run tells counts as false.
    pub state: Option<bool>,
    /// It gives `code` a value, which replaces the last one even when no call can be read from it.
    pub writes_code: bool,
    /// Why Odoo refuses the code, in any `state`.
    pub refused: Option<Spanned<String>>,
    /// The methods the code calls on the records of `model_id`.
    pub calls: Vec<Spanned<String>>,
}

/// One write to an `ir.actions.report`, by a `<record>` or a `<report>` tag.
#[derive(Debug)]
pub struct ReportFact {
    pub id: Option<String>,
    /// It comes from a `<report>` tag, which only some series read.
    pub tag: bool,
    /// What it gives `report_name`, where a name no scan can look up is opaque.
    pub template: ActionValue,
    /// Whether what it gives `report_type` renders QWeb, where a value only a run tells counts as false.
    pub qweb: Option<bool>,
    /// It gives `print_report_name` a value, which replaces the last one even when it is empty.
    pub writes_file_name: bool,
    /// Why Odoo refuses what it gives `print_report_name`.
    pub refused: Option<Spanned<String>>,
}

/// Facts from an XML or CSV data file.
#[derive(Debug, Default)]
pub struct DataFacts {
    /// The file is Odoo data (an `<odoo>` document or a CSV), not an asset template.
    pub is_data: bool,
    pub defs: Vec<XmlDef>,
    pub refs: Vec<XmlRef>,
    pub views: Vec<ViewFact>,
    /// The `key` of each view or page, a name `t-call` finds apart from the XML id.
    pub view_keys: Vec<String>,
    pub records: Vec<RecordFact>,
    pub writes: Vec<RecordWrite>,
    /// Domains and `group_by` fields of window actions, on their `res_model`.
    pub action_domains: Vec<DomainFact>,
    pub actions: Vec<ActionFact>,
    pub access: Vec<AccessRow>,
    pub rules: Vec<RuleFact>,
    pub servers: Vec<ServerFact>,
    pub reports: Vec<ReportFact>,
    pub functions: Vec<FunctionCall>,
    pub diags: Vec<RawDiag>,
}

/// What the leading comment of a JS file says about it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Annotation {
    #[default]
    None,
    /// `@odoo-module`: an ES module from 15.0 on, when unannotated files are still scripts.
    Module,
    /// `@odoo-module ignore`: never an ES module, such as a library under `static/src`.
    Ignore,
}

/// A name a JS file imports, or re-exports, from another module.
#[derive(Debug)]
pub struct JsName {
    pub specifier: String,
    /// The exported name asked for; `default` for a default import.
    pub name: Spanned<String>,
    /// The binding an import creates; none for `export { x } from`.
    pub local: Option<String>,
}

/// What a class extends.
#[derive(Debug, PartialEq, Eq)]
pub enum Heritage {
    None,
    Name(String),
    /// An expression, such as a mixin call, whose class only runs tell.
    Other,
}

#[derive(Debug)]
pub struct JsClass {
    pub name: String,
    pub extends: Heritage,
    /// Instance methods, accessors and fields.
    pub members: Vec<String>,
    /// The Owl template a literal `static template` or `Class.template = "..."` names.
    pub template: Option<Spanned<String>>,
}

/// `patch(Target.prototype, { ... })`.
#[derive(Debug)]
pub struct JsPatch {
    pub target: String,
    pub members: Vec<String>,
    /// Methods the patch calls through `super`.
    pub supers: Vec<Spanned<String>>,
}

/// A call of the `orm` service with a literal model or method, such as `this.orm.call("x", "m", ...)`.
#[derive(Debug)]
pub struct OrmCall {
    /// The model when it is a literal; a call naming a private method fails whatever the model.
    pub model: Option<Spanned<String>>,
    /// The method `call` names; the other service methods map to methods every model has.
    pub method: Option<Spanned<String>>,
}

#[derive(Debug, Default)]
pub struct JsFacts {
    pub annotation: Annotation,
    /// Every module specifier the file imports or re-exports from, at its first use.
    pub specifiers: Vec<Spanned<String>>,
    pub names: Vec<JsName>,
    /// Names the file exports itself or re-exports, `default` among them.
    pub exports: Vec<String>,
    /// Exported names of the file's own bindings, with the binding.
    pub export_locals: Vec<(String, String)>,
    /// `export { imported as exported } from "specifier"`, as (exported, specifier, imported).
    pub reexports: Vec<(String, String, String)>,
    /// Specifiers of `export * from`, whose names the file exports too.
    pub star_exports: Vec<String>,
    /// Module names given to `odoo.define()`, which a hand-written file registers under.
    pub defines: Vec<String>,
    pub classes: Vec<JsClass>,
    pub patches: Vec<JsPatch>,
    pub orm_calls: Vec<OrmCall>,
    pub syntax_error: Option<Spanned<String>>,
}

/// A template of a bundle's XML file, the child of its root element.
#[derive(Debug)]
pub struct OwlTemplate {
    /// The name it registers; none for an extension, which modifies its parent in place.
    pub name: Option<String>,
    pub inherit: Option<Spanned<String>>,
    /// The elements it adds and the elements its specs locate, as in a view arch.
    pub arch: ViewFact,
}

#[derive(Debug, Default)]
pub struct TemplateFacts {
    pub templates: Vec<OwlTemplate>,
}
