//! What one pass over a file learns: the facts the index needs and the diagnostics local rules raise.

use std::path::PathBuf;

use ruff_text_size::TextSize;

use crate::fix::Fix;

/// The type of a field the scan cannot tell: an `ir.model.fields` record gives no `ttype` it can
/// read, or a field class's name stands for two types.
pub(crate) const UNREAD_TYPE: &str = "Unread";

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
    pub fix: Option<Fix>,
}

impl RawDiag {
    pub fn new(code: &'static str, pos: Pos, message: impl Into<String>) -> Self {
        Self {
            code,
            pos,
            message: message.into(),
            flow: vec![],
            fix: None,
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
    /// A tuple of the same, which Odoo before saas~18.1 does not take as a `selection` list, so a
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
    /// The `domain` is a string rather than a list.
    pub domain_string: bool,
    /// Attributes given by keyword, or by position under the name the field type gives it.
    pub options: Vec<(String, Spanned<OptionValue>)>,
    /// The label of each value of literal `selection` and `selection_add` lists, none where only a
    /// run tells it.
    pub labels: Vec<Option<String>>,
    /// The call unpacks `*args` or `**kwargs`, so it may set any attribute it does not name, passes
    /// a field of unread type arguments by position, which name no known attribute, or there is no
    /// call to read: a key of `_columns`.
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
    /// `_inherit` is neither `_name`, a string, nor a list or tuple of strings only, or the class is
    /// `unread`, so which models it lists, and in which order, is unknown. Also set below 10.0 and
    /// on an unknown series by a `_columns` that is no dict, or has a `**` or computed key, as its
    /// fields are as unknown.
    pub inherit_unknown: bool,
    /// Read from the tokens of a file that does not parse, so the model and `table` are known and
    /// nothing else.
    pub unread: bool,
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
    /// The `def`s of the class body, outside `if`, `try` and `with` blocks, in order.
    pub defs: Vec<MethodDef>,
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
    /// test on that same `super()` and name, and `__getattr__` called from `__getattr__`.
    pub super_calls: Vec<Spanned<String>>,
    /// Methods of the class that call the method they override through `super()`.
    pub extended: Vec<String>,
    /// The base classes that are plain Python classes, whose methods only Python knows: any base
    /// but `Model`, `TransientModel`, `AbstractModel`, `CachedModel` and the `osv` aliases of the
    /// first three. Each is its bare name, or empty for a base a class of the file cannot be, such
    /// as `Layout()` or `mixins.Layout`.
    pub plain_bases: Vec<String>,
    /// The entries of `plain_bases` the scan does not read: all but the classes its file defines
    /// above it that can hold no field. Those still give the class methods, so they stay in
    /// `plain_bases`.
    pub opaque_bases: Vec<String>,
    /// The names the `plain_bases` the scan reads bind, other than those starting with `__`.
    pub base_methods: Vec<String>,
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
    /// A regular model class of unknown model, with nothing read from its body.
    pub fn new(pos: Pos, class_name: String) -> Self {
        Self {
            pos,
            class_name,
            model: String::new(),
            new_model: false,
            kind: ModelKind::Model,
            inherit: vec![],
            inherit_unknown: false,
            unread: false,
            inherits: vec![],
            delegations: vec![],
            inherits_unknown: false,
            delegate_unknown: false,
            fields: vec![],
            methods: vec![],
            defs: vec![],
            private_methods: vec![],
            paths: vec![],
            depends: vec![],
            order_fields: vec![],
            setup_fields: vec![],
            rec_names_search: vec![],
            super_calls: vec![],
            extended: vec![],
            table: None,
            attrs: vec![],
            unassigned_computes: vec![],
            views: vec![],
            own_view: false,
            definitions: vec![],
            plain_bases: vec![],
            opaque_bases: vec![],
            base_methods: vec![],
        }
    }

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
    /// plain Python base class, an `_inherit` element that is not a literal, or a `_columns` read no
    /// better than one.
    pub fn hidden_parents(&self) -> bool {
        !self.plain_bases.is_empty() || self.inherit_unknown
    }

    /// Whether a plain base the scan reads binds `name`, which the class then has unless it binds
    /// the name itself.
    pub fn base_binds(&self, name: &str) -> bool {
        self.base_methods.iter().any(|m| m == name)
    }

    /// The signature `name` has on the class, when every binding of it is a `def` of the body. The
    /// last one wins, as Python binds each in turn; any other binding may hold any callable.
    pub fn signature(&self, name: &str) -> Option<&Signature> {
        let mut defs = self.defs.iter().filter(|d| d.name.value == name);
        let bindings = self.methods.iter().filter(|m| *m == name).count();
        if bindings == 0 || defs.clone().count() != bindings {
            return None;
        }
        defs.next_back()?.signature.as_ref()
    }
}

/// A `def` of a class body.
#[derive(Debug)]
pub struct MethodDef {
    pub name: Spanned<String>,
    /// Each decorator dotted as written, a call by what it calls: `api.depends` for `@api.depends("x")`.
    pub decorators: Vec<String>,
    /// None when a decorator may change the arguments the method takes: any but those of `api`,
    /// `staticmethod`, `classmethod`, `override`, `ormcache` and `ormcache_context`.
    pub signature: Option<Signature>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    /// The default as the source spells it.
    pub default: Option<String>,
}

/// The parameters of a `def`, as a call through an instance fills them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Signature {
    /// The parameter the instance or the class fills, none for a `staticmethod` or a `def` that takes
    /// nothing by position, whose `*args` then takes it.
    pub bound: Option<String>,
    /// The parameters a call can fill by position, after `bound`.
    pub positional: Vec<Param>,
    /// How many of `positional`, from the first, a call cannot fill by name.
    pub positional_only: usize,
    pub var_positional: Option<String>,
    pub keyword_only: Vec<Param>,
    pub var_keyword: Option<String>,
}

/// Why an override refuses a call its parent takes, by the parameter that makes it fail.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal<'s> {
    /// A positional the parent requires, which the override has no place for by position.
    Lost(&'s Param),
    /// A parameter the override requires, which no call of the parent's form passes.
    Required(&'s Param),
    /// A keyword-only parameter of the parent, which the override cannot take by name.
    Keyword(&'s Param),
}

impl Signature {
    /// Why a call that the parent takes, and that passes every parameter the parent requires,
    /// fails on this override. Dropped optional parameters, defaults made required and renamed
    /// positionals are left out: core overrides do all three where no caller passes them.
    pub fn refuses<'s>(&'s self, parent: &'s Signature) -> Option<Refusal<'s>> {
        self.lost(parent)
            .or_else(|| self.required(parent))
            .or_else(|| self.keyword(parent))
    }

    fn lost<'s>(&self, parent: &'s Signature) -> Option<Refusal<'s>> {
        if self.var_positional.is_some() {
            return None;
        }
        let mut past = parent.positional.iter().skip(self.positional.len());
        past.find(|p| p.default.is_none()).map(Refusal::Lost)
    }

    /// A parent's `*args` says its callers may pass more by position, and its `**kwargs` by name.
    /// A positional the parent takes by name alone comes in by name; a call that omits it is one
    /// for a default made required.
    fn required<'s>(&'s self, parent: &Signature) -> Option<Refusal<'s>> {
        let named = |p: &Param| parent.keyword_only.iter().any(|k| k.name == p.name);
        let past = self.positional.iter().enumerate().skip(parent.positional.len());
        let positional = past
            .filter(|_| parent.var_positional.is_none())
            .find(|(i, p)| p.default.is_none() && !(*i >= self.positional_only && named(p)))
            .map(|(_, p)| p);
        let keyword = || {
            let unknown = |p: &&Param| p.default.is_none() && !parent.takes_name(&p.name);
            self.keyword_only
                .iter()
                .filter(|_| parent.var_keyword.is_none())
                .find(unknown)
        };
        positional.or_else(keyword).map(Refusal::Required)
    }

    /// A renamed keyword-only parameter shows as one the override adds, next to the one it lacks.
    fn keyword<'s>(&self, parent: &'s Signature) -> Option<Refusal<'s>> {
        if self.var_keyword.is_some() {
            return None;
        }
        let adds = self.keyword_only.iter().any(|p| !parent.takes_name(&p.name));
        let lacked = |p: &&Param| (p.default.is_none() || adds) && !self.takes_name(&p.name);
        parent.keyword_only.iter().find(lacked).map(Refusal::Keyword)
    }

    /// Whether a call through an instance that passes `count` arguments by position, and none by
    /// name, fills every parameter the `def` requires.
    pub fn takes_positional(&self, count: usize) -> bool {
        let room = self.var_positional.is_some() || self.positional.len() >= count;
        let filled = |p: &Param| p.default.is_some();
        room && self.positional.iter().skip(count).all(filled) && self.keyword_only.iter().all(filled)
    }

    /// Whether a call can pass `name` by keyword to a parameter of its own, `**` aside.
    fn takes_name(&self, name: &str) -> bool {
        let by_position = self.positional.iter().skip(self.positional_only);
        by_position.chain(&self.keyword_only).any(|p| p.name == name)
    }

    /// The `def` line of `method` without the annotations, such as `_prepare(self, order, line=None)`.
    pub fn spelled(&self, method: &str) -> String {
        // A default may span lines, which a one-line message cannot hold.
        let param = |p: &Param| match &p.default {
            Some(d) => format!("{}={}", p.name, d.split_whitespace().collect::<Vec<_>>().join(" ")),
            None => p.name.clone(),
        };
        let mut parts: Vec<String> = self.bound.iter().cloned().collect();
        parts.extend(self.positional[..self.positional_only].iter().map(param));
        if self.positional_only > 0 {
            parts.push("/".to_owned());
        }
        parts.extend(self.positional[self.positional_only..].iter().map(param));
        match &self.var_positional {
            Some(name) => parts.push(format!("*{name}")),
            None if !self.keyword_only.is_empty() => parts.push("*".to_owned()),
            None => {}
        }
        parts.extend(self.keyword_only.iter().map(param));
        parts.extend(self.var_keyword.iter().map(|name| format!("**{name}")));
        format!("{method}({})", parts.join(", "))
    }
}

