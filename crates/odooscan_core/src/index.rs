use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{
    ActionFact, Annotation, ArchElement, ArchEmptied, ArchSubview, ArchValued, ClassFact, DataFacts, FieldFact,
    FlowSummary, Heritage, JsClass, ModelKind, OptionValue, PyFacts, Receiver, RecordWrite, ReportFact, RuleFact,
    ServerFact, Signature, UNREAD_TYPE, ViewFact, WrittenValue,
};
use crate::manifest::AutoInstall;
use crate::project::{Module, SourceFile, code_loaded};
use crate::series::{self, Series};

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

/// The first series that spells a field's XML id `field_<model>__<field>`, where earlier ones spell
/// it `field_<table>_<field>` (saas-11.1 `base/models/ir_model.py:760`, `0c261df33d14`).
const FIELD_ID_BY_NAME: Series = Series { major: 11, minor: 2 };

/// The fields of `MAGIC_FIELDS` Odoo creates a column for when the model logs access: the ORM
/// computes the others.
const LOG_COLUMNS: &[&str] = &["create_uid", "create_date", "write_uid", "write_date"];

pub const RELATIONAL_TYPES: &[&str] = &["Many2one", "One2many", "Many2many"];

/// Types a path cannot be followed through: the value is not a record of a fixed comodel, or, for
/// `UNREAD_TYPE`, the scan cannot tell which comodel it is.
const OPAQUE_TYPES: &[&str] = &["Reference", "Many2oneReference", "Properties", "Json", UNREAD_TYPE];

/// The class Odoo builds for each `ttype`.
const FIELD_CLASSES: &[(&str, &str)] = &[
    ("binary", "Binary"),
    ("boolean", "Boolean"),
    ("char", "Char"),
    ("date", "Date"),
    ("datetime", "Datetime"),
    ("float", "Float"),
    ("html", "Html"),
    ("integer", "Integer"),
    ("json", "Json"),
    ("many2many", "Many2many"),
    ("many2one", "Many2one"),
    ("many2one_reference", "Many2oneReference"),
    ("monetary", "Monetary"),
    ("one2many", "One2many"),
    ("properties", "Properties"),
    ("properties_definition", "PropertiesDefinition"),
    ("reference", "Reference"),
    ("selection", "Selection"),
    ("text", "Text"),
];

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

/// The methods of `BaseModel` outside `BASE_MODEL_METHODS`, which OD116 reads as the members a
/// field must not replace in every series, by `def` or class-level alias, read off the heads 12.0
/// to 20.0 and saas~11.5 to saas~19.4: a button, a `<function>` or a `super()` call may name any.
/// `OLD_BASE_METHODS` holds those gone before these heads, dated by series.
/// Dunders stay out because buttons read this list, so `BASE_MODEL_DUNDERS` holds them for
/// `super()` alone. Any other `__` name stays out too: it is mangled, so `getattr` never finds it.
const BASE_MODEL_EXTRA: &[&str] = &[
    "_add_fake_fields",
    "_add_field",
    "_add_magic_fields",
    "_add_sql_constraints",
    "_apply_ir_rules",
    "_apply_onchange_methods",
    "_as_query",
    "_browse",
    "_build_model",
    "_build_model_attributes",
    "_build_model_check_base",
    "_build_model_check_parent",
    "_cache",
    "_check_company_domain",
    "_check_concurrency",
    "_check_field_access",
    "_check_parent_path",
    "_check_qorder",
    "_check_removed_columns",
    "_clean_properties",
    "_compute_concurrency_field",
    "_constraint_methods",
    "_context",
    "_convert_records",
    "_convert_to_cache",
    "_convert_to_cache_properties_definition",
    "_convert_to_column_properties_definition",
    "_convert_to_record",
    "_convert_to_write",
    "_cr",
    "_create_parent_columns",
    "_dependent_fields",
    "_execute_sql",
    "_export_rows",
    "_extract_records",
    "_fetch_field",
    "_field_properties_to_sql",
    "_fields_view_get",
    "_filter_access_rules_python",
    "_filtered_access",
    "_flush_search",
    "_generate_m2o_order_by",
    "_generate_order_by",
    "_generate_order_by_inner",
    "_generate_translated_field",
    "_get_access_action",
    "_get_default_activity_view",
    "_get_default_calendar_view",
    "_get_default_form_view",
    "_get_default_graph_view",
    "_get_default_kanban_view",
    "_get_default_pivot_view",
    "_get_default_search_view",
    "_get_default_tree_view",
    "_get_dirty",
    "_get_external_ids",
    "_get_redirect_suggested_company",
    "_get_stored_translations",
    "_get_xml_ids",
    "_has_cycle",
    "_has_onchange",
    "_in_cache_without",
    "_inherits_check",
    "_inherits_join_add",
    "_inherits_join_calc",
    "_init_constraints_onchanges",
    "_invalidate_cache",
    "_is_an_ordinary_table",
    "_is_dirty",
    "_mapped_cache",
    "_mapped_func",
    "_modified",
    "_modified_triggers",
    "_neutralize",
    "_onchange_eval",
    "_onchange_methods",
    "_onchange_spec",
    "_ondelete_methods",
    "_origin",
    "_parent_store_create",
    "_parent_store_update",
    "_parent_store_update_prepare",
    "_patch_method",
    "_pop_field",
    "_populate",
    "_populate_dependencies",
    "_populate_factories",
    "_populate_sizes",
    "_prefetch_field",
    "_prepare_setup",
    "_read_from_database",
    "_read_group_check_field_access_rights",
    "_read_group_empty_value",
    "_read_group_expand_full",
    "_read_group_fill_results",
    "_read_group_fill_temporal",
    "_read_group_format_result",
    "_read_group_format_result_properties",
    "_read_group_get_annotated_groupby",
    "_read_group_get_annoted_groupby",
    "_read_group_groupby_properties",
    "_read_group_having",
    "_read_group_postprocess_groupby",
    "_read_group_prepare",
    "_read_group_prepare_data",
    "_read_group_process_groupby",
    "_read_group_raw",
    "_read_group_resolve_many2one_fields",
    "_read_group_resolve_many2x_fields",
    "_rec_name_fallback",
    "_recompute_check",
    "_recompute_done",
    "_recompute_field",
    "_recompute_model",
    "_recompute_recordset",
    "_recompute_todo",
    "_reflect",
    "_revert_method",
    "_set_dirty",
    "_setup_base",
    "_setup_fields",
    "_sorted_order_to_function",
    "_sql_error_to_message",
    "_sql_error_to_message_generic",
    "_table_has_rows",
    "_table_sql",
    "_transient_clean_old_rows",
    "_transient_clean_rows_older_than",
    "_transient_max_count",
    "_transient_max_hours",
    "_transient_vacuum",
    "_traverse_related_sql",
    "_uid",
    "_update_cache",
    "_validate_properties_definition",
    "_write_multi",
    "check_field_access",
    "clear_caches",
    "compute_concurrency_field",
    "compute_concurrency_field_with_access",
    "concat",
    "fields_get_keys",
    "fields_view_get",
    "flush",
    "get_access_action",
    "get_empty_list_help",
    "get_external_id",
    "get_formview_action",
    "get_formview_id",
    "get_metadata",
    "get_property_definition",
    "get_xml_id",
    "has_field_access",
    "ids",
    "invalidate_cache",
    "is_transient",
    "load_views",
    "lock_for_update",
    "name_get",
    "recompute",
    "refresh",
    "resolve_2many_commands",
    "resolve_o2m_commands_to_record_dicts",
    "try_lock_for_update",
    "union",
    "user_has_groups",
    "view_header_get",
    "view_init",
];

/// The methods of `BaseModel` gone by saas~11.5, by the range of series that has them, read off the
/// heads 9.0 to saas~11.4, but for the undotted saas branches, which `Series` cannot name. Not in
/// `BASE_MODEL_EXTRA`, since a call to one fails on any later series and must stay a report.
/// `_all_columns` stays out: a property on 9.0, not a method.
const OLD_BASE_METHODS: &[(&str, &[&str])] = &[
    (
        "<=9.0",
        &[
            "_inherits_reload",
            "_init_function_fields",
            "_m2m_raise_or_create_relation",
            "_o2m_raise_on_missing_reference",
            "_read_group_get_domain",
            "_set_default_value_on_column",
            "_store_get_values",
            "_store_set_values",
            "_update_store",
            "_view_look_dom_arch",
            "check_recursion",
            "distinct_field_get",
            "import_data",
        ],
    ),
    (
        "<=10.0",
        &[
            "_add_manual_fields",
            "_auto_end",
            "_create_table",
            "_drop_constraint",
            "_field_create",
            "_m2o_add_foreign_key_checked",
            "_m2o_add_foreign_key_unchecked",
            "_m2o_fix_foreign_key",
            "_parent_columns_exist",
            "_save_constraint",
            "_save_relation_table",
            "_select_column_data",
            "_table_exist",
            "create_workflow",
            "delete_workflow",
            "print_report",
            "redirect_workflow",
            "signal_workflow",
            "step_workflow",
        ],
    ),
    ("<=11.1", &["_check_selection_field_value"]),
    ("==11.2", &["_parent_store_create_prepare"]),
    ("<=11.4", &["_check_record_rules_result_count"]),
];

/// Whether `name` is a method of `BaseModel` that `series` has and saas~11.5 has lost. A module of
/// unknown series gets none, so a true report on a later series stands.
pub(crate) fn old_base_method(name: &str, series: Option<Series>) -> bool {
    series.is_some_and(|s| {
        OLD_BASE_METHODS
            .iter()
            .any(|(spec, names)| names.contains(&name) && series::matches(spec, s))
    })
}

/// The dunder methods `super()` finds at the end of the MRO: those of `BaseModel` in any of 9.0 to
/// 20.0, read off each series, then those `object` adds. Data dunders such as `__slots__` stay out:
/// they are not methods.
const BASE_MODEL_DUNDERS: &[&str] = &[
    "__add__",
    "__and__",
    "__bool__",
    "__contains__",
    "__deepcopy__",
    "__delattr__",
    "__dir__",
    "__eq__",
    "__format__",
    "__ge__",
    "__getattribute__",
    "__getitem__",
    "__getstate__",
    "__gt__",
    "__hash__",
    "__init__",
    "__init_subclass__",
    "__int__",
    "__iter__",
    "__le__",
    "__len__",
    "__lt__",
    "__ne__",
    "__new__",
    "__nonzero__",
    "__or__",
    "__reduce__",
    "__reduce_ex__",
    "__repr__",
    "__reversed__",
    "__setattr__",
    "__setitem__",
    "__sizeof__",
    "__str__",
    "__sub__",
    "__subclasshook__",
    "__unicode__",
];

/// Series that decorate `BaseModel` with `@api.private`: a February 2025 backport reached 16.0 and
/// 17.0, while saas~16.1 to 16.4 and saas~17.1 to 17.3 keep the older regex and accept the names of
/// `BASE_PRIVATE_METHODS`.
const API_PRIVATE: &str = ">=16.0,!=16.1,!=16.2,!=16.3,!=16.4,!=17.1,!=17.2,!=17.3";

/// Series that refuse a button on `init`: 9.0 refuses only a leading `_`.
const INIT_REFUSED: &str = ">=10.0";

