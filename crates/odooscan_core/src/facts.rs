//! What one pass over a file learns: the facts the index needs and the diagnostics local rules raise.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Debug)]
pub struct Spanned<T> {
    pub value: T,
    pub pos: Pos,
}

#[derive(Debug)]
pub struct RawDiag {
    pub code: &'static str,
    pub pos: Pos,
    pub message: String,
}

impl RawDiag {
    pub fn new(code: &'static str, pos: Pos, message: impl Into<String>) -> Self {
        Self {
            code,
            pos,
            message: message.into(),
        }
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

#[derive(Debug)]
pub struct FieldFact {
    pub name: String,
    pub ftype: String,
    pub pos: Pos,
    pub comodel: Option<Spanned<String>>,
    pub inverse_name: Option<Spanned<String>>,
    pub related: Option<Spanned<String>>,
    /// Names given to compute, inverse and search.
    pub methods: Vec<Spanned<String>>,
    /// Fields the literal `domain` of a relational field names on its comodel.
    pub domain: Vec<Spanned<String>>,
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
    /// `_inherits`: parent model to delegating field.
    pub inherits: Vec<(String, String)>,
    pub fields: Vec<FieldFact>,
    pub methods: Vec<String>,
    /// Field paths given to `@api.depends`, `@api.onchange` and `@api.constrains`.
    pub paths: Vec<Spanned<String>>,
    /// The fields `_order` sorts by and `_rec_name` names, which must exist on the model.
    pub order_fields: Vec<Spanned<String>>,
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
    pub diags: Vec<RawDiag>,
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
    pub attr: &'static str,
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