/// The methods a call can run: those of a model, or of the same Python class in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Receiver {
    Model(String),
    Class(String),
}

/// Whose rights a called method runs with, as the receiver of the call shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunsAs {
    /// Under `sudo()`, `with_user(SUPERUSER_ID)` or an environment built for the superuser, with
    /// no later call that leaves superuser mode.
    Superuser,
    /// On `self` or the records of calls on it, which keep the caller's environment, as
    /// `with_user()` for a user and `sudo(False)` do not.
    AsCaller,
    User,
}

#[derive(Debug, PartialEq, Eq)]
pub struct CallSite {
    pub receiver: Receiver,
    pub method: String,
    pub pos: Pos,
    pub runs_as: RunsAs,
}

/// Where a tainted value comes from within a function: a source, a parameter (by position, without
/// `self`), or what the call of that index in `FlowSummary::calls` returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowOrigin {
    Source,
    Param(usize),
    /// A parameter of a public model method: a source when a client calls the method over RPC,
    /// and the caller's argument when Python code calls it.
    Rpc(usize),
    Return(usize),
}

impl FlowOrigin {
    /// Whether the user sets the value on entry to the function, so that a flow from it is a
    /// finding there.
    pub fn is_user(self) -> bool {
        matches!(self, Self::Source | Self::Rpc(_))
    }