/// The `@api.private` methods of `BaseModel`, by the range of series that decorates them, `init`
/// aside, which `INIT_REFUSED` covers. Names only, read off each branch head: when adding a series,
/// reread the decorators of `BaseModel` and add what changed.
const BASE_PRIVATE_METHODS: &[(&str, &[&str])] = &[
    (
        ">=16.0",
        &[
            "concat",
            "ensure_one",
            "filtered",
            "filtered_domain",
            "flush_model",
            "flush_recordset",
            "invalidate_model",
            "invalidate_recordset",
            "mapped",
            "new",
            "sorted",
            "union",
            "with_company",
            "with_context",
            "with_env",
            "with_prefetch",
            "with_user",
        ],
    ),
    (">=17.0", &["grouped"]),
    (">=18.0", &["sudo"]),
    (
        ">=18.2",
        &[
            "browse",
            "check_access",
            "exists",
            "fetch",
            "modified",
            "search_fetch",
            "update",
        ],
    ),
    (">=18.3", &["lock_for_update"]),
    (">=19.0", &["try_lock_for_update"]),
];

/// Whether Odoo refuses a button on `name` because `BaseModel` makes it private in `series`,
/// whatever a module overrides: the refusal walks the MRO. A module of unknown series gets only
/// `init`, which nearly every series refuses.
pub(crate) fn base_private_method(name: &str, series: Option<Series>) -> bool {
    if name == "init" {
        return series.is_none_or(|s| series::matches(INIT_REFUSED, s));
    }
    series.is_some_and(|s| {
        series::matches(API_PRIVATE, s)
            && BASE_PRIVATE_METHODS
                .iter()
                .any(|(spec, names)| names.contains(&name) && series::matches(spec, s))
    })
}

/// The public methods of `BaseModel` that `BASE_MODEL_METHODS` lacks, by the range of series that
/// has them, so OD604 keeps reporting a call a series cannot answer. Names only, read off each
/// branch head: when adding a series, reread the methods of `BaseModel` and date what changed.
/// `ids` stays out as a property and `lock_for_update` as private since it appeared.
/// `OLD_BASE_METHODS` stays out: it ends at saas~11.4, and the `orm` service that OD604 reads starts
/// on saas~14.5.
const BASE_RPC_METHODS: &[(&str, &[&str])] = &[
    (">=9.0", &["get_external_id", "get_metadata"]),
    (">=10.0", &["concat", "union"]),
    (">=17.0", &["get_property_definition"]),
    (">=18.3", &["try_lock_for_update"]),
    (">=19.1", &["check_field_access", "has_field_access"]),
    (
        "<=13.2",
        &["resolve_2many_commands", "resolve_o2m_commands_to_record_dicts"],
    ),
    (
        "<=14.3",
        &["compute_concurrency_field", "compute_concurrency_field_with_access"],
    ),
    ("<=15.1", &["get_access_action", "view_init"]),
    (
        "<=15.4",
        &[
            "fields_view_get",
            "get_empty_list_help",
            "get_formview_action",
            "get_formview_id",
            "view_header_get",
        ],
    ),
    (">=10.0,<=15.4", &["load_views"]),
    (
        "<=16.1",
        &[
            "fields_get_keys",
            "get_xml_id",
            "invalidate_cache",
            "recompute",
            "refresh",
        ],
    ),
    (">=12.5,<=16.1", &["flush"]),
    ("<=17.1", &["user_has_groups"]),
    ("<=17.2", &["name_get"]),
    ("<=18.0", &["clear_caches"]),
    ("<=18.4", &["is_transient"]),
];

/// Whether a client may call `name` as a public method of `BaseModel` in `series`. A module of
/// unknown series may run on any, so every listed name counts.
pub(crate) fn base_rpc_method(name: &str, series: Option<Series>) -> bool {
    !base_private_method(name, series)
        && BASE_RPC_METHODS
            .iter()
            .any(|(spec, names)| names.contains(&name) && series.is_none_or(|s| series::matches(spec, s)))
}

pub fn full_id(module: &str, id: &str) -> String {
    if id.contains('.') {
        id.to_owned()
    } else {
        format!("{module}.{id}")
    }
}

/// The addon, the files that may define the class, and the class of a dotted path
/// `odoo.addons.<addon>.<a>...<z>.<Class>`: `<a>/.../<z>.py` and `<a>/.../<z>/__init__.py`.
fn class_file(path: &str) -> Option<(&str, [String; 2], &str)> {
    let (addon, rest) = path.strip_prefix("odoo.addons.")?.split_once('.')?;
    let (module, class) = rest.rsplit_once('.')?;
    let dir = module.replace('.', "/");
    Some((addon, [format!("{dir}.py"), format!("{dir}/__init__.py")], class))
}

fn model_xmlid(model: &str) -> String {
    format!("model_{}", model.replace('.', "_"))
}

type Models<'m> = FxHashMap<&'m str, Vec<(usize, &'m ClassFact)>>;

/// Every write to a record, by full id, with where it loads.
type Writes<'m, T> = FxHashMap<String, Vec<Placed<&'m T>>>;

/// The modules whose data give a view each `key`.
type ViewKeys<'m> = FxHashMap<&'m str, Vec<usize>>;

/// The modules whose data gives a default to each (model, field), with whether a demo file does.
type IrDefaults<'m> = FxHashMap<(&'m str, &'m str), Vec<(usize, bool)>>;

type XmlIds<'m> = FxHashMap<String, Vec<XmlIdDef<'m>>>;

/// The writes that make up one record, in load order.
type FoldedRecord<'m> = Vec<PlacedWrite<'m>>;

/// The fields `ir.model.fields` records add, by name.
type DataFields<'m> = FxHashMap<&'m str, Vec<DataField<'m>>>;

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
    /// The subviews every view of the tree opens, in scope or not, each relative to its own view.
    pub subviews: Vec<&'m ArchSubview>,
    /// The elements the own tags of the tree's views add, in scope or not, repeated when several
    /// add one.
    pub valued: Vec<ArchValued>,
    /// What the specs of the tree may leave empty, in scope or not.
    pub emptied: Vec<ArchEmptied>,
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
        .push(XmlIdDef {
            module: base,
            model: Some("ir.module.module"),
            spelling: None,
            holder: None,
        });
    let Some(category) = &module.manifest.category else {
        return;
    };
    let parts: Vec<String> = category.split('/').map(|p| p.trim().to_lowercase()).collect();
    for n in 1..=parts.len() {
        let id = parts[..n].join("_").replace('&', "and").replace(' ', "_");
        xmlids
            .entry(format!("base.module_category_{id}"))
            .or_default()
            .push(XmlIdDef {
                module: base,
                model: Some("ir.module.category"),
                spelling: None,
                holder: None,
            });
    }
}

fn class_ids<'m>(
    mi: usize,
    module: &'m Module,
    models: &mut Models<'m>,
    model_by_xmlid: &mut FxHashMap<String, &'m str>,
    xmlids: &mut XmlIds<'m>,
) {
    let name = module.name.as_str();
    for class in module.python.iter().flat_map(|f| &f.facts.classes) {
        models.entry(class.model.as_str()).or_default().push((mi, class));
        let prefix = model_xmlid(&class.model);
        // Odoo gives every module that defines or extends a model its own ir.model XML id.
        let id = format!("{name}.{prefix}");
        model_by_xmlid.insert(id.clone(), class.model.as_str());
        xmlids.entry(id).or_default().push(XmlIdDef {
            module: mi,
            model: Some("ir.model"),
            spelling: None,
            holder: None,
        });
    }
}

/// What the id of a field of `model` may start with, by spelling. Classes that set different tables
/// leave each possible, as only the registry tells which wins.
fn spelled_prefixes(model: &str, classes: &[(usize, &ClassFact)]) -> Vec<(FieldSpelling, String)> {
    let tables = model_tables(model, literal_tables(classes).flatten().collect());
    let by_table = tables
        .into_iter()
        .map(|t| (FieldSpelling::ByTable, format!("field_{t}_")));
    let by_name = (FieldSpelling::ByName, format!("field_{}__", model.replace('.', "_")));
    by_table.chain(std::iter::once(by_name)).collect()
}

/// What the id of a field of `model` starts with in `series`; without a series, in either spelling.
fn field_id_prefixes(model: &str, classes: &[(usize, &ClassFact)], series: Option<Series>) -> Vec<String> {
    let wanted = series.map(FieldSpelling::of);
    let prefixes = spelled_prefixes(model, classes).into_iter();
    prefixes
        .filter(|(s, _)| wanted.is_none_or(|w| w == *s))
        .map(|(_, p)| p)
        .collect()
}

/// Whether `local` may be the id of a field of `class`, spelled from a table the index cannot tell:
/// Odoo spells it by table in `series`, and none of `classes`, those of the model, sets the table or
/// introduces the model. Only a declared field counts, so a typo on the common extension of an
/// outside model stays reported, and only after a table that is not `known_table`, so an id of an
/// indexed model without its module does too. `any_field` is for a class that may hold fields the
/// scan lacks: with no field to end on, `local` tells no table from a name.
fn unseen_table_id(
    classes: &[(usize, &ClassFact)],
    class: &ClassFact,
    local: &str,
    series: Option<Series>,
    known_table: impl Fn(&str) -> bool,
    any_field: bool,
) -> bool {
    let by_table = series.is_none_or(|s| FieldSpelling::of(s) == FieldSpelling::ByTable);
    let unseen = classes.iter().all(|(_, c)| !c.new_model && c.table.is_none());
    let Some(rest) = local.strip_prefix("field_").filter(|_| by_table && unseen) else {
        return false;
    };
    let names = |f: &FieldFact| {
        let table = rest.strip_suffix(f.name.as_str()).and_then(|t| t.strip_suffix('_'));
        table.is_some_and(|t| !t.is_empty() && !t.ends_with('_') && !known_table(t))
    };
    any_field || class.fields.iter().any(names)
}

/// The ids of the fields each class declares, in both spellings: `Index::xmlid_defs` picks one by
/// the module that reads the id, which a scan may place on another series than the field's module.
/// A pass of its own, as a `_table` that spells them may come from any module that extends the model.
fn field_ids<'m>(modules: &'m [Module], models: &Models<'m>, xmlids: &mut XmlIds<'m>) {
    for (mi, module) in modules.iter().enumerate() {
        for class in module.python.iter().flat_map(|f| &f.facts.classes) {
            let prefixes = spelled_prefixes(&class.model, &models[class.model.as_str()]);
            let magic = MAGIC_FIELDS.iter().filter(|_| class.new_model).copied();
            for field in class.fields.iter().map(|f| f.name.as_str()).chain(magic) {
                for (spelling, prefix) in &prefixes {
                    let id = format!("{}.{prefix}{field}", module.name);
                    xmlids.entry(id).or_default().push(XmlIdDef {
                        module: mi,
                        model: Some("ir.model.fields"),
                        spelling: Some(*spelling),
                        holder: None,
                    });
                }
            }
        }
    }
}

