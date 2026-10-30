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
    pub model: String,
    /// The class declares `_name` and that name is not among its `_inherit`.
    pub new_model: bool,
    pub kind: ModelKind,
    pub inherit: Vec<Spanned<String>>,
    /// `_inherits` and `delegate=True` fields: parent model to delegating field.
    pub inherits: Vec<(String, String)>,
    /// The delegating fields the `_inherits` dict names, at their entry.
    pub delegations: Vec<Spanned<String>>,
    pub fields: Vec<FieldFact>,
    /// Methods the class defines, and `_check_company_domain`, `_table_query` and `_table_sql`
    /// when it assigns them.
    pub methods: Vec<String>,
    /// The undotted names given to `@api.onchange` and `@api.constrains`.
    pub paths: Vec<Spanned<String>>,
    /// The field paths each method decorated with `@api.depends` names, by method.
    pub depends: Vec<(String, Vec<Spanned<String>>)>,
    /// The fields `_order` sorts by and `_rec_name`, `_parent_name`, `_fold_name` and `_active_name`
    /// name, which must exist on the model.
    pub order_fields: Vec<Spanned<String>>,
    /// The field paths of `_rec_names_search`.
    pub rec_names_search: Vec<Spanned<String>>,
    /// Methods the class's methods call through `super()`.
    pub super_calls: Vec<Spanned<String>>,
    /// A base class is a plain Python class, whose methods only Python knows.
    pub plain_bases: bool,
    /// The value of `_table`, when the class sets it.
    pub table: Option<OptionValue>,
    /// `_parent_store`, `_auto`, `_log_access` and `_register`, when the class sets them.
    pub attrs: Vec<(String, Spanned<OptionValue>)>,
    /// Compute methods, at their name, with the fields of the class they compute and can leave
    /// unassigned on a record.
    pub unassigned_computes: Vec<(Spanned<String>, Vec<String>)>,
    /// The views `init()` creates with a literal `CREATE VIEW`.
    pub views: Vec<SqlView>,
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

/// A `table.column` or `alias.column` reference of a literal query, on the table the query binds.
#[derive(Debug)]
pub struct SqlColumn {
    pub table: String,
    pub column: String,
    /// The reference as written.
    pub text: String,
    pub pos: Pos,
}

#[derive(Debug, Default)]
pub struct PyFacts {
    pub classes: Vec<ClassFact>,
    /// A class names its model with an expression, so models and their ids are built at runtime.
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
    /// A `CREATE VIEW` outside the `init()` or `_auto_init()` of a model names its view at runtime,
    /// so it may create any view.
    pub creates_any_view: bool,
    pub diags: Vec<RawDiag>,
    /// OD121 on the rollbacks of `ir.http._handle_exception()`, Odoo's hook for a failed request
    /// only before 16.0, which the series of the module decides.
    pub legacy_hook_rollbacks: Vec<RawDiag>,
}

#[derive(Debug)]
pub struct XmlDef {
    pub id: String,
    pub model: Option<String>,
    pub pos: Pos,
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

/// An arch element as an inheriting view locates it: its tag and its `name` or `id`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArchElement {
    pub tag: String,
    pub attr: String,
    pub value: String,
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
    /// Elements the arch's specs (`position` elements and xpaths) must find in the parent views.
    pub locators: Vec<Spanned<ArchElement>>,
    pub domains: Vec<ArchDomain>,
}

#[derive(Debug)]
pub struct RecordFact {
    pub model: Spanned<String>,
    pub fields: Vec<Spanned<String>>,
}

#[derive(Debug)]
pub struct AccessRow {
    /// An `ir.model` XML id, or a model name when `by_name` (the `ir.access` CSV format).
    pub model: String,
    pub by_name: bool,
}

/// Facts from an XML or CSV data file.
#[derive(Debug, Default)]
pub struct DataFacts {
    /// The file is Odoo data (an `<odoo>` document or a CSV), not an asset template.
    pub is_data: bool,
    pub defs: Vec<XmlDef>,
    pub refs: Vec<XmlRef>,
    pub views: Vec<ViewFact>,
    pub records: Vec<RecordFact>,
    /// Domains and `group_by` fields of window actions, on their `res_model`.
    pub action_domains: Vec<DomainFact>,
    pub access: Vec<AccessRow>,
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