    /// The parameter a caller's argument fills.
    pub fn param(self) -> Option<usize> {
        match self {
            Self::Param(i) | Self::Rpc(i) => Some(i),
            _ => None,
        }
    }
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
    /// `write()` or `create()` on the records of a model method, which run as superuser when the
    /// caller called the method under `sudo()`.
    SelfWrite,
    /// A call a rule reports, by the rule's code, only when the scanned modules grant public or
    /// portal users the right it needs, as a public attachment created with the user's rights.
    Granted(&'static str),
}

/// The OD7 codes a tainted value is harmless for, as bits by the number after `OD7`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Safe(u32);

impl Safe {
    pub fn of(codes: &[&str]) -> Self {
        Self(codes.iter().fold(0, |bits, code| bits | bit(code)))
    }

    pub fn has(self, code: &str) -> bool {
        self.0 & bit(code) != 0
    }

    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

fn bit(code: &str) -> u32 {
    code.strip_prefix("OD7")
        .and_then(|n| n.parse::<u32>().ok())
        .and_then(|n| 1u32.checked_shl(n))
        .unwrap_or(0)
}

#[derive(Debug)]
pub struct FlowEdge {
    pub origin: FlowOrigin,
    pub target: FlowTarget,
    /// The user still picks the keys of the value at the target, if they picked those of the origin.
    pub keys: bool,
    /// The value is harmless at the target for these codes, whatever it is at the origin.
    pub safe: Safe,
    /// The value is a file of the uploads picked by a field the code names.
    pub upload: bool,
    /// The value at the target is the `name` of the origin's, so a picked file's field.
    pub named: bool,
    /// The value at the target is the origin's own, copied but not derived, on every path.
    pub own: bool,
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

/// A class that is no model and has a base, so it may be a field class: Odoo takes any instance of
/// a subclass of `fields.Field` as a field of the class that holds it.
#[derive(Debug)]
pub struct FieldClass {
    pub name: String,
    /// The field type a base of the ORM's `fields` proves.
    pub ftype: Option<String>,
    /// The last name of each other base, which only the whole scan can resolve.
    pub bases: Vec<String>,
}

/// What a route's own decorator gives one key of its routing.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum RouteKey<T> {
    /// Left to the route it overrides, or to Odoo's default.
    #[default]
    Absent,
    Given(T),
    /// Given as an expression the scan does not read.
    Unknown,
}

/// A method's own `@route`, which Odoo merges with those of the methods it overrides.
#[derive(Clone, Debug, Default)]
pub struct RouteDef {
    pub method: String,
    pub urls: RouteKey<Vec<String>>,
    /// The literal methods listed, none for `methods=None`, which lets every method through.
    pub methods: RouteKey<Option<Vec<String>>>,
    pub kind: RouteKey<String>,
    pub auth: RouteKey<String>,
    pub csrf: RouteKey<bool>,
}

/// A base of a class that is no model, as the file's imports name it.
#[derive(Clone, Debug, PartialEq)]
pub enum ClassBase {
    /// `odoo.http.Controller`, from 20.0 `odoo.http.routing_map.Controller`, or `object`, which give
    /// no route.
    Root,
    /// The dotted path of a class an import from `odoo.addons` names.
    Addon(String),
    /// The class the file defines above that the name is bound to, by its place among the
    /// file's controllers.
    Local(usize),
    /// Any other, whose routes the scan does not read.
    Unread,
}

/// A class at the top of a file that is no model and has a base, so it may be a controller.
#[derive(Clone, Debug)]
pub struct ControllerFact {
    pub name: String,
    /// Where the class starts, which tells apart two classes of one name: Odoo registers both.
    pub start: TextSize,
    pub bases: Vec<ClassBase>,
    pub routes: Vec<RouteDef>,
}

#[derive(Debug, Default)]
pub struct PyFacts {
    pub classes: Vec<ClassFact>,
    pub field_classes: Vec<FieldClass>,
    /// The last name of each call a class attribute holds where the class counted it as a method:
    /// it declares a field instead when the scan finds a field class of that name.
    pub attr_calls: Vec<String>,
    pub controllers: Vec<ControllerFact>,
    /// A route of the file sits in a class whose bases the whole scan may hold, so its routing
    /// merges with that of the routes it overrides.
    pub inherits_routes: bool,
    /// A class names its model with an expression, so models and their ids are built at runtime,
    /// lacks `_name` in a module of unknown series and is named apart before and from saas~18.1, or
    /// sits in a file that does not parse with a `_name` or an `_inherit` its tokens do not spell.
    pub dynamic_models: bool,
    /// The literal start of each name the file's classes may give a model at runtime, empty when
    /// the expression shows none, which fits every name.
    pub built_model_starts: Vec<String>,
    /// The whole name of the reading a class of unknown series does not take, which fits that name
    /// alone.
    pub built_model_names: Vec<String>,
    /// Literal model names in `env['x']` outside code that runs once a runtime test of a model, field
    /// or module has passed.
    pub env_models: Vec<Spanned<String>>,
    /// Literal domains given to `search()` and the like on `self` or `env['x']`.
    pub domains: Vec<DomainFact>,
    /// XML ids named by `env.ref()` outside code that runs once a runtime test has passed.
    pub xml_refs: Vec<XmlRef>,
    /// XML ids named by `env.ref()` in such code, which may come from modules that are not installed.
    pub guarded_refs: Vec<GuardedRef>,
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
    /// Where the file stops parsing as Python 3, in a module of a series on Python 2 where it parses
    /// once its Python 2 forms are read: such a file gets a notice instead of OD000.
    pub python2: Option<Pos>,
    /// OD121 on the rollbacks of `ir.http._handle_exception()`, Odoo's hook for a failed request
    /// only before 16.0, which the series of the module decides.
    pub legacy_hook_rollbacks: Vec<RawDiag>,
    /// OD717 on a public attachment created with the rights of a user who sent its content, which
    /// `checks.rs` keeps when the scanned modules grant portal or public users create on it.
    pub granted_attachments: Vec<RawDiag>,
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
    /// A `<record>` or `<template>` whose `forcecreate` is false. No other tag reads the attribute.
    pub no_create: bool,
}

#[derive(Debug)]
pub struct XmlRef {
    pub id: String,
    pub pos: Pos,
}

/// What a runtime test shows installed once it has passed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Subject {
    Module(String),
    /// A model, there once a module that introduces it is installed.
    Model(String),
}

/// An `env.ref()` behind the runtime tests in reach of it.
#[derive(Debug)]
pub struct GuardedRef {
    pub xml_ref: XmlRef,
    /// What every one of those tests names, or `None` when one names nothing the scan reads, so
    /// that it may prove the module of any id.
    pub proven: Option<Vec<Subject>>,
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
    /// Odoo looks the fields up as it validates the view. It never does for a `filter_domain`, which
    /// waits for a search.
    pub validated: bool,
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
    /// OD325 on each element the arch adds with `t-esc`, which a spec only locates, reported only
    /// when the tree turns out to be QWeb.
    pub escapes: Vec<RawDiag>,
    /// A spec locates by `t-esc` or an xpath names it, so renaming the parent's would lose its
    /// target. Or an `<attribute>` sets `t-esc` or `t-field`, which the renamed `t-out` would hide,
    /// or sets `t-out` other than to plain text, which would join the renamed value or delete it.
    pub escape_specs: bool,
    /// OD306 on each `<tree>` of the arch, reported whatever the tree, whose fix the tree may forbid.
    pub renamed: Vec<RawDiag>,
    /// An xpath names `tree`, so renaming a `<tree>` of the tree would lose its target.
    pub tree_paths: bool,
    /// A spec is a `<tree>`, which finds the parent's `<tree>` by its tag.
    pub tree_specs: bool,
    /// The arch adds a `<tree>`, rather than locating one.
    pub tree_elements: bool,
    /// Markup the arch adds that the web client stops reading, reported only when the tree turns
    /// out not to be QWeb, which the server renders as it is.
    pub dropped: Vec<RawDiag>,
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
    pub csv: bool,
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

/// The operations an access row or a record rule applies to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Perms {
    pub read: bool,
    pub write: bool,
    pub create: bool,
    pub unlink: bool,
}

impl Perms {
    /// Whether it allows more than read.
    pub fn writes(self) -> bool {
        self.write || self.create || self.unlink
    }
}

/// A row that grants access: a row of `ir.access` with no group restricts instead.
#[derive(Debug)]
pub struct AccessRow {
    pub id: Option<String>,
    /// An `ir.model` XML id, or a model name when `by_name`: a plain `model_id` CSV column, which
    /// `ir.access` takes as the name and the name search of `ir.model` matches on every series.
    pub model: String,
    pub by_name: bool,
    pub pos: Pos,
    /// The XML id of the group, as the row's module spells it, `Some("")` for a database id.
    pub group: Option<String>,
    /// What a CSV row gives its group once `active` is read. A `<record>` grants nothing here.
    pub grants: Perms,
    /// A CSV row with no group gives every user a permission other than read.
    pub global_write: bool,
    /// The public or portal group a CSV row gives a permission other than read.
    pub public_write: Option<String>,
    /// A row of `ir.model.access`, not of `ir.access`.
    pub model_access: bool,
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

/// What one write gives the `view_id` of a window action. `Cleared` replaces the view an earlier
/// write gave, which `Unset` leaves.
#[derive(Debug, Default, PartialEq)]
pub enum ActionViewId {
    #[default]
    Unset,
    Cleared,
    /// An XML id, which the module of the write resolves.
    Ref(String),
    /// A view only a run tells.
    Unread,
}

/// One write to a window action: a `<record>`, an `<act_window>`, or a row of its `view_ids` in a
/// `<record>` of its own, whose `id` here is that of the action.
#[derive(Debug, Default)]
pub struct ActionFact {
    pub id: Option<String>,
    pub model: ActionValue,
    pub view_mode: ActionValue,
    pub view_id: ActionViewId,
    /// The types of the `<kind>_view_ref` keys of the `context` it writes, None without a write.
    pub context_kinds: Option<Vec<String>>,
    /// The types its rows of `view_ids` name a view for.
    pub view_kinds: Vec<String>,
    /// A row of `view_ids` names a view without saying the type, or an `eval` may hide such a row.
    pub view_of_any_kind: bool,
    /// As `view_kinds`, from a record nested in `view_ids`, which not every series loads.
    pub nested_view_kinds: Vec<String>,
    /// As `view_of_any_kind`, from a record nested in `view_ids`.
    pub nested_view_of_any_kind: bool,
}

impl ActionFact {
    /// Whether a row names a view of `kind`, counting the records nested in `view_ids` when `nested`.
    pub fn row_names_view(&self, kind: &str, nested: bool) -> bool {
        let of_kind = |kinds: &[String]| kinds.iter().any(|k| k == kind);
        self.view_of_any_kind
            || of_kind(&self.view_kinds)
            || (nested && (self.nested_view_of_any_kind || of_kind(&self.nested_view_kinds)))
    }
}

/// What one write gives a boolean field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Flag {
    #[default]
    Unset,
    Set(bool),
    Opaque,
}

/// What one write gives the groups of a rule: `groups` of an `ir.rule`, `group_id` of an `ir.access`.
#[derive(Debug)]
pub struct RuleGroups {
    /// The XML ids it adds, as the write's module spells them.
    pub added: Vec<String>,
    /// The groups written before stay.
    pub keeps: bool,
    /// Only a run tells which groups stay.
    pub opaque: bool,
}

impl Default for RuleGroups {
    /// What a write that leaves the field alone gives.
    fn default() -> Self {
        Self {
            added: vec![],
            keeps: true,
            opaque: false,
        }
    }
}

/// One write to an `ir.rule`, or to an `ir.access` with a domain, by a `<record>` or a CSV row.
#[derive(Debug)]
pub struct RuleFact {
    pub id: Option<String>,
    /// The record or the row.
    pub pos: Pos,
    /// A write to an `ir.access`, not to an `ir.rule`.
    pub access: bool,
    /// What it gives `model_id`: an `ir.model` XML id, or a model name when `by_name`.
    pub model: ActionValue,
    pub by_name: bool,
    /// An empty field or CSV cell, or an `eval` of `False`, `None` or `''`, is an empty literal,
    /// which `ir.rule` reads as no domain. The text `False` stays `Literal("False")` and a blank
    /// text is `Opaque`, as Odoo stores both as written and its domain check refuses them.
    pub domain: ActionValue,
    /// The field paths `domain` names, which a domain only a run can read leaves empty.
    pub fields: Vec<Spanned<String>>,
    /// What it gives `active`, where a value only a run tells counts as false.
    pub active: Option<bool>,
    pub groups: RuleGroups,
    /// What it gives read, write, create and unlink: the `perm_*` of an `ir.rule`, the letters of
    /// the `operation` of an `ir.access`.
    pub perms: [Flag; 4],
}

/// One write to an `ir.actions.server` or an `ir.cron`, by a `<record>`.
#[derive(Debug)]
pub struct ServerFact {
    pub id: Option<String>,
    pub cron: bool,
    /// What it gives `model_id`: an `ir.model` XML id, or a model name when `by_name`.
    pub model: ActionValue,
    pub by_name: bool,
    /// The XML id of the action it gives `parent_id`, or nests it under through `child_ids`.
    pub parent: ActionValue,
    /// Whether what it gives `state` is `code`, where a value only a run tells counts as false.
    pub state: Option<bool>,
    /// It gives `code` a value, which replaces the last one even when no call can be read from it.
    pub writes_code: bool,
    /// Why `compile()` refuses the code, which says nothing of the `state` or the series.
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
    /// The file does not decode or parse, so these facts say nothing of what it holds.
    pub unreadable: bool,
}

impl DataFacts {
    /// Drops every fix, for a file whose text is not its bytes: an offset into the decoded text,
    /// as in Latin-1 above 127, would land elsewhere in the file.
    pub fn drop_fixes(&mut self) {
        let views = self.views.iter_mut();
        let held = views.flat_map(|v| v.escapes.iter_mut().chain(&mut v.renamed).chain(&mut v.dropped));
        for diag in self.diags.iter_mut().chain(held) {
            diag.fix = None;
        }
    }
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
    /// The literal view kinds the file gives `view_registry.add`.
    pub view_kinds: Vec<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(def: &str) -> Signature {
        let src = format!(
            "from odoo import models\n\n\nclass X(models.Model):\n    _inherit = 'x'\n\n    {def}:\n        pass\n"
        );
        let mut facts = crate::python::analyze(&src, None);
        facts.classes.remove(0).defs.remove(0).signature.unwrap()
    }