/// Every class of `model` and of the models it inherits from, `base` included. `delegated` adds the
/// models it reaches through `_inherits`, whose fields it has but does not merge.
fn model_chain<'m>(
    models: &Models<'m>,
    model: &str,
    scope: Option<&Scope>,
    delegated: bool,
) -> Vec<(usize, &'m ClassFact)> {
    let mut out = vec![];
    let mut seen = FxHashSet::default();
    let mut stack = vec![model, "base"];
    while let Some(m) = stack.pop() {
        if !seen.insert(m) {
            continue;
        }
        for &(mi, class) in models.get(m).map(Vec::as_slice).unwrap_or_default() {
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

/// The fields `class` of module `mi` gets from its parents, each with a module that may hold its id.
/// Odoo tests a module against the model's original one, so a later class of the module that
/// introduces the model takes its place, and against the modules that declare the field, so every
/// series writes an id under a declarer that takes the parent itself. saas~11.5 leaves out a
/// delegated field of a parent an extension adds, as its `_modules` stay on the delegated model.
fn inherited_fields<'m>(
    mi: usize,
    class: &'m ClassFact,
    models: &Models<'m>,
) -> Vec<(usize, &'m str, Option<FieldHolder>)> {
    let classes = &models[class.model.as_str()];
    let introduces = classes.iter().any(|(m, c)| *m == mi && c.new_model);
    let own = if introduces {
        FieldHolder::Introducer
    } else {
        FieldHolder::Extender
    };
    let parents = class.inherit.iter().map(|i| i.value.as_str());
    let merged: Vec<_> = parents
        .filter(|p| *p != class.model)
        .flat_map(|p| model_chain(models, p, None, false))
        .collect();
    let mut out = vec![];
    for &(declarer, parent) in &merged {
        for field in &parent.fields {
            out.push((mi, field.name.as_str(), (declarer != mi).then_some(own)));
            if declarer != mi {
                out.push((declarer, field.name.as_str(), Some(FieldHolder::Mixin)));
            }
        }
    }
    let own_delegates = class.inherits.iter().filter(|_| introduces);
    let delegates = own_delegates.chain(merged.iter().flat_map(|(_, parent)| &parent.inherits));
    for (declarer, target) in delegates.flat_map(|(target, _)| model_chain(models, target, None, true)) {
        let holder = (!introduces && declarer != mi).then_some(FieldHolder::Delegator);
        out.extend(target.fields.iter().map(|f| (mi, f.name.as_str(), holder)));
    }
    out
}

/// The ids of the fields a class gets from its parents, spelled for the inheriting model. Odoo
/// reflects them for that model, while `field_ids` only knows the class that declares each.
fn inherited_field_ids<'m>(modules: &'m [Module], models: &Models<'m>, xmlids: &mut XmlIds<'m>) {
    for (mi, module) in modules.iter().enumerate() {
        for class in module.python.iter().flat_map(|f| &f.facts.classes) {
            let prefixes = spelled_prefixes(&class.model, &models[class.model.as_str()]);
            for (holder_module, field, holder) in inherited_fields(mi, class, models) {
                for (spelling, prefix) in &prefixes {
                    let id = format!("{}.{prefix}{field}", modules[holder_module].name);
                    xmlids.entry(id).or_default().push(XmlIdDef {
                        module: holder_module,
                        model: Some("ir.model.fields"),
                        spelling: Some(*spelling),
                        holder,
                    });
                }
            }
        }
    }
}

/// The data files Odoo loads for `module`: those its manifest lists and those it loads itself.
pub(crate) fn loaded_data(module: &Module, series: Option<Series>) -> impl Iterator<Item = &SourceFile<DataFacts>> {
    let mentions = module.code_mentions();
    let loaded =
        move |f: &&SourceFile<DataFacts>| f.rank.is_some() || code_loaded(&f.rel, &mentions, &module.manifest, series);
    module.data.iter().filter(loaded)
}

fn data_ids<'m>(
    mi: usize,
    module: &'m Module,
    by_name: &FxHashMap<&str, usize>,
    xmlids: &mut XmlIds<'m>,
    views: &mut Nodes<'m>,
    view_keys: &mut ViewKeys<'m>,
    series: Option<Series>,
) {
    let name = module.name.as_str();
    for file in loaded_data(module, series) {
        for key in &file.facts.view_keys {
            view_keys.entry(key).or_default().push(mi);
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
    // Every file feeds the ids it spells out: Python can load a whole folder without naming a file,
    // so the filter above would turn correct refs into false OD301. Ids derived from a record need
    // the record loaded, so `derived_ids` keeps that filter.
    for file in &module.data {
        for def in &file.facts.defs {
            // `<record id="other.x">` creates or updates an id that belongs to module other.
            let id = full_id(name, &def.id);
            let owner = id
                .split_once('.')
                .and_then(|(m, _)| by_name.get(m))
                .copied()
                .unwrap_or(mi);
            // A `theme_` module loads these tags as models of its own (18.0 `convert.py`, `_tag_template`).
            let model = def
                .model
                .as_deref()
                .filter(|_| !(def.themed && name.starts_with("theme_")));
            xmlids.entry(id).or_default().push(XmlIdDef {
                module: owner,
                model,
                spelling: None,
                holder: None,
            });
        }
    }
}

/// The fields each `ir.default.set()` of the files manifests load gives a default, with its module
/// and whether a demo file makes the call.
fn ir_defaults(modules: &[Module]) -> IrDefaults<'_> {
    let mut defaults: IrDefaults<'_> = FxHashMap::default();
    for (mi, module) in modules.iter().enumerate() {
        for file in module.data.iter().filter(|f| f.rank.is_some()) {
            for (model, field) in file.facts.functions.iter().filter_map(|c| c.default_of.as_ref()) {
                let key = (model.as_str(), field.as_str());
                defaults.entry(key).or_default().push((mi, file.demo));
            }
        }
    }
    defaults
}

/// Ids of records Odoo creates alongside the records of the files it loads, with their module and
/// model.
fn derived_ids<'m>(
    modules: &'m [Module],
    models: &Models<'m>,
    default_series: Option<Series>,
) -> Vec<(String, usize, &'m str)> {
    // A record of a model with _inherits also gets an id for each parent record Odoo creates,
    // e.g. <cron id>_ir_actions_server for an ir.cron.
    let mut derived = vec![];
    for (mi, module) in modules.iter().enumerate() {
        let files = loaded_data(module, module.series.or(default_series));
        for def in files.flat_map(|f| &f.facts.defs) {
            let Some(classes) = def.model.as_deref().and_then(|m| models.get(m)) else {
                continue;
            };
            for (parent, _) in classes.iter().flat_map(|(_, c)| &c.inherits) {
                derived.push((
                    format!("{}_{}", full_id(&module.name, &def.id), parent.replace('.', "_")),
                    mi,
                    parent.as_str(),
                ));
            }
        }
    }
    derived
}

fn access_models(
    modules: &[Module],
    model_by_xmlid: &FxHashMap<String, &str>,
    default_series: Option<Series>,
) -> FxHashSet<String> {
    let mut models_with_access = FxHashSet::default();
    for m in modules {
        let files = loaded_data(m, m.series.or(default_series));
        for access in files.flat_map(|f| &f.facts.access) {
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

/// The first series whose `must_install` keeps a module with `countries` off a database with no company
/// in those countries (`saas~17.1`). Before it the key decides nothing.
const COUNTRY_GATE: Series = Series { major: 17, minor: 1 };

/// `base`, and what Odoo always installs with it. `default_series` stands in for a manifest version
/// without a series, as no checked module is at hand to lend its own.
/// The manifest `countries` key is ignored on purpose, because Odoo creates a database without reading it.
fn always_installed(modules: &[Module], by_name: &FxHashMap<&str, usize>, default_series: Option<Series>) -> Vec<bool> {
    let seeds = modules.iter().map(|m| m.name == "base").collect();
    install_closure(modules, by_name, seeds, default_series, |_| true)
}

/// `seeds`, and each auto-install module `joins` admits whose triggers (see `triggers` for how a series
/// reads them) lie among them and whose depends closure is indexed, with that closure dragged in,
/// repeating until nothing is added.
fn install_closure(
    modules: &[Module],
    by_name: &FxHashMap<&str, usize>,
    seeds: Vec<bool>,
    series: Option<Series>,
    joins: impl Fn(&Module) -> bool,
) -> Vec<bool> {
    let mut installed = seeds;
    loop {
        let holds = |name: &str| by_name.get(name).is_some_and(|&di| installed[di]);
        let ready: Vec<usize> = (0..modules.len())
            .filter(|&mi| !installed[mi] && joins(&modules[mi]))
            .filter(|&mi| triggers(&modules[mi], series).is_some_and(|t| t.into_iter().all(holds)))
            .collect();
        let mut grew = false;
        for mi in ready {
            // Odoo cannot load a module whose depends closure is incomplete, so nothing joins through it.
            let (closure, complete) = depends_closure(modules, by_name, [mi]);
            if complete {
                installed.iter_mut().zip(closure).for_each(|(a, c)| *a |= c);
                grew = true;
            }
        }
        if !grew {
            return installed;
        }
    }
}

/// The depends whose install auto-installs `module`, or `None` when nothing does. From `saas~12.4` a list
/// names the triggers, and one naming a module outside `depends` fails the manifest. Before `saas~12.4`,
/// database creation fails on a list, and `update_list` keeps only whether the value is truthy, so `must_install`
/// waits on every depend for a non-empty list, like `True`, and `[]` is off.
fn triggers(module: &Module, series: Option<Series>) -> Option<Vec<&str>> {
    let depends: Vec<&str> = module.manifest.depends.iter().map(|d| d.value.as_str()).collect();
    match &module.manifest.auto_install {
        AutoInstall::Off => None,
        AutoInstall::Depends => Some(depends),
        AutoInstall::Listed(names) if module.series.or(series).is_none_or(|s| s >= LISTED_TRIGGERS) => names
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

/// Module indices in the order Odoo loads them: by depth in the `depends` graph, then by name. The
/// passes end at the deepest module, or at the module count when a cycle keeps deepening.
fn load_order(modules: &[Module], by_name: &FxHashMap<&str, usize>) -> Vec<usize> {
    let mut depth = vec![0_usize; modules.len()];
    for _ in modules {
        let mut grew = false;
        for (mi, module) in modules.iter().enumerate() {
            let depends = module.manifest.depends.iter();
            let below = depends.filter_map(|d| by_name.get(d.value.as_str()));
            let deepest = below.map(|&d| depth[d] + 1).max().unwrap_or_default();
            grew |= deepest > depth[mi];
            depth[mi] = depth[mi].max(deepest);
        }
        if !grew {
            break;
        }
    }
    let mut order: Vec<usize> = (0..modules.len()).collect();
    order.sort_unstable_by_key(|&mi| (depth[mi], modules[mi].name.as_str()));
    order
}

/// The place of each module in `load_order`.
fn load_rank(modules: &[Module], by_name: &FxHashMap<&str, usize>) -> Vec<usize> {
    let mut ranks = vec![0; modules.len()];
    for (rank, mi) in load_order(modules, by_name).into_iter().enumerate() {
        ranks[mi] = rank;
    }
    ranks
}

/// Folds writes of `(module name, write)`, in load order, into records: those of one id make one
/// record, and a write without an id is a record of its own.
fn fold_writes<'m>(writes: impl Iterator<Item = (&'m str, PlacedWrite<'m>)>) -> Vec<FoldedRecord<'m>> {
    let mut records: Vec<FoldedRecord<'m>> = vec![];
    let mut by_id: FxHashMap<String, usize> = FxHashMap::default();
    for (module, placed) in writes {
        let id = placed.fact.id.as_ref().map(|id| full_id(module, id));
        let known = id.as_ref().and_then(|id| by_id.get(id)).copied();
        if let Some(at) = known {
            records[at].push(placed);
            continue;
        }
        if let Some(id) = id {
            by_id.insert(id, records.len());
        }
        records.push(vec![placed]);
    }
    records
}

/// The value the last of `record`'s writes to give the field `name` leaves it.
fn last_written<'m>(record: &[PlacedWrite<'m>], name: &str) -> Option<(usize, &'m WrittenValue)> {
    record.iter().rev().find_map(|placed| {
        let field = placed.fact.fields.iter().find(|f| f.name == name)?;
        Some((placed.module, &field.value))
    })
}

/// The text `record` leaves in the field `name`.
fn last_text<'m>(record: &[PlacedWrite<'m>], name: &str) -> Option<&'m str> {
    match last_written(record, name)?.1 {
        WrittenValue::Text(text) => Some(text.value.trim()),
        _ => None,
    }
}

/// Every write of the files manifests load to each record, in the order Odoo loads them.
fn record_writes<'m, T>(
    modules: &'m [Module],
    by_name: &FxHashMap<&str, usize>,
    facts: fn(&DataFacts) -> &[T],
    id: fn(&T) -> Option<&String>,
) -> Writes<'m, T> {
    let mut writes: Writes<'m, T> = FxHashMap::default();
    for mi in load_order(modules, by_name) {
        let module = &modules[mi];
        for file in module.data.iter().filter(|f| f.rank.is_some()) {
            for fact in facts(&file.facts) {
                if let Some(id) = id(fact) {
                    writes.entry(full_id(&module.name, id)).or_default().push(Placed {
                        module: mi,
                        demo: file.demo,
                        fact,
                    });
                }
            }
        }
    }
    writes
}

/// Asset globs as Odoo expands them: `*` stops at a slash. An invalid glob matches nothing.
pub(crate) fn asset_glob_set<'g>(globs: impl IntoIterator<Item = &'g String>) -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    for glob in globs {
        if let Ok(g) = GlobBuilder::new(glob).literal_separator(true).build() {
            builder.add(g);
        }
    }
    builder.build().unwrap_or_else(|_| GlobSet::empty())
}

/// Files of a kind, as (module, index among the module's files of that kind).
type Loaded = FxHashSet<(usize, usize)>;

/// The scripts and the template files some bundle of an indexed manifest adds, as (module, file).
/// Odoo expands the globs against the addons directories, so a module may add another module's files.
fn loaded_static(modules: &[Module]) -> (Loaded, Loaded) {
    let set = asset_glob_set(modules.iter().flat_map(|m| &m.manifest.assets));
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

pub struct XmlIdDef<'m> {
    pub module: usize,
    /// The model of the record, none when only a run tells it.
    pub model: Option<&'m str>,
    /// How the id spells its field when the registry creates it. Such an id exists in some series
    /// only, where a record of a data file exists in all of them.
    spelling: Option<FieldSpelling>,
    /// Why the module holds the id of an inherited field, which only some series write there. None
    /// for an id every series writes there.
    holder: Option<FieldHolder>,
}

impl XmlIdDef<'_> {
    fn exists_in(&self, series: Option<Series>) -> bool {
        let spelled = self
            .spelling
            .is_none_or(|own| series.is_none_or(|s| FieldSpelling::of(s) == own));
        spelled && self.holder.is_none_or(|h| series.is_none_or(|s| h.holds_in(s)))
    }
}

/// Why a module may hold the id of a field that a model gets from an `_inherit` parent.
#[derive(Clone, Copy)]
enum FieldHolder {
    /// It introduces the model.
    Introducer,
    /// It declares the field on the parent.
    Mixin,
    /// It adds the parent to a model it extends.
    Extender,
    /// It adds to a model it extends a parent that delegates the field.
    Delegator,
}

impl FieldHolder {
    /// 9.0 takes the module that creates the row. 10.0 to saas~11.2 prefer the module of the class
    /// that declares the field, which a delegated field lacks. saas~11.3 and 11.4 take the one that
    /// creates the row or declares the field. From saas~11.5 it is the model's original module or one
    /// that declares the field, joined from saas~13.4 by each module that adds the parent. A declarer
    /// that does not take the parent is taken to load before the model, so to write nothing for it.
    fn holds_in(self, series: Series) -> bool {
        let by_declarer = series::matches(">=10.0,<11.3", series);
        let by_original = series::matches(">=11.5,<13.4", series);
        match self {
            Self::Introducer => !by_declarer,
            Self::Mixin => by_declarer,
            Self::Extender => !by_declarer && !by_original,
            Self::Delegator => !by_original,
        }
    }
}

/// The two forms of the XML id of a field.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldSpelling {
    ByTable,
    ByName,
}

impl FieldSpelling {
    fn of(series: Series) -> Self {
        if series >= FIELD_ID_BY_NAME {
            Self::ByName
        } else {
            Self::ByTable
        }
    }
}

/// The modules a module may use: itself, its transitive depends and the always-installed modules.
#[derive(Clone)]
pub struct Scope {
    visible: Vec<bool>,
    /// The modules whose `ir.model.fields` records have loaded when the reader of this scope runs.
    records: Vec<bool>,
    demo: bool,
}

impl Scope {
    pub fn sees(&self, module: usize) -> bool {
        self.visible[module]
    }

    /// This scope for a reader in a demo file, which loads after the data and only where demo data
    /// does.
    pub fn in_demo(&self) -> Scope {
        Scope {
            demo: true,
            ..self.clone()
        }
    }

    /// Whether `write` has loaded when the reader of this scope runs, whichever module makes it. Odoo
    /// loads a module's demo files after its data files and may skip them or roll them back alone, so
    /// a demo write says nothing of the value a reader in a data file leaves.
    pub fn loads<F>(&self, write: &Placed<F>) -> bool {
        self.demo || !write.demo
    }

    /// Whether `write` belongs to the state this scope's reader acts on.
    pub fn reads<F>(&self, write: &Placed<F>) -> bool {
        self.sees(write.module) && self.loads(write)
    }

    /// Whether the field `record` adds exists when the reader of this scope runs.
    fn holds(&self, record: &DataField) -> bool {
        self.records[record.module] && (self.demo || !record.demo)
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

/// A fact of a data file, with where it loads.
#[derive(Clone, Copy)]
pub struct Placed<F> {
    pub module: usize,
    /// Only a database with demo data gets it.
    pub demo: bool,
    pub fact: F,
}

/// A write of an `ir.model.fields` record, with where it loads.
type PlacedWrite<'m> = Placed<&'m RecordWrite>;

/// A field an `ir.model.fields` record adds.
struct DataField<'m> {
    module: usize,
    /// Only a database with demo data gets it.
    demo: bool,
    /// None when only a run tells the model, which then may be any.
    model: Option<&'m str>,
    ftype: &'static str,
    comodel: Option<&'m str>,
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
    /// Read through `xmlid_defs`: it holds the field ids of every series.
    xmlids: XmlIds<'m>,
    pub views: Inheritance<'m>,
    /// The (model, type) of every view of the files manifests load that is primary or may be, as
    /// Odoo looks a default view up among primary ones alone. None is a type only a run can tell.
    pub view_types: FxHashSet<(&'m str, Option<&'m str>)>,
    /// The kinds `view_registry.add` names that no entry of `view_types` has: on 9.0 the web client
    /// draws those from a view of another kind, so an action lists them with no view of their own.
    pub client_views: FxHashSet<&'m str>,
    pub action_writes: Writes<'m, ActionFact>,
    pub rule_writes: Writes<'m, RuleFact>,
    pub server_writes: Writes<'m, ServerFact>,
    pub report_writes: Writes<'m, ReportFact>,
    pub value_writes: Writes<'m, RecordWrite>,
    data_fields: DataFields<'m>,
    /// The ids of records Odoo creates alongside others.
    pub derived: FxHashSet<String>,
    ir_defaults: IrDefaults<'m>,
    view_keys: ViewKeys<'m>,
    pub templates: Inheritance<'m>,
    pub models_with_access: FxHashSet<String>,
    model_by_xmlid: FxHashMap<String, &'m str>,
    pub has_base: bool,
    js_paths: FxHashMap<String, JsTarget>,
    js_loaded: Loaded,
    templates_loaded: Loaded,
    always_installed: Vec<bool>,
    /// Each module's place in `load_order`.
    load_rank: Vec<usize>,
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
        let mut view_keys = FxHashMap::default();
        let base = by_name.get("base").copied();

        for (mi, module) in modules.iter().enumerate() {
            if let Some(b) = base {
                module_ids(module, b, &mut xmlids);
            }
            class_ids(mi, module, &mut models, &mut model_by_xmlid, &mut xmlids);
            let series = module.series.or(default_series);
            data_ids(mi, module, &by_name, &mut xmlids, &mut views, &mut view_keys, series);
        }
        field_ids(modules, &models, &mut xmlids);
        inherited_field_ids(modules, &models, &mut xmlids);
        let mut derived = FxHashSet::default();
        for (id, mi, parent) in derived_ids(modules, &models, default_series) {
            derived.insert(id.clone());
            xmlids.entry(id).or_default().push(XmlIdDef {
                module: mi,
                model: Some(parent),
                spelling: None,
                holder: None,
            });
        }
        let models_with_access = access_models(modules, &model_by_xmlid, default_series);
        let (js_loaded, templates_loaded) = loaded_static(modules);
        let mut index = Self {
            modules,
            has_base: base.is_some(),
            xmlids,
            views: Inheritance::new(views),
            view_types: FxHashSet::default(),
            client_views: FxHashSet::default(),
            action_writes: record_writes(modules, &by_name, |f| &f.actions, |a| a.id.as_ref()),
            rule_writes: record_writes(modules, &by_name, |f| &f.rules, |r| r.id.as_ref()),
            server_writes: record_writes(modules, &by_name, |f| &f.servers, |s| s.id.as_ref()),
            report_writes: record_writes(modules, &by_name, |f| &f.reports, |r| r.id.as_ref()),
            value_writes: record_writes(modules, &by_name, |f| &f.writes, |w| w.id.as_ref()),
            data_fields: FxHashMap::default(),
            derived,
            ir_defaults: ir_defaults(modules),
            view_keys,
            templates: Inheritance::new(template_nodes(modules, &templates_loaded)),
            models_with_access,
            model_by_xmlid,
            js_paths: js_paths(modules),
            js_loaded,
            templates_loaded,
            always_installed: always_installed(modules, &by_name, default_series),
            load_rank: load_rank(modules, &by_name),
            by_name,
            patched: FxHashMap::default(),
            template_bases: FxHashSet::default(),
            method_flows: method_flows(modules),
            table_models: table_models(&models),
            models,
        };
        index.patched = index.patched_classes();
        index.template_bases = index.template_bases();
        index.view_types = index.view_types();
        index.client_views = index.client_views();
        index.data_fields = index.data_fields(default_series);
        index
    }