    /// The refusal of `child` against `parent`, as its kind and the parameter's name.
    fn refusal(child: &str, parent: &str) -> Option<(&'static str, String)> {
        let (child, parent) = (sig(child), sig(parent));
        child.refuses(&parent).map(|r| match r {
            Refusal::Lost(p) => ("lost", p.name.clone()),
            Refusal::Required(p) => ("required", p.name.clone()),
            Refusal::Keyword(p) => ("keyword", p.name.clone()),
        })
    }

    fn lost(p: &str) -> Option<(&'static str, String)> {
        Some(("lost", p.to_owned()))
    }

    fn required(p: &str) -> Option<(&'static str, String)> {
        Some(("required", p.to_owned()))
    }

    fn keyword(p: &str) -> Option<(&'static str, String)> {
        Some(("keyword", p.to_owned()))
    }

    #[test]
    fn a_required_positional_the_override_has_no_place_for_is_lost() {
        assert_eq!(refusal("def f(self, a)", "def f(self, a, b)"), lost("b"));
        assert_eq!(refusal("def f(self, a, *, b)", "def f(self, a, b)"), lost("b"));
        assert_eq!(refusal("def f(self, a, **kw)", "def f(self, a, b)"), lost("b"));
        assert_eq!(refusal("def f(self, a, *args)", "def f(self, a, b)"), None);
        assert_eq!(refusal("def f(self, a)", "def f(self, a, b=None)"), None);
    }