    /// The fields the `ir.model.fields` records of the files Odoo loads add.
    fn data_fields(&self, default_series: Option<Series>) -> DataFields<'m> {
        let modules = self.modules;
        let writes = load_order(modules, &self.by_name).into_iter().flat_map(|mi| {
            let module = &modules[mi];
            let files = loaded_data(module, module.series.or(default_series));
            let name = module.name.as_str();
            files.flat_map(move |f| {
                let writes = f.facts.writes.iter().filter(|w| w.model == "ir.model.fields");
                writes.map(move |fact| {
                    let demo = f.demo;
                    (name, PlacedWrite { module: mi, demo, fact })
                })
            })
        });
        let mut fields: DataFields<'m> = FxHashMap::default();
        for record in fold_writes(writes) {
            if let Some((name, field)) = self.field_of_record(&record) {
                fields.entry(name).or_default().push(field);
            }
        }
        fields
    }

    /// The field `record` leaves, if Odoo builds one: only a `manual` record, the default state, adds
    /// a field, under a name Odoo holds to `x_`. `model_id` is required and `create` never derives
    /// it, so a record that does not give it fails to load; one nested in an `ir.model` is unread.
    fn field_of_record(&self, record: &[PlacedWrite<'m>]) -> Option<(&'m str, DataField<'m>)> {
        let name = last_text(record, "name").filter(|n| n.starts_with("x_"))?;
        last_written(record, "model_id")?;
        if last_text(record, "state").is_some_and(|s| s != "manual") {
            return None;
        }
        let ttype = last_text(record, "ttype");
        let class = FIELD_CLASSES.iter().find(|(t, _)| Some(*t) == ttype).map(|(_, c)| *c);
        let field = DataField {
            module: record[0].module,
            demo: record[0].demo,
            model: self.model_of_record(record),
            ftype: class.unwrap_or(UNREAD_TYPE),
            comodel: last_text(record, "relation"),
        };
        Some((name, field))
    }

    /// The model the last `model_id` of `record` names. For a `model_id` only a run tells, the `model`
    /// the record gives, which `create` holds to it, and else none.
    fn model_of_record(&self, record: &[PlacedWrite<'m>]) -> Option<&'m str> {
        let named = last_written(record, "model_id").and_then(|(mi, value)| match value {
            WrittenValue::Ids(ids) => self.model_of_xmlid(&full_id(&self.modules[mi].name, &ids.first()?.value)),
            WrittenValue::Model(model) => Some(model.value.as_str()),
            _ => None,
        });
        named.or_else(|| last_text(record, "model"))
    }

    fn view_types(&self) -> FxHashSet<(&'m str, Option<&'m str>)> {
        let mut types = FxHashSet::default();
        for module in self.modules {
            let loaded = module.data.iter().filter(|f| f.rank.is_some());
            let views = loaded.flat_map(|f| &f.facts.views).filter(|v| v.primary != Some(false));
            types.extend(views.filter_map(|view| self.view_type(module, view)));
        }
        types
    }

    fn client_views(&self) -> FxHashSet<&'m str> {
        let served: FxHashSet<&str> = self.view_types.iter().filter_map(|(_, kind)| *kind).collect();
        let registered = self
            .modules
            .iter()
            .flat_map(|m| &m.js)
            .flat_map(|f| &f.facts.view_kinds);
        registered
            .map(String::as_str)
            .filter(|kind| !served.contains(kind))
            .collect()
    }

    /// The entry of `view` in `view_types`, if it has one. Odoo gives a view with a parent the type
    /// of its tree's root, so with a view above it not indexed, its own root says nothing.
    fn view_type(&self, module: &'m Module, view: &'m ViewFact) -> Option<(&'m str, Option<&'m str>)> {
        let id = view.id.as_ref().map(|id| full_id(&module.name, id));
        let own_model = view.model.as_ref().map(|m| m.value.as_str());
        let model = own_model.or_else(|| self.view_model(id.clone()?))?;
        let parent = view.inherit.as_ref().map(|parent| full_id(&module.name, parent));
        let top = id.or(parent).and_then(|start| self.view_top(start));
        if view.inherit.is_some() && top.is_none() {
            return Some((model, None));
        }
        let top = top.unwrap_or_default();
        let kind = top.iter().find_map(|n| n.fact.kind.as_deref());
        let kind = kind.or(view.kind.as_deref());
        let unread = view.unread_parent || top.iter().any(|n| n.fact.unread_parent);
        (kind.is_some() || unread || view.primary.is_none()).then_some((model, kind))
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
        self.definer_status(self.templates.definers(name), scope)
    }

    /// Whether a view answers to `name` in scope, by the whole `key` or XML id without adding the
    /// calling module, so an undotted name can only find a key. That counts on every series, though
    /// through saas~18.3 Odoo refuses an undotted name, and from 10.0 a website lookup is exempt.
    pub fn qweb_status(&self, name: &str, scope: &Scope) -> Status<'m> {
        self.definer_status(self.qweb_definers(name), scope)
    }

    /// The modules with a view that answers to `name`, as `qweb_status` counts them.
    pub fn qweb_definers(&self, name: &str) -> impl Iterator<Item = usize> {
        let keys = self.view_keys.get(name).map(Vec::as_slice).unwrap_or_default();
        self.views.definers(name).chain(keys.iter().copied())
    }

    fn definer_status(&self, definers: impl Iterator<Item = usize>, scope: &Scope) -> Status<'m> {
        let mut definers = definers.peekable();
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
        Scope {
            records: visible.clone(),
            visible,
            demo: false,
        }
    }

    /// What `scope` of `module` grows to on the database that installs it: each module its install
    /// auto-installs, as `triggers` reads them, repeated until nothing is added. A module with
    /// `countries` stays out where the key counts, as whether it installs depends on the companies.
    /// `series` is that of `module`, and a manifest version without one takes it: both install on one Odoo.
    pub fn install_scope(&self, module: usize, series: Option<Series>) -> Scope {
        let gated = |m: &Module| m.manifest.countries && m.series.or(series).is_none_or(|s| s >= COUNTRY_GATE);
        let seeds = self.scope(module).visible;
        let visible = install_closure(self.modules, &self.by_name, seeds, series, |m| !gated(m));
        Scope {
            records: visible.clone(),
            visible,
            demo: false,
        }
    }

    /// What `scope` of `module` grows to once a runtime test has shown the `named` modules installed:
    /// their depends, and each module that auto-installs once its triggers are in that or in scope,
    /// as Odoo installs one when the last of its triggers comes in, so long as one trigger lies
    /// outside scope. One whose triggers all lie in scope owes nothing to the test, and an unguarded
    /// id of it reports. A module with `countries` joins too: the scan cannot see the companies.
    pub fn guard_scope(&self, module: usize, named: impl IntoIterator<Item = usize>, series: Option<Series>) -> Scope {
        let scope = self.scope(module).visible;
        let (proven, _) = depends_closure(self.modules, &self.by_name, named);
        let seeds = scope.iter().zip(proven).map(|(&a, b)| a || b).collect();
        let in_scope = |name: &str| self.by_name.get(name).is_some_and(|&mi| scope[mi]);
        let owed = |m: &Module| triggers(m, series).is_none_or(|t| !t.into_iter().all(in_scope));
        let visible = install_closure(self.modules, &self.by_name, seeds, series, owed);
        Scope {
            records: visible.clone(),
            visible,
            demo: false,
        }
    }

    /// `scope` of `module`, and of its `installs` what loads first, for a check that runs as `module`
    /// loads. The rest arrives too late on the database that installs them together, though one
    /// that already holds them passes.
    pub fn loaded_scope(&self, module: usize, scope: &Scope, installs: &Scope) -> Scope {
        let first = |mi: usize| self.load_rank[mi] < self.load_rank[module];
        let held = |mi: usize| scope.visible[mi] || (installs.visible[mi] && first(mi));
        let visible: Vec<bool> = (0..self.modules.len()).map(held).collect();
        Scope {
            records: visible.clone(),
            visible,
            demo: false,
        }
    }

    /// The scope of what Odoo reads as it sets up the Python fields and attributes of `module`, and
    /// of what `init()` and `_auto_init()` run. That comes ahead of the data files of `module`, so of
    /// the `ir.model.fields` records only those of its dependencies count, and a failure there
    /// refuses the install whatever the file order.
    pub fn setup_scope(&self, module: usize) -> Scope {
        let (mut records, _) = depends_closure(self.modules, &self.by_name, [module]);
        records[module] = false;
        Scope {
            records,
            ..self.scope(module)
        }
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

    /// Whether a module whose records have loaded when the reader of `scope` runs loads a data file
    /// the scan cannot read.
    pub fn lost_scope(&self, scope: &Scope) -> bool {
        let mut modules = self.modules.iter().enumerate();
        modules.any(|(mi, m)| scope.records[mi] && m.lost_data())
    }

    /// Whether `module`, or a module with it in its scope, loads a data file the scan cannot read that
    /// may hold an access row. A dependency loads first, so its rows cannot name the model.
    pub fn lost_access(&self, module: usize) -> bool {
        let mut modules = self.modules.iter().enumerate();
        modules.any(|(mi, m)| m.lost_access() && self.scope(mi).sees(module))
    }

    /// Whether the class a dotted path such as `odoo.addons.sale.models.sale_order.SaleOrder` names
    /// is a model class of the file the path imports it from, none when that addon or file is not
    /// indexed. A class without facts there is no model, since the parser keeps facts only for those.
    pub fn model_class(&self, path: &str) -> Option<bool> {
        let (addon, files, class) = class_file(path)?;
        let module = &self.modules[*self.by_name.get(addon)?];
        let file = module.python.iter().find(|f| files.contains(&f.rel))?;
        Some(file.facts.classes.iter().any(|c| c.class_name == class))
    }

    /// The definitions of a full XML id that a module of `series` can reach. A database runs one
    /// series, so the reader's series picks the spelling of a field id, whatever the manifest of the
    /// field's module says. A reader with none takes that module's, and with neither both spellings
    /// stand.
    pub fn xmlid_defs(&self, id: &str, series: Option<Series>) -> Vec<&XmlIdDef<'m>> {
        let defs = self.xmlids.get(id).into_iter().flatten();
        defs.filter(|d| d.exists_in(series.or(self.modules[d.module].series)))
            .collect()
    }

    /// The models of the records of a full XML id, as a module of `series` reads it. None when only
    /// a run tells the model of one of them.
    pub fn record_models(&self, id: &str, series: Option<Series>) -> Option<Vec<&'m str>> {
        self.xmlid_defs(id, series).iter().map(|d| d.model).collect()
    }

    /// Whether a record of `model` may be one of `comodel`: the same model, a model that inherits it
    /// by `_inherit` and so may share its ids, as `ir.actions.*` do for `ir.actions.actions`, or one
    /// on the same table. A table only a run tells, on either model, counts as the same: a wrong
    /// report costs more than a miss. None when a class of `model` is unknown to the index.
    pub fn stands_for(&self, model: &str, comodel: &str, scope: &Scope) -> Option<bool> {
        if model == comodel {
            return Some(true);
        }
        let classes = self.chain_read_from(model, scope);
        if !self.has_model(model) || classes.iter().any(|(_, c)| c.hidden_parents()) {
            return None;
        }
        let inherits = classes.iter().any(|(_, c)| c.model == comodel);
        let tables = self.table(model, scope).zip(self.table(comodel, scope));
        Some(inherits || tables.is_none_or(|(a, b)| a == b) || self.plain_base_table(comodel, scope))
    }

    /// Whether a class in scope of `model` has a plain Python base, which may set a `_table` that
    /// `table` cannot read. Only the model's own classes count: from 10.0 Odoo skips registry classes
    /// when it reads `_table`, and a mixin in the chain may have a plain base of its own, as
    /// `website.published.multi.mixin` does on `res.partner`.
    fn plain_base_table(&self, model: &str, scope: &Scope) -> bool {
        self.classes(model)
            .iter()
            .any(|(mi, c)| scope.sees(*mi) && !c.plain_bases.is_empty())
    }

    /// The model of the `ir.model` record Odoo generates under a full XML id.
    pub fn model_of_xmlid(&self, id: &str) -> Option<&'m str> {
        self.model_by_xmlid.get(id).copied()
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
        let introducers = classes.filter(|(_, c)| c.new_model).map(|(mi, _)| (*mi, ()));
        self.first_loaded(introducers).map(|(mi, _)| mi)
    }

    /// The entry, by module, that loads first. Independent modules keep index order, since Odoo's
    /// tie order between them is unknown, and so do the entries of one module.
    pub fn first_loaded<T>(&self, entries: impl IntoIterator<Item = (usize, T)>) -> Option<(usize, T)> {
        let loads_after = |first: usize, next: usize| first != next && self.reaches(first, next);
        entries
            .into_iter()
            .reduce(|first, next| if loads_after(first.0, next.0) { next } else { first })
    }

    /// Every class of `model` and of the models it inherits from: see `model_chain`.
    fn chain(&self, model: &str, scope: Option<&Scope>, delegated: bool) -> Vec<(usize, &'m ClassFact)> {
        model_chain(&self.models, model, scope, delegated)
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

    /// Whether a class in scope of `model`, or of a model it inherits or delegates to, has a parent
    /// the scan cannot see, whose fields and methods it does not know.
    pub fn hidden_parents(&self, model: &str, scope: &Scope) -> bool {
        self.chain(model, Some(scope), true)
            .iter()
            .any(|(_, c)| c.hidden_parents())
    }

    /// Whether a class in scope of `model`, or of a model it inherits or delegates to, sits in a file
    /// that does not parse, so the model may hold any field or method.
    pub fn unread_class(&self, model: &str, scope: &Scope) -> bool {
        self.chain(model, Some(scope), true).iter().any(|(_, c)| c.unread)
    }

    /// Whether a class in scope of `model`, or of a model it inherits or delegates to, may get fields
    /// from a parent the scan cannot read.
    pub fn hidden_field_parents(&self, model: &str, scope: &Scope) -> bool {
        self.unread_bases(model, scope, true, |c| &c.field_bases)
    }

    /// Whether a class in scope of `model`, or of a model it inherits, may get methods from a parent
    /// the scan cannot read. A model reached through `_inherits` gives fields, not methods.
    pub fn hidden_method_parents(&self, model: &str, scope: &Scope) -> bool {
        self.unread_bases(model, scope, false, |c| &c.plain_bases)
    }

    /// The signature of each definition of `method`, among `merged` (`merged_classes` of the model of
    /// `class`, of module `mi`), that `class` overrides: those of other modules, short of one that
    /// depends on `mi`, and of classes above `class` in its file. The order of the files of `mi` is
    /// that of the imports, which is not indexed, so its other files count neither way. None when one
    /// of them has no signature to compare.
    pub fn method_parents(
        &self,
        mi: usize,
        class: &ClassFact,
        merged: &[(usize, &'m ClassFact)],
        method: &str,
    ) -> Option<Vec<(usize, &'m Signature)>> {
        let above = self.classes_above(mi, class);
        let overridden = |m: usize, c: &ClassFact| {
            if m == mi {
                above.iter().any(|a| std::ptr::eq(a, c))
            } else {
                !self.reaches(m, mi)
            }
        };
        merged
            .iter()
            .filter(|&&(m, c)| c.methods.iter().any(|n| n == method) && overridden(m, c))
            .map(|&(m, c)| c.signature(method).map(|s| (m, s)))
            .collect()
    }

    /// Whether a class of the chain of `model` has a non-literal `_inherit` element or one of the
    /// `bases` the scan does not read. Unlike `hidden_parents`, a Python base defined above the class
    /// in its file that is another class of the chain counts as read: mixins subclass each other
    /// along their `_inherit`, and Odoo merges that class's fields either way. Every chain holds
    /// `base`, so its classes prove nothing.
    fn unread_bases(
        &self,
        model: &str,
        scope: &Scope,
        delegated: bool,
        bases: impl Fn(&ClassFact) -> &[String],
    ) -> bool {
        let chain = self.chain(model, Some(scope), delegated);
        let read = |mi: usize, class: &ClassFact, base: &str| {
            self.class_above(mi, class, base).is_some_and(|above| {
                above.model != "base" && chain.iter().any(|(_, other)| std::ptr::eq(*other, above))
            })
        };
        chain
            .iter()
            .any(|&(mi, c)| c.inherit_unknown || bases(c).iter().any(|base| !read(mi, c, base)))
    }

    /// The class `name` holds where `class` is defined, when its own file defines it above. The
    /// index keeps no imports, so a class of that name elsewhere may be another class.
    fn class_above(&self, mi: usize, class: &ClassFact, name: &str) -> Option<&'m ClassFact> {
        let above = self.classes_above(mi, class).iter();
        above.rev().find(|c| c.class_name == name)
    }

    /// The classes above `class` in its file of module `mi`.
    fn classes_above(&self, mi: usize, class: &ClassFact) -> &'m [ClassFact] {
        let mut files = self.modules[mi].python.iter().map(|f| f.facts.classes.as_slice());
        let at = |classes: &'m [ClassFact]| classes.iter().position(|c| std::ptr::eq(c, class));
        files
            .find_map(|classes| at(classes).map(|at| &classes[..at]))
            .unwrap_or_default()
    }

    /// Whether a class merged into `model` has a parent the scan cannot see. A parent reached through
    /// `_inherits` does not count: it gives the model its fields, so its hidden methods do not reach
    /// the model.
    pub fn merged_hidden_parents(&self, model: &str, scope: &Scope) -> bool {
        self.merged_classes(model, scope)
            .iter()
            .any(|(_, c)| c.hidden_parents())
    }

    /// Whether a class of `class`'s model, or a parent or delegate of it, is out of the scan's sight,
    /// so the model may hold fields the index does not list.
    pub fn unread_fields(&self, class: &ClassFact, scope: &Scope) -> bool {
        class.unread
            || self.unread_class(&class.model, scope)
            || self.hidden_field_parents(&class.model, scope)
            || self.unread_delegate(&class.model, scope)
    }

    /// Whether a class in scope of `model`, or of a model it inherits or delegates to, delegates to a
    /// parent the scan cannot read, or is such a parent, so the fields `model` has through `_inherits`
    /// are not all known. `hidden_parents` asks of methods too, which a delegate does not pass on.
    pub fn unread_delegate(&self, model: &str, scope: &Scope) -> bool {
        self.chain(model, Some(scope), true).iter().any(|(_, c)| {
            let delegates = c.inherits.iter().map(|(parent, _)| parent);
            let mut parents = delegates.chain(c.inherit.iter().map(|parent| &parent.value));
            c.inherits_unknown || c.delegate_unknown || parents.any(|parent| !self.has_model(parent))
        })
    }

    /// What `holds_fields` asks, and no plain base the scan does not read, which may hold methods
    /// whatever its fields.
    pub fn holds_model(&self, module: usize, model: &str, scope: &Scope) -> bool {
        self.holds_fields(module, model, scope) && !self.unread_bases(model, scope, true, |c| &c.plain_bases)
    }

    /// Whether `module` introduces `model` and holds all its fields: every class in scope of `model`
    /// and of the models it inherits or delegates to, and every parent they name. `base` is in every
    /// chain, so as a parent it proves nothing.
    pub fn holds_fields(&self, module: usize, model: &str, scope: &Scope) -> bool {
        let introduces = |m: &str| m == "base" || self.classes(m).iter().any(|(mi, c)| *mi == module && c.new_model);
        let chain = self.chain(model, Some(scope), true);
        model != "base"
            && introduces(model)
            && chain.iter().filter(|(_, c)| c.model != "base").all(|(mi, c)| {
                let delegates = c.inherits.iter().map(|(parent, _)| parent);
                let mut parents = delegates.chain(c.inherit.iter().map(|parent| &parent.value));
                *mi == module && parents.all(|parent| introduces(parent))
            })
            && !self.hidden_field_parents(model, scope)
            && !self.unread_delegate(model, scope)
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

    /// What the id of a field of `model` may start with in `series`.
    pub fn field_prefixes(&self, model: &str, series: Option<Series>) -> Vec<String> {
        field_id_prefixes(model, self.classes(model), series)
    }

    /// Whether `local` may be the id of a field of `class`, for a reader in `scope`, spelled from a
    /// table outside the index.
    pub fn unseen_table_id(&self, class: &ClassFact, local: &str, series: Option<Series>, scope: &Scope) -> bool {
        let known_table = |table: &str| !self.table_models(table).is_empty();
        let any_field = self.unread_fields(class, scope);
        unseen_table_id(self.classes(&class.model), class, local, series, known_table, any_field)
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

    /// `chain_classes` for a reader in `scope`. A class in a file that does not parse hides what the
    /// modules that see it get, so a module outside its reach is still checked on the model.
    pub fn chain_read_from(&self, model: &str, scope: &Scope) -> Vec<(usize, &'m ClassFact)> {
        let mut chain = self.chain_classes(model);
        chain.retain(|(mi, c)| !c.unread || scope.sees(*mi));
        chain
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

    /// Whether a class of `model` or of a model it inherits, or an `ir.model.fields` record, in any
    /// indexed module, defines `field`. A record in `scope` counts only if it has loaded by then.
    pub fn defines_field(&self, model: &str, field: &str, scope: &Scope) -> bool {
        let counts = |f: &DataField| !scope.sees(f.module) || scope.holds(f);
        self.chain(model, None, false)
            .iter()
            .any(|(_, c)| c.fields.iter().any(|f| f.name == field))
            || self.data_field(model, field, None, counts).is_some()
    }

    /// The `ir.model.fields` record that adds `field` to `model` or to a model of its chain. Odoo adds
    /// such a field before it copies delegated ones, so `_inherits` children get it. `_inherit`
    /// children do not, but the chain joins both, and staying silent there is the safe side. Only
    /// the records `counts` keeps qualify.
    fn data_field(
        &self,
        model: &str,
        field: &str,
        scope: Option<&Scope>,
        counts: impl Fn(&DataField) -> bool,
    ) -> Option<&DataField<'m>> {
        let candidates = self.data_fields.get(field)?;
        let chain = self.chain(model, scope, true);
        let on = |m: &str| m == model || chain.iter().any(|(_, c)| c.model == m);
        candidates.iter().filter(|f| counts(f)).find(|f| f.model.is_none_or(on))
    }

    /// Whether a module in `scope` gives `field` of `model` an `ir.default`. One of a demo file is no
    /// default for `demo` false: it loads after the data, and only where demo data does.
    pub fn ir_default(&self, model: &str, field: &str, scope: &Scope, demo: bool) -> bool {
        let defaults = self.ir_defaults.get(&(model, field));
        defaults.is_some_and(|d| d.iter().any(|&(mi, in_demo)| scope.sees(mi) && (demo || !in_demo)))
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

    /// `field` of `model`: the model's own definitions, which shadow those of a model it delegates
    /// to, and else the record that adds it. `Err(Some(module))` when the field exists only outside
    /// the scope.
    pub fn field(&self, model: &str, field: &str, scope: &Scope) -> Result<Option<FieldInfo<'m>>, Option<&'m str>> {
        self.field_from(model, field, scope, &mut vec![])
    }

    /// `field`, where `open` holds the related definitions being followed. One met again has no
    /// comodel: Odoo cannot resolve a related cycle either, and refuses it unless it is manual.
    fn field_from(
        &self,
        model: &str,
        field: &str,
        scope: &Scope,
        open: &mut Vec<&'m FieldFact>,
    ) -> Result<Option<FieldInfo<'m>>, Option<&'m str>> {
        if self.magic_field(field) || EXTRA_FIELDS.contains(&(model, field)) {
            return Ok(None);
        }
        let mut defs = self.merged_fields(model, scope);
        defs.retain(|d| d.2.name == field);
        if defs.is_empty() {
            defs = self.field_defs(model, field, Some(scope));
        }
        let Some((_, _, first)) = defs.first() else {
            // Odoo skips a manual field whose name a class already gave the model.
            if let Some(data) = self.data_field(model, field, Some(scope), |f| scope.holds(f)) {
                return Ok(Some(FieldInfo {
                    ftype: data.ftype,
                    comodel: data.comodel.map(str::to_owned),
                }));
            }
            let python = self.field_defs(model, field, None).first().map(|(mi, _, _)| *mi);
            // A record in scope that has not loaded yet is no matter of `depends`.
            let elsewhere = |f: &DataField| !scope.sees(f.module);
            let definer = python.or_else(|| self.data_field(model, field, None, elsewhere).map(|d| d.module));
            return Err(definer.map(|mi| self.modules[mi].name.as_str()));
        };
        let comodel = defs
            .iter()
            .find_map(|(_, _, f)| f.comodel.as_ref().map(|c| c.value.clone()))
            .or_else(|| {
                let (_, class, related) = defs.iter().find(|(_, _, f)| f.related.is_some())?;
                let path = &related.related.as_ref()?.value;
                if open.iter().any(|f| std::ptr::eq(*f, *related)) {
                    return None;
                }
                open.push(related);
                let comodel = self.related_comodel(&class.model, path, scope, open);
                // A path may repeat a field, so only the fields being followed stay open.
                open.pop();
                comodel
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
            Some((head, last)) => (self.path_comodel(model, head, scope)?, last),
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
        self.related_comodel(model, path, scope, &mut vec![])
    }

    fn related_comodel(&self, model: &str, path: &str, scope: &Scope, open: &mut Vec<&'m FieldFact>) -> Option<String> {
        let mut model = model.to_owned();
        for segment in path.split('.') {
            let info = self.field_from(&model, segment, scope, open).ok()??;
            model = info.comodel?;
        }
        Some(model)
    }

    /// Whether `BaseModel`, `object` or a class other than `except`, of `model` or of a model it
    /// inherits by `_inherit`, defines `method`. A parent it delegates to does not count, since
    /// `super()` follows the class bases, which Odoo builds from `_inherit` and `base`. Modules
    /// outside the scope count: an override of a method from a module that is not in depends is
    /// called only by that module.
    pub fn has_parent_method(&self, model: &str, method: &str, except: &ClassFact, series: Option<Series>) -> bool {
        BASE_MODEL_METHODS.contains(&method)
            || BASE_MODEL_EXTRA.contains(&method)
            || old_base_method(method, series)
            || BASE_MODEL_DUNDERS.contains(&method)
            || self
                .chain_classes(model)
                .iter()
                .any(|(_, c)| !std::ptr::eq(*c, except) && c.methods.iter().any(|m| m == method))
    }

    /// Whether a class in scope of `model` or of a model it inherits by `_inherit` defines `method`.
    /// A parent it delegates to does not count, since Odoo looks the method up on the model's
    /// records, which get the parent's fields, not its methods.
    pub fn has_method(&self, model: &str, method: &str, scope: &Scope) -> bool {
        BASE_METHODS.contains(&method)
            || self
                .merged_classes(model, scope)
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

    /// Whether a client may call `method` on `model`: a method of `BaseModel`, the dated ones in
    /// `series`, or of a class of the model or of a model it inherits by `_inherit`, in scope or in
    /// a module that is always installed. A parent it delegates to does not count, since RPC looks
    /// the name up on the model's class, which gets the parent's fields, not its methods. The scoped
    /// walk skips a class outside the scope with its `_inherit`, so a mixin of an always-installed
    /// module that only such a class adds is found only by the unscoped walk; without the
    /// always-installed arm it would be `Hidden` under a module the caller has.
    pub fn rpc_method(&self, model: &str, method: &str, scope: &Scope, series: Option<Series>) -> Status<'m> {
        if BASE_MODEL_METHODS.contains(&method)
            || base_rpc_method(method, series)
            || self.has_method(model, method, scope)
        {
            return Status::Visible;
        }
        let defining: Vec<usize> = self
            .chain_classes(model)
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

    /// Whether a button or a `<function>` finds `method` on `model`: any method of `BaseModel`, or
    /// what `rpc_method` finds. Python looks both up on the model's class.
    pub fn data_method(&self, model: &str, method: &str, scope: &Scope, series: Option<Series>) -> Status<'m> {
        if BASE_MODEL_EXTRA.contains(&method) || old_base_method(method, series) {
            return Status::Visible;
        }
        self.rpc_method(model, method, scope, series)
    }

    /// Whether a class in scope of `model` or of a model it inherits gives `kind` a default view. The
    /// `base` module does not count: its calendar default depends on the fields of the model.
    pub fn default_view(&self, model: &str, kind: &str, scope: &Scope) -> bool {
        let method = format!("_get_default_{kind}_view");
        self.merged_classes(model, scope)
            .iter()
            .filter(|(mi, _)| self.modules[*mi].name != "base")
            .any(|(_, c)| c.methods.contains(&method))
    }

    /// Whether a class in scope of `model` or of a model it inherits decorates `method` with
    /// `@api.private`: Odoo refuses a method that any class of the MRO marks.
    pub fn private_method(&self, model: &str, method: &str, scope: &Scope) -> bool {
        self.merged_classes(model, scope)
            .iter()
            .any(|(_, c)| c.private_methods.iter().any(|m| m == method))
    }

    /// Whether a class in scope of `model`, or of a model it inherits, lists in `_inherit` a model
    /// no class of the index defines, whose methods it does not know.
    pub fn unindexed_parent(&self, model: &str, scope: &Scope) -> bool {
        self.merged_classes(model, scope)
            .iter()
            .flat_map(|(_, c)| &c.inherit)
            .any(|parent| !self.has_model(&parent.value))
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

    /// The type of the view at the root of `id`'s inheritance, which Odoo gives every view of the
    /// tree. None when a view on the way is not indexed. A top with a parent out of reach is no root,
    /// so what its own arch opens with, such as a `<field>` spec, is no type.
    pub fn view_kind(&self, id: String) -> Option<&'m str> {
        let top = self.view_top(id)?;
        if top.iter().any(|n| n.fact.unread_parent) {
            return None;
        }
        top.iter().find_map(|n| n.fact.kind.as_deref())
    }

    /// The nodes of the view at the root of `id`'s inheritance.
    fn view_top(&self, mut id: String) -> Option<&[ViewNode<'m>]> {
        for _ in 0..32 {
            let nodes = self.views.nodes.get(&id)?;
            match nodes.iter().find_map(|n| n.inherit.clone()) {
                Some(parent) => id = parent,
                None => return Some(nodes),
            }
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
                tree.subviews.extend(node.fact.subviews.iter());
                tree.valued.extend(node.fact.valued.iter().map(|v| ArchValued {
                    elements: v.elements.iter().map(|e| located(e, module)).collect(),
                    attrs: v.attrs.clone(),
                }));
                tree.emptied.extend(node.fact.emptied.iter().map(|e| ArchEmptied {
                    attr: e.attr,
                    target: e.target.as_ref().map(|t| located(t, module)),
                }));
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
        let Some(tables) = literal_tables(classes).collect() else {
            continue;
        };
        for table in model_tables(model, tables) {
            by_table.entry(table).or_default().push(model);
        }
    }
    by_table
}

/// Each `_table` of `classes`, as None where only a run tells the value.
fn literal_tables<'c>(classes: &[(usize, &'c ClassFact)]) -> impl Iterator<Item = Option<&'c str>> {
    let set = classes.iter().filter_map(|(_, c)| c.table.as_ref());
    set.map(|t| match t {
        OptionValue::Str(t) => Some(t.as_str()),
        _ => None,
    })
}

/// `set`, the tables a model's classes name, or else its name with dots made underscores.
fn model_tables(model: &str, set: Vec<&str>) -> Vec<String> {
    let mut tables: Vec<String> = set.into_iter().map(str::to_owned).collect();
    if tables.is_empty() {
        tables.push(model.replace('.', "_"));
    }
    tables.sort_unstable();
    tables.dedup();
    tables
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        BASE_MODEL_DUNDERS, BASE_MODEL_EXTRA, BASE_MODEL_METHODS, BASE_PRIVATE_METHODS, BASE_RPC_METHODS,
        FIELD_CLASSES, FieldHolder, FieldSpelling, Index, Module, OLD_BASE_METHODS, PlacedWrite, Scope, XmlIdDef,
        base_private_method, base_rpc_method, class_file, field_id_prefixes, fold_writes, last_text, old_base_method,
        unseen_table_id,
    };
    use crate::manifest;
    use crate::python;
    use crate::series::Series;
    use crate::xml;

    #[test]
    fn every_field_class_is_its_ttype_in_camel_case() {
        for (ttype, class) in FIELD_CLASSES {
            assert_eq!(class.to_lowercase(), ttype.replace('_', ""));
        }
    }

    #[test]
    fn a_field_id_is_spelled_by_table_before_saas_11_2() {
        let facts = python::analyze(
            "class A(models.Model):\n    _name = 'a.b'\n\n\
             class B(models.Model):\n    _name = 'c.d'\n    _table = 't'\n\n\
             class C(models.Model):\n    _inherit = 'c.d'\n    _table = 'u'\n\n\
             class D(models.Model):\n    _name = 'e.f'\n    _table = 't'\n\n\
             class E(models.Model):\n    _inherit = 'e.f'\n    _table = TABLE\n",
            Series::parse("10.0"),
        );
        let classes = |model: &str| -> Vec<_> {
            let of_model = facts.classes.iter().filter(|c| c.model == model);
            of_model.map(|c| (0, c)).collect()
        };
        let prefixes =
            |model, series: Option<&str>| field_id_prefixes(model, &classes(model), series.and_then(Series::parse));
        assert_eq!(prefixes("a.b", Some("saas~11.1")), ["field_a_b_"]);
        assert_eq!(prefixes("a.b", Some("saas~11.2")), ["field_a_b__"]);
        assert_eq!(prefixes("a.b", None), ["field_a_b_", "field_a_b__"]);
        assert_eq!(prefixes("c.d", Some("saas~11.1")), ["field_t_", "field_u_"]);
        assert_eq!(prefixes("c.d", Some("saas~11.2")), ["field_c_d__"]);
        assert_eq!(prefixes("c.d", None), ["field_t_", "field_u_", "field_c_d__"]);
        assert_eq!(prefixes("e.f", Some("10.0")), ["field_t_"]);
    }

    #[test]
    fn a_table_outside_the_index_names_only_declared_fields() {
        let source = "class A(models.Model):\n    _inherit = 'a.b'\n    stock = fields.Integer()\n";
        let facts = python::analyze(source, Series::parse("10.0"));
        let class = &facts.classes[0];
        let id = |local, series: Option<&str>, any_field| {
            let series = series.and_then(Series::parse);
            unseen_table_id(&[(0, class)], class, local, series, |t| t == "known", any_field)
        };
        assert!(id("field_rack_stock", Some("saas~11.1"), false));
        assert!(!id("field_rack_stock", Some("saas~11.2"), false));
        assert!(!id("field_rack_stock", Some("saas~11.2"), true));
        assert!(id("field_rack_stock", None, false));
        assert!(!id("field_known_stock", Some("10.0"), false));
        assert!(id("field_known_stock", Some("10.0"), true));
        assert!(!id("field_rack_other", Some("10.0"), false));
        assert!(id("field_rack_other", Some("10.0"), true));
        assert!(!id("field_a_b__stock", Some("10.0"), false));
        assert!(!id("field_stock", Some("10.0"), false));
        assert!(!id("field__stock", Some("10.0"), false));
        assert!(!id("rack_stock", Some("10.0"), false));
        assert!(!id("rack_stock", Some("10.0"), true));

        let source = "class B(models.Model):\n    _name = 'a.b'\n";
        let introduced = python::analyze(source, Series::parse("10.0"));
        let classes = [(0, &introduced.classes[0]), (0, class)];
        assert!(!unseen_table_id(
            &classes,
            class,
            "field_rack_stock",
            Series::parse("10.0"),
            |_| false,
            true
        ));

        let source = "class C(models.Model):\n    _inherit = 'a.b'\n    _table = 'rack'\n";
        let tabled = python::analyze(source, Series::parse("10.0"));
        let classes = [(0, class), (0, &tabled.classes[0])];
        assert!(!unseen_table_id(
            &classes,
            class,
            "field_rack_stock",
            Series::parse("10.0"),
            |_| false,
            true
        ));
    }

    #[test]
    fn a_field_id_holds_in_the_series_of_its_spelling() {
        let def = |spelling| XmlIdDef {
            module: 0,
            model: None,
            spelling,
            holder: None,
        };
        let (old, new) = (Series::parse("saas~11.1"), Series::parse("saas~11.2"));
        assert!(def(Some(FieldSpelling::ByTable)).exists_in(old));
        assert!(!def(Some(FieldSpelling::ByTable)).exists_in(new));
        assert!(def(Some(FieldSpelling::ByName)).exists_in(new));
        assert!(!def(Some(FieldSpelling::ByName)).exists_in(old));
        assert!(def(Some(FieldSpelling::ByName)).exists_in(None));
        assert!(def(None).exists_in(old));
    }

    #[test]
    fn an_inherited_field_id_holds_by_series() {
        let holds = |holder: FieldHolder, series| holder.holds_in(Series::parse(series).unwrap());
        // Series, then whether the introducer, the mixin module, an extender and a delegator hold it.
        let bands = [
            ("9.0", true, false, true, true),
            ("10.0", false, true, false, true),
            ("saas~11.2", false, true, false, true),
            ("saas~11.3", true, false, true, true),
            ("saas~11.4", true, false, true, true),
            ("saas~11.5", true, false, false, false),
            ("saas~13.3", true, false, false, false),
            ("saas~13.4", true, false, true, true),
            ("20.0", true, false, true, true),
        ];
        for (series, introducer, mixin, extender, delegator) in bands {
            assert_eq!(holds(FieldHolder::Introducer, series), introducer, "{series}");
            assert_eq!(holds(FieldHolder::Mixin, series), mixin, "{series}");
            assert_eq!(holds(FieldHolder::Extender, series), extender, "{series}");
            assert_eq!(holds(FieldHolder::Delegator, series), delegator, "{series}");
        }
        let def = |holder| XmlIdDef {
            module: 0,
            model: None,
            spelling: None,
            holder,
        };
        assert!(def(Some(FieldHolder::Mixin)).exists_in(None));
        assert!(!def(Some(FieldHolder::Mixin)).exists_in(Series::parse("13.0")));
        assert!(def(None).exists_in(Series::parse("13.0")));
    }

    #[test]
    fn folds_the_writes_of_an_id_and_keeps_the_rest_apart() {
        let facts = xml::analyze(
            r#"<odoo>
                <record id="f" model="ir.model.fields">
                    <field name="name">x_old</field>
                    <field name="ttype">char</field>
                </record>
                <record model="ir.model.fields"><field name="name">x_idless</field></record>
                <record model="ir.model.fields"><field name="name">x_other_idless</field></record>
                <record id="a.f" model="ir.model.fields"><field name="name">x_new</field></record>
            </odoo>"#,
            None,
        );
        let elsewhere = xml::analyze(r#"<odoo><record id="f" model="ir.model.fields"/></odoo>"#, None);
        let placed = |module, name, fact| {
            let demo = false;
            (name, PlacedWrite { module, demo, fact })
        };
        let writes = facts.writes.iter().map(|w| placed(0, "a", w));
        let records = fold_writes(writes.chain(elsewhere.writes.iter().map(|w| placed(1, "b", w))));
        let names: Vec<_> = records.iter().map(|r| last_text(r, "name")).collect();
        assert_eq!(names, [Some("x_new"), Some("x_idless"), Some("x_other_idless"), None]);
        assert_eq!(last_text(&records[0], "ttype"), Some("char"));
    }

    #[test]
    fn maps_a_class_path_to_its_files() {
        let files = |dir: &str| [format!("{dir}.py"), format!("{dir}/__init__.py")];
        assert_eq!(
            class_file("odoo.addons.sale.models.sale_order.SaleOrder"),
            Some(("sale", files("models/sale_order"), "SaleOrder"))
        );
        assert_eq!(
            class_file("odoo.addons.base.wizard.LabelWizard"),
            Some(("base", files("wizard"), "LabelWizard"))
        );
        assert_eq!(class_file("odoo.addons.base.LabelWizard"), None);
        assert_eq!(class_file("odoo.models.BaseModel"), None);
    }

    fn private(name: &str, series: &str) -> bool {
        base_private_method(name, Series::parse(series))
    }

    #[test]
    fn every_private_method_starts_private_at_its_row() {
        let rows = [
            ("concat", "15.0", "16.0"),
            ("grouped", "16.4", "17.0"),
            ("sudo", "17.4", "18.0"),
            ("browse", "18.1", "18.2"),
            ("lock_for_update", "18.2", "18.3"),
            ("try_lock_for_update", "18.4", "19.0"),
        ];
        for (name, before, first) in rows {
            assert!(!private(name, before), "{name} on {before}");
            assert!(private(name, first), "{name} on {first}");
        }
        assert!(private("try_lock_for_update", "20.0"));
    }

    #[test]
    fn saas_series_without_the_decorator_report_only_init() {
        for series in ["16.1", "16.2", "16.3", "16.4", "17.1", "17.2", "17.3"] {
            assert!(!private("ensure_one", series), "{series}");
            assert!(private("init", series), "{series}");
        }
        assert!(private("ensure_one", "17.4"));
    }

    #[test]
    fn init_is_refused_from_10() {
        assert!(!private("init", "9.0"));
        assert!(private("init", "10.0"));
    }

    #[test]
    fn unknown_series_reports_only_init() {
        assert!(base_private_method("init", None));
        assert!(!base_private_method("ensure_one", None));
    }

    #[test]
    fn private_methods_are_known_to_data_method() {
        let names = BASE_PRIVATE_METHODS.iter().flat_map(|(_, names)| *names);
        for name in names {
            assert!(
                BASE_MODEL_METHODS.contains(name) || BASE_MODEL_EXTRA.contains(name),
                "{name} would raise OD312 on an earlier series"
            );
        }
    }

    fn rpc(name: &str, series: &str) -> bool {
        base_rpc_method(name, Series::parse(series))
    }

    #[test]
    fn every_rpc_method_starts_at_its_row() {
        let rows = [
            ("get_external_id", "8.0", "9.0"),
            ("get_metadata", "8.0", "9.0"),
            ("concat", "9.0", "10.0"),
            ("union", "9.0", "10.0"),
            ("load_views", "9.0", "10.0"),
            ("flush", "12.4", "12.5"),
            ("get_property_definition", "16.4", "17.0"),
            ("try_lock_for_update", "18.2", "18.3"),
            ("check_field_access", "19.0", "19.1"),
            ("has_field_access", "19.0", "19.1"),
        ];
        for (name, before, first) in rows {
            assert!(!rpc(name, before), "{name} on {before}");
            assert!(rpc(name, first), "{name} on {first}");
        }
    }

    #[test]
    fn every_rpc_method_stops_at_its_row() {
        let rows = [
            ("resolve_2many_commands", "13.2", "13.3"),
            ("resolve_o2m_commands_to_record_dicts", "13.2", "13.3"),
            ("compute_concurrency_field", "14.3", "14.4"),
            ("compute_concurrency_field_with_access", "14.3", "14.4"),
            ("get_access_action", "15.1", "15.2"),
            ("view_init", "15.1", "15.2"),
            ("fields_view_get", "15.4", "16.0"),
            ("get_empty_list_help", "15.4", "16.0"),
            ("get_formview_action", "15.4", "16.0"),
            ("get_formview_id", "15.4", "16.0"),
            ("view_header_get", "15.4", "16.0"),
            ("load_views", "15.4", "16.0"),
            ("fields_get_keys", "16.1", "16.2"),
            ("get_xml_id", "16.1", "16.2"),
            ("invalidate_cache", "16.1", "16.2"),
            ("recompute", "16.1", "16.2"),
            ("refresh", "16.1", "16.2"),
            ("flush", "16.1", "16.2"),
            ("user_has_groups", "17.1", "17.2"),
            ("name_get", "17.2", "17.3"),
            ("clear_caches", "18.0", "18.1"),
            ("is_transient", "18.4", "19.0"),
            ("try_lock_for_update", "18.4", "19.0"),
        ];
        for (name, last, after) in rows {
            assert!(rpc(name, last), "{name} on {last}");
            assert!(!rpc(name, after), "{name} on {after}");
        }
    }

    #[test]
    fn rpc_methods_follow_the_private_series() {
        assert!(rpc("concat", "15.4"));
        assert!(!rpc("concat", "16.0"));
        assert!(rpc("concat", "16.1"));
        assert!(rpc("union", "17.1"));
        assert!(!rpc("union", "17.4"));
    }

    #[test]
    fn unknown_series_counts_every_listed_rpc_method() {
        assert!(base_rpc_method("name_get", None));
        assert!(base_rpc_method("get_formview_action", None));
        assert!(base_rpc_method("concat", None));
        assert!(!base_rpc_method("ids", None));
        assert!(!base_rpc_method("lock_for_update", None));
    }

    #[test]
    fn every_public_extra_is_dated_or_named_out() {
        let dated: Vec<&str> = BASE_RPC_METHODS.iter().flat_map(|(_, names)| *names).copied().collect();
        for name in BASE_MODEL_EXTRA.iter().filter(|name| !name.starts_with('_')) {
            let out = ["ids", "lock_for_update"].contains(name);
            assert_ne!(dated.contains(name), out, "{name}");
        }
        for name in dated {
            assert!(BASE_MODEL_EXTRA.contains(&name), "{name} is missing from the flat list");
        }
    }

    #[test]
    fn old_base_methods_stop_at_their_last_series() {
        let has = |name: &str, series: &str| old_base_method(name, Series::parse(series));
        assert!(has("import_data", "9.0") && !has("import_data", "10.0"));
        assert!(has("create_workflow", "10.0") && !has("create_workflow", "11.0"));
        let selection = "_check_selection_field_value";
        assert!(has(selection, "saas~11.1") && !has(selection, "saas~11.2"));
        let prepare = "_parent_store_create_prepare";
        assert!(!has(prepare, "saas~11.1") && has(prepare, "saas~11.2") && !has(prepare, "saas~11.3"));
        let count = "_check_record_rules_result_count";
        assert!(has(count, "saas~11.4") && !has(count, "saas~11.5"));
        assert!(!old_base_method("create_workflow", None));
    }

    fn module(name: &str, text: &str) -> Module {
        let manifest = manifest::parse(text).unwrap();
        Module {
            name: name.to_owned(),
            dir: PathBuf::new(),
            manifest_path: PathBuf::new(),
            series: manifest.series(),
            manifest,
            checked: true,
            python: vec![],
            data: vec![],
            js: vec![],
            templates: vec![],
            sql_views: vec![],
        }
    }

    #[test]
    fn an_heir_of_the_module_and_of_a_proven_one_joins_the_guard_scope() {
        let manifest = |depends: &str, extra: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]{extra}}}");
        let modules = [
            module("base", &manifest("", "")),
            module("main", &manifest("'base'", "")),
            module("side", &manifest("'base'", "")),
            module("far", &manifest("'base'", "")),
            module("heir", &manifest("'main', 'side'", ", 'auto_install': True")),
            module(
                "local",
                &manifest("'main', 'side'", ", 'auto_install': True, 'countries': ['be']"),
            ),
            module("other", &manifest("'side', 'far'", ", 'auto_install': True")),
            module("own", &manifest("'main'", ", 'auto_install': True")),
            module("own_heir", &manifest("'own', 'side'", ", 'auto_install': True")),
        ];
        let index = Index::build(&modules, None);
        let sees = |scope: &Scope| -> Vec<&str> {
            let seen = modules.iter().enumerate().filter(|(mi, _)| scope.sees(*mi));
            seen.map(|(_, m)| m.name.as_str()).collect()
        };
        let series = Series::parse("18.0");
        assert_eq!(
            sees(&index.guard_scope(1, [2], series)),
            ["base", "main", "side", "heir", "local"]
        );
        assert_eq!(sees(&index.guard_scope(1, [], series)), ["base", "main"]);
        assert_eq!(sees(&index.guard_scope(1, [3], series)), ["base", "main", "far"]);
        assert_eq!(sees(&index.install_scope(1, series)), ["base", "main", "own"]);
    }

    #[test]
    fn old_base_methods_end_before_the_flat_lists_start() {
        for (spec, names) in OLD_BASE_METHODS {
            for series in ["saas~11.5", "12.0", "20.0"] {
                assert!(!crate::series::matches(spec, Series::parse(series).unwrap()), "{spec}");
            }
            for name in *names {
                assert!(
                    ![BASE_MODEL_METHODS, BASE_MODEL_EXTRA, BASE_MODEL_DUNDERS]
                        .iter()
                        .any(|l| l.contains(name)),
                    "{name} would count in every series"
                );
            }
        }
    }
}