    #[test]
    fn a_parameter_the_override_requires_and_the_parent_lacks_is_required() {
        assert_eq!(refusal("def f(self, a, b)", "def f(self, a)"), required("b"));
        assert_eq!(refusal("def f(self, *, b)", "def f(self)"), required("b"));
        assert_eq!(refusal("def f(self, a, b)", "def f(self, a, **kw)"), required("b"));
        assert_eq!(refusal("def f(self, a, b)", "def f(self, a, *args)"), None);
        assert_eq!(refusal("def f(self, *, b)", "def f(self, **kw)"), None);
        assert_eq!(refusal("def f(self, a, b=None)", "def f(self, a)"), None);
        // A positional the parent requires by name still comes in by name, unless only by position.
        assert_eq!(refusal("def f(self, a, b)", "def f(self, a, *, b)"), None);
        assert_eq!(refusal("def f(self, a, b, /)", "def f(self, a, *, b)"), required("b"));
        assert_eq!(refusal("def f(self, a, b)", "def f(self, a, *, b=None)"), None);
        // A default made required is left out.
        assert_eq!(refusal("def f(self, a, b)", "def f(self, a, b=None)"), None);
        assert_eq!(refusal("def f(self, *, b)", "def f(self, *, b=None)"), None);
    }

    #[test]
    fn a_keyword_the_override_cannot_take_by_name_is_refused_when_required_or_renamed() {
        assert_eq!(refusal("def f(self)", "def f(self, *, k)"), keyword("k"));
        assert_eq!(
            refusal("def f(self, *, s=None)", "def f(self, *, k=None)"),
            keyword("k")
        );
        assert_eq!(refusal("def f(self, k=None, /)", "def f(self, *, k)"), keyword("k"));
        assert_eq!(refusal("def f(self, k)", "def f(self, *, k)"), None);
        assert_eq!(refusal("def f(self)", "def f(self, *, k=None)"), None);
        assert_eq!(refusal("def f(self, **kw)", "def f(self, *, k)"), None);
    }

    #[test]
    fn a_positional_call_fills_what_has_a_place_and_leaves_only_defaults() {
        let takes = |def: &str, count: usize| sig(def).takes_positional(count);
        assert!(takes("def f(self)", 0));
        assert!(takes("def f(self, a=None, *, k=None)", 0));
        assert!(takes("def f(self, operator, value)", 2));
        assert!(takes("def f(self, *args)", 2));
        assert!(takes("def f(self, operator, *args)", 2));
        assert!(!takes("def f(self, html)", 0));
        assert!(!takes("def f(self, html)", 2));
        assert!(!takes("def f(self, *, k)", 0));
        assert!(!takes("def f(self, a, b, c)", 2));
    }

    #[test]
    fn positional_only_parameters_count_by_position() {
        assert_eq!(refusal("def f(self, a, /)", "def f(self, a, b, /)"), lost("b"));
        assert_eq!(refusal("def f(self, x, y, /)", "def f(self, a, b)"), None);
    }

    #[test]
    fn varargs_on_either_side_accept_everything() {
        assert_eq!(refusal("def f(self, *args, **kwargs)", "def f(self, a, *, k)"), None);
        assert_eq!(
            refusal("def f(self, a, *, k=None)", "def f(self, *args, **kwargs)"),
            None
        );
        assert_eq!(refusal("def f(self, a)", "def f(self, *args, **kwargs)"), None);
    }

    #[test]
    fn a_default_spelled_over_lines_fits_on_one() {
        let parent = sig("def f(self, a, b=(\n        1,\n        2,\n    ))");
        assert_eq!(parent.spelled("f"), "f(self, a, b=( 1, 2, ))");
    }

    #[test]
    fn a_staticmethod_compares_without_the_bound_parameter() {
        assert_eq!(refusal("@staticmethod\n    def f(a, b)", "def f(self, a, b)"), None);
        assert_eq!(refusal("@staticmethod\n    def f(a)", "def f(self, a, b)"), lost("b"));
    }
}
