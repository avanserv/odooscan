use std::borrow::Cow;
use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{
    ActionFact, Annotation, ArchElement, ArchEmptied, ArchSubview, ArchValued, BaseSource, ClassFact, ComponentItem,
    DataFacts, FieldFact, FlowSummary, Heritage, JsClass, JsFacts, JsName, ModelKind, OptionValue, OwlTemplate, Pos,
    PropItem, PyFacts, Receiver, RecordWrite, ReportFact, RouteKey, Rows, RuleFact, ServerFact, Signature, Static,
    StaticWrite, UNREAD_TYPE, ViewFact, WrittenValue, mangled, private_name,
};
use crate::js::{self, OWL_MODULE};
use crate::manifest::{AutoInstall, DEPENDS_BASE};
use crate::mentions::{HOOK_PREFIXES, Words};
use crate::project::{Module, SourceFile, Unindexed, builds_model_name, code_loaded, names_models_at_runtime};
use crate::series::{self, Series};
use crate::{bundles, imports, routing};

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
/// it `field_<table>_<field>` (`0c261df33d14`; 18.0 `odoo/addons/base/models/ir_model.py:70`).
const FIELD_ID_BY_NAME: Series = Series { major: 11, minor: 2 };

/// The fields of `MAGIC_FIELDS` Odoo creates a column for when the model logs access: the ORM
/// computes the others.
const LOG_COLUMNS: &[&str] = &["create_uid", "create_date", "write_uid", "write_date"];

/// The registry categories the web client looks field widgets, view widgets, client actions,
/// services and tours up in (18.0 `addons/web/static/src/views/fields/field.js:15`,
/// `addons/web/static/src/views/widgets/widget.js:5`,
/// `addons/web/static/src/webclient/actions/action_service.js:54`,
/// `addons/web/static/src/env.js:45`,
/// `addons/web_tour/static/src/tour_service/tour_service.js:62`).
pub const FIELD_REGISTRY: &str = "fields";
pub const VIEW_WIDGET_REGISTRY: &str = "view_widgets";
pub const ACTION_REGISTRY: &str = "actions";
pub const SERVICE_REGISTRY: &str = "services";
pub const TOUR_REGISTRY: &str = "web_tour.tours";

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

/// The members of `BaseModel` outside `BASE_MODEL_METHODS`, by `def` or class-level alias, read
/// off the heads 12.0 to 20.0 and saas~11.5 to saas~19.4: a button or a `<function>` may name any,
/// and a `super()` call any but `BASE_MODEL_VALUES`.
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

/// The members of `BASE_MODEL_EXTRA` that Odoo binds to a property or a plain value, never a
/// method, in every series that has them (`BASE_VALUE_SERIES` dates those `BaseModel` lacks in some,
/// and from saas~18.3 the `_transient_max_*` pair is on `TransientModel`): none of their values is
/// callable, so a `super()` call to one raises `TypeError`. They stay in `BASE_MODEL_EXTRA`, since a
/// button finds them as `getattr` does.
const BASE_MODEL_VALUES: &[&str] = &[
    "_cache",
    "_constraint_methods",
    "_context",
    "_cr",
    "_onchange_methods",
    "_ondelete_methods",
    "_origin",
    "_populate_dependencies",
    "_populate_sizes",
    "_table_sql",
    "_transient_max_count",
    "_transient_max_hours",
    "_uid",
    "ids",
];

/// The members of `BASE_MODEL_VALUES` that `BaseModel` lacks in some series, by the range that has
/// them; every other one holds from 9.0. `d0bb596f1d51` adds `_origin`, `4c0182dafa58` and
/// `052f4aad1e08` add and drop the `_populate_*` pair, `1c8a95809852` adds `_ondelete_methods`,
/// `8a34849b5c2a` adds `_table_sql`. 9.0 sets the `_transient_max_*` pair on the registry class of a
/// transient model alone, which `super()` skips; 10.0 binds it on `BaseModel` and saas~18.3 moves it
/// to `TransientModel` (20.0 `odoo/orm/models_transient.py:23-25`).
const BASE_VALUE_SERIES: &[(&str, &[&str])] = &[
    (">=saas~12.4", &["_origin"]),
    (">=saas~13.2,<18.0", &["_populate_dependencies", "_populate_sizes"]),
    (">=saas~14.1", &["_ondelete_methods"]),
    (">=saas~17.1", &["_table_sql"]),
    (">=10.0,<saas~18.3", &["_transient_max_count", "_transient_max_hours"]),
];

/// Whether `super()` finds `method` as a method of `BaseModel` or `object` in `series`.
fn base_parent_method(method: &str, series: Option<Series>) -> bool {
    BASE_MODEL_METHODS.contains(&method)
        || (BASE_MODEL_EXTRA.contains(&method) && !BASE_MODEL_VALUES.contains(&method))
        || old_base_method(method, series)
        || BASE_MODEL_DUNDERS.contains(&method)
}

/// The methods of `BaseModel` gone by saas~11.5, by the range of series that has them, read off the
/// heads 9.0 to saas~11.4, but for the saas branches `Series` cannot name. `16c26525a725` renames
/// `_set_default_value_on_column` and `bc4d10f48d54` drops `_read_group_get_domain`. Not in
/// `BASE_MODEL_EXTRA`, since a call to one fails on any later series and must stay a report.
/// `_all_columns` stays out: a property on 9.0, not a method.
const OLD_BASE_METHODS: &[(&str, &[&str])] = &[
    ("<=9.saas~9", &["_set_default_value_on_column"]),
    ("<=9.saas~10", &["_read_group_get_domain"]),
    (
        "<=9.saas~11",
        &[
            "_inherits_reload",
            "_init_function_fields",
            "_m2m_raise_or_create_relation",
            "_o2m_raise_on_missing_reference",
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

/// Series that refuse a button on `init`: 9.0 to 9.saas~11 refuse only a leading `_`.
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

/// Series whose RPC refuses a method whose class attribute is the instance attribute, so one that
/// binds to no records (20.0 `odoo/orm/models.py:215`, commit `1c15baf8d8e8`; 18.0
/// `odoo/service/model.py:29-46` lacks the test).
const UNBOUND_REFUSED: &str = ">=19.0";

/// The public methods of `BaseModel` that bind to no records in every series of `UNBOUND_REFUSED`
/// (20.0 `odoo/orm/models.py:5228`). When adding a series, reread its class methods.
const BASE_UNBOUND_METHODS: &[&str] = &["is_transient"];

/// The first series with `CachedModel`, so a model on it runs on this series or a later one.
pub(crate) const CACHED_MODEL_SINCE: Series = Series { major: 19, minor: 1 };

/// The methods `CachedModel` defines, by the range of series that has them (20.0
/// `odoo/orm/models_cached.py:30-63`). `_fetch_field` and the `_clear_cache_*` pair stay out:
/// `BaseModel` has them in every series that has `CachedModel`.
const CACHED_MODEL_METHODS: &[(&str, &[&str])] = &[(">=saas~19.1", &["_cached_data"]), (">=saas~19.4", &["get_all"])];

/// The plain values `CachedModel` binds in every series that has it (20.0
/// `odoo/orm/models_cached.py:19-24`): a field must not replace them, a button or a `<function>`
/// finds them as `getattr` does, and a `super()` call to one raises `TypeError`.
const CACHED_MODEL_VALUES: &[&str] = &["_cached_data_domain", "_cached_data_fields"];

/// The `@api.private` methods of `CachedModel`, by the range of series that decorates them (20.0
/// `odoo/orm/models_cached.py:60`).
const CACHED_PRIVATE_METHODS: &[(&str, &[&str])] = &[(">=saas~19.4", &["get_all"])];

/// Whether a row of `rows` lists `name` for `series`. A module of unknown series may run on any, so
/// every row counts.
pub(crate) fn dated_member(rows: &[(&str, &[&str])], name: &str, series: Option<Series>) -> bool {
    rows.iter()
        .any(|(spec, names)| names.contains(&name) && series.is_none_or(|s| series::matches(spec, s)))
}

/// Whether `CachedModel` defines the method `name` in `series`, or in any series when unknown.
pub(crate) fn cached_model_method(name: &str, series: Option<Series>) -> bool {
    dated_member(CACHED_MODEL_METHODS, name, series)
}

/// Whether `CachedModel` binds the value `name` in `series`, or in any series that has it when
/// unknown. A series before it may have a class of its own named so, whose values are not these.
pub(crate) fn cached_model_value(name: &str, series: Option<Series>) -> bool {
    CACHED_MODEL_VALUES.contains(&name) && series.is_none_or(|s| s >= CACHED_MODEL_SINCE)
}

/// Whether `BaseModel` binds `name` to a property or a plain value in `series`. A module of unknown
/// series has only the values of every series.
pub(crate) fn base_model_value(name: &str, series: Option<Series>) -> bool {
    let range = BASE_VALUE_SERIES.iter().find(|(_, names)| names.contains(&name));
    BASE_MODEL_VALUES.contains(&name) && range.is_none_or(|(spec, _)| series.is_some_and(|s| series::matches(spec, s)))
}

/// The public methods of `BaseModel` that `BASE_MODEL_METHODS` lacks, by the range of series that
/// has them, so OD604 keeps reporting a call a series cannot answer. Names only, read off each
/// branch head: when adding a series, reread the methods of `BaseModel` and date what changed.
/// `ids` stays out as a property and `lock_for_update` as private since it appeared.
/// `OLD_BASE_METHODS` stays out: its public names end at 10.0, and `_rpc`, the oldest client call
/// OD604 reads, starts on 11.0.
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
    (">=9.saas~11,<=15.4", &["load_views"]),
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
    !base_private_method(name, series) && dated_member(BASE_RPC_METHODS, name, series)
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
pub(crate) fn class_file(path: &str) -> Option<(&str, [String; 2], &str)> {
    let (addon, rest) = path.strip_prefix("odoo.addons.")?.split_once('.')?;
    let (module, class) = rest.rsplit_once('.')?;
    let dir = module.replace('.', "/");
    Some((addon, [format!("{dir}.py"), format!("{dir}/__init__.py")], class))
}

/// The absolute path of what an import of `path`, `level` dots up from the file `rel` of `addon`,
/// names; none for a level past the addon's root.
fn absolute_import(addon: &str, rel: &str, level: u32, path: &str) -> Option<String> {
    if level == 0 {
        return Some(path.to_owned());
    }
    // A file's package is its directory, `__init__.py` included, and each dot past the first goes
    // one package up.
    let mut package: Vec<&str> = rel.split('/').collect();
    package.pop();
    let up = usize::try_from(level - 1).ok()?;
    package.truncate(package.len().checked_sub(up)?);
    let package = package.iter().map(|p| format!(".{p}")).collect::<String>();
    Some(format!("odoo.addons.{addon}{package}.{path}"))
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

/// Whether a view that may have the models `may`, None when any, is a view of one of `models`, a
/// view naming none matching None.
fn of_models(may: Option<&[Option<&str>]>, models: &[Option<&str>]) -> bool {
    may.is_none_or(|may| may.iter().any(|m| models.contains(m)))
}

/// Whether a stray that may have the models `may` may extend a tree of `models`, either None when
/// any.
fn stray_on(may: Option<&[Option<&str>]>, models: Option<&[Option<&str>]>) -> bool {
    models.is_none_or(|models| of_models(may, models))
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

/// The definitions named `field` in the classes of `chain`, in its order.
fn defs_named<'m>(chain: Vec<(usize, &'m ClassFact)>, field: &str) -> Vec<FieldDef<'m>> {
    chain
        .into_iter()
        .flat_map(|(mi, c)| c.fields.iter().filter(|f| f.name == field).map(move |f| (mi, c, f)))
        .collect()
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

/// The name Odoo registers a script as: `@module/` and its path from `static/src`, or
/// `@module/../tests/` and its path from `static/tests` (18.0 `odoo/tools/js_transpiler.py:61-96`).
pub fn js_module_path(module: &str, rel: &str) -> Option<String> {
    let path = rel.strip_suffix(".js")?;
    if let Some(src) = path.strip_prefix("static/src/") {
        return Some(format!("@{module}/{src}"));
    }
    Some(format!("@{module}/../tests/{}", path.strip_prefix("static/tests/")?))
}

/// The URL of a module's file, `module/rel`, as the importer of a relative specifier.
pub fn js_url(module: &str, rel: &str) -> String {
    format!("{module}/{rel}")
}

/// The module name of the URL `segments`, `module/static/...` with `.js` or without, when the scan
/// reads a script there: odooscan skips the libraries under `static/lib`, which Odoo names
/// `@module/../lib/...`, and the rest is no module.
fn url_module_path(segments: &[&str]) -> Option<String> {
    let (module, folder, rest) = match segments {
        [module, "static", folder @ ("src" | "tests"), rest @ ..] if !rest.is_empty() => (module, folder, rest),
        _ => return None,
    };
    let rel = format!("static/{folder}/{}", rest.join("/"));
    let rel = if rel.ends_with(".js") { rel } else { rel + ".js" };
    js_module_path(module, &rel)
}

/// Instance members of Owl 2's `Component`, which odooscan does not index: Owl is a library.
const OWL_COMPONENT: &[&str] = &["setup", "render"];

/// The first series on Owl 2: saas~15.3 replaces Owl 1 (commit `966abc39b573`).
const OWL2: Series = Series { major: 15, minor: 3 };

/// Instance members of Owl 1's `Component`, read off the Owl that 15.0 ships, whose `patch()`
/// reaches each as `_super` along the prototype chain.
const OWL1_COMPONENT: &[&str] = &[
    "el",
    "setup",
    "willStart",
    "mounted",
    "willUpdateProps",
    "willPatch",
    "patched",
    "willUnmount",
    "mount",
    "unmount",
    "render",
    "destroy",
    "shouldUpdate",
    "trigger",
    "__destroy",
    "__callMounted",
    "__callWillUnmount",
    "__trigger",
    "__updateProps",
    "__patch",
    "__prepare",
    "__applyStyles",
    "__getTemplate",
    "__prepareAndRender",
    "__render",
    "__applyDefaultProps",
];

/// What a class extends, as far as the index follows it.
enum Parent<'h> {
    Root,
    /// Owl's `Component` imported from `@odoo/owl`, a module only Owl 2 series ship.
    Owl2,
    /// Owl's `Component` read off the `owl` global, of the Owl `Index::owl_global` picks.
    OwlGlobal,
    /// The class the file binds to this name.
    Name(&'h str),
}

/// A class by the file that declares it and its name there.
type ClassKey = (usize, usize, String);

/// What a component's `components` give a tag.
enum Lookup<'m> {
    /// The binding of the file that names the class.
    Found(JsTarget, &'m str),
    Absent,
    /// A value only a run tells.
    Unknown,
}

/// The props a component declares, as Owl validates those a template passes it (18.0
/// `addons/web/static/lib/owl/owl.js:2975-3002`).
pub struct PropSchema<'m> {
    /// Declared as a list of names, which only a list spreads into.
    list: bool,
    /// `*` lets any other prop through.
    pub any: bool,
    /// Each prop, and whether it is required.
    pub props: FxHashMap<&'m str, bool>,
}

/// The keys the loaded scripts outside tests, and for tours those of browser tests too, add to
/// registry categories, each with the modules that add it: by (category, key), by every dot suffix
/// of a key of `fields`, which the web client tries after a view type or a `js_class` and a dot,
/// and by category for keys only a run tells.
#[derive(Default)]
struct Registries<'m> {
    keys: FxHashMap<(&'m str, &'m str), Vec<usize>>,
    field_widgets: FxHashMap<&'m str, Vec<usize>>,
    open: FxHashMap<&'m str, Vec<usize>>,
}

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

/// A script a path names whose series does not transpile it.
#[derive(Debug, PartialEq, Eq)]
pub struct Untranspiled<'m> {
    pub rel: &'m str,
    /// The change to its tag that makes Odoo transpile it, none before saas~14.3.
    pub tag_fix: Option<TagFix>,
    /// The name its `odoo.define()` registers, which a script can use instead of the path.
    pub defined: Option<&'m str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagFix {
    Annotate,
    DropIgnore,
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

/// Whether the script's path and alias name it: only transpiling registers both (18.0
/// `odoo/tools/js_transpiler.py:22-59`). On an unknown series every script claims them, so OD601
/// does not report a name some series registers.
fn transpiled_names(facts: &JsFacts, series: Option<Series>) -> bool {
    series.is_none() || facts.es_module(series)
}

/// The files that claim each module name, in load order. Odoo keeps the first file that defines a
/// name and a bundle lists modules in load order (18.0 `addons/web/static/src/module_loader.js:59`),
/// but a page that loads only a later claimant gets that one.
fn js_paths(
    modules: &[Module],
    by_name: &FxHashMap<&str, usize>,
    default_series: Option<Series>,
) -> FxHashMap<String, Vec<JsTarget>> {
    let mut paths: FxHashMap<String, Vec<JsTarget>> = FxHashMap::default();
    let mut claim = |name: &str, target: JsTarget| {
        let claims = paths.entry(name.to_owned()).or_default();
        if !claims
            .iter()
            .any(|t| t.module == target.module && t.file == target.file)
        {
            claims.push(target);
        }
    };
    for mi in load_order(modules, by_name, default_series) {
        let module = &modules[mi];
        let series = module.series.or(default_series);
        for (fi, file) in module.js.iter().enumerate() {
            let target = JsTarget {
                module: mi,
                file: fi,
                opaque: file.facts.annotation(series) == Annotation::Ignore,
            };
            if let Some(path) =
                js_module_path(&module.name, &file.rel).filter(|_| transpiled_names(&file.facts, series))
            {
                if let Some(dir) = path.strip_suffix("/index") {
                    claim(dir, target);
                }
                claim(&path, target);
            }
            for name in file.facts.defines.iter().filter_map(|d| d.name.as_ref()) {
                claim(name, JsTarget { opaque: true, ..target });
            }
            // An alias stands for the default export, whose names only a run tells, unless it proxies:
            // it has `default=`, and before saas~17.2 one of `false`, `False` or `0`.
            let alias = file
                .facts
                .alias(series)
                .filter(|_| transpiled_names(&file.facts, series));
            if let Some((name, proxies)) = alias.and_then(|a| a.registered(series)) {
                claim(
                    name,
                    JsTarget {
                        opaque: target.opaque || !proxies,
                        ..target
                    },
                );
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
pub(crate) fn always_installed(
    modules: &[Module],
    by_name: &FxHashMap<&str, usize>,
    default_series: Option<Series>,
) -> Vec<bool> {
    let seeds = modules.iter().map(|m| m.name == "base").collect();
    install_closure(modules, by_name, seeds, default_series, |_| true)
}

/// `seeds`, and each auto-install module `joins` admits whose triggers (see `triggers` for how a series
/// reads them) lie among them, or are a `base` the scan leaves out, and whose depends closure is
/// indexed, with that closure dragged in, repeating until nothing is added.
fn install_closure(
    modules: &[Module],
    by_name: &FxHashMap<&str, usize>,
    seeds: Vec<bool>,
    series: Option<Series>,
    joins: impl Fn(&Module) -> bool,
) -> Vec<bool> {
    let mut installed = seeds;
    loop {
        // Odoo installs `base` on every database, so the `base` an empty `depends` reads as still
        // holds when the scan leaves it out, for the checks that run without it.
        let holds = |name: &str| by_name.get(name).map_or(name == "base", |&di| installed[di]);
        let ready: Vec<usize> = (0..modules.len())
            .filter(|&mi| !installed[mi] && joins(&modules[mi]))
            .filter(|&mi| triggers(&modules[mi], series).is_some_and(|t| t.into_iter().all(holds)))
            .collect();
        let mut grew = false;
        for mi in ready {
            // Odoo cannot load a module whose depends closure is incomplete, so nothing joins through it.
            let (closure, complete) = depends_closure(modules, by_name, [mi], series);
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
/// waits on every depend for a non-empty list, like `True`, and `[]` is off. Odoo reads `depends`
/// first (`read_depends`, 20.0 `odoo/modules/module.py:485-493`), so `True` on an empty one waits
/// on `base` from `saas~18.2`.
fn triggers(module: &Module, series: Option<Series>) -> Option<Vec<&str>> {
    let depends: Vec<&str> = read_depends(module, series).collect();
    let series = module.series.or(series);
    match &module.manifest.auto_install {
        AutoInstall::Off => None,
        AutoInstall::Depends => Some(depends),
        AutoInstall::Listed(names) if series.is_none_or(|s| s >= LISTED_TRIGGERS) => names
            .iter()
            .map(|n| n.value.as_str())
            .map(|n| depends.contains(&n).then_some(n))
            .collect(),
        AutoInstall::Listed(names) => (!names.is_empty()).then_some(depends),
    }
}

/// The `depends` names of `module` as Odoo reads them, its own series first and `series` in its place
/// when unknown. From `saas~18.2` an empty `depends` reads as `['base']` for every module but `base`
/// (20.0 `odoo/modules/module.py:471-475`), and the graph ranks it from that list
/// (`odoo/modules/module_graph.py:257-262`); 18.0 keeps it empty (`odoo/modules/module.py:56`).
fn read_depends(module: &Module, series: Option<Series>) -> impl Iterator<Item = &str> {
    let depends = &module.manifest.depends;
    let series = module.series.or(series);
    let base = depends.is_empty() && module.name != "base" && series.is_none_or(|s| s >= DEPENDS_BASE);
    depends.iter().map(|d| d.value.as_str()).chain(base.then_some("base"))
}

/// The modules `seeds` reach through `depends` as `read_depends` reads them by `series`, and whether
/// every depend on the way is indexed, the `base` an empty `depends` reads as excepted.
fn depends_closure(
    modules: &[Module],
    by_name: &FxHashMap<&str, usize>,
    seeds: impl IntoIterator<Item = usize>,
    series: Option<Series>,
) -> (Vec<bool>, bool) {
    let mut reached = vec![false; modules.len()];
    let mut complete = true;
    let mut stack: Vec<usize> = seeds.into_iter().collect();
    while let Some(mi) = stack.pop() {
        if std::mem::replace(&mut reached[mi], true) {
            continue;
        }
        // Only the `base` an empty `depends` reads as can be missing with no `depends` written, and
        // Odoo installs `base` on every database, so the scan leaving it out loses nothing.
        let unwritten = modules[mi].manifest.depends.is_empty();
        for name in read_depends(&modules[mi], series) {
            match by_name.get(name) {
                Some(&di) => stack.push(di),
                None => complete &= unwritten,
            }
        }
    }
    (reached, complete)
}

/// The modules `seeds` may use: their `depends` closure as `read_depends` reads them by `series`.
/// Every module `always` marks is seeded with them, as Odoo installs those wherever `base` is; they
/// are closed under `depends`, so they add no others.
pub(crate) fn scope_of(
    modules: &[Module],
    by_name: &FxHashMap<&str, usize>,
    always: &[bool],
    seeds: impl IntoIterator<Item = usize>,
    series: Option<Series>,
) -> Vec<bool> {
    let always = (0..modules.len()).filter(|&mi| always[mi]);
    let (visible, _) = depends_closure(modules, by_name, seeds.into_iter().chain(always), series);
    visible
}

/// Whether each module is a depend, direct or not, of a module whose imports the scan cannot read in full.
/// An unread module on a `depends` cycle counts only when another unread module reaches it.
fn unread_dependents(
    modules: &[Module],
    by_name: &FxHashMap<&str, usize>,
    imports: &imports::Graph,
    series: Option<Series>,
) -> Vec<bool> {
    let mut marked = vec![false; modules.len()];
    for u in (0..modules.len()).filter(|&u| imports.unread(&modules[u].name)) {
        let (reached, _) = depends_closure(modules, by_name, [u], series);
        for (mi, r) in reached.into_iter().enumerate() {
            marked[mi] |= r && mi != u;
        }
    }
    marked
}

/// Module indices in the order Odoo loads them: `base` first, as Odoo loads it alone before marking
/// the rest (18.0 `odoo/modules/loading.py:407-409`, `:444`; 20.0 `:359-361`, `:399`), then by depth
/// in the `depends` graph as `read_depends` reads it by `series`, then by name. The passes end at the
/// deepest module, or at the module count when a cycle keeps deepening.
fn load_order(modules: &[Module], by_name: &FxHashMap<&str, usize>, series: Option<Series>) -> Vec<usize> {
    let mut depth = vec![0_usize; modules.len()];
    for _ in modules {
        let mut grew = false;
        for (mi, module) in modules.iter().enumerate() {
            let below = read_depends(module, series).filter_map(|d| by_name.get(d));
            let deepest = below.map(|&d| depth[d] + 1).max().unwrap_or_default();
            grew |= deepest > depth[mi];
            depth[mi] = depth[mi].max(deepest);
        }
        if !grew {
            break;
        }
    }
    let mut order: Vec<usize> = (0..modules.len()).collect();
    order.sort_unstable_by_key(|&mi| (modules[mi].name != "base", depth[mi], modules[mi].name.as_str()));
    order
}

/// The place of each module in `load_order`.
fn load_rank(modules: &[Module], by_name: &FxHashMap<&str, usize>, series: Option<Series>) -> Vec<usize> {
    let mut ranks = vec![0; modules.len()];
    for (rank, mi) in load_order(modules, by_name, series).into_iter().enumerate() {
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
    series: Option<Series>,
    facts: fn(&DataFacts) -> &[T],
    id: fn(&T) -> Option<&String>,
) -> Writes<'m, T> {
    let mut writes: Writes<'m, T> = FxHashMap::default();
    for mi in load_order(modules, by_name, series) {
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

/// Whether the view the `records` of one id leave may be stored as primary, None when only a run
/// can tell. Odoo loads an id of another module only once that module is installed (18.0
/// `odoo/tools/convert.py:235-237`), so the data records of `creator`, the module the id names,
/// come first and the first creates the view. After it, a `mode` wins; without one, a falsy
/// `inherit_id` makes the view primary, and a truthy one makes a view with no parent an extension
/// and keeps the mode of one with a parent (18.0 `odoo/addons/base/models/ir_ui_view.py:460-466,535,557`,
/// 20.0 `:575-581,650,671`). A parent the scan does not read counts as one. Only those records load
/// on every database that has the view, so any other can make it primary but never take that away;
/// when `creator` writes none of them, no record can. An update loads every record again as a write
/// onto the stored view, so the fold repeats from the state each pass leaves until one repeats, and
/// a mode any pass leaves counts.
fn stored_primary(records: &[Placed<&ViewFact>], creator: Option<usize>) -> Option<bool> {
    let has_parent = |view: &ViewFact| view.inherit.is_some() || view.unread_parent;
    let (sure, others): (Vec<&Placed<&ViewFact>>, Vec<_>) =
        records.iter().partition(|r| Some(r.module) == creator && !r.demo);
    let mut state = match sure.first() {
        Some(first) => (first.fact.primary, has_parent(first.fact)),
        None => (Some(true), false),
    };
    // On install the first record creates the view; an update writes it like any later one.
    let mut pass: &[&Placed<&ViewFact>] = sure.get(1..).unwrap_or_default();
    let mut seen = Vec::new();
    // `Some(false)` leaves the first pass's mode as it is in `may_be_primary`.
    let mut stored = Some(false);
    while !seen.contains(&state) {
        seen.push(state);
        let (mut primary, mut parented) = state;
        for record in pass {
            primary = written_primary(primary, parented, record.fact);
            if record.fact.writes_parent {
                parented = has_parent(record.fact);
            }
        }
        for record in &others {
            primary = may_be_primary(primary, written_primary(primary, parented, record.fact));
            if record.fact.writes_parent {
                parented = has_parent(record.fact);
            }
        }
        stored = may_be_primary(stored, primary);
        state = (primary, parented);
        pass = &sure;
    }
    stored
}

/// The mode a later record of a view leaves it with, `parented` saying whether it had a parent.
fn written_primary(primary: Option<bool>, parented: bool, view: &ViewFact) -> Option<bool> {
    if view.writes_mode {
        return view.primary;
    }
    if !view.writes_parent {
        return primary;
    }
    // Without `mode`, `primary` says whether the parent is truthy: false when it is.
    match view.primary {
        Some(false) if parented => primary,
        other => other,
    }
}

/// The mode a view may have when a database may skip the record that leaves it `written`.
fn may_be_primary(before: Option<bool>, written: Option<bool>) -> Option<bool> {
    match (before, written) {
        (_, Some(false)) => before,
        (Some(false), _) => written,
        _ if before == written => before,
        _ => None,
    }
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

/// Whether `path` holds one of the characters Odoo reads as a glob (`WILDCARD_CHARACTERS`).
pub(crate) fn wildcard(path: &str) -> bool {
    path.contains(['*', '?', '[', ']'])
}

/// The parts of `path` as `os.path.normpath` leaves them: without empty ones or `.`, and with each
/// `..` taking the part before it away.
pub(crate) fn normal_parts(path: &str) -> Vec<&str> {
    let mut parts = vec![];
    for part in path.split('/').filter(|p| !p.is_empty() && *p != ".") {
        if part == ".." {
            parts.pop();
        } else {
            parts.push(part);
        }
    }
    parts
}

/// Whether `parts`, as `normal_parts` leaves them, name something under `module/static/`, the only
/// place Odoo globs assets from (18.0 `odoo/addons/base/models/ir_asset.py:349-362`, 20.0
/// `:342-352`).
pub(crate) fn in_static(module: &str, parts: &[&str]) -> bool {
    parts.len() > 2 && parts[0] == module && parts[1] == "static"
}

/// Files of a kind.
type Loaded = FxHashSet<bundles::File>;

/// The modules that load a file of `loaded` the scan cannot read, sorted.
fn template_losers(modules: &[Module], loaded: &Loaded) -> Vec<usize> {
    let unread = loaded
        .iter()
        .filter(|&&(mi, fi)| modules[mi].templates[fi].facts.unreadable);
    let mut losers: Vec<usize> = unread.map(|&(mi, _)| mi).collect();
    losers.sort_unstable();
    losers.dedup();
    losers
}

/// The bundles each loaded script is in, as indices into the sorted bundle names.
type ScriptBundles = FxHashMap<bundles::File, Vec<usize>>;

/// The bundles some indexed manifest names, sorted, the bundles each script is in, and the template
/// files listed by some bundle that loads templates in the file's series, as (module, file). A
/// bundle may load another module's files, and a file only removed from bundles is not loaded.
fn loaded_static(
    modules: &[Module],
    by_name: &FxHashMap<&str, usize>,
    series: Option<Series>,
) -> (Vec<String>, ScriptBundles, Loaded) {
    let order = load_order(modules, by_name, series);
    let bundles = bundles::of_modules(modules, by_name, &order, series);
    let mut names: Vec<String> = bundles.keys().cloned().collect();
    names.sort_unstable();
    let mut js: ScriptBundles = FxHashMap::default();
    for (bi, name) in names.iter().enumerate() {
        for &file in &bundles[name].js {
            js.entry(file).or_default().push(bi);
        }
    }
    let file_series = |(mi, _): &bundles::File| modules[*mi].series.or(series);
    let templates = bundles
        .iter()
        .flat_map(|(name, c)| {
            c.templates
                .iter()
                .filter(|f| bundles::loads_templates(name, file_series(f)))
        })
        .copied()
        .collect();
    (names, js, templates)
}

/// Whether only a test runner page calls the bundle: one of the words of its name after the module
/// is `test`, `tests`, `qunit` or `mocha`. The pages are those of 18.0
/// `addons/web/views/webclient_templates.xml:5-12,197,217-246,265` and
/// `addons/web_editor/views/editor.xml:159-181`.
pub(crate) fn test_bundle(name: &str) -> bool {
    let mut words = name.split_once('.').map_or("", |(_, rest)| rest).split('_');
    words.any(|w| matches!(w, "test" | "tests" | "qunit" | "mocha"))
}

/// The scripts that are tests, as (module, file): by their path, or loaded only by test bundles.
fn test_scripts(modules: &[Module], bundle_names: &[String], js_bundles: &ScriptBundles) -> Loaded {
    let by_bundles = js_bundles
        .iter()
        .filter(|(_, bundles)| bundles.iter().all(|&bi| test_bundle(&bundle_names[bi])))
        .map(|(&file, _)| file);
    let by_path = modules.iter().enumerate().flat_map(|(mi, m)| {
        let files = m.js.iter().enumerate().filter(|(_, f)| f.in_tests());
        files.map(move |(fi, _)| (mi, fi))
    });
    by_bundles.chain(by_path).collect()
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

/// What a specifier names: the name Odoo looks up, and for a module path the module and the folder
/// under `static` that should hold the file.
pub struct JsLookup<'s> {
    pub name: Cow<'s, str>,
    home: Option<(&'s str, &'s str)>,
}

/// What a specifier imported by the script at the URL `from` names. Odoo turns a relative specifier
/// into the name of the file at its URL, and looks any other up as written (18.0
/// `odoo/tools/js_transpiler.py:523-548`): `@m/x` names a script under `m/static/src`,
/// `@m/../tests/x` one under `m/static/tests`, and the rest an alias or a name given to `odoo.define()`.
/// None when the specifier names a library or climbs out of the module.
pub fn js_lookup<'s>(from: &'s str, specifier: &'s str) -> Option<JsLookup<'s>> {
    if specifier.starts_with('.') {
        let mut dir: Vec<&str> = from.split('/').collect();
        dir.pop();
        let url = join_segments(dir, specifier)?;
        let name = url_module_path(&url)?.into();
        return Some(JsLookup {
            name,
            home: Some((url[0], url[2])),
        });
    }
    let name = specifier.into();
    let Some((module, rest)) = specifier.strip_prefix('@').and_then(|s| s.split_once('/')) else {
        return Some(JsLookup { name, home: None });
    };
    let folder = match rest.strip_prefix("../") {
        None => "src",
        Some(r) if r.starts_with("tests/") => "tests",
        Some(_) => return None,
    };
    Some(JsLookup {
        name,
        home: Some((module, folder)),
    })
}

/// Appends the segments of a specifier to a URL's, or None when it climbs out of the module.
fn join_segments<'s>(mut parts: Vec<&'s str>, specifier: &'s str) -> Option<Vec<&'s str>> {
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
    Some(parts)
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
    /// 9.0 to 9.saas~9 take the module that creates the row. 9.saas~10 (`408330a65509`) to
    /// saas~11.2 prefer the module of the class that declares the field, which a delegated field
    /// lacks. saas~11.3 and 11.4 take the one that creates the row or declares the field. From
    /// saas~11.5 it is the model's original module or one that declares the field, joined from
    /// saas~13.4 by each module that adds the parent. A declarer that does not take the parent is
    /// taken to load before the model, so to write nothing for it.
    fn holds_in(self, series: Series) -> bool {
        let by_declarer = series::matches(">=9.saas~10,<11.3", series);
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
    /// Defined only by modules outside the scope. Names, for a model, the module that introduces it,
    /// else the first module the index lists among the extenders `model_status` takes for a source;
    /// for a template, view, registry key or field widget, the first module the index lists as
    /// defining it; for a method, the module of the first class in the model's inheritance chain that
    /// defines it.
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
    /// The controllers of every module.
    pub routes: routing::Table,
    /// The imports of every module.
    pub imports: imports::Graph,
    pub models_with_access: FxHashSet<String>,
    model_by_xmlid: FxHashMap<String, &'m str>,
    pub has_base: bool,
    /// The series `--target-series` or `--odoo-version` names, for a module whose own is unknown.
    pub default_series: Option<Series>,
    js_paths: FxHashMap<String, Vec<JsTarget>>,
    bundle_names: Vec<String>,
    js_bundles: ScriptBundles,
    test_scripts: Loaded,
    templates_loaded: Loaded,
    /// The modules that load a template file the scan cannot read, sorted.
    template_losers: Vec<usize>,
    always_installed: Vec<bool>,
    /// Each module's place in `load_order`.
    load_rank: Vec<usize>,
    /// Whether each module is a depend of another module whose imports the scan cannot read in full.
    unread_dependent: Vec<bool>,
    patched: Patched<'m>,
    /// Classes a subclass with a template of its own extends.
    template_bases: FxHashSet<ClassKey>,
    /// The loaded classes that render each template, as `template_owners` reads them.
    owners: FxHashMap<&'m str, Vec<(JsTarget, &'m JsClass)>>,
    /// Classes whose `props` or `components` a script changes other than by a literal assignment.
    static_writes: FxHashSet<(ClassKey, Static)>,
    registries: Registries<'m>,
    /// The templates that a loaded template calls.
    templates_called: FxHashSet<&'m str>,
    /// The loaded templates with an attributes spec, by name, or by their parent's for an extension.
    templates_set_attributes: FxHashSet<&'m str>,
    /// Flow summaries of model methods, by model and method name.
    method_flows: MethodFlows<'m>,
    /// The models whose table each table name may be, from any module.
    table_models: FxHashMap<String, Vec<&'m str>>,
    /// Where the classes define each field, method or `selection_add` value name.
    members: FxHashMap<&'m str, Vec<Member<'m>>>,
    /// The hook names and the prefixes of built ones that any file of any module mentions.
    hook_names: FxHashSet<&'m str>,
    hook_prefixes: Vec<&'m str>,
    /// The modules in the scope of a module with a file the scan cannot read at all.
    unread_reach: Vec<bool>,
    /// When each module first runs each of its files (`imports::Graph::run_order`), none for one
    /// whose order the scan cannot read.
    build_order: Vec<Option<FxHashMap<String, imports::RunKey>>>,
    /// The modules whose manifest odooscan rejects, so none of them is in `modules`, with how the
    /// Odoo of each checked module's series takes it; `build` leaves it empty for the caller to
    /// fill.
    pub rejected: FxHashMap<String, Unindexed>,
    /// The modules whose manifest odooscan reads and sets `installable` to false, none of them in
    /// `modules`; `build` leaves it empty for the caller to fill.
    pub uninstallable: FxHashSet<String>,
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
        let (bundle_names, js_bundles, templates_loaded) = loaded_static(modules, &by_name, default_series);
        let test_scripts = test_scripts(modules, &bundle_names, &js_bundles);
        let template_losers = template_losers(modules, &templates_loaded);
        let (hook_names, hook_prefixes) = hook_mentions(modules, &test_scripts);
        let imports = imports::Graph::of_modules(modules, default_series);
        let unread_dependent = unread_dependents(modules, &by_name, &imports, default_series);
        let mut index = Self {
            modules,
            has_base: base.is_some(),
            default_series,
            xmlids,
            views: Inheritance::new(views),
            view_types: FxHashSet::default(),
            client_views: FxHashSet::default(),
            action_writes: record_writes(modules, &by_name, default_series, |f| &f.actions, |a| a.id.as_ref()),
            rule_writes: record_writes(modules, &by_name, default_series, |f| &f.rules, |r| r.id.as_ref()),
            server_writes: record_writes(modules, &by_name, default_series, |f| &f.servers, |s| s.id.as_ref()),
            report_writes: record_writes(modules, &by_name, default_series, |f| &f.reports, |r| r.id.as_ref()),
            value_writes: record_writes(modules, &by_name, default_series, |f| &f.writes, |w| w.id.as_ref()),
            data_fields: FxHashMap::default(),
            derived,
            ir_defaults: ir_defaults(modules),
            view_keys,
            templates: Inheritance::new(template_nodes(modules, &templates_loaded)),
            routes: routing::Table::of_modules(modules),
            imports,
            models_with_access,
            model_by_xmlid,
            js_paths: js_paths(modules, &by_name, default_series),
            bundle_names,
            js_bundles,
            test_scripts,
            templates_loaded,
            template_losers,
            always_installed: always_installed(modules, &by_name, default_series),
            load_rank: load_rank(modules, &by_name, default_series),
            unread_dependent,
            by_name,
            patched: FxHashMap::default(),
            template_bases: FxHashSet::default(),
            owners: FxHashMap::default(),
            static_writes: FxHashSet::default(),
            registries: Registries::default(),
            templates_called: FxHashSet::default(),
            templates_set_attributes: FxHashSet::default(),
            method_flows: method_flows(modules),
            table_models: table_models(&models),
            members: members(&models),
            models,
            hook_names,
            hook_prefixes,
            unread_reach: Vec::new(),
            build_order: Vec::new(),
            rejected: FxHashMap::default(),
            uninstallable: FxHashSet::default(),
        };
        index.patched = index.patched_classes();
        index.template_bases = index.template_bases();
        index.owners = index.owners();
        index.static_writes = index.static_writes();
        index.registries = index.registries();
        (index.templates_called, index.templates_set_attributes) = index.template_uses();
        index.view_types = index.view_types(default_series);
        index.client_views = index.client_views();
        index.data_fields = index.data_fields(default_series);
        index.unread_reach = index.unread_reach();
        index.build_order = index.build_order();
        index
    }

    /// `imports::Graph::run_order` of each module, none for one a module other than its dependents
    /// imports a file of, as that module may run the file first.
    fn build_order(&self) -> Vec<Option<FxHashMap<String, imports::RunKey>>> {
        let order = self.modules.iter().enumerate().map(|(mi, m)| {
            let mut importers = self.imports.elsewhere(&m.name).map(|(_, importer)| importer);
            let foreign = importers.any(|importer| !self.imports_later(importer, mi));
            if foreign { None } else { self.imports.run_order(&m.name) }
        });
        order.collect()
    }

    /// The union of the scopes of the modules with `unread_text`.
    fn unread_reach(&self) -> Vec<bool> {
        let mut reach = vec![false; self.modules.len()];
        for (mi, _) in self.modules.iter().enumerate().filter(|(_, m)| m.unread_text) {
            let scope = self.scope(mi);
            reach.iter_mut().zip(&scope.visible).for_each(|(r, v)| *r |= v);
        }
        reach
    }

    /// The fields the `ir.model.fields` records of the files Odoo loads add.
    fn data_fields(&self, default_series: Option<Series>) -> DataFields<'m> {
        let modules = self.modules;
        let order = load_order(modules, &self.by_name, default_series);
        let writes = order.into_iter().flat_map(|mi| {
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

    /// Each record of a view counts with the mode the records of its id leave, as `stored_primary`
    /// folds them: only the data records of the module the id names can take its type away, as a
    /// type counts when some database may have it. A record without an id creates a view of its own.
    fn view_types(&self, series: Option<Series>) -> FxHashSet<(&'m str, Option<&'m str>)> {
        let mut types = FxHashSet::default();
        let ids = record_writes(self.modules, &self.by_name, series, |f| &f.views, |v| v.id.as_ref());
        for (id, records) in &ids {
            let creator = id.split_once('.').and_then(|(name, _)| self.by_name.get(name));
            let primary = stored_primary(records, creator.copied());
            if primary != Some(false) {
                let types_of = records
                    .iter()
                    .map(|r| self.view_type(&self.modules[r.module], r.fact, primary));
                types.extend(types_of.flatten());
            }
        }
        for module in self.modules {
            let loaded = module.data.iter().filter(|f| f.rank.is_some());
            let views = loaded
                .flat_map(|f| &f.facts.views)
                .filter(|v| v.id.is_none() && v.primary != Some(false));
            types.extend(views.filter_map(|view| self.view_type(module, view, view.primary)));
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

    /// The entry of `view` in `view_types`, if it has one, with `primary` the mode its id is left
    /// with. Odoo gives a view with a parent the type of its tree's root, so with a view above it not
    /// indexed, it counts under the type its record writes, which Odoo keeps over the parent's, else
    /// under any. With a parent above the top not read, its own root says nothing.
    fn view_type(
        &self,
        module: &'m Module,
        view: &'m ViewFact,
        primary: Option<bool>,
    ) -> Option<(&'m str, Option<&'m str>)> {
        let id = view.id.as_ref().map(|id| full_id(&module.name, id));
        let own_model = view.model.as_ref().map(|m| m.value.as_str());
        let model = own_model.or_else(|| self.view_model(id.clone()?))?;
        let parent = view.inherit.as_ref().map(|parent| full_id(&module.name, parent));
        let start = id.or(parent);
        let top = start.as_ref().and_then(|start| self.view_top(start.clone()));
        if start.is_some() && top.is_none() {
            return Some((model, view.written_type.as_deref()));
        }
        let top = top.map(|(_, nodes)| nodes).unwrap_or_default();
        let kind = top.iter().find_map(|n| n.fact.kind.as_deref());
        let kind = kind.or(view.kind.as_deref());
        let unread = view.unread_parent || top.iter().any(|n| n.fact.unread_parent);
        (kind.is_some() || unread || primary.is_none()).then_some((model, kind))
    }

    /// A test that subclasses a component with a template of its own leaves the base's template used.
    fn template_bases(&self) -> FxHashSet<ClassKey> {
        let mut bases = FxHashSet::default();
        for (mi, module) in self.modules.iter().enumerate() {
            for (fi, file) in module.js.iter().enumerate().filter(|&(fi, _)| !self.js_test(mi, fi)) {
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

    /// The loaded classes outside tests by the template each renders: its own, or else the first
    /// one up its chain of parents.
    fn owners(&self) -> FxHashMap<&'m str, Vec<(JsTarget, &'m JsClass)>> {
        let mut owners: FxHashMap<&'m str, Vec<_>> = FxHashMap::default();
        for (mi, module) in self.modules.iter().enumerate() {
            let files = module.js.iter().enumerate();
            for (fi, file) in files.filter(|&(fi, _)| !self.js_test(mi, fi) && self.js_loaded(mi, fi)) {
                let here = self.js_target(mi, fi);
                for class in &file.facts.classes {
                    if let Some(template) = self.rendered_template(here, class) {
                        owners.entry(template).or_default().push((here, class));
                    }
                }
            }
        }
        owners
    }

    fn rendered_template(&self, t: JsTarget, class: &'m JsClass) -> Option<&'m str> {
        let (mut t, mut class) = (t, class);
        for _ in 0..32 {
            if let Some(template) = &class.template {
                return Some(&template.value);
            }
            let Heritage::Name(parent) = &class.extends else {
                return None;
            };
            (t, class) = self.js_class(t, parent, 0)?;
        }
        None
    }

    /// A test bundle such as `web.assets_tests` loads the tours a browser test runs, so the tours of
    /// test scripts count, and nothing else of them. A unit test bundle runs on its own page, which
    /// `start_tour()` never opens.
    fn registries(&self) -> Registries<'m> {
        let mut found = Registries::default();
        for (mi, module) in self.modules.iter().enumerate() {
            let files = module.js.iter().enumerate();
            for (fi, file) in files.filter(|(fi, _)| self.js_loaded(mi, *fi)) {
                let toured = || {
                    self.js_bundles(mi, fi)
                        .any(|b| !b.contains("unit_tests") && !b.contains("qunit"))
                };
                let counts = |category: &str| !self.js_test(mi, fi) || category == TOUR_REGISTRY && toured();
                for (category, key) in file.facts.registrations.iter().filter(|(c, _)| counts(c)) {
                    found.keys.entry((category, key)).or_default().push(mi);
                    if category == FIELD_REGISTRY {
                        let suffixes = key.match_indices('.').map(|(at, _)| &key[at + 1..]);
                        for suffix in std::iter::once(key.as_str()).chain(suffixes) {
                            found.field_widgets.entry(suffix).or_default().push(mi);
                        }
                    }
                }
                for category in file.facts.open_categories.iter().filter(|c| counts(c)) {
                    found.open.entry(category).or_default().push(mi);
                }
            }
        }
        found
    }

    /// A write in a test applies only while that test runs.
    fn static_writes(&self) -> FxHashSet<(ClassKey, Static)> {
        let mut written = FxHashSet::default();
        for (mi, module) in self.modules.iter().enumerate() {
            for (fi, file) in module.js.iter().enumerate().filter(|&(fi, _)| !self.js_test(mi, fi)) {
                let here = self.js_target(mi, fi);
                for write in &file.facts.static_writes {
                    let Some((t, class)) = self.written_class(here, write) else {
                        continue;
                    };
                    written.insert(((t.module, t.file, class.name.clone()), write.member));
                    if write.inner {
                        self.shared_writes(t, class, write, &mut written);
                    }
                }
            }
        }
        written
    }

    /// The class whose member `write` changes: the one `binding` names, or the class its
    /// `components` hold under each key of `through`. This pass fills the writes `component_child`
    /// heeds, so the lookup reads the literal `components` alone.
    fn written_class(&self, here: JsTarget, write: &StaticWrite) -> Option<(JsTarget, &'m JsClass)> {
        let mut found = self.js_class(here, &write.binding, 0)?;
        for key in &write.through {
            found = self.component_child(found.0, found.1, key)?;
        }
        Some(found)
    }

    /// Marks what a write into the member of `class` changes: the object of the class up the chain
    /// that declares it, and the objects that one shares, through `X.props` or `X.components` and,
    /// for a write that reaches into a description, through `{ ...X.props }`, as a spread copies
    /// only the keys. Each class is visited once, so a cycle of aliases or spreads ends.
    fn shared_writes(
        &self,
        t: JsTarget,
        class: &'m JsClass,
        write: &StaticWrite,
        written: &mut FxHashSet<(ClassKey, Static)>,
    ) {
        let mut seen = FxHashSet::default();
        let mut todo = vec![(t, class)];
        while let Some((t, class)) = todo.pop() {
            let Some((t, class)) = self.static_declarer(t, class, write.member) else {
                continue;
            };
            if !seen.insert((t.module, t.file, class.name.as_str())) {
                continue;
            }
            written.insert(((t.module, t.file, class.name.clone()), write.member));
            for source in Self::shared_sources(class, write) {
                if let Some(Some(found)) = self.static_source(t, class, source) {
                    todo.push(found);
                }
            }
        }
    }

    /// The classes, `super` among them, whose object the member of `class` shares as far as `write`
    /// reaches into it. A `{ ...X.components }` copy shares only class references, so a write past
    /// it changes a class, not X's mapping.
    fn shared_sources<'c>(class: &'c JsClass, write: &StaticWrite) -> Vec<&'c str> {
        match write.member {
            Static::Props => class
                .props
                .iter()
                .flat_map(|p| &p.items)
                .filter_map(|item| match item {
                    PropItem::Alias(source) => Some(source.as_str()),
                    PropItem::Spread(source) if write.deep => Some(source.as_str()),
                    _ => None,
                })
                .collect(),
            Static::Components => class
                .components
                .iter()
                .flatten()
                .filter_map(|item| match item {
                    ComponentItem::Alias(source) => Some(source.as_str()),
                    _ => None,
                })
                .collect(),
        }
    }

    /// The class up the chain from `class` that declares the static member, whose object a
    /// subclass without its own reads as its own.
    fn static_declarer(&self, t: JsTarget, class: &'m JsClass, member: Static) -> Option<(JsTarget, &'m JsClass)> {
        let (mut t, mut class) = (t, class);
        for _ in 0..32 {
            let declared = match member {
                Static::Props => class.props.is_some(),
                Static::Components => class.components.is_some(),
            };
            if declared {
                return Some((t, class));
            }
            match self.js_parent(t, &class.extends)? {
                Parent::Name(parent) => (t, class) = self.js_class(t, parent, 0)?,
                Parent::Root | Parent::Owl2 | Parent::OwlGlobal => return None,
            }
        }
        None
    }

    /// The templates the loaded ones call, and those with a node that sets attributes.
    fn template_uses(&self) -> (FxHashSet<&'m str>, FxHashSet<&'m str>) {
        let (mut called, mut set) = (FxHashSet::default(), FxHashSet::default());
        for &(mi, fi) in &self.templates_loaded {
            for t in &self.modules[mi].templates[fi].facts.templates {
                called.extend(t.arch.calls.iter().map(|c| c.value.as_str()));
                let key = t.name.as_ref().or(t.inherit.as_ref().map(|i| &i.value));
                set.extend(key.filter(|_| t.sets_attributes).map(String::as_str));
            }
        }
        (called, set)
    }

    /// The classes that render the template `name`, read for a node of it that `module` loads, or
    /// None when another template may hold its elements: one that calls it, a primary child, or one
    /// in a file the scan cannot read (`loses_template`). A class outside the index may render it too.
    pub fn template_owners(&self, name: &str, module: usize) -> Option<&[(JsTarget, &'m JsClass)]> {
        let shared = self.templates_called.contains(name) || self.templates.has_children(name);
        if shared || self.loses_template(name, module) {
            return None;
        }
        self.owners.get(name).map(Vec::as_slice)
    }

    /// Whether a module that loads a template file the scan cannot read may know the template
    /// `name`: its scope holds `module` or a module defining `name`. With no readable definer, a loser
    /// in the scope of `module` counts too, as its file may define `name`. The file names nothing the
    /// scan reads, so a module that cannot know the template is not taken to call, inherit or extend it.
    fn loses_template(&self, name: &str, module: usize) -> bool {
        let undefined = self.templates.definers(name).next().is_none();
        let known = || self.templates.definers(name).chain(std::iter::once(module));
        let mut losers = self.template_losers.iter();
        losers.any(|&loser| (undefined && self.sees(module, loser)) || known().any(|m| self.sees(loser, m)))
    }

    /// Whether a node of the template `name` sets attributes, which may give an element any prop.
    pub fn template_sets_attributes(&self, name: &str) -> bool {
        self.templates_set_attributes.contains(name)
    }

    /// The class the component `owner` names `tag` in its `components`, inherited or its own, as
    /// Owl looks it up to render the tag (18.0 `addons/web/static/lib/owl/owl.js:6119-6127`). None
    /// when the key is missing, or a write or a value only a run tells may change it.
    pub fn component_child(&self, t: JsTarget, owner: &'m JsClass, tag: &str) -> Option<(JsTarget, &'m JsClass)> {
        match self.component_lookup(t, owner, tag, 0) {
            Lookup::Found(t, binding) => self.js_class(t, binding, 0),
            Lookup::Absent | Lookup::Unknown => None,
        }
    }

    fn component_lookup(&self, t: JsTarget, class: &'m JsClass, tag: &str, depth: u8) -> Lookup<'m> {
        let Some((t, class, items)) = self.static_holder(t, class, Static::Components, |c| c.components.as_deref())
        else {
            return Lookup::Unknown;
        };
        for item in items.unwrap_or_default().iter().rev() {
            let found = match item {
                ComponentItem::Entry { key, binding } if key == tag => {
                    binding.as_deref().map_or(Lookup::Unknown, |b| Lookup::Found(t, b))
                }
                ComponentItem::Entry { .. } => Lookup::Absent,
                ComponentItem::Spread(source) => self.spread_lookup(t, class, source, tag, depth),
                ComponentItem::Alias(_) | ComponentItem::Open => Lookup::Unknown,
            };
            if !matches!(found, Lookup::Absent) {
                return found;
            }
        }
        Lookup::Absent
    }

    /// The tag in the `components` of the class `...source.components` reads.
    fn spread_lookup(&self, t: JsTarget, class: &'m JsClass, source: &str, tag: &str, depth: u8) -> Lookup<'m> {
        match self.static_source(t, class, source) {
            _ if depth > 8 => Lookup::Unknown,
            None => Lookup::Unknown,
            Some(None) => Lookup::Absent,
            Some(Some((st, sc))) => self.component_lookup(st, sc, tag, depth + 1),
        }
    }

    /// The props the component declares, its own or inherited, folded with the props it spreads;
    /// None when it declares none, so Owl validates nothing, or a write or a value only a run tells
    /// may change them.
    pub fn component_props(&self, t: JsTarget, class: &'m JsClass) -> Option<PropSchema<'m>> {
        self.props_lookup(t, class, 0).flatten()
    }

    /// `Some(None)` for a class that declares no props, which a spread reads as none.
    fn props_lookup(&self, t: JsTarget, class: &'m JsClass, depth: u8) -> Option<Option<PropSchema<'m>>> {
        let (t, class, props) = self.static_holder(t, class, Static::Props, |c| c.props.as_ref())?;
        let Some(props) = props else {
            return Some(None);
        };
        let mut schema = PropSchema {
            list: props.list,
            any: false,
            props: FxHashMap::default(),
        };
        for item in &props.items {
            match item {
                PropItem::Prop { name, .. } if name == "*" => schema.any = true,
                PropItem::Prop { name, required } => {
                    schema.props.insert(name, *required);
                }
                PropItem::Spread(source) => self.spread_props(t, class, source, depth, &mut schema)?,
                PropItem::Alias(_) | PropItem::Open => return None,
            }
        }
        Some(Some(schema))
    }

    /// Adds the props of the class `...source.props` reads, or None when they are unknown.
    fn spread_props(
        &self,
        t: JsTarget,
        class: &'m JsClass,
        source: &str,
        depth: u8,
        schema: &mut PropSchema<'m>,
    ) -> Option<()> {
        if depth > 8 {
            return None;
        }
        let Some((st, sc)) = self.static_source(t, class, source)? else {
            return Some(());
        };
        let Some(spread) = self.props_lookup(st, sc, depth + 1)? else {
            return Some(());
        };
        // An object spread of a list, or a list spread of an object, gives no props.
        if spread.list != schema.list {
            return None;
        }
        schema.any |= spread.any;
        schema.props.extend(spread.props);
        Some(())
    }

    /// The class up the chain from `class` whose static member `get` reads, with that member: None
    /// for none up to Owl's `Component` or a class that extends nothing, and None as a whole when a
    /// parent cannot be resolved or a script writes the member on the way.
    fn static_holder<T>(
        &self,
        t: JsTarget,
        class: &'m JsClass,
        kind: Static,
        get: impl Fn(&'m JsClass) -> Option<T>,
    ) -> Option<(JsTarget, &'m JsClass, Option<T>)> {
        let (mut t, mut class) = (t, class);
        for _ in 0..32 {
            if self
                .static_writes
                .contains(&((t.module, t.file, class.name.clone()), kind))
            {
                return None;
            }
            if let Some(value) = get(class) {
                return Some((t, class, Some(value)));
            }
            match self.js_parent(t, &class.extends)? {
                Parent::Name(parent) => (t, class) = self.js_class(t, parent, 0)?,
                Parent::Root | Parent::Owl2 | Parent::OwlGlobal => return Some((t, class, None)),
            }
        }
        None
    }

    /// The class `...source.props` of a static member of `class` reads: `super` is its parent.
    /// `Some(None)` for Owl's `Component` or none, which give nothing; None when it is unresolved.
    fn static_source(&self, t: JsTarget, class: &'m JsClass, source: &str) -> Option<Option<(JsTarget, &'m JsClass)>> {
        if source != "super" {
            return self.js_class(t, source, 0).map(Some);
        }
        match self.js_parent(t, &class.extends)? {
            Parent::Name(parent) => self.js_class(t, parent, 0).map(Some),
            Parent::Root | Parent::Owl2 | Parent::OwlGlobal => Some(None),
        }
    }

    /// Whether every subclass of the class may replace its template, as for an abstract base.
    pub fn template_base(&self, module: usize, file: usize, class: &str) -> bool {
        self.template_bases.contains(&(module, file, class.to_owned()))
    }

    /// A patch in a test applies only while that test runs.
    fn patched_classes(&self) -> Patched<'m> {
        let mut patched: Patched<'m> = FxHashMap::default();
        for (mi, module) in self.modules.iter().enumerate() {
            for (fi, file) in module.js.iter().enumerate().filter(|&(fi, _)| !self.js_test(mi, fi)) {
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

    /// The URL of the file `t`, which a relative specifier is read from.
    fn js_from(&self, t: JsTarget) -> String {
        let module = &self.modules[t.module];
        js_url(&module.name, &module.js[t.file].rel)
    }

    /// The files that claim the name a specifier imported at the URL `from` stands for, in load order.
    pub fn js_claims(&self, from: &str, specifier: &str) -> &[JsTarget] {
        js_lookup(from, specifier)
            .and_then(|l| self.js_paths.get(l.name.as_ref()))
            .map_or(&[], Vec::as_slice)
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

    /// How the file `t` binds `local` to Owl's `Component`: by an import or from the `owl` global.
    fn owl_component(&self, t: JsTarget, local: &str) -> Option<Parent<'static>> {
        let facts = &self.modules[t.module].js[t.file].facts;
        let imported = |n: &JsName| n.specifier == OWL_MODULE && n.name.value == "Component";
        if facts
            .names
            .iter()
            .any(|n| n.local.as_deref() == Some(local) && imported(n))
        {
            return Some(Parent::Owl2);
        }
        let global = facts
            .owl_bindings
            .iter()
            .any(|(l, name)| l == local && name == "Component");
        global.then_some(Parent::OwlGlobal)
    }

    /// The members of a class, its parents, and every prototype patch of them but `except`, or
    /// None when a parent cannot be resolved. `reader` is the series of the module reading them.
    pub fn js_members(
        &self,
        t: JsTarget,
        class: &'m JsClass,
        except: PatchId,
        reader: Option<Series>,
    ) -> Option<FxHashSet<&'m str>> {
        self.class_chain(t, class, Some(except), reader)
            .map(|(members, _)| members)
    }

    /// The members of a component, as `js_members` reads them with every patch: None for a class
    /// whose chain does not end at Owl's `Component`, or when a parent cannot be resolved.
    pub fn component_members(
        &self,
        t: JsTarget,
        class: &'m JsClass,
        reader: Option<Series>,
    ) -> Option<FxHashSet<&'m str>> {
        let (members, component) = self.class_chain(t, class, None, reader)?;
        component.then_some(members)
    }

    /// The members of `js_members`, and whether the chain ends at Owl's `Component`.
    fn class_chain(
        &self,
        t: JsTarget,
        class: &'m JsClass,
        except: Option<PatchId>,
        reader: Option<Series>,
    ) -> Option<(FxHashSet<&'m str>, bool)> {
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
                .filter(|(p, _)| Some(*p) != except)
            {
                members.extend(added.iter().map(String::as_str));
            }
            let parent = match self.js_parent(t, &class.extends)? {
                Parent::Root => return Some((members, false)),
                Parent::Owl2 => {
                    members.extend(OWL_COMPONENT);
                    return Some((members, true));
                }
                Parent::OwlGlobal => {
                    members.extend(self.owl_global(t.module, reader));
                    return Some((members, true));
                }
                Parent::Name(parent) => parent,
            };
            (t, class) = self.js_class(t, parent, 0)?;
        }
        None
    }

    /// The members of the `Component` of the `owl` global in `module`, by its series, else
    /// `default_series`, else the reader's, as core's `web` gives `'version': '1.0'`. With no
    /// series known, Owl 1's, a superset of Owl 2's, so neither OD602 nor OD613 reports a member
    /// as missing.
    fn owl_global(&self, module: usize, reader: Option<Series>) -> &'static [&'static str] {
        match self.modules[module].series.or(self.default_series).or(reader) {
            Some(series) if series >= OWL2 => OWL_COMPONENT,
            _ => OWL1_COMPONENT,
        }
    }

    /// What a class of the file `t` extends, or None for a class beyond reach: an expression, or
    /// an Owl class other than `Component`, whose members are not indexed.
    fn js_parent<'h>(&self, t: JsTarget, heritage: &'h Heritage) -> Option<Parent<'h>> {
        match heritage {
            Heritage::None => Some(Parent::Root),
            Heritage::Owl(name) if name == "Component" => Some(Parent::OwlGlobal),
            Heritage::Owl(_) | Heritage::Other => None,
            Heritage::Name(parent) => Some(self.owl_component(t, parent).unwrap_or(Parent::Name(parent))),
        }
    }

    /// Whether Odoo installs `module` with `base`, as it does `web`, so any module may use it.
    fn always_installed(&self, module: usize) -> bool {
        self.always_installed[module]
    }

    pub fn js_loaded(&self, module: usize, file: usize) -> bool {
        self.js_bundles.contains_key(&(module, file))
    }

    /// The bundles that load the script, sorted by name.
    pub fn js_bundles(&self, module: usize, file: usize) -> impl Iterator<Item = &str> {
        let of_file = self
            .js_bundles
            .get(&(module, file))
            .map(Vec::as_slice)
            .unwrap_or_default();
        of_file.iter().map(|&bi| self.bundle_names[bi].as_str())
    }

    /// Whether the script is a test: by its path, as `SourceFile::in_tests` reads it, or loaded only
    /// by test bundles, as `test_bundle` names them.
    pub fn js_test(&self, module: usize, file: usize) -> bool {
        self.test_scripts.contains(&(module, file))
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

    /// Whether a loaded script adds `key` to the registry `category` in scope, as `registries` counts
    /// them.
    pub fn registry_status(&self, category: &str, key: &str, scope: &Scope) -> Status<'m> {
        let definers = self.registries.keys.get(&(category, key));
        self.definer_status(definers.into_iter().flatten().copied(), scope)
    }

    /// Whether the field registry holds `widget` in scope, alone or after any prefix and a dot.
    pub fn field_widget_status(&self, widget: &str, scope: &Scope) -> Status<'m> {
        let definers = self.registries.field_widgets.get(widget);
        self.definer_status(definers.into_iter().flatten().copied(), scope)
    }

    /// Whether a module in scope adds a key only a run tells to the registry `category`.
    pub fn registry_open(&self, category: &str, scope: &Scope) -> bool {
        let openers = self.registries.open.get(category);
        openers.into_iter().flatten().any(|&m| scope.sees(m))
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

    /// The file a specifier imported by the script at the URL `from` names, as `js_lookup` reads it.
    /// A name two files claim goes to the first `js_paths` lists: the file of the module loaded
    /// first, or of one module the one whose path sorts first. Opaque since a page that loads only
    /// the other gets that one.
    pub fn js_resolve(&self, from: &str, specifier: &str) -> JsSpec {
        let Some(JsLookup { name, home }) = js_lookup(from, specifier) else {
            return JsSpec::Outside;
        };
        if let Some(claims) = self.js_paths.get(name.as_ref()) {
            let first = claims[0];
            return JsSpec::Found(JsTarget {
                opaque: first.opaque || claims.len() > 1,
                ..first
            });
        }
        let Some((module, folder)) = home else {
            return JsSpec::Outside;
        };
        // A core cached by an earlier odooscan or a sparse checkout made without tests has none of
        // their scripts, so a test path names no file there.
        let folder = format!("static/{folder}/");
        match self.by_name.get(module) {
            Some(&mi) if self.modules[mi].js.iter().any(|f| f.rel.starts_with(&folder)) => JsSpec::Missing(mi),
            _ => JsSpec::Outside,
        }
    }

    /// The script of module `mi` whose path the specifier names but which its series does not
    /// transpile, with what would let a script load it. Call it only for a name no file
    /// registers: it does not check that another `odoo.define()` leaves the name unregistered.
    pub fn js_untranspiled(&self, mi: usize, from: &str, specifier: &str) -> Option<Untranspiled<'m>> {
        let name = js_lookup(from, specifier)?.name;
        let module = &self.modules[mi];
        let series = module.series.or(self.default_series);
        let names = |rel: &str| {
            js_module_path(&module.name, rel)
                .is_some_and(|p| p == name || p.strip_suffix("/index") == Some(name.as_ref()))
        };
        let file = module
            .js
            .iter()
            .find(|f| names(&f.rel) && !transpiled_names(&f.facts, series))?;
        let tag_fix = match file.facts.annotation(series) {
            Annotation::Ignore => Some(TagFix::DropIgnore),
            _ if js::transpiles(series) => Some(TagFix::Annotate),
            _ => None,
        };
        Some(Untranspiled {
            rel: &file.rel,
            tag_fix,
            defined: file.facts.defines.iter().find_map(|d| d.name.as_deref()),
        })
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
        let from = js_url(&module.name, &file.rel);
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

    /// The modules `module` may use, as `scope_of` reads them in the series of `module`
    /// (`series_of`).
    pub fn scope(&self, module: usize) -> Scope {
        let visible = scope_of(
            self.modules,
            &self.by_name,
            &self.always_installed,
            [module],
            self.series_of(module),
        );
        Scope {
            records: visible.clone(),
            visible,
            demo: false,
        }
    }

    /// The series of `module`, else `default_series`, under which a dependency naming no series reads
    /// its `depends`: it installs on the same Odoo as the module that depends on it.
    fn series_of(&self, module: usize) -> Option<Series> {
        self.modules[module].series.or(self.default_series)
    }

    /// A scope that sees every indexed module, as on a database that installs them all.
    fn every_module(&self) -> Scope {
        let all = vec![true; self.modules.len()];
        Scope {
            records: all.clone(),
            visible: all,
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
    /// `series` is the checked module's own, else the default: the `named` modules read their
    /// `depends` and triggers in it.
    pub fn guard_scope(&self, module: usize, named: impl IntoIterator<Item = usize>, series: Option<Series>) -> Scope {
        let scope = self.scope(module).visible;
        let (proven, _) = depends_closure(self.modules, &self.by_name, named, series);
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
    /// of the queries it runs ahead of the module's data, from `pre_init_hook` to `init()`. That
    /// comes ahead of the data files of `module`, so the `ir.model.fields` records that count are
    /// those of its dependencies and of the always-installed modules that load first: one that
    /// loads after has not run its data files yet on the database that installs both. A failure
    /// there refuses the install whatever the file order.
    pub fn setup_scope(&self, module: usize) -> Scope {
        let (mut records, _) = depends_closure(self.modules, &self.by_name, [module], self.series_of(module));
        for (mi, held) in records.iter_mut().enumerate() {
            *held |= self.always_installed[mi] && self.loads_before(mi, module);
        }
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

    /// Whether `from` may use `to`, as `scope` of `from` holds it: Odoo loads `to` wherever `from`
    /// is installed.
    pub fn sees(&self, from: usize, to: usize) -> bool {
        from == to || self.always_installed(to) || self.reaches(from, to)
    }

    /// Whether Odoo loads `a` ahead of `b` on a database that installs both.
    pub fn loads_before(&self, a: usize, b: usize) -> bool {
        self.load_rank[a] < self.load_rank[b]
    }

    /// Whether `importer` is a module that loads after `module` and not before it, so that its
    /// imports of `module`'s files run once `module` has loaded.
    pub fn imports_later(&self, importer: &str, module: usize) -> bool {
        let d = self.by_name.get(importer).copied();
        d.is_some_and(|d| self.loads_after(d, module) && !self.loads_after(module, d))
    }

    /// Whether a module other than `module` that depends on it, directly or not, has a file that may
    /// import any file of it (`imports::Graph::unread`).
    pub fn has_unread_dependent(&self, module: usize) -> bool {
        self.unread_dependent[module]
    }

    /// Whether Odoo loads `from` after `to` on every database that installs both: `from` reaches
    /// `to`, or `to` is `base`, which Odoo loads ahead of every other module on every series (18.0
    /// `odoo/modules/loading.py:407-409`) whatever `depends` a dependency naming no series reads.
    pub fn loads_after(&self, from: usize, to: usize) -> bool {
        from != to && (self.modules[to].name == "base" || self.reaches(from, to))
    }

    /// Whether `from` depends on `to`, directly or through other modules, as `read_depends` reads
    /// them in the series of `from` (`series_of`), so that `sees` agrees with `scope`.
    pub fn reaches(&self, from: usize, to: usize) -> bool {
        let series = self.series_of(from);
        let mut seen = vec![false; self.modules.len()];
        let mut stack = vec![from];
        while let Some(m) = stack.pop() {
            if m == to {
                return true;
            }
            if std::mem::replace(&mut seen[m], true) {
                continue;
            }
            stack.extend(read_depends(&self.modules[m], series).filter_map(|d| self.by_name.get(d)));
        }
        false
    }

    /// The `depends` entries of `module` that another entry already brings, as (entry, implier),
    /// both positions in `depends`. The implier is the first entry no other one brings, so the
    /// top of a chain. An entry a list `auto_install` names stays, since the list must stay inside
    /// `depends`, and an entry in a cycle with the module is OD504's, so it implies nothing.
    pub fn implied_depends(&self, module: usize) -> Vec<(usize, usize)> {
        let manifest = &self.modules[module].manifest;
        let closures: Vec<Option<(usize, Vec<bool>)>> = manifest
            .depends
            .iter()
            .map(|d| self.candidate_closure(module, &d.value))
            .collect();
        let implies = |e: usize, d: usize| match (&closures[e], &closures[d]) {
            (Some((em, ec)), Some((dm, dc))) => ec[*dm] && !dc[*em],
            _ => false,
        };
        let entries = 0..closures.len();
        let implied: Vec<bool> = entries
            .clone()
            .map(|d| entries.clone().any(|e| implies(e, d)))
            .collect();
        let triggers = match &manifest.auto_install {
            AutoInstall::Listed(names) => names.iter().map(|n| n.value.as_str()).collect(),
            _ => vec![],
        };
        let implier = |d: usize| {
            let mut impliers = entries.clone().filter(|&e| implies(e, d));
            let first = impliers.clone().next();
            impliers.find(|&e| !implied[e]).or(first)
        };
        entries
            .clone()
            .filter(|&d| !triggers.contains(&manifest.depends[d].value.as_str()))
            .filter_map(|d| implier(d).map(|e| (d, e)))
            .collect()
    }

    /// What each `depends` entry of `module` alone brings it, as (entry, modules), the entry a
    /// position in `depends`: the entry, and each module of its closure that Odoo does not install
    /// on every database (`always_installed`) and no other entry's closure holds, implied entries'
    /// closures not taken away. Leaves out `base`, which saas~18.2 restores in an empty `depends`,
    /// and the entries whose use is not to be read from the module: one not indexed or with a
    /// dependency that is not, one another entry already brings (OD520), and one leading back to
    /// the module (OD504), whose closure holds the module's own and so is taken from no other.
    pub fn sole_brought(&self, module: usize) -> Vec<(usize, Vec<bool>)> {
        let depends = &self.modules[module].manifest.depends;
        let series = self.series_of(module);
        let closures: Vec<Option<(usize, Vec<bool>, bool)>> = depends
            .iter()
            .map(|d| {
                let &mi = self.by_name.get(d.value.as_str())?;
                let (closure, complete) = depends_closure(self.modules, &self.by_name, [mi], series);
                Some((mi, closure, complete)).filter(|(_, closure, _)| !closure[module])
            })
            .collect();
        let implied: Vec<usize> = self.implied_depends(module).into_iter().map(|(d, _)| d).collect();
        let mut brought = vec![];
        for (d, entry) in closures.iter().enumerate() {
            let Some((mi, closure, true)) = entry else {
                continue;
            };
            if depends[d].value == "base" || implied.contains(&d) {
                continue;
            }
            let mut sole: Vec<bool> = closure
                .iter()
                .zip(&self.always_installed)
                .map(|(&c, &a)| c && !a)
                .collect();
            // An implied entry comes with another, so dropping that one loses it: what it brings
            // stays in the other's share.
            let others = closures
                .iter()
                .enumerate()
                .filter(|&(e, _)| e != d && !implied.contains(&e));
            for (_, other, _) in others.filter_map(|(_, c)| c.as_ref()) {
                sole.iter_mut().zip(other).for_each(|(s, &o)| *s &= !o);
            }
            sole[*mi] = true;
            brought.push((d, sole));
        }
        brought
    }

    /// Whether `words` take anything from the modules of `set`: a word joining the name of one with
    /// others, as an XML id, a Python import or a JS specifier does; a name `gives` holds; or a URL
    /// path one routes.
    pub fn takes_from(&self, words: &Words, set: &[bool], scope: &Scope, module: usize) -> bool {
        let gives = |word: &str| self.gives(word, set, scope, module);
        let module_named = |segment: &str| self.by_name.get(segment).is_some_and(|&mi| set[mi]);
        let routes = self.sole_routes(set, scope, module);
        let routed = |word: &str| routes.iter().any(|r| r.matches(word));
        words.iter().any(|word| {
            let mut segments = Words::segments(word);
            gives(word) || routed(word) || segments.any(|s| module_named(s) || gives(s))
        })
    }

    /// Whether `name` is something the modules of `set` give: a model one introduces, or its
    /// table; a template or QWeb view one defines; or a member `gives_member` holds.
    fn gives(&self, name: &str, set: &[bool], scope: &Scope, module: usize) -> bool {
        let introduces = |model: &str| self.introduced_in(model, set);
        introduces(name)
            || self.table_models(name).iter().any(|m| introduces(m))
            || self.templates.definers(name).any(|mi| set[mi])
            || self.qweb_definers(name).any(|mi| set[mi])
            || self.gives_member(name, set, scope, module)
    }

    /// Whether the modules of `set` define field, method or `selection_add` value `name` where it
    /// is theirs. On a model they introduce, or as a method every model has or a hook the ORM runs,
    /// which a word names on any model: when no module of `scope` outside them but `module` defines
    /// the name. Otherwise when no such module defines it on the model, or when it is a method,
    /// whose override may be all that makes a call do anything (18.0 `base_vat` on `_run_vat_test`).
    fn gives_member(&self, name: &str, set: &[bool], scope: &Scope, module: usize) -> bool {
        let Some(members) = self.members.get(name) else {
            return false;
        };
        let elsewhere = |m: &&Member| !set[m.module] && m.module != module && scope.sees(m.module);
        let others: Vec<&Member> = members.iter().filter(elsewhere).collect();
        let generic = HOOK_PREFIXES.iter().any(|p| name.starts_with(p))
            || [BASE_MODEL_METHODS, BASE_MODEL_EXTRA, BASE_MODEL_DUNDERS, BASE_METHODS]
                .iter()
                .any(|list| list.contains(&name));
        members.iter().filter(|m| set[m.module]).any(|m| {
            if self.introduced_in(m.model, set) || (m.method && generic) {
                others.is_empty()
            } else {
                m.method || others.iter().all(|o| o.model != m.model)
            }
        })
    }

    fn introduced_in(&self, model: &str, set: &[bool]) -> bool {
        self.classes(model).iter().any(|&(mi, c)| c.new_model && set[mi])
    }

    /// The URL paths the controllers of `set` route and no module of `scope` outside it but `module`
    /// does.
    fn sole_routes(&self, set: &[bool], scope: &Scope, module: usize) -> Vec<RoutePath<'_>> {
        let mut ours = vec![];
        let mut theirs = FxHashSet::default();
        for addon in self.routes.addons() {
            let Some(&mi) = self.by_name.get(addon) else {
                continue;
            };
            let paths = self.routes.module(addon).flat_map(|at| &at.class.routes);
            let urls = paths.filter_map(|r| match &r.urls {
                RouteKey::Given(urls) => Some(urls),
                _ => None,
            });
            for url in urls.flatten() {
                if set[mi] {
                    ours.extend(RoutePath::of(url));
                } else if mi != module && scope.sees(mi) {
                    theirs.insert(url.as_str());
                }
            }
        }
        ours.retain(|r| !theirs.contains(r.url));
        ours
    }

    /// Whether a module of `set` loads a data file the scan cannot read that may hold a view.
    fn loses_views(&self, set: &[bool]) -> bool {
        let mut modules = self.modules.iter().enumerate();
        modules.any(|(mi, other)| set[mi] && other.lost_views())
    }

    /// Whether a module of `set` loads a template file the scan cannot read. Its `t-inherit` is
    /// unread too, so it may extend any template tree.
    fn loses_templates(&self, set: &[bool]) -> bool {
        self.template_losers.iter().any(|&mi| set[mi])
    }

    /// The views the modules of `set` load, each with its module, read from the data files the view
    /// index reads, so a view without an XML id counts.
    fn set_views<'s>(&'s self, set: &'s [bool]) -> impl Iterator<Item = (usize, &'m ViewFact)> + 's {
        let modules = self.modules.iter().enumerate().filter(|&(mi, _)| set[mi]);
        modules.flat_map(|(mi, module)| {
            let files = loaded_data(module, module.series.or(self.default_series));
            files.flat_map(move |f| f.facts.views.iter().map(move |v| (mi, v)))
        })
    }

    /// Whether a module of `set` loads a view that may be of one of `models`, as `view_models` and
    /// `of_models` tell.
    fn has_view_of(&self, set: &[bool], models: &[Option<&str>]) -> bool {
        self.set_views(set)
            .any(|(mi, v)| of_models(self.view_models(mi, v).as_deref(), models))
    }

    /// For each view of `set` naming its parent in a form only Odoo resolves, when it loads it, and
    /// not setting `mode` to `primary`, the models it may have: such a stray may extend any tree of
    /// one of them. None when any, as for a stray whose model the scan does not read.
    fn strays(&self, set: &[bool]) -> Vec<Option<Vec<Option<&'m str>>>> {
        let strays = self.stray_views(set);
        strays.map(|(mi, v)| self.view_models(mi, v)).collect()
    }

    /// The views of `set` that `strays` gives the models of, with their modules.
    fn stray_views<'s>(&'s self, set: &'s [bool]) -> impl Iterator<Item = (usize, &'m ViewFact)> + 's {
        self.set_views(set)
            .filter(|(_, v)| v.unread_parent && v.primary != Some(true))
    }

    /// The models the view `v` of module `mi` may have, None when any. Odoo writes only the fields
    /// a record gives, so one naming no model under an XML id may update a view in place, which
    /// keeps the model the index holds for it.
    fn view_models(&self, mi: usize, v: &'m ViewFact) -> Option<Vec<Option<&'m str>>> {
        match (&v.model, &v.id) {
            _ if v.model_unread => None,
            (Some(model), _) => Some(vec![Some(model.value.as_str())]),
            (None, None) => Some(vec![None]),
            (None, Some(id)) => self.views.stored(&full_id(&self.modules[mi].name, id), self.modules),
        }
    }

    /// Whether a spec of a loaded view or template of `module` locates an element that, of the
    /// modules of `scope`, only those of `set` add to the tree it extends. A spec locating what no
    /// readable view outside `set` adds counts too, when `set` extends the tree with a view whose
    /// arch the scan did not read (`arch_base`, `search`, `ref`, `eval`, file, text, a cell of an
    /// `ir.ui.view` CSV), loads an unreadable data file that may hold one or an unreadable template
    /// file, or holds a stray, a view naming its parent in a form the scan does not read, as by name
    /// or database id in an `ir.ui.view` CSV, which may extend any tree of its model: Odoo combines
    /// all extensions below the root. A spec on a tree the index does not hold up to the root counts,
    /// unless the index holds that view tree all the way up from modules parsing every data file
    /// that may hold a view, each naming its parent and model in a form the scan reads, with an
    /// unread arch on the way: that spec counts only when `set` loads a view of a model on the way or
    /// of an unread model, or an unreadable file that may hold one.
    pub fn locates_in(&self, module: usize, set: &[bool], scope: &Scope) -> bool {
        let m = &self.modules[module];
        let name = m.name.as_str();
        let lost_views = self.loses_views(set);
        // Walking every data file of `set` costs, so only a module with a spec pays for it.
        let strays = OnceCell::new();
        let views = m.data.iter().filter(|f| f.rank.is_some()).flat_map(|f| &f.facts.views);
        let mut views = views.filter(|v| !v.locators.is_empty());
        let templates = m.templates.iter().enumerate();
        let templates = templates.filter(|&(fi, _)| self.template_loaded(module, fi));
        let mut templates = templates.flat_map(|(_, f)| &f.facts.templates);
        let lost_templates = self.loses_templates(set);
        views.any(|v| {
            let strays = strays.get_or_init(|| self.strays(set));
            self.view_locates_in(v, name, set, scope, lost_views, strays)
        }) || templates.any(|t| self.template_locates_in(t, set, scope, lost_templates))
    }

    /// `locates_in` for the template `t`. `lost` says `set` loads a template file the scan cannot
    /// read, which may extend any tree, as a template has no model to tell the trees it may add to.
    fn template_locates_in(&self, t: &OwlTemplate, set: &[bool], scope: &Scope, lost: bool) -> bool {
        let Some(parent) = t.inherit.as_ref().filter(|_| !t.arch.locators.is_empty()) else {
            return false;
        };
        let elements = t.arch.locators.iter().map(|l| l.value.clone()).collect();
        let sole = self.sole(&self.templates, set, scope, lost, parent.value.clone(), elements);
        sole.unwrap_or(true)
    }

    /// `locates_in` for the view `v` of the module `name`. On a tree whose arch the scan did not read
    /// all the way up, `set` may add to it only with a view of a model on the way, or a data file it
    /// cannot read that may hold one: Odoo combines only extensions whose model equals their
    /// parent's. Above a view the index does not hold from its own module, or whose module loads a
    /// data file that may hold a view and does not parse, or one whose parent or model the scan does
    /// not read, any model may sit, so the spec counts, as it does when `v`'s own model is unread.
    /// A stray of `strays`, given as the models it may have, extends the tree unread when one is on
    /// the way or is `v`'s.
    fn view_locates_in(
        &self,
        v: &ViewFact,
        name: &str,
        set: &[bool],
        scope: &Scope,
        lost: bool,
        strays: &[Option<Vec<Option<&str>>>],
    ) -> bool {
        let Some(parent) = &v.inherit else { return false };
        let parent = full_id(name, parent);
        let elements = v.locators.iter().map(|l| located(&l.value, name)).collect();
        let models = self.tree_models(v, parent.clone());
        let lost = lost || strays.iter().any(|x| stray_on(x.as_deref(), models.as_deref()));
        self.sole(&self.views, set, scope, lost, parent, elements)
            .unwrap_or_else(|| models.is_none_or(|models| lost || self.has_view_of(set, &models)))
    }

    /// The models on the way up from `parent`, the parent of the view `v`, with `v`'s own: those a
    /// view may have to extend `v`'s tree unread. None when any, as `Inheritance::models` tells or
    /// when `v`'s model is unread.
    fn tree_models<'a>(&'a self, v: &'a ViewFact, parent: String) -> Option<Vec<Option<&'a str>>> {
        let mut models = self.views.models(parent, self.modules).filter(|_| !v.model_unread)?;
        models.push(v.model.as_ref().map(|m| m.value.as_str()));
        Some(models)
    }

    /// The elements views of `scope` may add to the tree of the view `v`, whose parent is `parent`
    /// and whose root is `root`, beyond those the tree holds: what each stray whose model is on the
    /// way or is `v`'s adds, with the views below it, since Odoo combines every extension of the
    /// tree from a loaded module. None when any, as when a view below the root or on such a branch
    /// writes its arch in a form the scan does not read.
    pub fn extra_elements(
        &self,
        v: &ViewFact,
        parent: String,
        root: &str,
        scope: &Scope,
    ) -> Option<FxHashSet<ArchElement>> {
        if self.views.any(root, |mi, f| scope.sees(mi) && f.arch_unread) {
            return None;
        }
        let models = self.tree_models(v, parent);
        let on = |mi, s| stray_on(self.view_models(mi, s).as_deref(), models.as_deref());
        let mut extra = FxHashSet::default();
        for (mi, s) in self.stray_views(&scope.visible).filter(|&(mi, s)| on(mi, s)) {
            extra.extend(self.branch(mi, s, scope)?);
        }
        Some(extra)
    }

    /// The elements the stray `s` of module `mi` and the views of `scope` below it add, None when
    /// one of them writes its arch in a form the scan does not read. One without an XML id has
    /// nothing below it.
    fn branch(&self, mi: usize, s: &ViewFact, scope: &Scope) -> Option<FxHashSet<ArchElement>> {
        let name = self.modules[mi].name.as_str();
        let Some(id) = &s.id else {
            return (!s.arch_unread).then(|| s.elements.iter().map(|e| located(e, name)).collect());
        };
        let id = full_id(name, id);
        if self.views.any(&id, |m, f| scope.sees(m) && f.arch_unread) {
            return None;
        }
        Some(self.views.tree(id, self.modules, |m| scope.sees(m)).visible)
    }

    /// Whether one of `elements`, located below `parent` in `tree`, is one only `set` adds of the
    /// modules of `scope`, or one no readable view outside `set` adds when `set` extends the tree
    /// unread. `lost` says `set` may do so in a file the scan cannot read. None when the index does
    /// not hold the way up to the tree's root.
    fn sole(
        &self,
        tree: &Inheritance<'m>,
        set: &[bool],
        scope: &Scope,
        lost: bool,
        parent: String,
        elements: Vec<ArchElement>,
    ) -> Option<bool> {
        let root = tree.root(parent)?;
        let unread = lost || tree.any(&root, |mi, fact| set[mi] && fact.arch_unread);
        let all = tree.tree(root.clone(), self.modules, |mi| scope.sees(mi));
        let without = tree.tree(root, self.modules, |mi| scope.sees(mi) && !set[mi]);
        let mut elements = elements.iter();
        Some(elements.any(|e| (unread || all.visible.contains(e)) && !without.visible.contains(e)))
    }

    /// The index and the `depends` closure of the dependency `name` of `module`, when it is
    /// indexed, is another module, and does not lead back to `module`.
    fn candidate_closure(&self, module: usize, name: &str) -> Option<(usize, Vec<bool>)> {
        let &mi = self.by_name.get(name).filter(|&&mi| mi != module)?;
        let (closure, _) = depends_closure(self.modules, &self.by_name, [mi], self.series_of(module));
        (!closure[module]).then_some((mi, closure))
    }

    /// Whether a module in `scope` depends on a module that is not indexed.
    pub fn partial_scope(&self, scope: &Scope) -> bool {
        self.unindexed_depends(scope).next().is_some()
    }

    /// The `depends` names of the modules in `scope` that the index does not hold.
    fn unindexed_depends<'s>(&'s self, scope: &'s Scope) -> impl Iterator<Item = &'m str> + 's {
        let seen = self.modules.iter().enumerate().filter(|(mi, _)| scope.sees(*mi));
        let names = seen.flat_map(|(_, m)| &m.manifest.depends).map(|d| d.value.as_str());
        names.filter(|name| !self.by_name.contains_key(name))
    }

    /// Per unindexed dependency, by name, the checked modules whose scope reaches it while none of
    /// their own `depends` is unindexed. Those get no OD503, yet `partial_scope` silences rules in
    /// them; one with an unindexed `depends` of its own, which OD503 or the notice on that
    /// dependency's manifest names, would stay silent after indexing just this dependency.
    pub fn deep_gaps(&self) -> BTreeMap<&'m str, Vec<&'m str>> {
        let mut gaps: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (mi, module) in self.modules.iter().enumerate().filter(|(_, m)| m.checked) {
            for name in self.reached_gaps(mi) {
                gaps.entry(name).or_default().push(module.name.as_str());
            }
        }
        gaps.values_mut().for_each(|modules| modules.sort_unstable());
        gaps
    }

    /// The unindexed dependencies the scope of module `mi` reaches, or none when one of its own
    /// `depends` is unindexed.
    fn reached_gaps(&self, mi: usize) -> BTreeSet<&'m str> {
        let own = &self.modules[mi].manifest.depends;
        if own.iter().any(|d| !self.by_name.contains_key(d.value.as_str())) {
            return BTreeSet::new();
        }
        self.unindexed_depends(&self.scope(mi)).collect()
    }

    /// Whether a module whose records have loaded when the reader of `scope` runs loads a data file
    /// the scan cannot read.
    pub fn lost_scope(&self, scope: &Scope) -> bool {
        let mut modules = self.modules.iter().enumerate();
        modules.any(|(mi, m)| scope.records[mi] && m.lost_data())
    }

    /// Whether a module of `scope` loads a template file the scan cannot read, which may define any
    /// template name and extend any template tree.
    pub fn lost_templates(&self, scope: &Scope) -> bool {
        self.loses_templates(&scope.visible)
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
        let (file, class) = self.path_file(path)?;
        Some(file.facts.classes.iter().any(|c| c.class_name == class))
    }

    /// The indexed file a dotted path of `odoo.addons` imports a class from, and the class name: see
    /// `class_file`.
    fn path_file<'p>(&self, path: &'p str) -> Option<(&'m SourceFile<PyFacts>, &'p str)> {
        let (addon, files, class) = class_file(path)?;
        let module = &self.modules[*self.by_name.get(addon)?];
        let file = module.python.iter().find(|f| files.contains(&f.rel))?;
        Some((file, class))
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
    /// report costs more than a miss. None when a class of `model` is unknown to the index, or no
    /// class introduces `model`. A plain base of a class of `base` is not unknown here: Odoo reads
    /// `_inherit` from each class body (18.0 `odoo/models.py:246-248`, `:734-736`; 20.0
    /// `odoo/orm/models.py:263-266`, `odoo/orm/model_classes.py:181-183`) and `_table` from the
    /// classes that define a model (18.0 `odoo/models.py:826-832`, 20.0
    /// `odoo/orm/model_classes.py:291-297`).
    pub fn stands_for(&self, model: &str, comodel: &str, scope: &Scope) -> Option<bool> {
        if model == comodel {
            return Some(true);
        }
        let classes = self.chain_read_from(model, scope);
        let unknown = |c: &ClassFact| match c.model.as_str() {
            "base" => c.inherit_unknown,
            _ => c.hidden_parents(),
        };
        if !self.has_introducer(model) || classes.iter().any(|(_, c)| unknown(c)) {
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

    /// Whether `model` is defined in scope for module `reader` (None counts every class), and for
    /// its class when a class lists it as a parent. Odoo builds a module's classes in the order
    /// Python creates them (18.0 `odoo/modules/registry.py:290-292`, 20.0
    /// `odoo/orm/registry.py:449-451`), and building one whose parent is not in the registry yet
    /// fails (18.0 `odoo/models.py:759-761`, 20.0 `odoo/orm/model_classes.py:208-210`), so an
    /// introducer of the class's module counts unless `builds_before` puts it after the class. A
    /// module that loads after the reader's (`follows`) has none of its classes count
    /// (`dependent_introducer` names it): naming it would ask for a cycle in `depends`, or, from
    /// `base`, for a module Odoo loads once `base` has loaded alone (18.0
    /// `odoo/modules/loading.py:407-409`, 20.0 `:359-361`). With no introducer among the counted
    /// classes, only an extender whose scope may hide one (`may_hide_introducer`) is a source, else
    /// the model is missing.
    pub fn model_status(&self, model: &str, scope: &Scope, reader: Option<(usize, Option<&ClassFact>)>) -> Status<'m> {
        let classes = self.classes(model);
        let others = || classes.iter().filter(|(mi, c)| self.counts(*mi, c, reader));
        let introducer = self.introducer(others());
        // A class-less reader in a module that extends the model: the `_inherit` there gets the finding.
        if introducer.is_none() && others().any(|(mi, _)| reader.is_some_and(|(e, _)| *mi == e)) {
            return Status::Visible;
        }
        let mut first = None;
        let sources = others().map(|(mi, _)| *mi);
        for mi in sources.filter(|&mi| introducer.is_some() || self.may_hide_introducer(model, mi)) {
            if scope.sees(mi) {
                return Status::Visible;
            }
            first = first.or(Some(mi));
        }
        match introducer.or(first) {
            Some(mi) => Status::Hidden(&self.modules[mi].name),
            None => Status::Missing,
        }
    }

    /// Whether the scope of module `mi` may introduce `model` out of the scan's sight: a module in it
    /// depends on one the index does not hold or has a file the scan cannot read at all, `mi` names a
    /// model at runtime (`names_models_at_runtime`, which covers a Python file that does not parse),
    /// or another module in it builds a name `model` fits (`builds_model_name`).
    fn may_hide_introducer(&self, model: &str, mi: usize) -> bool {
        let scope = self.scope(mi);
        let mut seen = self.modules.iter().enumerate().filter(|(i, _)| scope.sees(*i));
        self.partial_scope(&scope)
            || names_models_at_runtime(&self.modules[mi])
            || seen.any(|(i, m)| m.unread_text || (i != mi && builds_model_name(m, model)))
    }

    /// The first-loaded module other than `except` with a class of `model` that `model_status`
    /// counts for `reader`: with no introducer, one that only extends the model.
    pub fn extender(&self, model: &str, except: usize, reader: Option<(usize, Option<&ClassFact>)>) -> Option<&'m str> {
        let classes = self.classes(model).iter();
        let extenders = classes.filter(|(mi, c)| *mi != except && self.counts(*mi, c, reader));
        let first = self.first_loaded(extenders.map(|(mi, _)| (*mi, ())));
        first.map(|(mi, _)| self.modules[mi].name.as_str())
    }

    /// Whether class `c` of module `mi` puts its model in the registry for `model_status`.
    fn counts(&self, mi: usize, c: &ClassFact, reader: Option<(usize, Option<&ClassFact>)>) -> bool {
        match reader {
            Some((e, Some(class))) if mi == e => c.new_model && !self.builds_before(e, class, c),
            Some((e, _)) => !self.follows(mi, e),
            None => true,
        }
    }

    /// Whether module `mi` loads after module `e`: it depends on `e`, or `e` is `base`
    /// (`loads_after`). `loads_before` goes first, as a lookup: outside a cycle, which is OD504's,
    /// a module that reaches `e` loads after it.
    pub fn follows(&self, mi: usize, e: usize) -> bool {
        self.loads_before(e, mi) && self.loads_after(mi, e)
    }

    /// The module that introduces `model` among those that load after module `e` (`follows`), which
    /// `model_status` does not count for a reader in `e`.
    pub fn dependent_introducer(&self, model: &str, e: usize) -> Option<&'m str> {
        let later = self.classes(model).iter().filter(|(mi, _)| self.follows(*mi, e));
        self.introducer(later).map(|mi| self.modules[mi].name.as_str())
    }

    /// Whether module `mi` builds its class `first` before its class `then`: in one file by place,
    /// else by `build_order` of their files. False when the scan cannot tell.
    fn builds_before(&self, mi: usize, first: &ClassFact, then: &ClassFact) -> bool {
        let (Some((a, _)), Some((b, _))) = (self.file_of(mi, first), self.file_of(mi, then)) else {
            return false;
        };
        if std::ptr::eq(a, b) {
            return first.pos < then.pos;
        }
        let Some(order) = &self.build_order[mi] else {
            return false;
        };
        let key = |rel: &str, class: &ClassFact| {
            let mut key = order.get(rel)?.clone();
            key.push((class.pos, 0));
            Some(key)
        };
        key(&a.rel, first).zip(key(&b.rel, then)).is_some_and(|(a, b)| a < b)
    }

    /// The module of the class that introduces the model with `_name`, the first in load order
    /// (ties: see `first_loaded`).
    fn introducer<'c>(&self, classes: impl Iterator<Item = &'c (usize, &'m ClassFact)>) -> Option<usize>
    where
        'm: 'c,
    {
        let introducers = classes.filter(|(_, c)| c.new_model).map(|(mi, _)| (*mi, ()));
        self.first_loaded(introducers).map(|(mi, _)| mi)
    }

    /// The entry, by module, that loads first. Entries of independent modules, or of one module,
    /// keep the order of `entries` (for OD229, the chain order of `merged_classes`), since Odoo's
    /// tie order between independent modules is unknown.
    pub fn first_loaded<T>(&self, entries: impl IntoIterator<Item = (usize, T)>) -> Option<(usize, T)> {
        entries
            .into_iter()
            .reduce(|first, next| if self.loads_after(first.0, next.0) { next } else { first })
    }

    /// Every class of `model` and of the models it inherits from: see `model_chain`.
    fn chain(&self, model: &str, scope: Option<&Scope>, delegated: bool) -> Vec<(usize, &'m ClassFact)> {
        model_chain(&self.models, model, scope, delegated)
    }

    pub fn field_defs(&self, model: &str, field: &str, scope: Option<&Scope>) -> Vec<FieldDef<'m>> {
        defs_named(self.chain(model, scope, true), field)
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
        let chain = self.chain(model, Some(scope), true);
        self.hides_parents_in(&chain, &self.merged_classes(model, scope))
    }

    /// Whether a class of `chain` has a parent the scan cannot see. A class of `base` is in every
    /// chain, so for it a plain base that is a class of `merged`, the `merged_classes` of the model,
    /// counts as seen: else a `class Base(Base)` would hide the parents of every model of its module
    /// and its dependents. A class of a model reached only through `_inherits` does not count, as it
    /// gives the model its methods through the MRO of `base`.
    fn hides_parents_in(&self, chain: &[(usize, &'m ClassFact)], merged: &[(usize, &'m ClassFact)]) -> bool {
        chain.iter().any(|&(mi, c)| match c.model.as_str() {
            "base" => self.unread_parents(mi, c, merged, &c.plain_sources),
            _ => c.hidden_parents(),
        })
    }

    /// Whether a class in scope of `model`, or of a model it inherits or delegates to, sits in a file
    /// that does not parse, so the model may hold any field or method.
    pub fn unread_class(&self, model: &str, scope: &Scope) -> bool {
        self.chain(model, Some(scope), true).iter().any(|(_, c)| c.unread)
    }

    /// Whether a class in scope of `model`, or of a model it inherits or delegates to, may get fields
    /// from a parent the scan cannot read. `unread_delegate` covers a delegate the scan cannot read:
    /// Odoo adds each field of an `_inherits` parent that the model lacks (18.0
    /// `odoo/models.py:3559-3587`, 20.0 `odoo/orm/model_classes.py:512-540`), so a gate on whether
    /// `model` has a field asks both.
    pub fn hidden_field_parents(&self, model: &str, scope: &Scope) -> bool {
        self.unread_bases(model, scope, true, |c| &c.opaque_bases)
    }

    /// Whether a class in scope of `model`, or of a model it inherits, may get methods from a parent
    /// the scan cannot read. A model reached through `_inherits` gives fields, not methods.
    pub fn hidden_method_parents(&self, model: &str, scope: &Scope) -> bool {
        self.unread_bases(model, scope, false, |c| &c.opaque_bases)
    }

    /// The signature of each definition of `method`, among `merged` (`merged_classes` of the model of
    /// `class`, of module `mi`), that `class` overrides: those of other modules, short of one that
    /// loads after `mi`, and of classes above `class` in its file, whose name mangles `method` alike.
    /// The order of the files of `mi` is that of the imports, which is not indexed, so its other files
    /// count neither way. None when one of them has no signature to compare, as a class whose plain
    /// base binds `method`, as `class` writes it, has unless it defines `method` itself.
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
                !self.loads_after(m, mi)
            }
        };
        let defines = |c: &ClassFact| {
            mangles_alike(method, &class.class_name, &c.class_name) && c.methods.iter().any(|n| n == method)
        };
        merged
            .iter()
            .filter(|&&(m, c)| overridden(m, c))
            .filter_map(|&(m, c)| match defines(c) {
                true => Some(c.signature(method).map(|s| (m, s))),
                false => c.base_binds(method, &class.class_name).then_some(None),
            })
            .collect()
    }

    /// Whether a class of the chain of `model` has a non-literal `_inherit` element or one of the
    /// `bases` the scan does not read. A Python base that is another class of the chain counts as
    /// read on every class, where `hidden_parents` reads it so only on a class of `base`, whether the
    /// file defines it above the class or imports it: mixins subclass each other along their
    /// `_inherit`, and Odoo merges that class's fields either way. A class of `base` counts too: Odoo appends `base` to the parents of every other model
    /// (18.0 `odoo/models.py:732-736`, 20.0 `odoo/orm/model_classes.py:179-183`), so its fields and
    /// methods reach the model whether or not it is a Python base.
    fn unread_bases(
        &self,
        model: &str,
        scope: &Scope,
        delegated: bool,
        bases: impl Fn(&ClassFact) -> &[BaseSource],
    ) -> bool {
        let chain = self.chain(model, Some(scope), delegated);
        chain
            .iter()
            .any(|&(mi, c)| self.unread_parents(mi, c, &chain, bases(c)))
    }

    /// Whether `class`, of module `mi`, has a non-literal `_inherit` element or one of `bases` that
    /// the scan does not read as a class of `chain`.
    pub fn unread_parents(
        &self,
        mi: usize,
        class: &ClassFact,
        chain: &[(usize, &'m ClassFact)],
        bases: &[BaseSource],
    ) -> bool {
        let read = |base: &BaseSource| {
            self.base_class(mi, class, base)
                .is_some_and(|parent| chain.iter().any(|(_, other)| std::ptr::eq(*other, parent)))
        };
        class.inherit_unknown || bases.iter().any(|base| !read(base))
    }

    /// The model class a plain base of `class`, of module `mi`, names: see `file_base_class`.
    fn base_class(&self, mi: usize, class: &ClassFact, base: &BaseSource) -> Option<&'m ClassFact> {
        let (file, _) = self.file_of(mi, class)?;
        self.file_base_class(mi, file, base)
    }

    /// The model class a plain base read in `file`, of module `mi`, names, when the scan reads it: a
    /// model class of the file, or the one an import names in an indexed file. Odoo imports an addon
    /// as `odoo.addons.<name>` (18.0 `odoo/modules/module.py:379-383`, 20.0 `:536-540`), which
    /// `absolute_import` spells a relative import as.
    pub fn file_base_class(
        &self,
        mi: usize,
        file: &'m SourceFile<PyFacts>,
        base: &BaseSource,
    ) -> Option<&'m ClassFact> {
        match base {
            BaseSource::Local(pos) => file.facts.classes.iter().find(|c| c.pos == *pos),
            BaseSource::Imported { level, path } => {
                let path = absolute_import(&self.modules[mi].name, &file.rel, *level, path)?;
                self.imported_class(&path)
            }
            BaseSource::Unread => None,
        }
    }

    /// The class an import of the dotted `path` gets: the model class its file binds the name to
    /// last.
    fn imported_class(&self, path: &str) -> Option<&'m ClassFact> {
        let (file, class) = self.path_file(path)?;
        let classes = file.facts.classes.iter();
        classes.rev().find(|c| c.importable && c.class_name == class)
    }

    /// The classes above `class` in its file of module `mi`.
    fn classes_above(&self, mi: usize, class: &ClassFact) -> &'m [ClassFact] {
        self.file_of(mi, class)
            .map(|(file, at)| &file.facts.classes[..at])
            .unwrap_or_default()
    }

    /// The file of module `mi` that holds `class`, and where `class` sits among its classes.
    fn file_of(&self, mi: usize, class: &ClassFact) -> Option<(&'m SourceFile<PyFacts>, usize)> {
        let at = |f: &'m SourceFile<PyFacts>| f.facts.classes.iter().position(|c| std::ptr::eq(c, class));
        self.modules[mi].python.iter().find_map(|f| at(f).map(|at| (f, at)))
    }

    /// Whether classes `a` and `b` of module `mi` sit in one file.
    pub fn in_one_file(&self, mi: usize, a: &ClassFact, b: &ClassFact) -> bool {
        let file = |c: &ClassFact| self.file_of(mi, c).map(|(f, _)| f);
        file(a).zip(file(b)).is_some_and(|(fa, fb)| std::ptr::eq(fa, fb))
    }

    /// Whether the registry certainly merges what module `a.0`'s class `a.1` gives `model` at `a.2`
    /// before what `b` gives. The classes of `model` come after those of the models they inherit,
    /// unless one of them lists another model before its own in `_inherit`, which puts that model
    /// after the classes loaded before it. Among them, `base`'s and a dependency's come first, then
    /// in each module the class that introduces the model, then the others of one file in order;
    /// files load in the order of the imports, which is not indexed.
    pub fn merges_before(
        &self,
        model: &str,
        a: (usize, &ClassFact, Pos),
        b: (usize, &ClassFact, Pos),
        scope: &Scope,
    ) -> bool {
        match (a.1.model == model, b.1.model == model) {
            (_, false) => false,
            (false, true) => !self.inherits_ahead(model, scope),
            _ if a.0 != b.0 => self.loads_after(b.0, a.0) && !self.loads_after(a.0, b.0),
            _ if a.1.new_model != b.1.new_model => a.1.new_model,
            _ => self.in_one_file(a.0, a.1, b.1) && a.2 < b.2,
        }
    }

    /// Whether a class of `model` in scope lists another model before `model` in `_inherit`.
    pub fn inherits_ahead(&self, model: &str, scope: &Scope) -> bool {
        let ahead = |c: &ClassFact| c.inherit.iter().position(|i| i.value == model).is_some_and(|p| p > 0);
        self.classes(model).iter().any(|(mi, c)| scope.sees(*mi) && ahead(c))
    }

    /// Whether a class merged into `model` has a parent the scan cannot see. A parent reached through
    /// `_inherits` does not count: it gives the model its fields, so its hidden methods do not reach
    /// the model.
    pub fn merged_hidden_parents(&self, model: &str, scope: &Scope) -> bool {
        let merged = self.merged_classes(model, scope);
        self.hides_parents_in(&merged, &merged)
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
    /// are not all known. A parent no class introduces counts, but not a class's own model: else
    /// every extension of a model introduced outside the scan would read its fields as unknown.
    /// `hidden_parents` asks of methods too, which a delegate does not pass on.
    pub fn unread_delegate(&self, model: &str, scope: &Scope) -> bool {
        self.chain(model, Some(scope), true)
            .iter()
            .any(|(_, c)| self.delegates_unread(c))
    }

    /// Whether class `c` delegates to a parent the scan cannot read, or is such a parent: see
    /// `unread_delegate`.
    fn delegates_unread(&self, c: &ClassFact) -> bool {
        let delegates = c.inherits.iter().map(|(parent, _)| parent);
        let mut parents = delegates.chain(c.inherit.iter().map(|parent| &parent.value));
        let unread = |parent: &String| *parent != c.model && !self.has_introducer(parent);
        c.inherits_unknown || c.delegate_unknown || parents.any(unread)
    }

    /// Whether `model`, defined only by modules outside `scope`, may get fields from a parent the
    /// scan cannot read on any class of it or its parents in any indexed module, or of `base` in a
    /// module the closure of the model's introducer sees, which installs with the model. Odoo builds
    /// the model from the classes of every installed module (18.0 `odoo/modules/registry.py:290-292`,
    /// `odoo/modules/loading.py:195`; 20.0 `odoo/orm/registry.py:449-451`,
    /// `odoo/modules/loading.py:181`) and takes fields from every class of its MRO (18.0
    /// `odoo/models.py:3640`, `:3653-3657`; 20.0 `odoo/orm/model_classes.py:382`, `:395-399`).
    pub fn hidden_outside(&self, model: &str, scope: &Scope) -> bool {
        let Status::Hidden(owner) = self.model_status(model, scope, None) else {
            return false;
        };
        let installed = self.scope(self.by_name[owner]);
        let mut chain = self.chain(model, Some(&self.every_module()), true);
        // Outside the owner's closure a class of `base` may not install with the model, so it
        // neither hides fields nor reads as the parent of another class.
        chain.retain(|&(mi, c)| c.model != "base" || installed.sees(mi));
        chain
            .iter()
            .any(|&(mi, c)| self.unread_parents(mi, c, &chain, &c.opaque_bases) || self.delegates_unread(c))
    }

    /// What `holds_fields` asks, and no plain base the scan does not read, which may hold methods
    /// whatever its fields.
    pub fn holds_model(&self, module: usize, model: &str, scope: &Scope) -> bool {
        self.holds_fields(module, model, scope) && !self.unread_bases(model, scope, true, |c| &c.plain_sources)
    }

    /// Whether `module` introduces `model` and holds all its fields: every class in scope of `model`
    /// and of the models it inherits or delegates to, and every parent they name. `base` is in every
    /// chain, so as a parent it proves nothing.
    pub fn holds_fields(&self, module: usize, model: &str, scope: &Scope) -> bool {
        let introduces = |m: &str| self.introduces_model(module, m);
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

    /// Whether `module` has a class that introduces `model` with `_name`, or `model` is `base`, which
    /// every module holds.
    pub fn introduces_model(&self, module: usize, model: &str) -> bool {
        model == "base" || self.classes(model).iter().any(|(mi, c)| *mi == module && c.new_model)
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

    /// Whether a field of a module in `scope` that may be a stored Many2many keeps its rows in
    /// `table`, which it names with `relation`, or which Odoo names after the tables of its model and
    /// comodel. Odoo gives a field it does not store no table (18.0 `odoo/fields.py:4989-5009`, 20.0
    /// `odoo/orm/fields_relational.py:1318-1340`).
    pub fn relation_table(&self, table: &str, scope: &Scope) -> bool {
        let named = |model: &str, field: &FieldFact| match field.rows.as_ref().and_then(|r| r.table.as_ref()) {
            Some(Rows::Relation(relation)) => relation == table,
            Some(Rows::Comodel(comodel)) => {
                let mut tables = [self.table(model, scope), self.table(comodel, scope)];
                tables.sort();
                matches!(tables, [Some(a), Some(b)] if format!("{a}_{b}_rel") == table)
            }
            None => false,
        };
        self.scoped_fields(scope)
            .any(|(model, field)| field.may_be_stored() && named(model, field))
    }

    /// Whether a field of a module in `scope` surely stored keeps its rows in a table the scan cannot
    /// name, which may then be any.
    pub fn stored_unread_rows(&self, scope: &Scope) -> bool {
        self.scoped_fields(scope)
            .any(|(_, f)| f.rows.as_ref().is_some_and(|r| r.unread) && f.proven_stored())
    }

    /// The fields of the classes of modules in `scope`, each with the model of its class.
    fn scoped_fields<'a>(&'a self, scope: &'a Scope) -> impl Iterator<Item = (&'m str, &'m FieldFact)> + 'a {
        let classes = self.models.values().flatten().filter(|(mi, _)| scope.sees(*mi));
        classes.flat_map(|(_, class)| class.fields.iter().map(|f| (class.model.as_str(), f)))
    }

    /// Whether a class of `model` or of a model it inherits, or an `ir.model.fields` record, in any
    /// indexed module, defines `field`; with `delegated`, a record on a model it reaches through
    /// `_inherits` counts too, though a class is never taken through `_inherits`. A record in
    /// `scope` counts only if it has loaded by then.
    pub fn defines_field(&self, model: &str, field: &str, scope: &Scope, delegated: bool) -> bool {
        let counts = |f: &DataField| !scope.sees(f.module) || scope.holds(f);
        self.chain(model, None, false)
            .iter()
            .any(|(_, c)| c.fields.iter().any(|f| f.name == field))
            || self.data_field(model, field, None, delegated, counts).is_some()
    }

    /// The `ir.model.fields` record that adds `field` to `model` or to a model of its chain, which
    /// takes in the `_inherits` parents only with `delegated`. Odoo adds such a field before it
    /// copies delegated ones, so `_inherits` children get it as a related field, though their table
    /// holds no column for it. `_inherit` children do not get it, yet the chain joins them to the
    /// model and staying silent there is the safe side. Only the records `counts` keeps qualify.
    fn data_field(
        &self,
        model: &str,
        field: &str,
        scope: Option<&Scope>,
        delegated: bool,
        counts: impl Fn(&DataField) -> bool,
    ) -> Option<&DataField<'m>> {
        let candidates = self.data_fields.get(field)?;
        let chain = self.chain(model, scope, delegated);
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
    /// to, and a delegated model's definitions or own record shadow those of its own parents, so a
    /// later parent's record beats an earlier parent's class; else the record that adds it.
    /// `Err(Some(module))` when the field exists only outside the scope.
    pub fn field(&self, model: &str, field: &str, scope: &Scope) -> Result<Option<FieldInfo<'m>>, Option<&'m str>> {
        self.field_from(model, field, scope, &mut vec![])
    }

    /// `field`, where `open` holds the related definitions being followed.
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
        let (level, defs) = self.nearest_defs(model, field, scope);
        if defs.is_empty() {
            return self.level_record_field(model, level, field, scope);
        }
        let merge = self.merge_order(level, defs, scope);
        // Odoo builds the field with the class of the definition it merges last (18.0
        // `odoo/models.py:3653-3657`, `:3687`; 20.0 `odoo/orm/model_classes.py:395-399`, `:441`, `:446`).
        let last = merge.last(|_| true);
        let Some(ftype) = agreed(last.iter().map(|&i| merge.defs[i].2.ftype.as_str())) else {
            return Ok(Some(FieldInfo {
                ftype: UNREAD_TYPE,
                comodel: None,
            }));
        };
        let comodel = self.def_comodel(&merge, ftype, scope, open);
        Ok(Some(FieldInfo { ftype, comodel }))
    }

    /// `field` of `model` when no class in scope gives it at `level`, which is either `model` or
    /// the delegated model whose own record stopped `nearest_defs`.
    fn level_record_field(
        &self,
        model: &str,
        level: &str,
        field: &str,
        scope: &Scope,
    ) -> Result<Option<FieldInfo<'m>>, Option<&'m str>> {
        // Odoo adds the model's own manual fields before it copies any from a parent.
        let own = || self.own_record(model, field, scope);
        if level != model
            && let Some(data) = own().or_else(|| self.own_record(level, field, scope))
        {
            return Ok(Some(record_info(data)));
        }
        self.record_field(model, field, scope)
    }

    /// `field` of `model` when no class in scope defines it: the record that adds it, else the
    /// module outside the scope that defines it.
    fn record_field(&self, model: &str, field: &str, scope: &Scope) -> Result<Option<FieldInfo<'m>>, Option<&'m str>> {
        // Odoo skips a manual field whose name a class already gave the model.
        if let Some(data) = self.data_field(model, field, Some(scope), true, |f| scope.holds(f)) {
            return Ok(Some(record_info(data)));
        }
        let definer = self.outside_definer(model, field, scope, |_| true);
        Err(definer.map(|mi| self.modules[mi].name.as_str()))
    }

    /// The record in scope that adds `field` to `model` itself. Odoo adds a manual field only to
    /// the model the record names, so neither an inherited model's nor an `_inherits` parent's
    /// record counts, and neither does one whose model only a run tells.
    fn own_record(&self, model: &str, field: &str, scope: &Scope) -> Option<&DataField<'m>> {
        let candidates = self.data_fields.get(field)?;
        candidates.iter().find(|f| f.model == Some(model) && scope.holds(f))
    }

    /// The module that defines `field` of `model` outside `scope`: of the model's own definitions
    /// (its classes, its `_inherit` parents', `base`'s), the first `prefer` keeps, else of those of
    /// its `_inherits` parents, else the first own, else the first delegated, else that of a record
    /// that adds it. Odoo copies a delegated field only under a name the model lacks (18.0
    /// `odoo/models.py:3572-3573`, 20.0 `odoo/orm/model_classes.py:525-526`), so a parent only
    /// names the field when no own definition `prefer` keeps, as one may be absent while it loads.
    fn outside_definer(
        &self,
        model: &str,
        field: &str,
        scope: &Scope,
        prefer: impl Fn(usize) -> bool,
    ) -> Option<usize> {
        let own = defs_named(self.chain(model, None, false), field);
        // Every own definition is also in `all`, so `all` adds the delegated ones after them.
        let all = self.field_defs(model, field, None);
        let module = |(mi, _, _): &FieldDef| *mi;
        let python = own
            .iter()
            .chain(&all)
            .map(module)
            .find(|&mi| prefer(mi))
            .or(own.first().or(all.first()).map(module));
        // A record in scope that has not loaded yet is no matter of `depends`.
        let elsewhere = |f: &DataField| !scope.sees(f.module);
        python.or_else(|| self.data_field(model, field, None, true, elsewhere).map(|d| d.module))
    }

    /// The module to name for `field` of `model` when `field` finds it only outside `scope`, and
    /// whether it loads after module `reader` (`follows`). One that does not goes first, as adding it
    /// to `depends` may fix the read, which a dependent of `reader` never does. On a model only
    /// modules loading after `reader` introduce, no module `reader` may depend on gives the field,
    /// so one of those goes first rather than, say, one that adds the field to `base`.
    pub fn field_source(&self, model: &str, field: &str, scope: &Scope, reader: usize) -> Option<(&'m str, bool)> {
        let late = self.introduced_after(model, reader);
        let mi = self.outside_definer(model, field, scope, |mi| self.follows(mi, reader) == late)?;
        Some((self.modules[mi].name.as_str(), self.follows(mi, reader)))
    }

    /// Whether a class introduces `model` and every such class is of a module that loads after
    /// module `e`.
    fn introduced_after(&self, model: &str, e: usize) -> bool {
        let mut introducers = self.classes(model).iter().filter(|(_, c)| c.new_model).peekable();
        introducers.peek().is_some() && introducers.all(|(mi, _)| self.follows(*mi, e))
    }

    /// The comodel of a field built as `ftype` from the definitions of `merge` Odoo keeps: the one
    /// the last that may name one agree on, else what the related path of the last related ones
    /// ends on when they agree on it. A definition that unpacks arguments may name it unread.
    fn def_comodel(
        &self,
        merge: &Merge<'m>,
        ftype: &str,
        scope: &Scope,
        open: &mut Vec<&'m FieldFact>,
    ) -> Option<String> {
        let kept = merge.kept(ftype)?;
        let def = |i: usize| merge.defs[i].2;
        let named = merge.last(|i| kept[i] && (def(i).splat || def(i).comodel.is_some()));
        if !named.is_empty() {
            let comodels = named.iter().map(|&i| def(i).comodel.as_ref().map(|c| c.value.as_str()));
            return agreed(comodels).flatten().map(str::to_owned);
        }
        let related = merge.last(|i| kept[i] && def(i).related.is_some());
        let path = |i: usize| {
            (
                merge.defs[i].1.model.as_str(),
                def(i).related.as_ref().map(|r| r.value.as_str()),
            )
        };
        agreed(related.iter().map(|&i| path(i)))?;
        let (_, class, first) = merge.defs[*related.first()?];
        self.related_def_comodel(&class.model, first, scope, open)
    }

    /// The comodel the related path of `def`, a field of `model`, ends on. A definition met again
    /// has none: Odoo cannot resolve a related cycle either, and refuses it unless it is manual.
    fn related_def_comodel(
        &self,
        model: &str,
        def: &'m FieldFact,
        scope: &Scope,
        open: &mut Vec<&'m FieldFact>,
    ) -> Option<String> {
        let path = &def.related.as_ref()?.value;
        if open.iter().any(|f| std::ptr::eq(*f, def)) {
            return None;
        }
        open.push(def);
        let comodel = self.related_comodel(model, path, scope, open);
        // A path may repeat a field, so only the fields being followed stay open.
        open.pop();
        comodel
    }

    /// `defs`, which `level` merges, with the order of each pair, worked out once since a pair may
    /// need a walk of the dependency graph.
    fn merge_order(&self, level: &str, defs: Vec<FieldDef<'m>>, scope: &Scope) -> Merge<'m> {
        let before = |a: &FieldDef<'m>, b: &FieldDef<'m>| {
            // Two classes of one model another level inherits keep that model's order in its MRO.
            let model = if a.1.model == b.1.model {
                a.1.model.as_str()
            } else {
                level
            };
            self.merges_before(model, (a.0, a.1, a.2.pos), (b.0, b.1, b.2.pos), scope)
        };
        let n = defs.len();
        let row = |i: usize| (0..n).map(|j| i != j && before(&defs[i], &defs[j])).collect();
        let before = (0..n).map(row).collect();
        Merge { defs, before }
    }

    /// The in-scope definitions of `field` the registry gives `model`, with the model whose merge
    /// orders them: its own when it has any, which shadow every delegated level's, else those of
    /// `delegated_defs`.
    fn nearest_defs<'a>(&self, model: &'a str, field: &str, scope: &Scope) -> (&'a str, Vec<FieldDef<'m>>)
    where
        'm: 'a,
    {
        let mut defs = self.merged_fields(model, scope);
        defs.retain(|d| d.2.name == field);
        if !defs.is_empty() {
            return (model, defs);
        }
        self.delegated_defs(model, field, scope, &mut FxHashSet::default())
            .unwrap_or((model, defs))
    }

    /// The in-scope definitions of `field` at the nearest level `model` delegates to that has it,
    /// with that level. Odoo sets up each `_inherits` parent before the child and copies its resolved
    /// fields, so a parent's own field wins over its parents', and of two parents the later entry
    /// wins. A parent's record decides when no class of it gives the field, the parent then coming
    /// with no definitions. `seen` ends a delegation ring.
    fn delegated_defs(
        &self,
        model: &str,
        field: &str,
        scope: &Scope,
        seen: &mut FxHashSet<&'m str>,
    ) -> Option<(&'m str, Vec<FieldDef<'m>>)> {
        let classes = self.merged_classes(model, scope);
        let parents: Vec<&'m str> = classes
            .iter()
            .flat_map(|(_, c)| c.inherits.iter().map(|(parent, _)| parent.as_str()))
            .collect();
        for parent in parents.into_iter().rev() {
            if !seen.insert(parent) {
                continue;
            }
            let mut defs = self.merged_fields(parent, scope);
            defs.retain(|d| d.2.name == field);
            // Odoo adds a parent's manual fields before those it copies from the parent's parents.
            if !defs.is_empty() || self.own_record(parent, field, scope).is_some() {
                return Some((parent, defs));
            }
            if let Some(found) = self.delegated_defs(parent, field, scope, seen) {
                return Some(found);
            }
        }
        None
    }

    /// The in-scope definitions of the field a dotted path ends on, or None when the path does not
    /// resolve or ends on a field every model has. As in `field`, the model's own definitions shadow
    /// a delegated level's, since Odoo reads the target from the resolved fields of the model.
    pub fn path_target(&self, model: &str, path: &str, scope: &Scope) -> Option<Vec<&'m FieldFact>> {
        let (head, last) = match path.rsplit_once('.') {
            Some((head, last)) => (self.path_comodel(model, head, scope)?, last),
            None => (model.to_owned(), path),
        };
        let defs: Vec<_> = self
            .nearest_defs(&head, last, scope)
            .1
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

    /// Whether `super().method()` in class `except` of `model` finds a definition. It looks in
    /// `BaseModel`, `object`, `CachedModel` when a class reads it, the classes of `model` and of the
    /// models it inherits by `_inherit`, other than `except` itself, and the plain bases all of those
    /// classes read, `except`'s too. A model it delegates to is not searched: Odoo builds the class
    /// bases from `_inherit` and `base` alone. A module outside the scope is searched, as an override
    /// of its method is called only by that module. A property or plain value of `BaseModel` or
    /// `CachedModel` does not count, as calling it raises `TypeError`, and a `__name` counts only
    /// from a class or base whose name mangles it alike.
    pub fn has_parent_method(&self, model: &str, method: &str, except: &ClassFact, series: Option<Series>) -> bool {
        let parent = |c: &ClassFact| c.base_binds(method, &except.class_name) || defines_for(c, method, except);
        base_parent_method(method, series)
            || self.chain_classes(model).iter().any(|(_, c)| parent(c))
            || (cached_model_method(method, series) && self.cached_model(model))
    }

    /// The class, other than `except`, of `model` or of a model it inherits that defines the `__name`
    /// `method`, which Python mangles with that class's name, so `except`'s `super()` call misses it.
    /// Of several, the first in `chain_classes` order. A class named all underscores binds the name
    /// unmangled, so it owns nothing private.
    pub fn private_owner(&self, model: &str, method: &str, except: &ClassFact) -> Option<&'m str> {
        if !mangled(method) {
            return None;
        }
        let owns = |c: &ClassFact| {
            !std::ptr::eq(c, except)
                && private_name(method, &c.class_name) != method
                && c.methods.iter().any(|m| m == method)
        };
        let owner = self.chain_classes(model).into_iter().find(|(_, c)| owns(c));
        owner.map(|(_, c)| c.class_name.as_str())
    }

    /// Whether a class of `model` or of a model it inherits, in any indexed module, is on
    /// `CachedModel`, which then sits in the MRO of the model's registry class. A `super()` call from
    /// a class past it in that MRO misses its members; the callers do not read the order, so they
    /// let such a call pass.
    pub fn cached_model(&self, model: &str) -> bool {
        self.chain_classes(model).iter().any(|(_, c)| c.cached)
    }

    /// Whether Odoo refuses a button on `name` because `CachedModel` makes it private in `series`
    /// and a class in `scope` puts `model` on it. An unknown series counts none: saas~19.1 to
    /// saas~19.3 have `CachedModel` without a private `get_all`, so a module's own one works there.
    pub fn cached_private_method(&self, model: &str, name: &str, scope: &Scope, series: Option<Series>) -> bool {
        series.is_some_and(|s| dated_member(CACHED_PRIVATE_METHODS, name, Some(s)))
            && self.merged_classes(model, scope).iter().any(|(_, c)| c.cached)
    }

    /// Whether Odoo refuses a button or an RPC call on `name`: a leading `_` or a private method of
    /// `BaseModel` whatever the model, and with a known `model` a private method of `CachedModel` or
    /// an `@api.private` one on any class in scope, since the refusal walks the whole MRO.
    pub fn refused_call(&self, model: Option<&str>, name: &str, scope: &Scope, series: Option<Series>) -> bool {
        name.starts_with('_')
            || base_private_method(name, series)
            || model.is_some_and(|model| {
                self.cached_private_method(model, name, scope, series) || self.private_method(model, name, scope)
            })
    }

    /// Whether Odoo refuses a button or an RPC call on `name` because the model's class binds it to
    /// no records: a class method, a static method or a `partial`. Every class in scope of a known
    /// `model` that binds it, or whose plain base does, must, since the index does not order the
    /// MRO, and with none or no model `BaseModel` decides. A base the scan cannot read, which a
    /// class in a file that does not parse counts as having, may bind it to the records. An
    /// unknown series counts none, as 18.0 calls such a method.
    pub fn unbound_method(&self, model: Option<&str>, name: &str, scope: &Scope, series: Option<Series>) -> bool {
        if !series.is_some_and(|s| series::matches(UNBOUND_REFUSED, s)) {
            return false;
        }
        let Some(model) = model else {
            return BASE_UNBOUND_METHODS.contains(&name);
        };
        if self.hidden_method_parents(model, scope) {
            return false;
        }
        let classes = self.merged_classes(model, scope);
        let bindings: Vec<bool> = classes.iter().filter_map(|(_, c)| c.unbound(name)).collect();
        if bindings.is_empty() {
            BASE_UNBOUND_METHODS.contains(&name)
        } else {
            bindings.iter().all(|&unbound| unbound)
        }
    }

    /// Whether a parent class defines `name` as `has_parent_method` reads it, or `BaseModel` binds
    /// it to a value, which a `def` of the same name replaces where Odoo reads it.
    pub fn has_parent_member(&self, model: &str, name: &str, except: &ClassFact, series: Option<Series>) -> bool {
        BASE_MODEL_VALUES.contains(&name) || self.has_parent_method(model, name, except, series)
    }

    /// Whether a file of any module names the hook method `name`, whole or by a prefix it is built
    /// from. A module outside the scope counts: what it names may run once it is installed.
    pub fn names_hook(&self, name: &str) -> bool {
        self.hook_names.contains(name) || self.hook_prefixes.iter().any(|p| name.starts_with(p))
    }

    /// Whether a module `scope` sees has a file the scan cannot read at all, which may name any hook
    /// method of a module of `scope`.
    pub fn unread_in(&self, scope: &Scope) -> bool {
        let mut modules = self.modules.iter().enumerate();
        modules.any(|(mi, m)| scope.sees(mi) && m.unread_text)
    }

    /// Whether a module with a class of `model` is in the scope of a module with a file the scan
    /// cannot read at all, which may name any hook method of `model` once both are installed.
    pub fn unread_beside(&self, model: &str) -> bool {
        self.classes(model).iter().any(|&(mi, _)| self.unread_reach[mi])
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
        self.unscoped_status(model, |c| c.methods.iter().any(|m| m == method))
    }

    /// The status of what a class of `model` or of a model it inherits matching `holds` gives,
    /// once the scoped walk missed it: `Hidden` by the first such module, unless one is always
    /// installed.
    fn unscoped_status(&self, model: &str, holds: impl Fn(&ClassFact) -> bool) -> Status<'m> {
        let defining: Vec<usize> = self
            .chain_classes(model)
            .iter()
            .filter(|(_, c)| holds(c))
            .map(|(mi, _)| *mi)
            .collect();
        match defining.first() {
            None => Status::Missing,
            Some(_) if defining.iter().any(|&mi| self.always_installed(mi)) => Status::Visible,
            Some(&mi) => Status::Hidden(&self.modules[mi].name),
        }
    }

    /// Whether a button or a `<function>` finds `method` on `model`: any method of `BaseModel`, what
    /// `rpc_method` finds, or a member of `CachedModel` on a model on it, `Hidden` as a method is
    /// when only a module outside `scope` puts the model there. Python looks both up on the model's
    /// class.
    pub fn data_method(&self, model: &str, method: &str, scope: &Scope, series: Option<Series>) -> Status<'m> {
        if BASE_MODEL_EXTRA.contains(&method) || old_base_method(method, series) {
            return Status::Visible;
        }
        let found = self.rpc_method(model, method, scope, series);
        let cached = cached_model_method(method, series) || cached_model_value(method, series);
        if matches!(found, Status::Visible) || !cached {
            return found;
        }
        if self.merged_classes(model, scope).iter().any(|(_, c)| c.cached) {
            return Status::Visible;
        }
        match self.unscoped_status(model, |c| c.cached) {
            Status::Missing => found,
            status => status,
        }
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
    /// no class of the index introduces, whose methods it does not know. A model the index only
    /// extends counts: its introducer is outside the scan.
    pub fn unindexed_parent(&self, model: &str, scope: &Scope) -> bool {
        self.merged_classes(model, scope)
            .iter()
            .flat_map(|(_, c)| &c.inherit)
            .any(|parent| !self.has_introducer(&parent.value))
    }

    /// As `unindexed_parent`, counting only the classes of `module`. A class of another module
    /// listing such a parent leaves a typo in `module` reported.
    pub fn lists_unindexed_parent(&self, module: usize, model: &str, scope: &Scope) -> bool {
        self.merged_classes(model, scope)
            .iter()
            .filter(|(mi, _)| *mi == module)
            .flat_map(|(_, c)| &c.inherit)
            .any(|parent| !self.has_introducer(&parent.value))
    }

    /// Whether a class of any module introduces `model` with `_name`. Odoo refuses to extend or
    /// inherit a model absent from the registry (18.0 `odoo/models.py:739-741`, `:759-761`; 20.0
    /// `odoo/orm/model_classes.py:186-188`, `:208-210`), so one only extended here is introduced
    /// outside the scan, and copies an `_inherits` parent's fields from that registry model (18.0
    /// `odoo/models.py:3565-3569`; 20.0 `odoo/orm/model_classes.py:518-522`).
    pub fn has_introducer(&self, model: &str) -> bool {
        self.classes(model).iter().any(|(_, c)| c.new_model)
    }

    /// Follows a dotted field path from `model`. Paths through models no class introduces, or
    /// through a field such a model supplies as a parent, are accepted.
    pub fn resolve_path(&self, model: &str, path: &str, scope: &Scope) -> Result<(), PathError<'m>> {
        let mut model = model.to_owned();
        let segments: Vec<&str> = path.split('.').collect();
        for (i, segment) in segments.iter().enumerate() {
            if !self.has_introducer(&model) {
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

    /// `segment` of `model`, or `Ok(None)` for unknown when a field only outside the scope comes
    /// from a parent no class introduces: its introducer outside the scan may give the field too.
    /// See `unintroduced_parent_defines`.
    fn step(&self, model: &str, segment: &str, scope: &Scope) -> Result<Option<FieldInfo<'m>>, PathError<'m>> {
        match self.field(model, segment, scope) {
            Err(Some(_)) if self.unintroduced_parent_defines(model, segment, scope) => Ok(None),
            Err(None) => Err(PathError::Missing {
                segment: segment.to_owned(),
                model: model.to_owned(),
            }),
            Err(Some(module)) => Err(PathError::Hidden {
                segment: segment.to_owned(),
                model: model.to_owned(),
                module,
            }),
            Ok(info) => Ok(info),
        }
    }

    /// Whether a class of a parent no class introduces defines `field`, where a class in scope of
    /// `model`, or of a model it inherits or delegates to, names the parent. Odoo builds a model on
    /// the registry class of each `_inherit` parent (18.0 `odoo/models.py:759-768`; 20.0
    /// `odoo/orm/model_classes.py:208-217`) and copies an `_inherits` parent's fields from the
    /// registry model: see `has_introducer`. A parent only a module outside the scope names is not
    /// there when the reader runs. `base` may have no indexed introducer, so a field a module
    /// outside `depends` adds through `_inherit = 'base'` stays reported as that module's.
    pub fn unintroduced_parent_defines(&self, model: &str, field: &str, scope: &Scope) -> bool {
        self.chain(model, Some(scope), true)
            .iter()
            .flat_map(|(_, c)| {
                let delegates = c.inherits.iter().map(|(parent, _)| parent.as_str());
                c.inherit.iter().map(|i| i.value.as_str()).chain(delegates)
            })
            .filter(|parent| *parent != "base" && !self.has_introducer(parent))
            .flat_map(|parent| self.classes(parent))
            .any(|(_, c)| c.fields.iter().any(|f| f.name == field))
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
        let (_, top) = self.view_top(id)?;
        if top.iter().any(|n| n.fact.unread_parent) {
            return None;
        }
        top.iter().find_map(|n| n.fact.kind.as_deref())
    }

    /// The id and nodes of the view at the root of `id`'s inheritance. None when the top is a view
    /// only other modules' untyped records write, none of them a parent: Odoo sets the type only at
    /// create, from the owner's record, which gives the parent too unless a later record writes it.
    fn view_top(&self, mut id: String) -> Option<(String, &[ViewNode<'m>])> {
        for _ in 0..32 {
            let nodes = self.views.nodes.get(&id)?;
            match nodes.iter().find_map(|n| n.inherit.clone()) {
                Some(parent) => id = parent,
                None if owner_node(&id, nodes, self.modules).is_none()
                    && nodes.iter().all(|n| n.fact.kind.is_none()) =>
                {
                    return None;
                }
                None => return Some((id, nodes)),
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

    /// Whether a node inherits `id` and defines another id, as a primary child does.
    pub fn has_children(&self, id: &str) -> bool {
        self.children.contains_key(id)
    }

    /// The modules whose nodes define `id`.
    pub fn definers(&self, id: &str) -> impl Iterator<Item = usize> {
        let nodes = self.nodes.get(id).map(Vec::as_slice).unwrap_or_default();
        nodes.iter().filter(|n| n.defines).map(|n| n.module)
    }

    /// The root of the inheritance tree above `id`, or None when an id on the way is not defined,
    /// its arch was not read, as when written to `arch_base`, by `search`, `ref`, `eval`, a file,
    /// text or a cell of an `ir.ui.view` CSV, or it names its parent by a `search`, an `eval` other
    /// than `False`, or by name or database id in an `ir.ui.view` CSV, which Odoo resolves at load,
    /// so the tree's elements would be incomplete.
    pub fn root(&self, mut id: String) -> Option<String> {
        for _ in 0..32 {
            let nodes = self.nodes.get(&id)?;
            if !nodes.iter().any(|n| n.defines && n.fact.has_arch) || nodes.iter().any(|n| n.fact.unread_parent) {
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
        for node in self.under(&root) {
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
        tree.root = root;
        tree
    }

    /// Whether a node of the tree under `root` holds, given its module and its view.
    pub fn any(&self, root: &str, holds: impl Fn(usize, &ViewFact) -> bool) -> bool {
        self.under(root).into_iter().any(|n| holds(n.module, n.fact))
    }

    /// The nodes of the tree under `root`, each once.
    fn under(&self, root: &str) -> Vec<&ViewNode<'m>> {
        let mut nodes = vec![];
        let mut seen = FxHashSet::default();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            nodes.extend(self.nodes.get(id).into_iter().flatten());
            stack.extend(self.children.get(id).into_iter().flatten().map(String::as_str));
        }
        nodes
    }

    /// The models of every node on the way up from `id`, None for a node that names no model. None
    /// when the index does not hold an id on the way from the module the id names, that module
    /// loads a data file that may hold a view and does not parse, or a node names its parent or its
    /// model in a form the scan does not read, such as a `search`, or a parent by name or database
    /// id in an `ir.ui.view` CSV, so a model on the way is unknown.
    pub fn models(&self, mut id: String, modules: &[Module]) -> Option<Vec<Option<&'m str>>> {
        let mut models = vec![];
        for _ in 0..32 {
            let nodes = self.nodes.get(&id)?;
            // Odoo writes only the fields a record gives, so one naming no model or parent may update
            // the view in place: from another module, or from the owner when its defining record sits
            // in a file the scan lost. Only the owner's node, while it loses no view file, tells.
            let owned = owner_node(&id, nodes, modules);
            let unread = nodes.iter().any(|n| n.fact.unread_parent || n.fact.model_unread);
            if unread || owned.is_none_or(|n| modules[n.module].lost_views()) {
                return None;
            }
            models.extend(nodes.iter().map(|n| n.fact.model.as_ref().map(|m| m.value.as_str())));
            match nodes.iter().find_map(|n| n.inherit.clone()) {
                Some(parent) => id = parent,
                None => return Some(models),
            }
        }
        None
    }

    /// The models every record of the view `id` names, None for one naming none. None when the
    /// index does not hold the record of the module the id names, that module loads a data file
    /// that may hold a view and does not parse, or a record names its model in a form the scan does
    /// not read, so the stored model is unknown.
    pub fn stored(&self, id: &str, modules: &[Module]) -> Option<Vec<Option<&'m str>>> {
        let nodes = self.nodes.get(id)?;
        let owned = owner_node(id, nodes, modules);
        if owned.is_none_or(|n| modules[n.module].lost_views()) || nodes.iter().any(|n| n.fact.model_unread) {
            return None;
        }
        let models = nodes.iter().map(|n| n.fact.model.as_ref().map(|m| m.value.as_str()));
        Some(models.collect())
    }

    /// The top of `id`'s inheritance as far as the index holds it: the first id on the way whose
    /// nodes name no parent, or that no indexed view defines, whose indexed children are then all
    /// the tree the scan knows.
    pub fn top(&self, mut id: String) -> String {
        for _ in 0..32 {
            match self
                .nodes
                .get(&id)
                .and_then(|nodes| nodes.iter().find_map(|n| n.inherit.clone()))
            {
                Some(parent) => id = parent,
                None => break,
            }
        }
        id
    }
}

/// The node of `id` from the module its id names.
fn owner_node<'a, 'm>(id: &str, nodes: &'a [ViewNode<'m>], modules: &[Module]) -> Option<&'a ViewNode<'m>> {
    let owner = id.split_once('.').map(|(owner, _)| owner);
    nodes.iter().find(|n| Some(modules[n.module].name.as_str()) == owner)
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

/// A field, method or `selection_add` value a module defines on a model.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Member<'m> {
    module: usize,
    model: &'m str,
    method: bool,
}

/// The definitions of a field one level of the registry merges.
struct Merge<'m> {
    defs: Vec<FieldDef<'m>>,
    /// `before[i][j]` when the level certainly merges `defs[i]` before `defs[j]`.
    before: Vec<Vec<bool>>,
}

impl<'m> Merge<'m> {
    /// The definitions `pick` keeps after which no other one it keeps certainly merges.
    fn last(&self, pick: impl Fn(usize) -> bool) -> Vec<usize> {
        let picked: Vec<usize> = (0..self.defs.len()).filter(|&i| pick(i)).collect();
        let open = |i: &usize| !picked.iter().any(|&j| self.before[*i][j]);
        picked.iter().copied().filter(open).collect()
    }

    /// Which definitions a field built as `ftype` keeps of those of `ftype` that may set its
    /// comodel. A definition of another class drops what came before it (18.0
    /// `odoo/fields.py:415-421`, 20.0 `odoo/orm/fields.py:446-455`). None when one of another type
    /// is not certainly before or after such a definition, or one of an unread type, which may be
    /// of either class, is not certainly before it.
    fn kept(&self, ftype: &str) -> Option<Vec<bool>> {
        let other: Vec<usize> = (0..self.defs.len())
            .filter(|&j| self.defs[j].2.ftype != ftype)
            .collect();
        let sets = |d: &FieldFact| d.ftype == ftype && (d.splat || d.comodel.is_some() || d.related.is_some());
        (0..self.defs.len())
            .map(|i| {
                if sets(self.defs[i].2) {
                    self.survives(i, &other)
                } else {
                    Some(false)
                }
            })
            .collect()
    }

    /// Whether every definition of `other` certainly merges before `defs[i]`, false when one of a
    /// read type certainly merges after it, None when that is unknown.
    fn survives(&self, i: usize, other: &[usize]) -> Option<bool> {
        let mut survives = true;
        for &j in other {
            if self.before[j][i] {
                continue;
            }
            if !self.before[i][j] || self.defs[j].2.ftype == UNREAD_TYPE {
                return None;
            }
            survives = false;
        }
        Some(survives)
    }
}

/// The field `data` adds, as `Index::field` reads it.
fn record_info<'m>(data: &DataField<'m>) -> FieldInfo<'m> {
    FieldInfo {
        ftype: data.ftype,
        comodel: data.comodel.map(str::to_owned),
    }
}

/// The value every item of `values` has, or None when they differ or there are none.
fn agreed<T: PartialEq>(mut values: impl Iterator<Item = T>) -> Option<T> {
    let first = values.next()?;
    values.all(|v| v == first).then_some(first)
}

/// What `Index::members` holds: the name of each field, method and `selection_add` value of each
/// class, to the modules and models that define it.
fn members<'m>(models: &Models<'m>) -> FxHashMap<&'m str, Vec<Member<'m>>> {
    let mut members: FxHashMap<&str, Vec<Member>> = FxHashMap::default();
    for (&model, classes) in models {
        for &(module, class) in classes {
            let added = class.fields.iter().filter_map(|f| f.option("selection_add"));
            let values = added.filter_map(|o| o.value.listed(true)).flatten();
            let fields = class.fields.iter().map(|f| &f.name).chain(values);
            let named = fields
                .map(|n| (n, false))
                .chain(class.methods.iter().map(|n| (n, true)));
            for (name, method) in named {
                let member = Member { module, model, method };
                let definers = members.entry(name.as_str()).or_default();
                if !definers.contains(&member) {
                    definers.push(member);
                }
            }
        }
    }
    members
}

/// A URL path a controller routes, up to its first converter.
struct RoutePath<'m> {
    url: &'m str,
    head: &'m str,
    converted: bool,
}

impl<'m> RoutePath<'m> {
    /// None for a path that is all converter, or the root, which no word names.
    fn of(url: &'m str) -> Option<Self> {
        let (head, converted) = url.find('<').map_or((url, false), |i| (&url[..i], true));
        let head = head.trim_end_matches('/');
        (!head.is_empty()).then_some(Self { url, head, converted })
    }

    /// Whether `word` is this path, or one it takes with its converters filled in.
    fn matches(&self, word: &str) -> bool {
        word == self.head || self.converted && word.strip_prefix(self.head).is_some_and(|r| r.starts_with('/'))
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

/// Whether `method` gets the same name in classes `a` and `b`. Python renames a `__name` method
/// to `_<class>__name`, with the class's leading underscores stripped, and leaves it as is in a class
/// whose name is all underscores, so an extension of the same class name overrides it.
fn mangles_alike(method: &str, a: &str, b: &str) -> bool {
    !mangled(method) || a.trim_start_matches('_') == b.trim_start_matches('_')
}

/// Whether `class`, other than `caller`, defines `method` under the name `caller`'s call looks up.
fn defines_for(class: &ClassFact, method: &str, caller: &ClassFact) -> bool {
    !std::ptr::eq(class, caller)
        && mangles_alike(method, &caller.class_name, &class.class_name)
        && class.methods.iter().any(|m| m == method)
}

/// What `Index::names_hook` reads: the union of the mentions of every file of `modules`, those that
/// do not decode included, but the test scripts in `tests`, which run on a mock server.
fn hook_mentions<'m>(modules: &'m [Module], tests: &Loaded) -> (FxHashSet<&'m str>, Vec<&'m str>) {
    let files = modules.iter().enumerate().flat_map(|(mi, m)| {
        let python = m.python.iter().map(|f| &f.mentions);
        let shipped = move |&(fi, _): &(usize, _)| !tests.contains(&(mi, fi));
        let scripts = m.js.iter().enumerate().filter(shipped);
        python
            .chain(m.data.iter().map(|f| &f.mentions))
            .chain(scripts.map(|(_, f)| &f.mentions))
            .chain(m.templates.iter().map(|f| &f.mentions))
            .chain(&m.lost_mentions)
    });
    let mut names = FxHashSet::default();
    let mut prefixes: Vec<&str> = vec![];
    for mentions in files {
        names.extend(mentions.names.iter().map(String::as_str));
        prefixes.extend(mentions.prefixes.iter().map(String::as_str));
    }
    prefixes.sort_unstable();
    prefixes.dedup();
    (names, prefixes)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rustc_hash::FxHashSet;

    use super::{
        BASE_MODEL_DUNDERS, BASE_MODEL_EXTRA, BASE_MODEL_METHODS, BASE_MODEL_VALUES, BASE_PRIVATE_METHODS,
        BASE_RPC_METHODS, BASE_VALUE_SERIES, CACHED_MODEL_VALUES, CACHED_PRIVATE_METHODS, Export, FIELD_CLASSES,
        FieldHolder, FieldSpelling, Index, JsSpec, OLD_BASE_METHODS, OWL_COMPONENT, OWL1_COMPONENT, Placed,
        PlacedWrite, Scope, Status, TagFix, XmlIdDef, absolute_import, base_model_value, base_parent_method,
        base_private_method, base_rpc_method, class_file, depends_closure, field_id_prefixes, fold_writes,
        js_module_path, last_text, mangles_alike, old_base_method, stored_primary, test_bundle, triggers,
        unseen_table_id,
    };
    use crate::facts::{BaseSource, UNREAD_TYPE};
    use crate::mentions::{Mentions, Words};
    use crate::project::{Module, SourceFile, module};
    use crate::python;
    use crate::series::Series;
    use crate::xml;

    fn python_18(source: &str) -> SourceFile<crate::facts::PyFacts> {
        SourceFile {
            path: PathBuf::new(),
            rel: String::new(),
            rank: None,
            demo: false,
            facts: python::analyze(source, Series::parse("18.0")),
            suppressions: Default::default(),
            mentions: Mentions::default(),
        }
    }

    #[test]
    fn a_private_name_mangles_alike_only_in_classes_of_one_name() {
        assert!(mangles_alike("__seal", "_Foo", "Foo"));
        assert!(!mangles_alike("__seal", "Foo", "Bar"));
        assert!(mangles_alike("__init__", "Foo", "Bar"));
        assert!(mangles_alike("_seal", "Foo", "Bar"));
        assert!(mangles_alike("__seal", "__", "_"));
        assert!(!mangles_alike("__seal", "__", "Foo"));
    }

    /// `seal.box`, whose classes each define `__seal`: `__` binds it unmangled, and `_Seal` mangles
    /// it as `Seal` does.
    fn seal_box() -> [Module; 1] {
        let mut modules = [module("base", "{'version': '18.0.1.0'}")];
        modules[0].python.push(python_18(
            "class Seal(models.Model):\n    _name = 'seal.box'\n\n    def __seal(self):\n        return 1\n\n    def _seal(self):\n        return 1\n\n\
             class __(models.Model):\n    _inherit = 'seal.box'\n\n    def __seal(self):\n        return 0\n\n\
             class Other(models.Model):\n    _inherit = 'seal.box'\n\n    def __seal(self):\n        return 2\n\n    def _seal(self):\n        return 2\n\n\
             class _Seal(models.Model):\n    _inherit = 'seal.box'\n\n    def __seal(self):\n        return 3\n",
        ));
        modules
    }

    #[test]
    fn a_private_super_call_finds_only_a_class_of_its_name() {
        let modules = seal_box();
        let index = Index::build(&modules, None);
        let classes = &modules[0].python[0].facts.classes;
        let series = Series::parse("18.0");
        assert!(!index.has_parent_method("seal.box", "__seal", &classes[2], series));
        assert!(index.has_parent_method("seal.box", "__seal", &classes[3], series));
        assert!(index.has_parent_method("seal.box", "_seal", &classes[2], series));
    }

    #[test]
    fn a_private_name_names_the_class_that_owns_it() {
        let modules = seal_box();
        let index = Index::build(&modules, None);
        let seal = &modules[0].python[0].facts.classes[0];
        assert_eq!(index.private_owner("seal.box", "__seal", seal), Some("Other"));
        assert_eq!(index.private_owner("seal.box", "_seal", seal), None);
    }

    #[test]
    fn a_base_value_is_worded_only_in_the_series_that_binds_it() {
        let value = |name: &str, series: Option<&str>| base_model_value(name, series.and_then(Series::parse));
        assert!(value("ids", Some("9.0")) && value("ids", Some("20.0")) && value("ids", None));
        assert!(value("_origin", Some("saas~12.4")) && !value("_origin", Some("saas~12.3")));
        assert!(value("_populate_sizes", Some("saas~13.2")) && !value("_populate_sizes", Some("saas~13.1")));
        assert!(value("_populate_dependencies", Some("saas~17.4")) && !value("_populate_dependencies", Some("18.0")));
        assert!(!value("_populate_sizes", None));
        assert!(value("_ondelete_methods", Some("saas~14.1")) && !value("_ondelete_methods", Some("14.0")));
        assert!(value("_table_sql", Some("saas~17.1")) && !value("_table_sql", Some("17.0")));
        assert!(value("_transient_max_count", Some("10.0")) && !value("_transient_max_count", Some("9.saas~11")));
        assert!(value("_transient_max_hours", Some("saas~18.2")) && !value("_transient_max_hours", Some("saas~18.3")));
        assert!(!value("create", Some("18.0")) && !value("create", None));
    }

    #[test]
    fn value_series_rows_parse_and_name_values() {
        for (spec, names) in BASE_VALUE_SERIES {
            for clause in spec.split(',') {
                let bound = clause.trim_start_matches(['<', '>', '=']);
                assert!(Series::parse(bound).is_some(), "{spec}");
                assert!(bound.starts_with("saas~") || bound.ends_with(".0"), "{spec}");
            }
            for name in *names {
                assert!(BASE_MODEL_VALUES.contains(name), "{name}");
            }
        }
    }

    #[test]
    fn a_private_super_call_finds_a_plain_base_of_its_name() {
        let mut modules = [module("base", "{'version': '18.0.1.0'}")];
        modules[0].python.push(python_18(
            "class _Steps:\n    def __step(self):\n        return 1\n\n\n\
             class Order(models.Model, _Steps):\n    _name = 'step.order'\n",
        ));
        modules[0].python.push(python_18(
            "class Steps(models.Model):\n    _inherit = 'step.order'\n\n\
             class Other(models.Model):\n    _inherit = 'step.order'\n",
        ));
        let index = Index::build(&modules, None);
        let order = &modules[0].python[0].facts.classes[0];
        let [steps, other] = &modules[0].python[1].facts.classes[..] else {
            panic!("two classes");
        };
        let series = Series::parse("18.0");
        assert!(index.has_parent_method("step.order", "__step", steps, series));
        assert!(!index.has_parent_method("step.order", "__step", other, series));
        assert!(!index.has_parent_method("step.order", "__step", order, series));
    }

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
            ("9.saas~9", true, false, true, true),
            ("9.saas~10", false, true, false, true),
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

    #[test]
    fn a_relative_import_resolves_below_the_addon_package() {
        let import = |rel, level| absolute_import("kit", rel, level, "mixins.Layout");
        assert_eq!(import("models/x.py", 0).as_deref(), Some("mixins.Layout"));
        assert_eq!(
            import("models/x.py", 1).as_deref(),
            Some("odoo.addons.kit.models.mixins.Layout")
        );
        assert_eq!(
            import("models/__init__.py", 1).as_deref(),
            Some("odoo.addons.kit.models.mixins.Layout")
        );
        assert_eq!(
            import("models/x.py", 2).as_deref(),
            Some("odoo.addons.kit.mixins.Layout")
        );
        assert_eq!(import("x.py", 1).as_deref(), Some("odoo.addons.kit.mixins.Layout"));
        assert_eq!(import("models/x.py", 3), None);
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

    const TARIFFS: &str = "class Tariff(models.CachedModel):\n    _name = 'x.tariff'\n\n\
                           class TariffMore(models.Model):\n    _inherit = 'x.tariff'\n\n\
                           class Plain(models.Model):\n    _name = 'x.plain'\n";

    #[test]
    fn a_super_call_finds_the_methods_of_cached_model_on_a_model_on_it() {
        let mut modules = [module("base", "{'version': '20.0.1.0'}")];
        modules[0].python.push(python_18(TARIFFS));
        let index = Index::build(&modules, None);
        let [_, more, plain] = &modules[0].python[0].facts.classes[..] else {
            panic!("three classes");
        };
        let tariff = |name: &str, series: &str| index.has_parent_method("x.tariff", name, more, Series::parse(series));
        assert!(!tariff("get_all", "saas~19.3") && tariff("get_all", "saas~19.4"));
        assert!(index.has_parent_method("x.tariff", "get_all", more, None));
        assert!(tariff("_cached_data", "saas~19.1") && !tariff("_cached_data_domain", "20.0"));
        assert!(!index.has_parent_method("x.plain", "get_all", plain, Series::parse("20.0")));
    }

    #[test]
    fn get_all_is_private_on_a_model_on_cached_model_from_saas_19_4() {
        let mut modules = [module("base", "{'version': '20.0.1.0'}")];
        modules[0].python.push(python_18(TARIFFS));
        let index = Index::build(&modules, None);
        let scope = index.scope(0);
        let private = |model: &str, series: Option<&str>| {
            index.cached_private_method(model, "get_all", &scope, series.and_then(Series::parse))
        };
        assert!(!private("x.tariff", Some("saas~19.3")) && private("x.tariff", Some("saas~19.4")));
        assert!(!private("x.tariff", None));
        assert!(!private("x.plain", Some("20.0")));
    }

    #[test]
    fn refused_call_reads_base_model_cached_model_and_a_leading_underscore() {
        let mut modules = [module("base", "{'version': '20.0.1.0'}")];
        modules[0].python.push(python_18(TARIFFS));
        let index = Index::build(&modules, None);
        let scope = index.scope(0);
        let refused = |model: Option<&str>, name: &str, series: Option<&str>| {
            index.refused_call(model, name, &scope, series.and_then(Series::parse))
        };
        for series in [Some("15.0"), Some("saas~16.2"), None] {
            assert!(!refused(None, "ensure_one", series), "{series:?}");
        }
        assert!(refused(None, "ensure_one", Some("18.0")));
        assert!(refused(None, "init", Some("15.0")) && refused(None, "init", None));
        let tariff = Some("x.tariff");
        assert!(refused(tariff, "get_all", Some("saas~19.4")));
        for series in [Some("saas~19.3"), None] {
            assert!(!refused(tariff, "get_all", series), "{series:?}");
        }
        assert!(!refused(None, "get_all", Some("20.0")));
        assert!(!refused(Some("x.plain"), "get_all", Some("20.0")));
        assert!(refused(None, "_x", None));
    }

    #[test]
    fn unbound_method_needs_every_binding_unbound_from_19_0() {
        let mut modules = [module("base", "{'version': '20.0.1.0'}")];
        modules[0].python.push(python_18(
            "class Clock(models.Model):\n    _name = 'x.clock'\n\n    \
             @classmethod\n    def tick(cls):\n        pass\n\n    \
             @classmethod\n    def swing(cls):\n        pass\n\n\
             class ClockMore(models.Model):\n    _inherit = 'x.clock'\n\n    \
             def swing(self):\n        pass\n\n\
             class Dial(models.Model):\n    _name = 'x.dial'\n\n    \
             def is_transient(self):\n        pass\n",
        ));
        let index = Index::build(&modules, None);
        let scope = index.scope(0);
        let unbound = |model: Option<&str>, name: &str, series: Option<&str>| {
            index.unbound_method(model, name, &scope, series.and_then(Series::parse))
        };
        let clock = Some("x.clock");
        assert!(unbound(clock, "tick", Some("19.0")) && unbound(clock, "tick", Some("20.0")));
        for series in [Some("saas~18.4"), None] {
            assert!(!unbound(clock, "tick", series), "{series:?}");
            assert!(!unbound(None, "is_transient", series), "{series:?}");
        }
        assert!(!unbound(clock, "swing", Some("20.0")));
        assert!(unbound(clock, "is_transient", Some("19.0")) && unbound(None, "is_transient", Some("20.0")));
        assert!(!unbound(Some("x.dial"), "is_transient", Some("20.0")));
        assert!(!unbound(clock, "missing", Some("20.0")) && !unbound(None, "tick", Some("20.0")));
        assert!(!index.refused_call(clock, "tick", &scope, Series::parse("20.0")));
    }

    #[test]
    fn unbound_method_stays_silent_on_a_binding_it_cannot_read() {
        let mut modules = [module("base", "{'version': '20.0.1.0'}")];
        modules[0].python.push(python_18(
            "class Lamp(models.Model):\n    _name = 'x.lamp'\n\n    \
             @classmethod\n    def glow(cls):\n        pass\n\n\
             class Bulb(models.Model):\n    _name = 'x.bulb'\n\n    \
             @classmethod\n    def glow(cls):\n        pass\n\n\
             class Fan(models.Model):\n    _name = 'x.fan'\n\n    \
             @classmethod\n    def spin(cls):\n        pass\n\n\
             class Clock(models.Model):\n    _name = 'x.clock'\n\n    \
             @classmethod\n    def tick(cls):\n        pass\n\n\
             class GlowMixin:\n    def glow(self):\n        pass\n\n\
             class LampMore(models.Model, GlowMixin):\n    _inherit = 'x.lamp'\n\n\
             class BulbMore(models.Model, GlowMixin):\n    _inherit = 'x.bulb'\n\n    \
             @staticmethod\n    def glow():\n        pass\n\n\
             class FanMore(models.Model, Blade):\n    _inherit = 'x.fan'\n",
        ));
        let unparsed = python_18("class ClockMore(models.Model):\n    _inherit = 'x.clock'\n\nprint 'x'\n");
        assert!(unparsed.facts.classes.iter().all(|c| c.unread));
        modules[0].python.push(unparsed);
        let index = Index::build(&modules, None);
        let scope = index.scope(0);
        let unbound = |model: &str, name: &str| index.unbound_method(Some(model), name, &scope, Series::parse("20.0"));
        assert!(!unbound("x.lamp", "glow") && unbound("x.lamp", "is_transient"));
        assert!(unbound("x.bulb", "glow"));
        assert!(!unbound("x.fan", "spin") && !unbound("x.fan", "is_transient"));
        assert!(!unbound("x.clock", "tick"));
    }

    #[test]
    fn a_data_file_finds_the_members_of_cached_model_on_a_model_on_it() {
        let mut modules = [module("base", "{'version': '20.0.1.0'}")];
        modules[0].python.push(python_18(TARIFFS));
        let index = Index::build(&modules, None);
        let scope = index.scope(0);
        let found = |model: &str, name: &str, series: &str| {
            matches!(
                index.data_method(model, name, &scope, Series::parse(series)),
                Status::Visible
            )
        };
        for name in ["get_all", "_cached_data"].iter().chain(CACHED_MODEL_VALUES) {
            assert!(
                found("x.tariff", name, "20.0") && !found("x.plain", name, "20.0"),
                "{name}"
            );
            assert!(!found("x.tariff", name, "18.0"), "{name}");
        }
    }

    /// A module that extends a model on `CachedModel` without the module that puts it there in its
    /// `depends`: OD116 and OD211 read the whole index, OD313, OD314 and OD320 only the scope.
    #[test]
    fn cached_model_reads_a_class_of_another_module() {
        let mut modules = [
            module("base", "{'version': '20.0.1.0'}"),
            module("tariff", "{'version': '20.0.1.0', 'depends': ['base']}"),
            module("reader", "{'version': '20.0.1.0', 'depends': ['base']}"),
        ];
        modules[1]
            .python
            .push(python_18("class Tariff(models.CachedModel):\n    _name = 'x.tariff'\n"));
        modules[2]
            .python
            .push(python_18("class More(models.Model):\n    _inherit = 'x.tariff'\n"));
        let index = Index::build(&modules, None);
        let twenty = Series::parse("20.0");
        let (tariff, reader) = (
            index.scope(index.by_name["tariff"]),
            index.scope(index.by_name["reader"]),
        );
        assert!(index.cached_model("x.tariff"));
        assert!(index.cached_private_method("x.tariff", "get_all", &tariff, twenty));
        assert!(!index.cached_private_method("x.tariff", "get_all", &reader, twenty));
        let status = |scope: &Scope, name: &str| match index.data_method("x.tariff", name, scope, twenty) {
            Status::Visible => "visible".to_owned(),
            Status::Hidden(m) => format!("hidden {m}"),
            Status::Missing => "missing".to_owned(),
        };
        assert_eq!(status(&tariff, "_cached_data"), "visible");
        assert_eq!(status(&reader, "_cached_data"), "hidden tariff");
        assert_eq!(status(&reader, "_cached_data_fields"), "hidden tariff");
        assert_eq!(status(&reader, "x_none"), "missing");
    }

    #[test]
    fn cached_private_methods_are_known_to_data_method() {
        let mut modules = [module("base", "{'version': '20.0.1.0'}")];
        modules[0].python.push(python_18(TARIFFS));
        let index = Index::build(&modules, None);
        let scope = index.scope(0);
        for (spec, names) in CACHED_PRIVATE_METHODS {
            let first = Series::parse(spec.trim_start_matches(">="));
            for name in *names {
                let found = index.data_method("x.tariff", name, &scope, first);
                assert!(matches!(found, Status::Visible), "{name} would raise OD312 on {spec}");
            }
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
            ("load_views", "9.saas~10", "9.saas~11"),
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
    fn values_are_no_parent_methods() {
        let series = [None, Series::parse("15.0"), Series::parse("20.0")];
        for (name, s) in BASE_MODEL_VALUES.iter().flat_map(|n| series.map(|s| (n, s))) {
            assert!(!base_parent_method(name, s), "{name} on {s:?}");
        }
        for (name, s) in ["name_get", "_populate", "__init__"]
            .iter()
            .flat_map(|n| series.map(|s| (n, s)))
        {
            assert!(base_parent_method(name, s), "{name} on {s:?}");
        }
    }

    #[test]
    fn values_stay_base_model_members() {
        for name in BASE_MODEL_VALUES {
            assert!(BASE_MODEL_EXTRA.contains(name), "{name} is missing from the flat list");
        }
    }

    #[test]
    fn old_base_methods_stop_at_their_last_series() {
        let has = |name: &str, series: &str| old_base_method(name, Series::parse(series));
        assert!(has("import_data", "9.saas~11") && !has("import_data", "10.0"));
        let renamed = "_set_default_value_on_column";
        assert!(has(renamed, "9.0") && has(renamed, "9.saas~9") && !has(renamed, "9.saas~10"));
        let domain = "_read_group_get_domain";
        assert!(has(domain, "9.saas~10") && !has(domain, "9.saas~11"));
        assert!(has("create_workflow", "10.0") && !has("create_workflow", "11.0"));
        let selection = "_check_selection_field_value";
        assert!(has(selection, "saas~11.1") && !has(selection, "saas~11.2"));
        let prepare = "_parent_store_create_prepare";
        assert!(!has(prepare, "saas~11.1") && has(prepare, "saas~11.2") && !has(prepare, "saas~11.3"));
        let count = "_check_record_rules_result_count";
        assert!(has(count, "saas~11.4") && !has(count, "saas~11.5"));
        assert!(!old_base_method("create_workflow", None));
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
    fn a_deep_gap_names_the_checked_modules_without_their_own_od503() {
        let manifest = |depends: &str| format!("{{'depends': [{depends}]}}");
        let mut modules = [
            module("base", &manifest("")),
            module("mid", &manifest("'base', 'ghost'")),
            module("top", &manifest("'mid'")),
            module("far", &manifest("'top', 'ghost'")),
            module("above", &manifest("'top'")),
            module("side", &manifest("'top', 'spectre'")),
        ];
        modules[4].checked = false;
        let index = Index::build(&modules, None);
        let gaps: Vec<_> = index.deep_gaps().into_iter().collect();
        assert_eq!(gaps, [("ghost", vec!["top"])]);
        assert!(index.partial_scope(&index.scope(2)) && !index.partial_scope(&index.scope(0)));
    }

    #[test]
    fn a_module_sees_its_depends_and_the_always_installed_and_loads_by_depth_then_name() {
        let manifest = |depends: &str, extra: &str| format!("{{'depends': [{depends}]{extra}}}");
        let modules = [
            module("base", &manifest("", "")),
            module("web", &manifest("'base'", ", 'auto_install': True")),
            module("main", &manifest("'base'", "")),
            module("heir", &manifest("'main'", "")),
            module("aside", &manifest("'web'", ", 'auto_install': True")),
            module("side", &manifest("'base'", "")),
        ];
        let index = Index::build(&modules, None);
        let sees = |from: &str, to: &str| index.sees(index.by_name[from], index.by_name[to]);
        assert!(sees("heir", "main") && sees("heir", "base") && sees("main", "main"));
        assert!(sees("main", "web") && sees("main", "aside") && sees("web", "aside"));
        assert!(!sees("main", "heir") && !sees("main", "side") && !sees("aside", "main"));
        let before = |a: &str, b: &str| index.loads_before(index.by_name[a], index.by_name[b]);
        assert!(before("base", "web") && before("main", "heir") && before("web", "aside"));
        // At one depth the name decides.
        assert!(before("main", "side") && before("aside", "heir") && !before("side", "main"));
        assert!(!before("main", "main"));
    }

    #[test]
    fn setup_scope_records_the_depends_and_the_always_installed_that_load_first() {
        let manifest = |depends: &str, extra: &str| format!("{{'depends': [{depends}]{extra}}}");
        let modules = [
            module("base", &manifest("", "")),
            module("aaa_auto", &manifest("'base'", ", 'auto_install': True")),
            module("zzz_auto", &manifest("'base'", ", 'auto_install': True")),
            module("lib", &manifest("'base'", "")),
            module("mid", &manifest("'base'", "")),
            module("top", &manifest("'mid'", "")),
        ];
        let index = Index::build(&modules, None);
        let names = |held: &[bool]| -> Vec<&str> {
            let set = modules.iter().zip(held).filter(|(_, held)| **held);
            set.map(|(m, _)| m.name.as_str()).collect()
        };
        for (reader, records) in [
            ("mid", vec!["base", "aaa_auto"]),
            ("top", vec!["base", "aaa_auto", "zzz_auto", "mid"]),
        ] {
            let mi = index.by_name[reader];
            let setup = index.setup_scope(mi);
            assert_eq!(names(&setup.records), records, "{reader}");
            assert_eq!(setup.visible, index.scope(mi).visible, "{reader}");
        }
    }

    #[test]
    fn a_module_has_an_unread_dependent_when_another_unread_module_reaches_it() {
        let manifest = |depends: &str| format!("{{'depends': [{depends}]}}");
        let mut modules = [
            module("base", &manifest("")),
            module("lib", &manifest("'base'")),
            module("mid", &manifest("'lib'")),
            module("reader", &manifest("'mid', 'missing'")),
            module("side", &manifest("'base'")),
            module("top", &manifest("'reader'")),
            module("alone", &manifest("'ring'")),
            module("ring", &manifest("'alone'")),
        ];
        let dynamic = "import importlib\nimportlib.import_module(name)\n";
        for name in ["reader", "side", "alone"] {
            let module = modules.iter_mut().find(|m| m.name == name).unwrap();
            module.python.push(SourceFile {
                path: PathBuf::new(),
                rel: "__init__.py".to_owned(),
                rank: None,
                demo: false,
                facts: python::analyze(dynamic, Series::parse("18.0")),
                suppressions: Default::default(),
                mentions: Mentions::default(),
            });
        }
        let index = Index::build(&modules, None);
        let marked = (0..modules.len()).filter(|&mi| index.has_unread_dependent(mi));
        let marked: Vec<_> = marked.map(|mi| modules[mi].name.as_str()).collect();
        assert_eq!(marked, ["base", "lib", "mid", "ring"]);
        let unread = |mi: usize| index.imports.unread(&modules[mi].name);
        for mi in 0..modules.len() {
            let walked = (0..modules.len()).any(|o| o != mi && unread(o) && index.reaches(o, mi));
            assert_eq!(index.has_unread_dependent(mi), walked, "{}", modules[mi].name);
        }
    }

    #[test]
    fn an_empty_depends_waits_on_base_from_saas_18_2() {
        let modules = [
            module("base", "{'auto_install': True}"),
            module("bare", "{'auto_install': ['base']}"),
            module("whole", "{'auto_install': True}"),
        ];
        for name in ["18.0", "saas~18.1"] {
            let series = Series::parse(name);
            let index = Index::build(&modules, series);
            assert!(!index.always_installed[1], "{name}");
            assert_eq!(triggers(&modules[1], series), None, "{name}");
            assert_eq!(triggers(&modules[2], series), Some(vec![]), "{name}");
            assert_eq!(triggers(&modules[0], series), Some(vec![]), "{name}");
        }
        for series in [Series::parse("saas~18.2"), Series::parse("20.0"), None] {
            let index = Index::build(&modules, series);
            assert!(index.always_installed[1], "{series:?}");
            assert_eq!(triggers(&modules[1], series), Some(vec!["base"]), "{series:?}");
            assert_eq!(triggers(&modules[2], series), Some(vec!["base"]), "{series:?}");
            assert_eq!(triggers(&modules[0], series), Some(vec![]), "{series:?}");
        }
    }

    #[test]
    fn an_empty_depends_still_auto_installs_when_base_is_not_indexed() {
        let modules = [
            module("bare", "{'auto_install': True}"),
            module("listed", "{'auto_install': ['base']}"),
            module("named", "{'depends': ['base'], 'auto_install': True}"),
        ];
        for series in [Series::parse("18.0"), Series::parse("20.0"), None] {
            let index = Index::build(&modules, series);
            assert!(index.always_installed[0], "{series:?}");
            // A depend off the index keeps the module from loading, `base` included.
            assert!(!index.always_installed[2], "{series:?}");
        }
        assert!(!Index::build(&modules, Series::parse("18.0")).always_installed[1]);
        assert!(Index::build(&modules, Series::parse("20.0")).always_installed[1]);
    }

    #[test]
    fn an_empty_depends_reads_as_base_from_saas_18_2() {
        let modules = [module("base", "{}"), module("bare", "{}"), module("aaa", "{}")];
        let closure = |index: &Index, mi: usize| {
            let (reached, complete) = depends_closure(&modules, &index.by_name, [mi], index.default_series);
            let names = (0..modules.len())
                .filter(|&m| reached[m])
                .map(|m| modules[m].name.as_str());
            (names.collect::<Vec<_>>(), complete)
        };
        for name in ["18.0", "saas~18.1"] {
            let index = Index::build(&modules, Series::parse(name));
            assert_eq!(closure(&index, 1), (vec!["bare"], true), "{name}");
            assert!(!index.reaches(1, 0), "{name}");
        }
        for series in [Series::parse("saas~18.2"), Series::parse("20.0"), None] {
            let index = Index::build(&modules, series);
            assert_eq!(closure(&index, 1), (vec!["base", "bare"], true), "{series:?}");
            assert_eq!(closure(&index, 0), (vec!["base"], true), "{series:?}");
            assert!(index.reaches(1, 0) && !index.reaches(0, 1), "{series:?}");
        }
        // Odoo loads `base` alone before the rest whatever their `depends` read.
        for series in ["18.0", "saas~18.1", "saas~18.2", "20.0"]
            .map(Series::parse)
            .into_iter()
            .chain([None])
        {
            let index = Index::build(&modules, series);
            assert!(index.loads_before(0, 2) && index.loads_before(0, 1), "{series:?}");
            assert!(!index.loads_before(2, 0), "{series:?}");
        }
        // Odoo installs `base` on every database, so leaving it out of the scan loses nothing.
        let unbased = &modules[1..];
        let by_name = unbased.iter().enumerate().map(|(i, m)| (m.name.as_str(), i)).collect();
        assert_eq!(
            depends_closure(unbased, &by_name, [0], Series::parse("20.0")),
            (vec![true, false], true)
        );
    }

    #[test]
    fn base_loads_first_whatever_the_depends_of_the_rest() {
        let data_file = |id: &str, kind: &str| SourceFile {
            path: PathBuf::new(),
            rel: "data/partner.xml".to_owned(),
            rank: Some(0),
            demo: false,
            facts: xml::analyze(
                &format!(
                    r#"<odoo><record id="{id}" model="res.partner"><field name="type">{kind}</field></record></odoo>"#
                ),
                Series::parse("18.0"),
            ),
            suppressions: Default::default(),
            mentions: Mentions::default(),
        };
        let mut modules = [
            module("base", "{'version': '18.0.1.0.0'}"),
            module("abc", "{'version': '18.0.1.0.0'}"),
        ];
        modules[0].data.push(data_file("partner_root", "contact"));
        modules[1].data.push(data_file("base.partner_root", "invoice"));
        for series in [Series::parse("18.0"), Series::parse("saas~18.1"), None] {
            let index = Index::build(&modules, series);
            assert!(index.loads_before(0, 1) && !index.loads_before(1, 0), "{series:?}");
            let order = index.value_writes["base.partner_root"].iter().map(|w| w.module);
            assert_eq!(order.collect::<Vec<_>>(), [0, 1], "{series:?}");
        }
    }

    #[test]
    fn a_dependency_another_one_brings_is_implied_by_the_top_of_its_chain() {
        let manifest = |depends: &str, extra: &str| format!("{{'depends': [{depends}]{extra}}}");
        let modules = [
            module("base", &manifest("", "")),
            module("mail", &manifest("'base'", "")),
            module("digest", &manifest("'mail'", "")),
            module("back", &manifest("'base', 'loop'", "")),
            module("chain", &manifest("'base', 'mail'", "")),
            module("top", &manifest("'base', 'mail', 'digest'", "")),
            module("loop", &manifest("'base', 'back'", "")),
            module("trigger", &manifest("'base', 'mail'", ", 'auto_install': ['base']")),
            module("unindexed", &manifest("'base', 'ghost'", "")),
            module("twice", &manifest("'base', 'base'", "")),
        ];
        let index = Index::build(&modules, None);
        let implied = |name: &str| index.implied_depends(index.by_name[name]);
        assert_eq!(implied("chain"), [(0, 1)]);
        assert_eq!(implied("top"), [(0, 2), (1, 2)]);
        assert_eq!(implied("loop"), []);
        assert_eq!(implied("trigger"), []);
        assert_eq!(implied("unindexed"), []);
        assert_eq!(implied("twice"), []);
    }

    #[test]
    fn a_seriesless_dependency_reads_in_its_dependents_series() {
        let modules = [
            module("base", "{}"),
            module("lib", "{'version': '1.0'}"),
            module("old", "{'version': '18.0.1.0.0', 'depends': ['lib', 'base']}"),
            module("new", "{'version': '20.0.1.0.0', 'depends': ['lib', 'base']}"),
            module("via", "{'version': '18.0.1.0.0', 'depends': ['lib']}"),
            module("via20", "{'version': '20.0.1.0.0', 'depends': ['lib']}"),
            module("bare", "{'depends': ['lib', 'base']}"),
        ];
        let index = Index::build(&modules, None);
        let at = |name: &str| index.by_name[name];
        assert_eq!(index.implied_depends(at("old")), []);
        assert_eq!(index.implied_depends(at("new")), [(1, 0)]);
        assert_eq!(index.implied_depends(at("bare")), [(1, 0)]);
        assert!(!index.reaches(at("via"), at("base")));
        assert!(index.reaches(at("via20"), at("base")));
        let sole: Vec<usize> = index.sole_brought(at("old")).into_iter().map(|(d, _)| d).collect();
        assert_eq!(sole, [0]);
        let index = Index::build(&modules, Series::parse("18.0"));
        let at = |name: &str| index.by_name[name];
        assert_eq!(index.implied_depends(at("bare")), []);
        assert_eq!(index.implied_depends(at("new")), [(1, 0)]);
    }

    #[test]
    fn base_loads_ahead_of_a_module_whose_depends_never_reach_it() {
        let modules = [
            module("aaa", "{'version': '18.0.1.0.0'}"),
            module("base", "{}"),
            module("lib", "{'version': '1.0'}"),
            module("via", "{'version': '18.0.1.0.0', 'depends': ['lib']}"),
        ];
        for series in [None, Series::parse("18.0")] {
            let index = Index::build(&modules, series);
            let at = |name: &str| index.by_name[name];
            for name in ["aaa", "via"] {
                assert!(!index.reaches(at(name), at("base")), "{name} {series:?}");
                assert!(index.loads_after(at(name), at("base")), "{name} {series:?}");
                assert!(!index.loads_after(at("base"), at(name)), "{name} {series:?}");
                let entries = [(at(name), name), (at("base"), "base")];
                assert_eq!(index.first_loaded(entries), Some((at("base"), "base")), "{series:?}");
            }
            assert!(!index.loads_after(at("base"), at("base")));
            assert!(index.loads_after(at("via"), at("lib")) && !index.loads_after(at("lib"), at("via")));
        }
    }

    #[test]
    fn an_override_in_base_overrides_no_module_with_empty_depends() {
        // In 18.0 `aaa` does not reach `base`, yet Odoo loads `base` alone first, so `base`'s `act`
        // overrides nothing of `aaa`'s.
        let mut modules = [module("base", "{}"), module("aaa", "{'version': '18.0.1.0.0'}")];
        modules[0].python.push(python_18(
            "class T(models.Model):\n    _name = 'x.thing'\n\n    def act(self, a):\n        pass\n",
        ));
        modules[1].python.push(python_18(
            "class U(models.Model):\n    _inherit = 'x.thing'\n\n    def act(self):\n        pass\n",
        ));
        let index = Index::build(&modules, None);
        let [base, aaa] = ["base", "aaa"].map(|name| index.by_name[name]);
        let merged = index.merged_classes("x.thing", &index.scope(aaa));
        assert_eq!(merged.len(), 2);
        let parents = |mi: usize| {
            let class = merged.iter().find(|(m, _)| *m == mi).unwrap().1;
            let found = index.method_parents(mi, class, &merged, "act").unwrap();
            found.into_iter().map(|(m, _)| m).collect::<Vec<_>>()
        };
        assert_eq!(parents(base), Vec::<usize>::new());
        assert_eq!(parents(aaa), [base]);
    }

    #[test]
    fn an_entry_alone_brings_what_no_other_entry_or_base_does() {
        let manifest = |depends: &str, extra: &str| format!("{{'depends': [{depends}]{extra}}}");
        let modules = [
            module("base", &manifest("", "")),
            module("web", &manifest("'base'", ", 'auto_install': True")),
            module("mail", &manifest("'base'", "")),
            module("product", &manifest("'mail'", "")),
            module("team", &manifest("'base'", "")),
            module("sale", &manifest("'product', 'team'", "")),
            module("stock", &manifest("'product'", "")),
            module("partial", &manifest("'ghost'", "")),
            module("back", &manifest("'main'", "")),
            module(
                "main",
                &manifest(
                    "'base', 'web', 'sale', 'stock', 'mail', 'ghost', 'partial', 'back', 'main'",
                    "",
                ),
            ),
            module("solo", &manifest("'sale'", "")),
            module("pair", &manifest("'sale', 'mail'", "")),
        ];
        let index = Index::build(&modules, None);
        let brought = |name: &str| -> Vec<(usize, Vec<&str>)> {
            let sole = index.sole_brought(index.by_name[name]);
            let names = |set: &[bool]| {
                modules
                    .iter()
                    .filter(|m| set[index.by_name[m.name.as_str()]])
                    .map(|m| m.name.as_str())
                    .collect()
            };
            sole.iter().map(|(d, set)| (*d, names(set))).collect()
        };
        assert_eq!(
            brought("main"),
            [(1, vec!["web"]), (2, vec!["team", "sale"]), (3, vec!["stock"])]
        );
        assert_eq!(brought("solo"), [(0, vec!["mail", "product", "team", "sale"])]);
        // `mail` comes with `sale`, so dropping `sale` would lose it.
        assert_eq!(brought("pair"), [(0, vec!["mail", "product", "team", "sale"])]);
    }

    #[test]
    fn words_take_from_a_set_by_any_name_only_it_gives() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = [
            module("base", &manifest("")),
            module("prov", &manifest("'base'")),
            module("main", &manifest("'prov'")),
        ];
        modules[0].python.push(python_18(
            "class P(models.Model):\n    _name = 'res.partner'\n    name = fields.Char()\n    tz = fields.Char()\n\n    def _engine(self):\n        raise NotImplementedError()\n\n    def copy(self):\n        pass\n\n    def _compute_kind(self):\n        pass\n\n\
             class C(models.Model):\n    _name = 'res.company'\n    name = fields.Char()\n",
        ));
        modules[1].python.push(python_18(
            "from odoo import http\n\n\
             class T(models.Model):\n    _name = 'prov.thing'\n    name = fields.Char()\n    prov_code = fields.Char()\n\n    def action_prov(self):\n        pass\n\n\
             class P(models.Model):\n    _inherit = 'res.partner'\n    name = fields.Char(required=True)\n    kind = fields.Selection(selection_add=[('prov_kind', 'Prov')])\n\n    def _engine(self):\n        return 1\n\n    def copy(self):\n        pass\n\n    def _compute_kind(self):\n        pass\n\n\
             class C(models.Model):\n    _inherit = 'res.company'\n    tz = fields.Char()\n\n    def copy(self):\n        pass\n\n    def _compute_zone(self):\n        pass\n\n\
             class Map(http.Controller):\n    @http.route('/prov_map', auth='public')\n    def map(self):\n        pass\n\n    @http.route(['/prov/item/<int:x>'])\n    def item(self, x):\n        pass\n",
        ));
        modules[2].python.push(python_18(
            "class P(models.Model):\n    _inherit = 'res.partner'\n    prov_code = fields.Char()\n",
        ));
        let index = Index::build(&modules, None);
        let main = index.by_name["main"];
        let set: Vec<bool> = modules.iter().map(|m| m.name == "prov").collect();
        let scope = index.scope(main);
        let takes = |text: &str| index.takes_from(&Words::scan(text), &set, &scope, main);
        for text in [
            "'prov.thing'",
            "FROM prov_thing",
            "rec.prov_code",
            "action_prov",
            "'prov_kind'",
            "ref('prov.menu')",
            "from odoo.addons.prov.models import x",
            "import '@prov/static/a'",
            "import '@prov/../lib/a'",
            "company.tz",
            "self._engine()",
            "src='/prov_map?z=1'",
            "'/prov/item/4'",
            "compute='_compute_zone'",
        ] {
            assert!(takes(text), "{text}");
        }
        for text in [
            "name",
            "rec.name",
            "'prov'",
            "res.partner",
            "other.thing",
            "prov_map",
            "'/prov'",
            "rec.copy()",
            "_compute_kind",
        ] {
            assert!(!takes(text), "{text}");
        }
    }

    #[test]
    fn an_inherit_parent_is_indexed_only_where_a_class_introduces_it() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = [
            module("base", &manifest("")),
            module("side", &manifest("'base'")),
            module("main", &manifest("'base'")),
        ];
        modules[0]
            .python
            .push(python_18("class P(models.Model):\n    _name = 'res.partner'\n"));
        modules[1]
            .python
            .push(python_18("class T(models.Model):\n    _name = 'side.thing'\n"));
        modules[2].python.push(python_18(
            "class W(models.AbstractModel):\n    _inherit = 'ghost.widget'\n\n\
             class S(models.AbstractModel):\n    _name = 'parent.shelf'\n    _inherit = ['ghost.tray']\n\n\
             class G(models.AbstractModel):\n    _inherit = 'ghost.tray'\n\n\
             class B(models.AbstractModel):\n    _name = 'parent.bench'\n    _inherit = ['res.partner']\n\n\
             class P(models.Model):\n    _inherit = 'res.partner'\n\n\
             class T(models.Model):\n    _inherit = 'side.thing'\n",
        ));
        let index = Index::build(&modules, None);
        let scope = index.scope(index.by_name["main"]);
        for model in ["ghost.widget", "parent.shelf", "ghost.tray"] {
            assert!(index.unindexed_parent(model, &scope), "{model}");
        }
        for model in ["parent.bench", "res.partner", "side.thing"] {
            assert!(!index.unindexed_parent(model, &scope), "{model}");
        }
    }

    #[test]
    fn a_model_no_class_introduces_reads_as_unknown() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = [
            module("base", &manifest("")),
            module("side", &manifest("'base'")),
            module("main", &manifest("'base'")),
        ];
        modules[0]
            .python
            .push(python_18("class P(models.Model):\n    _name = 'res.partner'\n"));
        modules[1].python.push(python_18(
            "class G(models.AbstractModel):\n    _inherit = 'ghost.lantern'\n    wick = fields.Char()\n\n\
             class S(models.AbstractModel):\n    _name = 'side.lantern'\n    wick = fields.Char()\n\n\
             class U(models.Model):\n    _name = 'refs.cub'\n    _inherit = ['refs.cub', 'ghost.lantern']\n",
        ));
        modules[2].python.push(python_18(
            "class G(models.Model):\n    _inherit = 'ghost.thing'\n\n\
             class R(models.Model):\n    _name = 'refs.thing'\n\n\
             class C(models.Model):\n    _name = 'refs.child'\n    _inherits = {'ghost.lantern': 'lantern_id'}\n\n\
             class H(models.Model):\n    _name = 'refs.heir'\n    _inherits = {'side.lantern': 'lantern_id'}\n\n\
             class K(models.Model):\n    _name = 'refs.kid'\n    _inherit = ['ghost.lantern']\n\n\
             class U(models.Model):\n    _name = 'refs.cub'\n    _inherit = ['side.lantern']\n",
        ));
        let index = Index::build(&modules, None);
        let scope = index.scope(index.by_name["main"]);
        assert!(index.resolve_path("ghost.thing", "wick", &scope).is_ok());
        assert!(index.resolve_path("refs.thing", "wick", &scope).is_err());
        for model in ["refs.heir", "refs.cub"] {
            assert!(index.resolve_path(model, "wick", &scope).is_err(), "{model}");
        }
        for model in ["refs.child", "refs.kid"] {
            assert!(index.resolve_path(model, "wick", &scope).is_ok(), "{model}");
            assert!(index.unread_delegate(model, &scope), "{model}");
            assert!(index.unintroduced_parent_defines(model, "wick", &scope), "{model}");
        }
        for model in ["refs.heir", "refs.cub"] {
            assert!(!index.unintroduced_parent_defines(model, "wick", &scope), "{model}");
        }
        for model in ["ghost.thing", "refs.heir"] {
            assert!(!index.unread_delegate(model, &scope), "{model}");
        }
        assert_eq!(index.stands_for("ghost.thing", "res.partner", &scope), None);
        assert_eq!(index.stands_for("refs.thing", "res.partner", &scope), Some(false));
    }

    #[test]
    fn a_module_lists_an_unindexed_parent_only_in_its_own_classes() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = [
            module("base", &manifest("")),
            module("vendor", &manifest("'base'")),
            module("main", &manifest("'vendor'")),
        ];
        modules[0]
            .python
            .push(python_18("class P(models.Model):\n    _name = 'res.partner'\n"));
        modules[1].python.push(python_18(
            "class T(models.Model):\n    _name = 'vendor.thing'\n    _inherit = ['ghost.mixin']\n",
        ));
        modules[2].python.push(python_18(
            "class T(models.Model):\n    _inherit = 'vendor.thing'\n\n\
             class B(models.Model):\n    _name = 'main.box'\n    _inherit = ['ghost.box']\n\n\
             class R(models.Model):\n    _name = 'main.rack'\n    _inherit = ['main.box']\n",
        ));
        let index = Index::build(&modules, None);
        let (vendor, main) = (index.by_name["vendor"], index.by_name["main"]);
        let scope = index.scope(main);
        for model in ["main.box", "main.rack"] {
            assert!(index.lists_unindexed_parent(main, model, &scope), "{model}");
        }
        assert!(!index.lists_unindexed_parent(main, "vendor.thing", &scope));
        assert!(index.unindexed_parent("vendor.thing", &scope));
        assert!(index.lists_unindexed_parent(vendor, "vendor.thing", &index.scope(vendor)));
    }

    #[test]
    fn a_delegated_field_comes_from_the_nearest_level_that_defines_it() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = [module("base", &manifest("")), module("main", &manifest("'base'"))];
        modules[0]
            .python
            .push(python_18("class P(models.Model):\n    _name = 'res.partner'\n"));
        modules[1].python.push(python_18(
            "class A(models.Model):\n    _name = 'a'\n    x = fields.Many2one('res.partner')\n\n\
             class A2(models.Model):\n    _name = 'a2'\n    x = fields.Many2one('res.partner')\n\n\
             class B(models.Model):\n    _name = 'b'\n    _inherits = {'a': 'a_id'}\n    x = fields.Char()\n\n\
             class D(models.Model):\n    _name = 'd'\n    _inherits = {'b': 'b_id'}\n\n\
             class B2(models.Model):\n    _name = 'b2'\n    _inherits = {'a': 'a_id'}\n\n\
             class E(models.Model):\n    _name = 'e'\n    _inherits = {'b2': 'b2_id'}\n\n\
             class F(models.Model):\n    _name = 'f'\n    _inherits = {'a2': 'a2_id', 'b': 'b_id'}\n\n\
             class G(models.Model):\n    _name = 'g'\n    _inherits = {'b': 'b_id', 'a2': 'a2_id'}\n\n\
             class H(models.Model):\n    _name = 'h'\n    _inherits = {'b': 'b_id', 'b2': 'b2_id'}\n",
        ));
        let index = Index::build(&modules, None);
        let scope = index.scope(index.by_name["main"]);
        let x = |model: &str| {
            let info = index.field(model, "x", &scope).ok().flatten().unwrap();
            (info.ftype, info.comodel)
        };
        let partner = Some("res.partner".to_owned());
        assert_eq!(x("d"), ("Char", None));
        assert_eq!(x("e"), ("Many2one", partner.clone()));
        assert_eq!(x("f"), ("Char", None));
        assert_eq!(x("g"), ("Many2one", partner.clone()));
        // b2 has no `x` of its own, yet as the later entry its delegated Many2one beats b's Char.
        assert_eq!(x("h"), ("Many2one", partner));
    }

    #[test]
    fn a_later_parent_s_record_decides_a_delegated_field() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let record = |model: &str, relation: &str| {
            format!(
                r#"<record id="field_{model}_x_f" model="ir.model.fields">
                    <field name="name">x_f</field>
                    <field name="model_id" ref="model_{model}"/>
                    <field name="ttype">many2one</field>
                    <field name="relation">{relation}</field>
                </record>"#
            )
        };
        let mut modules = [module("base", &manifest("")), module("main", &manifest("'base'"))];
        modules[0].python.push(python_18(
            "class P(models.Model):\n    _name = 'res.partner'\n\n\
             class U(models.Model):\n    _name = 'res.users'\n",
        ));
        modules[1].python.push(python_18(
            "class P1(models.Model):\n    _name = 'p1'\n    x_f = fields.Many2one('res.partner')\n\n\
             class P2(models.Model):\n    _name = 'p2'\n\n\
             class C(models.Model):\n    _name = 'c'\n    _inherits = {'p1': 'p1_id', 'p2': 'p2_id'}\n\n\
             class R(models.Model):\n    _name = 'r'\n    _inherits = {'p2': 'p2_id', 'p1': 'p1_id'}\n\n\
             class G(models.Model):\n    _name = 'g'\n    x_f = fields.Many2one('res.partner')\n\n\
             class P3(models.Model):\n    _name = 'p3'\n    _inherits = {'g': 'g_id'}\n\n\
             class K(models.Model):\n    _name = 'k'\n    _inherits = {'p3': 'p3_id'}\n\n\
             class D(models.Model):\n    _name = 'd'\n    _inherits = {'p2': 'p2_id'}\n\n\
             class A(models.AbstractModel):\n    _name = 'a'\n\n\
             class Q(models.Model):\n    _name = 'q'\n    _inherit = ['a']\n\n\
             class M(models.Model):\n    _name = 'm'\n    _inherits = {'p1': 'p1_id', 'q': 'q_id'}\n",
        ));
        modules[1].data.push(SourceFile {
            path: PathBuf::new(),
            rel: "data/fields.xml".to_owned(),
            rank: Some(0),
            demo: false,
            facts: xml::analyze(
                &format!(
                    "<odoo>{}{}{}{}</odoo>",
                    record("p2", "res.users"),
                    record("p3", "res.users"),
                    record("d", "res.partner"),
                    record("a", "res.users"),
                ),
                Series::parse("18.0"),
            ),
            suppressions: Default::default(),
            mentions: Mentions::default(),
        });
        let index = Index::build(&modules, None);
        let scope = index.scope(index.by_name["main"]);
        let comodel = |model: &str| index.field(model, "x_f", &scope).ok().flatten().unwrap().comodel;
        let users = Some("res.users".to_owned());
        // p2 gives `x_f` only by its record, yet as the later entry it beats p1's Python field.
        assert_eq!(comodel("c"), users);
        assert_eq!(comodel("r"), Some("res.partner".to_owned()));
        // p3's own record comes before the field it would copy from g.
        assert_eq!(comodel("k"), users);
        // d's own record comes before the one it would copy from p2.
        assert_eq!(comodel("d"), Some("res.partner".to_owned()));
        // a's record adds `x_f` to a only, so q gives none and p1's field stands.
        assert_eq!(comodel("m"), Some("res.partner".to_owned()));
        for model in ["c", "k"] {
            assert!(index.path_target(model, "x_f", &scope).is_none(), "{model}");
        }
        assert!(index.path_target("r", "x_f", &scope).is_some());
    }

    #[test]
    fn a_redefined_field_takes_the_type_and_comodel_of_its_last_definition() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = [
            module("base", &manifest("")),
            module("main", &manifest("'base'")),
            module("left", &manifest("'main'")),
            module("right", &manifest("'main'")),
            module("top", &manifest("'left', 'right'")),
        ];
        modules[0].python.push(python_18(
            "class P(models.Model):\n    _name = 'res.partner'\n\n\
             class U(models.Model):\n    _name = 'res.users'\n",
        ));
        modules[1].python.push(python_18(
            "class A(models.Model):\n    _name = 'a'\n    x = fields.Char()\n    \
             y = fields.Many2one('res.partner')\n    z = fields.Many2one('res.partner')\n    w = fields.Char()\n\n\
             class A1(models.Model):\n    _inherit = 'a'\n    x = fields.Integer()\n    y = fields.Char()\n    \
             z = fields.Many2one('res.users')\n\n\
             class B(models.Model):\n    _name = 'b'\n    _inherits = {'a': 'a_id'}\n",
        ));
        modules[2].python.push(python_18(
            "class A(models.Model):\n    _inherit = 'a'\n    w = fields.Char()\n",
        ));
        modules[3].python.push(python_18(
            "class A(models.Model):\n    _inherit = 'a'\n    w = fields.Integer()\n",
        ));
        let index = Index::build(&modules, None);
        let info = |module: &str, model: &str, name: &str| {
            let scope = index.scope(index.by_name[module]);
            let info = index.field(model, name, &scope).ok().flatten().unwrap();
            (info.ftype, info.comodel)
        };
        assert_eq!(info("main", "a", "x"), ("Integer", None));
        assert_eq!(info("main", "a", "y"), ("Char", None));
        assert_eq!(info("main", "a", "z"), ("Many2one", Some("res.users".to_owned())));
        assert_eq!(info("main", "b", "x"), ("Integer", None));
        assert_eq!(info("left", "a", "w"), ("Char", None));
        assert_eq!(info("top", "a", "w"), (UNREAD_TYPE, None));
    }

    #[test]
    fn a_redefined_field_takes_its_comodel_from_the_definitions_odoo_keeps() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = [
            module("base", &manifest("")),
            module("main", &manifest("'base'")),
            module("left", &manifest("'main'")),
            module("right", &manifest("'main'")),
            module("top", &manifest("'left', 'right'")),
        ];
        modules[0].python.push(python_18(
            "class P(models.Model):\n    _name = 'res.partner'\n\n\
             class U(models.Model):\n    _name = 'res.users'\n\n\
             class D(models.Model):\n    _name = 'd'\n    q = fields.Many2one('res.users')\n",
        ));
        modules[1].python.push(python_18(
            "class A(models.Model):\n    _name = 'a'\n    p = fields.Many2one('d')\n    \
             v = fields.Many2one('res.partner')\n    k = fields.Many2one('res.partner')\n    \
             j = fields.Many2one(string='J')\n    s = fields.Many2one('res.partner')\n    \
             g = fields.Many2one('res.partner')\n\n\
             class A1(models.Model):\n    _inherit = 'a'\n    v = fields.Char()\n    k = fields.Many2one(string='K')\n    \
             j = fields.Many2one(related='p.q')\n    s = fields.Many2one(**kwargs)\n\n\
             class A2(models.Model):\n    _inherit = 'a'\n    v = fields.Many2one(related='p.q')\n\n\
             class M(models.AbstractModel):\n    _name = 'm'\n    t = fields.Char()\n\n\
             class M1(models.AbstractModel):\n    _inherit = 'm'\n    t = fields.Integer()\n\n\
             class X(models.Model):\n    _name = 'x'\n    _inherit = 'm'\n",
        ));
        modules[2].python.push(python_18(
            "class A(models.Model):\n    _inherit = 'a'\n    g = fields.Char()\n    \
             h = fields.Many2one('res.partner')\n    r = fields.Many2one(related='p.q')\n    \
             u = fields.Many2one('res.partner')\n",
        ));
        modules[3].python.push(python_18(
            "class A(models.Model):\n    _inherit = 'a'\n    h = fields.Char()\n    \
             r = fields.Many2one(related='p.q')\n    u = fields.Many2one('res.users')\n",
        ));
        modules[4].python.push(python_18(
            "class A(models.Model):\n    _inherit = 'a'\n    g = fields.Many2one(string='G')\n    \
             h = fields.Many2one(string='H')\n",
        ));
        let index = Index::build(&modules, None);
        let info = |module: &str, model: &str, name: &str| {
            let scope = index.scope(index.by_name[module]);
            let info = index.field(model, name, &scope).ok().flatten().unwrap();
            (info.ftype, info.comodel)
        };
        let users = Some("res.users".to_owned());
        // The Char drops the first comodel, so the related path names it.
        assert_eq!(info("main", "a", "v"), ("Many2one", users.clone()));
        assert_eq!(info("main", "a", "k"), ("Many2one", Some("res.partner".to_owned())));
        assert_eq!(info("main", "a", "j"), ("Many2one", users.clone()));
        assert_eq!(info("main", "a", "s"), ("Many2one", None));
        assert_eq!(info("main", "x", "t"), ("Integer", None));
        assert_eq!(info("top", "a", "g"), ("Many2one", None));
        assert_eq!(info("top", "a", "h"), ("Many2one", None));
        assert_eq!(info("top", "a", "r"), ("Many2one", users));
        assert_eq!(info("top", "a", "u"), ("Many2one", None));
    }

    #[test]
    fn a_path_ends_on_the_model_s_own_definition_before_a_delegated_one() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = [module("base", &manifest("")), module("main", &manifest("'base'"))];
        modules[0]
            .python
            .push(python_18("class P(models.Model):\n    _name = 'res.partner'\n"));
        modules[1].python.push(python_18(
            "class A(models.Model):\n    _name = 'a'\n    x = fields.Many2one('res.partner')\n\n\
             class B(models.Model):\n    _name = 'b'\n    _inherits = {'a': 'a_id'}\n    x = fields.Char()\n\n\
             class D(models.Model):\n    _name = 'd'\n    _inherits = {'b': 'b_id'}\n    b_link = fields.Many2one('b')\n\n\
             class E(models.Model):\n    _name = 'e'\n    _inherits = {'a': 'a_id'}\n",
        ));
        let index = Index::build(&modules, None);
        let scope = index.scope(index.by_name["main"]);
        let types = |model: &str, path: &str| -> Vec<&str> {
            let targets = index.path_target(model, path, &scope).unwrap();
            targets.iter().map(|f| f.ftype.as_str()).collect()
        };
        assert_eq!(types("b", "x"), ["Char"]);
        assert_eq!(types("d", "x"), ["Char"]);
        assert_eq!(types("d", "b_link.x"), ["Char"]);
        assert_eq!(types("e", "x"), ["Many2one"]);
    }

    #[test]
    fn a_class_parent_ignores_the_module_s_own_extensions_and_later_introducers() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = [
            module("base", &manifest("")),
            module("side", &manifest("'base'")),
            module("main", &manifest("'base'")),
            module("heir", &manifest("'main'")),
        ];
        modules[0]
            .python
            .push(python_18("class P(models.Model):\n    _name = 'res.partner'\n"));
        modules[1]
            .python
            .push(python_18("class S(models.Model):\n    _name = 'side.thing'\n"));
        modules[2].python.push(python_18(
            "class G0(models.Model):\n    _inherit = 'ghost.lamp'\n\n\
             class G1(models.Model):\n    _inherit = 'ghost.lamp'\n\n\
             class S0(models.Model):\n    _inherit = 'side.thing'\n\n\
             class S1(models.Model):\n    _inherit = 'side.thing'\n\n\
             class P0(models.Model):\n    _inherit = 'res.partner'\n\n\
             class P1(models.Model):\n    _inherit = 'res.partner'\n\n\
             class T0(models.Model):\n    _inherit = 'side.thing'\n\n\
             class T(models.Model):\n    _name = 'side.thing'\n\n\
             class T1(models.Model):\n    _inherit = 'side.thing'\n\n\
             class X0(models.Model):\n    _inherit = 'main.lamp'\n\n\
             class L(models.Model):\n    _name = 'main.lamp'\n\n\
             class X(models.Model):\n    _inherit = 'main.lamp'\n",
        ));
        modules[3]
            .python
            .push(python_18("class G(models.Model):\n    _inherit = 'ghost.lamp'\n"));
        let index = Index::build(&modules, None);
        let (main, heir) = (index.by_name["main"], index.by_name["heir"]);
        let class = |mi: usize, name: &str| {
            let classes = modules[mi].python.iter().flat_map(|f| &f.facts.classes);
            classes.into_iter().find(|c| c.class_name == name).unwrap()
        };
        let status = |model: &str, mi: usize, reader: Option<(usize, Option<&str>)>| {
            let scope = index.scope(mi);
            let reader = reader.map(|(e, name)| (e, name.map(|name| class(e, name))));
            match index.model_status(model, &scope, reader) {
                Status::Visible => "visible".to_owned(),
                Status::Hidden(m) => format!("hidden {m}"),
                Status::Missing => "missing".to_owned(),
            }
        };
        assert_eq!(status("ghost.lamp", main, Some((main, Some("G1")))), "missing");
        assert_eq!(status("side.thing", main, Some((main, Some("S1")))), "hidden side");
        assert_eq!(status("side.thing", main, Some((main, Some("T0")))), "hidden side");
        assert_eq!(status("side.thing", main, Some((main, Some("T1")))), "visible");
        assert_eq!(status("res.partner", main, Some((main, Some("P1")))), "visible");
        assert_eq!(status("main.lamp", main, Some((main, Some("X0")))), "missing");
        assert_eq!(status("main.lamp", main, Some((main, Some("X")))), "visible");
        // No class introduces ghost.lamp, so Odoo can build none of its extensions.
        assert_eq!(status("ghost.lamp", main, None), "missing");
        assert_eq!(status("side.thing", main, None), "visible");
        assert_eq!(status("ghost.lamp", heir, Some((heir, Some("G")))), "missing");
        assert_eq!(status("ghost.lamp", main, Some((main, None))), "visible");
        assert_eq!(index.extender("ghost.lamp", main, None), Some("heir"));
        // `heir` loads after `main`, so its extension is not one `main` could have named.
        let g1 = Some((main, Some(class(main, "G1"))));
        assert_eq!(index.extender("ghost.lamp", main, g1), None);
        assert_eq!(
            index.extender("ghost.lamp", heir, Some((heir, Some(class(heir, "G"))))),
            Some("main")
        );
    }

    #[test]
    fn an_extender_whose_scope_may_hide_the_introducer_is_a_source() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let extends = |model: &str| python_18(&format!("class E(models.Model):\n    _inherit = '{model}'\n"));
        let mut modules = [
            module("base", &manifest("")),
            module("partial", &manifest("'base', 'unindexed'")),
            module("runtime", &manifest("'base'")),
            module("built_heir", &manifest("'runtime'")),
            module("unread", &manifest("'base'")),
            module("plain", &manifest("'base'")),
            module("main", &manifest("'base'")),
            module("over", &manifest("'partial'")),
        ];
        modules[1].python.push(extends("ghost.ticket"));
        modules[2].python.push(python_18(
            "class R(models.Model):\n    _name = 'ghost.kind.' + KIND\n\n\
             class E(models.Model):\n    _inherit = 'ghost.desk'\n",
        ));
        modules[3].python.push(extends("ghost.kind.lamp"));
        modules[3].python.push(extends("other.lamp"));
        modules[4].python.push(extends("ghost.note"));
        modules[4].unread_text = true;
        modules[5].python.push(extends("ghost.plain"));
        let index = Index::build(&modules, None);
        let status = |model: &str, mi: usize| {
            let reader = Some((mi, None));
            match index.model_status(model, &index.scope(mi), reader) {
                Status::Visible => "visible".to_owned(),
                Status::Hidden(m) => format!("hidden {m}"),
                Status::Missing => "missing".to_owned(),
            }
        };
        let (main, over) = (index.by_name["main"], index.by_name["over"]);
        assert_eq!(status("ghost.ticket", main), "hidden partial");
        assert_eq!(status("ghost.ticket", over), "visible");
        assert_eq!(status("ghost.desk", main), "hidden runtime");
        // A dependency's runtime name hides only the models it fits.
        assert_eq!(status("ghost.kind.lamp", main), "hidden built_heir");
        assert_eq!(status("other.lamp", main), "missing");
        assert_eq!(status("ghost.note", main), "hidden unread");
        assert_eq!(status("ghost.plain", main), "missing");
    }

    #[test]
    fn module_code_ignores_a_model_only_its_dependents_introduce() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = [
            module("base", &manifest("")),
            module("side", &manifest("'base'")),
            module("main", &manifest("'base'")),
            module("heir", &manifest("'main'")),
            module("kin", &manifest("'heir'")),
            module("aa", &manifest("'zz'")),
            module("zz", &manifest("'aa'")),
        ];
        let sources = [
            "class S(models.Model):\n    _name = 'shared.lamp'\n",
            "class S(models.Model):\n    _name = 'side.thing'\n",
            "class X0(models.Model):\n    _inherit = 'main.lamp'\n\n\
             class L(models.Model):\n    _name = 'main.lamp'\n",
            "class H(models.Model):\n    _name = 'heir.lamp'\n\n\
             class T(models.Model):\n    _name = 'twin.lamp'\n\n\
             class S(models.Model):\n    _name = 'shared.lamp'\n",
            "class D(models.Model):\n    _name = 'deep.lamp'\n\n\
             class T(models.Model):\n    _name = 'twin.lamp'\n",
            "class A(models.Model):\n    _name = 'aa.lamp'\n",
            "class Z(models.Model):\n    _name = 'zz.lamp'\n",
        ];
        for (module, source) in modules.iter_mut().zip(sources) {
            module.python.push(python_18(source));
        }
        let index = Index::build(&modules, None);
        let status = |model: &str, mi: usize, reader: Option<usize>| match index.model_status(
            model,
            &index.scope(mi),
            reader.map(|e| (e, None)),
        ) {
            Status::Visible => "visible".to_owned(),
            Status::Hidden(m) => format!("hidden {m}"),
            Status::Missing => "missing".to_owned(),
        };
        let [main, heir, aa, zz] = ["main", "heir", "aa", "zz"].map(|name| index.by_name[name]);
        // Without a class, the module's own introducer counts wherever it sits in the file.
        assert_eq!(status("main.lamp", main, Some(main)), "visible");
        assert_eq!(status("heir.lamp", main, Some(main)), "missing");
        assert_eq!(status("heir.lamp", main, None), "hidden heir");
        assert_eq!(index.dependent_introducer("heir.lamp", main), Some("heir"));
        assert_eq!(status("deep.lamp", main, Some(main)), "missing");
        assert_eq!(index.dependent_introducer("deep.lamp", main), Some("kin"));
        assert_eq!(status("deep.lamp", heir, Some(heir)), "missing");
        assert_eq!(index.dependent_introducer("deep.lamp", heir), Some("kin"));
        assert_eq!(status("twin.lamp", main, Some(main)), "missing");
        assert_eq!(index.dependent_introducer("twin.lamp", main), Some("heir"));
        assert_eq!(status("shared.lamp", main, Some(main)), "visible");
        assert_eq!(status("side.thing", main, Some(main)), "hidden side");
        assert_eq!(index.dependent_introducer("side.thing", main), None);
        // In a cycle, which is OD504's, `aa` loads first by name, so only `zz` counts as its dependent.
        assert_eq!(status("zz.lamp", aa, Some(aa)), "missing");
        assert_eq!(index.dependent_introducer("zz.lamp", aa), Some("zz"));
        assert_eq!(status("aa.lamp", zz, Some(zz)), "visible");
        assert_eq!(index.dependent_introducer("aa.lamp", zz), None);
    }

    #[test]
    fn a_field_source_prefers_a_module_that_loads_first() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = [
            module("base", &manifest("")),
            module("main", &manifest("'base'")),
            module("heir", &manifest("'main'")),
            module("side", &manifest("'base'")),
        ];
        let sources = [
            "class B(models.Model):\n    _name = 'res.partner'\n",
            "class L(models.Model):\n    _name = 'main.lamp'\n",
            "class L(models.Model):\n    _inherit = 'main.lamp'\n    glow = fields.Char()\n    shade = fields.Char()\n\n\
             class H(models.Model):\n    _name = 'heir.lamp'\n    wick = fields.Char()\n",
            "class L(models.Model):\n    _inherit = 'main.lamp'\n    shade = fields.Char()\n\n\
             class B(models.Model):\n    _inherit = 'base'\n    wick = fields.Char()\n",
        ];
        for (module, source) in modules.iter_mut().zip(sources) {
            module.python.push(python_18(source));
        }
        let index = Index::build(&modules, None);
        let main = index.by_name["main"];
        let scope = index.scope(main);
        let source = |model: &str, field: &str| index.field_source(model, field, &scope, main);
        assert_eq!(source("main.lamp", "glow"), Some(("heir", true)));
        assert_eq!(source("main.lamp", "shade"), Some(("side", false)));
        // `heir.lamp` is not there while `main` loads, whatever adds `wick` to `base`.
        assert_eq!(source("heir.lamp", "wick"), Some(("heir", true)));
        assert_eq!(source("main.lamp", "wick"), Some(("side", false)));
        assert_eq!(source("main.lamp", "flame"), None);
        assert!(matches!(index.field("main.lamp", "shade", &scope), Err(Some("heir"))));
    }

    #[test]
    fn a_hidden_field_names_the_model_s_own_module() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = [
            module("base", &manifest("")),
            module("main", &manifest("'base'")),
            module("side", &manifest("'base'")),
            module("deleg", &manifest("'base'")),
        ];
        let sources = [
            (
                2,
                "class T(models.Model):\n    _name = 'side.thing'\n    name = fields.Char()\n\n\
                 class B(models.Model):\n    _name = 'side.bare'\n",
            ),
            (
                3,
                "class D(models.Model):\n    _name = 'deleg.depot'\n    name = fields.Char()\n\n\
                 class B(models.AbstractModel):\n    _inherit = 'base'\n    _inherits = {'deleg.depot': 'x_depot_id'}\n",
            ),
        ];
        for (mi, source) in sources {
            modules[mi].python.push(python_18(source));
        }
        let index = Index::build(&modules, None);
        let main = index.by_name["main"];
        let scope = index.scope(main);
        // `base` delegates to `deleg.depot`, yet Odoo copies its `name` only to a model without one.
        assert!(matches!(index.field("side.thing", "name", &scope), Err(Some("side"))));
        assert_eq!(
            index.field_source("side.thing", "name", &scope, main),
            Some(("side", false))
        );
        assert!(matches!(index.field("side.bare", "name", &scope), Err(Some("deleg"))));
    }

    #[test]
    fn a_hidden_field_source_falls_back_to_a_delegated_parent_that_loads_first() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = [
            module("base", &manifest("")),
            module("main", &manifest("'base'")),
            module("side", &manifest("'base'")),
            module("heir", &manifest("'main', 'side'")),
            module("deleg", &manifest("'base'")),
        ];
        let sources = [
            (2, "class T(models.Model):\n    _name = 'side.thing'\n"),
            (
                3,
                "class T(models.Model):\n    _inherit = 'side.thing'\n    name = fields.Char()\n",
            ),
            (
                4,
                "class D(models.Model):\n    _name = 'deleg.depot'\n    name = fields.Char()\n\n\
                 class B(models.AbstractModel):\n    _inherit = 'base'\n    _inherits = {'deleg.depot': 'x_depot_id'}\n",
            ),
        ];
        for (mi, source) in sources {
            modules[mi].python.push(python_18(source));
        }
        let index = Index::build(&modules, None);
        let main = index.by_name["main"];
        // `heir` is absent while `main` loads, so `side.thing` then gets `name` from `deleg.depot`.
        assert_eq!(
            index.field_source("side.thing", "name", &index.scope(main), main),
            Some(("deleg", false))
        );
    }

    #[test]
    fn a_hidden_field_source_falls_back_to_a_delegated_parent_that_loads_after() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = [
            module("base", &manifest("")),
            module("main", &manifest("'base'")),
            module("heir", &manifest("'main'")),
            module("loose", &manifest("'base'")),
        ];
        let sources = [
            (
                2,
                "class D(models.Model):\n    _name = 'heir.depot'\n    name = fields.Char()\n\n\
                 class T(models.Model):\n    _name = 'heir.thing'\n    _inherits = {'heir.depot': 'x_depot_id'}\n",
            ),
            (
                3,
                "class B(models.AbstractModel):\n    _inherit = 'base'\n    name = fields.Char()\n",
            ),
        ];
        for (mi, source) in sources {
            modules[mi].python.push(python_18(source));
        }
        let index = Index::build(&modules, None);
        let main = index.by_name["main"];
        // Only `heir` introduces `heir.thing`, so no module `main` may depend on fixes the read.
        assert_eq!(
            index.field_source("heir.thing", "name", &index.scope(main), main),
            Some(("heir", true))
        );
    }

    #[test]
    fn base_code_ignores_a_model_a_module_loading_after_it_introduces() {
        // Below saas~18.2 neither `via`, through a dependency naming no series, nor `aaa`, with no
        // `depends`, reaches `base`, yet Odoo loads `base` alone before both.
        let mut modules = [
            module("base", "{}"),
            module("lib", "{'version': '1.0'}"),
            module("via", "{'version': '18.0.1.0', 'depends': ['lib']}"),
            module("aaa", "{'version': '18.0.1.0'}"),
            module("side", "{'version': '18.0.1.0', 'depends': ['base']}"),
            module("all", "{'version': '18.0.1.0', 'auto_install': True}"),
        ];
        let sources = [
            (2, "class V(models.Model):\n    _name = 'via.lamp'\n"),
            (3, "class A(models.Model):\n    _name = 'aaa.lamp'\n"),
            (5, "class L(models.Model):\n    _name = 'all.lamp'\n"),
        ];
        for (mi, source) in sources {
            modules[mi].python.push(python_18(source));
        }
        for series in [None, Series::parse("18.0")] {
            let index = Index::build(&modules, series);
            let status = |model: &str, mi: usize, reader: Option<usize>| match index.model_status(
                model,
                &index.scope(mi),
                reader.map(|e| (e, None)),
            ) {
                Status::Visible => "visible".to_owned(),
                Status::Hidden(m) => format!("hidden {m}"),
                Status::Missing => "missing".to_owned(),
            };
            let [base, side] = ["base", "side"].map(|name| index.by_name[name]);
            assert_eq!(status("via.lamp", base, Some(base)), "missing");
            assert_eq!(index.dependent_introducer("via.lamp", base), Some("via"));
            assert_eq!(status("aaa.lamp", base, Some(base)), "missing");
            assert_eq!(index.dependent_introducer("aaa.lamp", base), Some("aaa"));
            assert_eq!(status("via.lamp", base, None), "hidden via");
            assert_eq!(status("via.lamp", side, Some(side)), "hidden via");
            // Always installed, `all` is in the scope of `base`, but still loads after it.
            assert!(index.scope(base).sees(index.by_name["all"]));
            assert_eq!(status("all.lamp", base, Some(base)), "missing");
            assert_eq!(index.dependent_introducer("all.lamp", base), Some("all"));
            assert_eq!(status("all.lamp", side, Some(side)), "visible");
        }
    }

    #[test]
    fn a_spec_locates_in_a_set_that_extends_its_tree_unread() {
        let view = |record: &str| {
            data_file(
                "views/views.xml",
                xml::analyze(&format!("<odoo>{record}</odoo>"), Series::parse("18.0")),
            )
        };
        let extension = |id: &str, arch: &str| {
            format!(
                r#"<record id="{id}" model="ir.ui.view">
                    <field name="model">res.partner</field>
                    <field name="inherit_id" ref="base.view_partner_simple"/>
                    {arch}
                </record>"#
            )
        };
        let adds = |field: &str, id: &str| {
            let spec = format!(r#"<xpath expr="//div[@id='notes']" position="after"><div id="{id}"/></xpath>"#);
            format!(r#"<field name="{field}" type="xml">{spec}</field>"#)
        };
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        // Whether main's spec on `haze_block` locates in `member`, haze writing `haze_arch` and
        // loading `files`.
        let files_locate = |haze_arch: &str, files: Vec<SourceFile<crate::facts::DataFacts>>, member: &str| {
            let mut modules = [
                module("base", &manifest("")),
                module("haze", &manifest("'base'")),
                module("veil", &manifest("'base'")),
                module("main", &manifest("'haze', 'veil'")),
            ];
            modules[0].data.push(view(
                r#"<record id="view_partner_simple" model="ir.ui.view">
                    <field name="model">res.partner</field>
                    <field name="arch" type="xml"><form><div id="notes"/></form></field>
                </record>"#,
            ));
            modules[1].data.push(view(&extension("view_haze", haze_arch)));
            modules[1].data.extend(files);
            modules[2]
                .data
                .push(view(&extension("view_veil", &adds("arch", "veil_block"))));
            let spec = r#"<xpath expr="//div[@id='haze_block']" position="inside"><span/></xpath>"#;
            let spec = format!(r#"<field name="arch" type="xml">{spec}</field>"#);
            modules[3].data.push(view(&extension("view_main", &spec)));
            let index = Index::build(&modules, None);
            let main = index.by_name["main"];
            let set: Vec<bool> = modules.iter().map(|m| m.name == member).collect();
            index.locates_in(main, &set, &index.scope(main))
        };
        // The same, haze loading a file at `lost` the scan cannot read.
        let locates = |haze_arch: &str, lost: Option<&str>, member: &str| {
            let unreadable = || crate::facts::DataFacts {
                unreadable: true,
                ..Default::default()
            };
            files_locate(
                haze_arch,
                lost.map(|rel| data_file(rel, unreadable())).into_iter().collect(),
                member,
            )
        };
        assert!(locates(&adds("arch_base", "haze_block"), None, "haze"));
        assert!(!locates(&adds("arch_base", "haze_block"), None, "veil"));
        assert!(!locates(&adds("arch", "haze_other"), None, "haze"));
        let cdata = r#"<![CDATA[<xpath expr="//div[@id='notes']" position="after"><div id="haze_block"/></xpath>]]>"#;
        for arch in [
            format!(r#"<field name="arch">{cdata}</field>"#),
            r#"<field name="arch" eval="'&lt;form/&gt;'"/>"#.to_owned(),
            r#"<field name="arch" file="haze/views/haze_arch.xml"/>"#.to_owned(),
        ] {
            assert!(locates(&arch, None, "haze"), "{arch}");
        }
        // Under `type="xml"`, Odoo drops the text and writes an empty arch.
        let typed = format!(r#"<field name="arch" type="xml">{cdata}</field>"#);
        assert!(!locates(&typed, None, "haze"));
        // A record that writes no arch adds nothing.
        assert!(!locates("", None, "haze"));
        assert!(!locates(
            &adds("arch", "haze_other"),
            Some("data/res.partner.csv"),
            "haze"
        ));
        assert!(locates(
            &adds("arch", "haze_other"),
            Some("data/ir.ui.view.csv"),
            "haze"
        ));
        assert!(locates(&adds("arch", "haze_other"), Some("views/lost.xml"), "haze"));
        // A readable `ir.ui.view` CSV holds its arch in a cell the scan does not parse.
        let csv = |rel: &str, model: &str, header: &str, row: &str| {
            let text = format!("{header}\n{row}\n");
            vec![data_file(rel, crate::csvdata::analyze(&text, model, &[]))]
        };
        let arch = r#""<xpath expr=""//div[@id='notes']"" position=""after""><div id=""haze_block""/></xpath>""#;
        let row = format!("view_haze_csv,res.partner,base.view_partner_simple,{arch}");
        let header = "id,model,inherit_id:id,arch";
        let view_csv = || csv("data/ir.ui.view.csv", "ir.ui.view", header, &row);
        assert!(files_locate("", view_csv(), "haze"));
        assert!(!files_locate("", view_csv(), "veil"));
        assert!(!files_locate(
            "",
            csv("data/res.partner.csv", "res.partner", header, &row),
            "haze"
        ));
        let by_name = csv(
            "data/ir.ui.view.csv",
            "ir.ui.view",
            "id,model,inherit_id,arch",
            &format!("view_haze_csv,res.partner,res.partner.simple,{arch}"),
        );
        assert!(files_locate("", by_name, "haze"));
    }

    #[test]
    fn a_spec_locates_in_a_set_with_a_view_of_an_unread_parent() {
        let view = |record: &str| {
            data_file(
                "views/views.xml",
                xml::analyze(&format!("<odoo>{record}</odoo>"), Series::parse("18.0")),
            )
        };
        let record = |id: &str, model: &str, fields: &str, arch: &str| {
            format!(
                r#"<record id="{id}" model="ir.ui.view">
                    <field name="model" {model}/>{fields}
                    <field name="arch" type="xml">{arch}</field>
                </record>"#
            )
        };
        let search = r#"<field name="inherit_id" search="[('name', '=', 'res.partner.simple')]"/>"#;
        let by_ref = r#"<field name="inherit_id" ref="base.view_partner_simple"/>"#;
        let partner = r#"eval="'res.partner'""#;
        let country = r#"eval="'res.country'""#;
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let notes = r#"<form><div id="notes"/></form>"#;
        let parent = record("view_partner_simple", partner, "", notes);
        // Whether main's spec on `haze_block`, main naming `main_model`, locates in haze, which loads
        // `haze`, base loading `base`.
        let run = |base: &str, haze: &str, main_model: &str| {
            let mut modules = [
                module("base", &manifest("")),
                module("haze", &manifest("'base'")),
                module("main", &manifest("'haze'")),
            ];
            modules[0].data.push(view(base));
            modules[1].data.push(view(haze));
            let spec = r#"<xpath expr="//div[@id='haze_block']" position="inside"><span/></xpath>"#;
            modules[2]
                .data
                .push(view(&record("view_main", main_model, by_ref, spec)));
            let index = Index::build(&modules, None);
            let main = index.by_name["main"];
            let set: Vec<bool> = modules.iter().map(|m| m.name == "haze").collect();
            index.locates_in(main, &set, &index.scope(main))
        };
        let adds = |id: &str| format!(r#"<xpath expr="//div[@id='notes']" position="after"><div id="{id}"/></xpath>"#);
        // The same, haze's view naming `haze_model` and writing `haze_fields`, adding `haze_id`.
        let locates = |haze_model: &str, haze_fields: &str, haze_id: &str, main_model: &str| {
            run(
                &parent,
                &record("view_haze", haze_model, haze_fields, &adds(haze_id)),
                main_model,
            )
        };
        assert!(locates(partner, search, "haze_block", partner));
        assert!(!locates(partner, by_ref, "haze_other", partner));
        assert!(!locates(country, search, "haze_block", partner));
        assert!(locates(r#"eval="'res.' + 'country'""#, search, "haze_block", partner));
        let primary = format!(r#"{search}<field name="mode">primary</field>"#);
        assert!(!locates(partner, &primary, "haze_block", partner));
        let text = r#"<field name="inherit_id">42</field>"#;
        assert!(locates(partner, text, "haze_block", partner));
        assert!(locates(country, search, "haze_block", r#"eval="'res.' + 'partner'""#));
        // Naming no model, haze's view updates base's in place, which keeps its model.
        let update = format!(
            r#"<record id="base.view_kept" model="ir.ui.view">{search}
                <field name="arch" type="xml">{}</field>
            </record>"#,
            adds("haze_block")
        );
        let kept = |model: &str| record("view_kept", model, "", r#"<form><div id="kept"/></form>"#);
        assert!(run(&format!("{parent}{}", kept(partner)), &update, partner));
        assert!(!run(&format!("{parent}{}", kept(country)), &update, partner));
        // Under a parent whose arch the scan does not read, the update in place still keeps the model.
        let unread = parent.replace(r#"name="arch""#, r#"name="arch_base""#);
        let update = update.replace(search, by_ref);
        assert!(run(&format!("{unread}{}", kept(partner)), &update, partner));
        assert!(!run(&format!("{unread}{}", kept(country)), &update, partner));
    }

    #[test]
    fn a_tree_extended_unread_in_scope() {
        let view = |record: &str| {
            data_file(
                "views/views.xml",
                xml::analyze(&format!("<odoo>{record}</odoo>"), Series::parse("18.0")),
            )
        };
        let record = |id: &str, model: &str, fields: &str| {
            format!(
                r#"<record id="{id}" model="ir.ui.view">
                    <field name="model" {model}/>{fields}
                </record>"#
            )
        };
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let by_ref = r#"<field name="inherit_id" ref="base.view_partner_simple"/>"#;
        let search = r#"<field name="inherit_id" search="[('name', '=', 'res.partner.simple')]"/>"#;
        let partner = r#"eval="'res.partner'""#;
        let country = r#"eval="'res.country'""#;
        // An arch written to `field` adding `block` after the element of id `after`.
        let arch = |field: &str, after: &str, block: &str| {
            format!(
                r#"<field name="{field}" type="xml"><xpath expr="//div[@id='{after}']" position="after"><div id="{block}"/></xpath></field>"#
            )
        };
        let notes = r#"<field name="arch" type="xml"><form><div id="notes"/></form></field>"#;
        // Whether views of haze writing `haze` may add the element of main's spec to main's tree in
        // main's scope, main naming `main_model` and depending on `main_depends`.
        let extends = |haze: &str, main_model: &str, main_depends: &str| {
            let mut modules = [
                module("base", &manifest("")),
                module("haze", &manifest("'base'")),
                module("main", &manifest(main_depends)),
            ];
            modules[0]
                .data
                .push(view(&record("view_partner_simple", partner, notes)));
            modules[1].data.push(view(haze));
            let spec = r#"<field name="arch" type="xml"><xpath expr="//div[@id='haze_block']" position="inside"><span/></xpath></field>"#;
            modules[2]
                .data
                .push(view(&record("view_main", main_model, &format!("{by_ref}{spec}"))));
            let index = Index::build(&modules, None);
            let main = index.by_name["main"];
            let v = &modules[2].data[0].facts.views[0];
            let e = super::located(&v.locators[0].value, "main");
            let parent = "base.view_partner_simple".to_owned();
            let root = index.views.root(parent.clone()).unwrap();
            let extra = index.extra_elements(v, parent, &root, &index.scope(main));
            extra.is_none_or(|extra| extra.contains(&e))
        };
        let haze = |model: &str, fields: &str| record("view_haze", model, fields);
        let below = |field: &str| haze(partner, &format!("{by_ref}{}", arch(field, "notes", "haze_block")));
        assert!(extends(&below("arch_base"), partner, "'haze'"));
        assert!(!extends(&below("arch_base"), partner, "'base'"));
        assert!(!extends(&below("arch"), partner, "'haze'"));
        let stray = |model: &str, mode: &str, block: &str| {
            haze(model, &format!("{search}{mode}{}", arch("arch", "notes", block)))
        };
        assert!(extends(&stray(partner, "", "haze_block"), partner, "'haze'"));
        assert!(!extends(&stray(partner, "", "mist_block"), partner, "'haze'"));
        assert!(!extends(&stray(country, "", "haze_block"), partner, "'haze'"));
        let primary = r#"<field name="mode">primary</field>"#;
        assert!(!extends(&stray(partner, primary, "haze_block"), partner, "'haze'"));
        assert!(extends(
            &stray(country, "", "haze_block"),
            r#"eval="'res.' + 'partner'""#,
            "'haze'"
        ));
        // A view below the stray joins the tree with it.
        let low = |field: &str, block: &str| {
            let by_stray = r#"<field name="inherit_id" ref="view_haze"/>"#;
            let fields = format!("{by_stray}{}", arch(field, "mist_block", block));
            format!(
                "{}{}",
                stray(partner, "", "mist_block"),
                record("view_haze_low", partner, &fields)
            )
        };
        assert!(extends(&low("arch", "haze_block"), partner, "'haze'"));
        assert!(!extends(&low("arch", "dusk_block"), partner, "'haze'"));
        assert!(extends(&low("arch_base", "dusk_block"), partner, "'haze'"));
        // A stray without an XML id has no view below it, so only its own arch counts.
        let anonymous = |field: &str, block: &str| {
            format!(
                r#"<record model="ir.ui.view"><field name="model" {partner}/>{search}{}</record>"#,
                arch(field, "notes", block)
            )
        };
        assert!(extends(&anonymous("arch", "haze_block"), partner, "'haze'"));
        assert!(!extends(&anonymous("arch", "mist_block"), partner, "'haze'"));
        assert!(extends(&anonymous("arch_base", "mist_block"), partner, "'haze'"));
    }

    #[test]
    fn a_view_with_an_unread_parent_roots_no_tree() {
        let manifest = "{'version': '18.0.1.0', 'depends': []}";
        let arch = r#"<field name="arch" type="xml"><form/></field>"#;
        let roots = |mid_parent: &str, top: &str| {
            let records = format!(
                r#"{top}<record id="view_mid" model="ir.ui.view">
                    <field name="model">res.partner</field>
                    <field name="inherit_id" {mid_parent}/>
                    {arch}
                </record>
                <record id="view_low" model="ir.ui.view">
                    <field name="inherit_id" ref="view_mid"/>
                    {arch}
                </record>"#
            );
            let mut modules = [module("m", manifest)];
            let facts = xml::analyze(&format!("<odoo>{records}</odoo>"), Series::parse("18.0"));
            modules[0].data.push(data_file("views/views.xml", facts));
            let index = Index::build(&modules, None);
            ["m.view_mid", "m.view_low"].map(|id| index.views.root(id.to_owned()))
        };
        assert_eq!(roots(r#"search="[('name', '=', 'x')]""#, ""), [None, None]);
        let top = format!(
            r#"<record id="view_top" model="ir.ui.view"><field name="model">res.partner</field>{arch}</record>"#
        );
        let held = roots(r#"ref="view_top""#, &top);
        assert_eq!(held[1].as_deref(), Some("m.view_top"));
    }

    #[test]
    fn the_records_of_a_view_fold_to_the_mode_odoo_stores() {
        // The first record loads as data of module `first`, and the later ones at `(module, demo)`;
        // the id names module 0.
        let fold_from = |first: usize, (module, demo): (usize, bool), records: &[&str]| {
            let records: String = records
                .iter()
                .map(|fields| format!(r#"<record id="v" model="ir.ui.view">{fields}</record>"#))
                .collect();
            let facts = xml::analyze(&format!("<odoo>{records}</odoo>"), Series::parse("18.0"));
            let placed: Vec<_> = facts
                .views
                .iter()
                .enumerate()
                .map(|(at, fact)| Placed {
                    module: if at == 0 { first } else { module },
                    demo: at != 0 && demo,
                    fact,
                })
                .collect();
            stored_primary(&placed, Some(0))
        };
        let fold_at = |later: (usize, bool), records: &[&str]| fold_from(0, later, records);
        let fold = |records: &[&str]| fold_at((0, false), records);
        let parent = r#"<field name="inherit_id" ref="p"/>"#;
        let other = r#"<field name="inherit_id" ref="q"/>"#;
        let mode = r#"<field name="mode">primary</field>"#;
        let key = r#"<field name="key">m.v</field>"#;
        let search = r#"<field name="inherit_id" search="[('name', '=', 'p')]"/>"#;
        let falsy = r#"<field name="inherit_id" eval="False"/>"#;
        assert_eq!(fold(&["", parent]), Some(false), "a primary view later given a parent");
        assert_eq!(
            fold(&[&format!("{parent}{mode}"), other]),
            Some(true),
            "a clone re-parented"
        );
        assert_eq!(
            fold(&[parent, falsy]),
            Some(true),
            "an extension later given a falsy parent"
        );
        assert_eq!(fold(&[parent, key]), Some(false), "an extension later given a key");
        assert_eq!(
            fold(&[mode, parent]),
            Some(true),
            "an update writes the mode again onto a view that keeps its parent"
        );
        assert_eq!(fold(&[parent, mode]), Some(true), "an extension later given a mode");
        assert_eq!(
            fold(&["", search]),
            None,
            "a primary view later given a searched parent"
        );
        assert_eq!(
            fold(&[&format!("{search}{mode}"), parent]),
            Some(true),
            "a searched parent counts as one"
        );
        assert_eq!(
            fold_at((1, false), &["", parent]),
            Some(true),
            "a record of another module keeps the view primary"
        );
        assert_eq!(
            fold_at((0, true), &["", parent]),
            Some(true),
            "a demo record keeps the view primary"
        );
        assert_eq!(
            fold_at((1, false), &["", search]),
            None,
            "a record of another module may leave the mode unknown"
        );
        assert_eq!(
            fold_at((1, false), &[parent, mode]),
            Some(true),
            "a record of another module can make the view primary"
        );
        assert_eq!(
            fold_from(1, (0, false), &[parent, ""]),
            Some(true),
            "a record of another module loading first still loads after the view's own"
        );
        assert_eq!(
            fold_from(1, (1, false), &[parent, key]),
            Some(true),
            "records of other modules alone never take the type away"
        );
    }

    #[test]
    fn an_extension_of_an_unindexed_parent_made_primary_serves_any_type() {
        let mut modules = [
            module("a", "{'version': '18.0.1.0', 'depends': []}"),
            module("b", "{'version': '18.0.1.0', 'depends': ['a']}"),
        ];
        let extension = r#"<odoo><record id="ext" model="ir.ui.view">
            <field name="model">res.partner</field>
            <field name="inherit_id" ref="ghost.v"/>
            <field name="arch" type="xml"><xpath expr="." position="inside"/></field>
        </record></odoo>"#;
        let raised =
            r#"<odoo><record id="a.ext" model="ir.ui.view"><field name="mode">primary</field></record></odoo>"#;
        modules[0].data.push(data_file(
            "views/views.xml",
            xml::analyze(extension, Series::parse("18.0")),
        ));
        modules[1].data.push(data_file(
            "views/views.xml",
            xml::analyze(raised, Series::parse("18.0")),
        ));
        let index = Index::build(&modules, None);
        assert!(index.view_types.contains(&("res.partner", None)));
        modules[1].data.clear();
        assert!(Index::build(&modules, None).view_types.is_empty());
    }

    #[test]
    fn a_typed_clone_of_an_unindexed_parent_serves_its_written_type() {
        let types = |typed: &str| {
            let mut modules = [module("a", "{'version': '18.0.1.0', 'depends': []}")];
            let clone = format!(
                r#"<odoo><record id="clone" model="ir.ui.view">
                <field name="model">res.partner</field>
                <field name="inherit_id" ref="ghost.v"/>
                <field name="mode">primary</field>{typed}
                <field name="arch" type="xml"><xpath expr="." position="inside"/></field>
            </record></odoo>"#
            );
            modules[0].data.push(data_file(
                "views/views.xml",
                xml::analyze(&clone, Series::parse("18.0")),
            ));
            let index = Index::build(&modules, None);
            let owned = index
                .view_types
                .iter()
                .map(|&(m, t)| (m.to_owned(), t.map(str::to_owned)));
            owned.collect::<Vec<_>>()
        };
        let partner = |t: Option<&str>| vec![("res.partner".to_owned(), t.map(str::to_owned))];
        assert_eq!(types(r#"<field name="type">gantt</field>"#), partner(Some("gantt")));
        assert_eq!(types(""), partner(None));
    }

    #[test]
    fn a_view_only_another_module_updates_roots_no_tree() {
        let build = |update: &str| {
            let mut modules = [
                module("a", "{'version': '18.0.1.0', 'depends': []}"),
                module("b", "{'version': '18.0.1.0', 'depends': ['a']}"),
            ];
            let root = r#"<odoo><record id="root" model="ir.ui.view">
                <field name="model">res.country</field>
                <field name="arch" type="xml"><gantt/></field>
            </record></odoo>"#;
            let records = format!(
                r#"<odoo><record id="ghost.v" model="ir.ui.view">{update}</record>
                <record id="clone" model="ir.ui.view">
                    <field name="model">res.partner</field>
                    <field name="inherit_id" ref="ghost.v"/>
                    <field name="mode">primary</field>
                    <field name="arch" type="xml"><xpath expr="." position="inside"/></field>
                </record></odoo>"#
            );
            modules[0]
                .data
                .push(data_file("views/views.xml", xml::analyze(root, Series::parse("18.0"))));
            modules[1].data.push(data_file(
                "views/views.xml",
                xml::analyze(&records, Series::parse("18.0")),
            ));
            modules
        };
        let keyed = build(r#"<field name="key">ghost.v</field>"#);
        let index = Index::build(&keyed, None);
        assert!(index.view_types.contains(&("res.partner", None)));
        let rehomed = build(r#"<field name="inherit_id" ref="a.root"/>"#);
        let index = Index::build(&rehomed, None);
        assert!(index.view_types.contains(&("res.partner", Some("gantt"))));
        assert_eq!(index.view_kind("ghost.v".to_owned()), Some("gantt"));
    }

    #[test]
    fn a_module_loading_before_the_view_it_writes_never_takes_its_type_away() {
        // `a_fix` does not depend on `b`, so it sorts first, though Odoo loads it after `b`.
        let mut modules = [
            module("b", "{'version': '18.0.1.0', 'depends': []}"),
            module("a_fix", "{'version': '18.0.1.0', 'depends': []}"),
        ];
        let views = r#"<odoo><record id="view_g" model="ir.ui.view">
            <field name="model">res.partner</field>
            <field name="arch" type="xml"><gantt/></field>
        </record><record id="other" model="ir.ui.view">
            <field name="model">res.country</field>
            <field name="arch" type="xml"><gantt/></field>
        </record></odoo>"#;
        let fix = r#"<odoo><record id="b.view_g" model="ir.ui.view">
            <field name="inherit_id" ref="b.other"/>
        </record></odoo>"#;
        modules[0]
            .data
            .push(data_file("views/views.xml", xml::analyze(views, Series::parse("18.0"))));
        modules[1]
            .data
            .push(data_file("views/fix.xml", xml::analyze(fix, Series::parse("18.0"))));
        let index = Index::build(&modules, None);
        assert!(index.view_types.contains(&("res.partner", Some("gantt"))));
    }

    #[test]
    fn a_view_with_an_arch_the_scan_does_not_read_roots_no_tree() {
        let manifest = "{'version': '18.0.1.0', 'depends': []}";
        let root = |top_arch: &str| {
            let records = format!(
                r#"<record id="view_top" model="ir.ui.view">
                    <field name="model">res.partner</field>
                    {top_arch}
                </record>
                <record id="view_low" model="ir.ui.view">
                    <field name="inherit_id" ref="view_top"/>
                    <field name="arch" type="xml"><form/></field>
                </record>"#
            );
            let mut modules = [module("m", manifest)];
            let facts = xml::analyze(&format!("<odoo>{records}</odoo>"), Series::parse("18.0"));
            modules[0].data.push(data_file("views/views.xml", facts));
            Index::build(&modules, None).views.root("m.view_low".to_owned())
        };
        for top_arch in [
            r#"<field name="arch"><![CDATA[<form/>]]></field>"#,
            r#"<field name="arch" eval="'&lt;form/&gt;'"/>"#,
            r#"<field name="arch" file="m/views/top_arch.xml"/>"#,
        ] {
            assert_eq!(root(top_arch), None, "{top_arch}");
        }
        let read = root(r#"<field name="arch" type="xml"><form/></field>"#);
        assert_eq!(read.as_deref(), Some("m.view_top"));
    }

    #[test]
    fn a_spec_on_a_rootless_tree_locates_in_a_set_with_a_view_of_its_model() {
        let view = |records: &str| {
            data_file(
                "views/views.xml",
                xml::analyze(&format!("<odoo>{records}</odoo>"), Series::parse("18.0")),
            )
        };
        let record = |id: &str, model: &str, parent: &str, arch: &str| {
            let model = format!(r#"<field name="model">{model}</field>"#);
            let parent = format!(r#"<field name="inherit_id" ref="{parent}"/>"#);
            format!(r#"<record id="{id}" model="ir.ui.view">{model}{parent}{arch}</record>"#)
        };
        let spec = r#"<xpath expr="//div[@id='prov_block']" position="inside"><span/></xpath>"#;
        let arch = format!(r#"<field name="arch" type="xml">{spec}</field>"#);
        let partner = record("view_prov", "res.partner", "base.view_other", &arch);
        let country = record("view_prov_country", "res.country", "base.view_other", &arch);
        let unread = format!(r#"<field name="arch_base" type="xml">{spec}</field>"#);
        // Base holds the top of each tree, with an arch the scan does not read.
        let base = format!(
            r#"<record id="view_top" model="ir.ui.view"><field name="model">res.partner</field>{unread}</record>
            <record id="page_top" model="ir.ui.view">{unread}</record>"#
        );
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        // Whether main's spec, `main_records` holding it, locates in prov, which loads `prov_file`
        // and a file at `lost` the scan cannot read.
        let locates_from = |main_records: &str, prov_file: SourceFile<crate::facts::DataFacts>, lost: Option<&str>| {
            let mut modules = [
                module("base", &manifest("")),
                module("prov", &manifest("'base'")),
                module("main", &manifest("'prov'")),
            ];
            modules[0].data.push(view(&base));
            modules[1].data.push(prov_file);
            let unreadable = crate::facts::DataFacts {
                unreadable: true,
                ..Default::default()
            };
            modules[1].data.extend(lost.map(|rel| data_file(rel, unreadable)));
            modules[2].data.push(view(main_records));
            let index = Index::build(&modules, None);
            let main = index.by_name["main"];
            let set: Vec<bool> = modules.iter().map(|m| m.name == "prov").collect();
            index.locates_in(main, &set, &index.scope(main))
        };
        let locates =
            |main_records: &str, prov_records: &str, lost| locates_from(main_records, view(prov_records), lost);
        let held = record("view_main", "res.partner", "base.view_top", &arch);
        assert!(locates(&held, &partner, None));
        assert!(!locates(&held, &country, None));
        assert!(locates(&held, &country, Some("views/lost.xml")));
        assert!(!locates(&held, &country, Some("data/res.partner.csv")));
        // Odoo loads a file under `data/template/` though no manifest lists it.
        let unlisted = SourceFile {
            rel: "data/template/views.xml".to_owned(),
            rank: None,
            ..view(&partner)
        };
        assert!(locates_from(&held, unlisted, None));
        // The tree's model comes from the views on the way up, which a model-less spec leaves to them.
        let main = |mid: String| {
            let main = r#"<record id="view_main" model="ir.ui.view"><field name="inherit_id" ref="view_mid"/>"#;
            format!("{mid}{main}{arch}</record>")
        };
        let below = main(record("view_mid", "res.partner", "base.view_top", &unread));
        assert!(locates(&below, &partner, None));
        assert!(!locates(&below, &country, None));
        // Above a view the index does not hold, or a parent it does not read, any model may sit, such
        // as that of the view a primary view of another model extends.
        let undefined = record("view_main", "res.partner", "base.view_missing", &arch);
        assert!(locates(&undefined, &country, None));
        let primary = format!(r#"<field name="mode">primary</field>{arch}"#);
        let gap = main(record("view_mid", "res.country", "base.view_missing", &primary));
        assert!(locates(&gap, &partner, None));
        let opaque = record("view_mid", "res.partner", "base.view_top", &unread)
            .replace(r#"ref="base.view_top""#, r#"eval="ref('base.view_top')""#);
        assert!(locates(&main(opaque), &country, None));
        // Odoo resolves the parent at load, so a read arch does not make the view a root.
        let evaluated = record("view_mid", "res.partner", "base.view_top", &arch)
            .replace(r#"ref="base.view_top""#, r#"eval="ref('base.view_top')""#);
        assert!(locates(&main(evaluated), &country, None));
        // A record updating in place a view of a module the scan does not index names no model or
        // parent, so the view may sit below one of any model.
        let update =
            r#"<record id="product.view_normal" model="ir.ui.view"><field name="active" eval="True"/></record>"#;
        let updated = format!(
            "{update}{}",
            record("view_main", "res.partner", "product.view_normal", &arch)
        );
        assert!(locates(&updated, &country, None));
        // A QWeb template has no model, so it takes only the extensions of views without one.
        let template = format!(r#"<template id="main_page" inherit_id="base.page_top">{spec}</template>"#);
        let page = format!(r#"<template id="prov_page" inherit_id="base.other_page">{spec}</template>"#);
        assert!(locates(&template, &page, None));
        assert!(!locates(&template, &partner, None));
        // A model only a run can tell may be any, on prov's view, on the way up or on main's own.
        let literal = r#"<field name="model">res.country</field>"#;
        let prov_by = |model: &str| {
            let prov = country.replacen(literal, &format!(r#"<field name="model" {model}/>"#), 1);
            locates(&held, &prov, None)
        };
        assert!(prov_by(r#"eval="'res.' + 'country'""#));
        assert!(prov_by(r#"ref="model_res_country""#));
        assert!(!prov_by(r#"eval="'res.country'""#));
        let partner_model = r#"<field name="model">res.partner</field>"#;
        let computed =
            |records: String| records.replacen(partner_model, r#"<field name="model" eval="'res.' + 'partner'"/>"#, 1);
        let mid = computed(record("view_mid", "res.partner", "base.view_top", &unread));
        assert!(locates(&main(mid), &country, None));
        assert!(locates(&computed(held.clone()), &country, None));
    }

    #[test]
    fn a_view_whose_module_loses_a_view_file_has_no_models() {
        let view = |records: &str| {
            data_file(
                "views/views.xml",
                xml::analyze(&format!("<odoo>{records}</odoo>"), Series::parse("18.0")),
            )
        };
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let country = r#"<record id="view_prov" model="ir.ui.view"><field name="model">res.country</field>
            <field name="arch" type="xml"><form><div id="prov_block"/></form></field></record>"#;
        // The defining record of `view_mid` may sit in the lost file, so this one may only update it.
        let main = r#"<record id="view_mid" model="ir.ui.view"><field name="priority">20</field></record>
            <record id="view_main" model="ir.ui.view">
                <field name="model">res.partner</field>
                <field name="inherit_id" ref="view_mid"/>
                <field name="arch" type="xml">
                    <xpath expr="//div[@id='prov_block']" position="inside"><span/></xpath>
                </field>
            </record>"#;
        // The models above main's spec, and whether it locates in prov, main losing a file at `lost`.
        let scan = |lost: Option<&str>| {
            let mut modules = [
                module("base", &manifest("")),
                module("prov", &manifest("'base'")),
                module("main", &manifest("'prov'")),
            ];
            modules[1].data.push(view(country));
            modules[2].data.push(view(main));
            let unreadable = crate::facts::DataFacts {
                unreadable: true,
                ..Default::default()
            };
            modules[2].data.extend(lost.map(|rel| data_file(rel, unreadable)));
            let index = Index::build(&modules, None);
            let main = index.by_name["main"];
            let set: Vec<bool> = modules.iter().map(|m| m.name == "prov").collect();
            let models = index.views.models("main.view_mid".to_owned(), &modules);
            let models = models.map(|m| m.into_iter().map(|m| m.map(str::to_owned)).collect::<Vec<_>>());
            (models, index.locates_in(main, &set, &index.scope(main)))
        };
        assert_eq!(scan(Some("views/lost.xml")), (None, true));
        assert_eq!(scan(Some("data/res.partner.csv")), (Some(vec![None]), false));
        assert_eq!(scan(None), (Some(vec![None]), false));
    }

    #[test]
    fn a_template_spec_locates_in_a_set_that_loads_an_unreadable_template() {
        let file = |text: &str| template_file("static/src/a.xml", text);
        let manifest = |name: &str, depends: &str, bundled: bool| {
            let assets = if bundled {
                format!("'assets': {{'web.assets_backend': ['{name}/static/src/*.xml']}}")
            } else {
                String::new()
            };
            format!("{{'version': '18.0.1.0.0', 'depends': [{depends}], {assets}}}")
        };
        let extension =
            |parent: &str, spec: &str| format!(r#"<t t-inherit="{parent}" t-inherit-mode="extension">{spec}</t>"#);
        // Whether main's spec on `o_smudge_panel` locates in `member`, smudge's file bundled or not
        // and naming `parent`.
        let locates = |member: &str, bundled: bool, parent: &str| {
            let mut modules = [
                module("web", &manifest("web", "", true)),
                module("smudge", &manifest("smudge", "'web'", bundled)),
                module("spark", &manifest("spark", "'web'", true)),
                module("main", &manifest("main", "'smudge', 'spark'", true)),
            ];
            let root = r#"<templates><t t-name="web.Root"><div class="o_root"/></t></templates>"#;
            modules[0].templates.push(file(root));
            let unclosed = r#"<xpath expr="//div" position="inside">"#;
            modules[1].templates.push(file(&extension(parent, unclosed)));
            let adds = r#"<xpath expr="//div[hasclass('o_root')]" position="inside">"#;
            let adds = format!(r#"{adds}<div class="o_spark_panel"/></xpath>"#);
            modules[2].templates.push(file(&extension("web.Root", &adds)));
            let spec = r#"<xpath expr="//div[hasclass('o_smudge_panel')]" position="inside"><span/></xpath>"#;
            modules[3].templates.push(file(&extension("web.Root", spec)));
            let index = Index::build(&modules, None);
            let main = index.by_name["main"];
            let set: Vec<bool> = modules.iter().map(|m| m.name == member).collect();
            index.locates_in(main, &set, &index.scope(main))
        };
        assert!(locates("smudge", true, "web.Root"));
        // The unread file's `t-inherit` is unread too, so it counts for every tree.
        assert!(locates("smudge", true, "web.Other"));
        assert!(!locates("spark", true, "web.Root"));
        assert!(!locates("smudge", false, "web.Root"));
    }

    #[test]
    fn lost_templates_reads_the_scope() {
        let file = |text: &str| template_file("static/src/a.xml", text);
        let manifest = |name: &str, depends: &str, bundled: bool| {
            let assets = if bundled {
                format!("'assets': {{'web.assets_backend': ['{name}/static/src/*.xml']}}")
            } else {
                String::new()
            };
            format!("{{'version': '18.0.1.0.0', 'depends': [{depends}], {assets}}}")
        };
        // Whether the scope of `reader` loses a template, smudge's unclosed file bundled or not.
        let lost = |reader: &str, bundled: bool| {
            let mut modules = [
                module("web", &manifest("web", "", true)),
                module("smudge", &manifest("smudge", "'web'", bundled)),
                module("main", &manifest("main", "'smudge'", true)),
                module("aside", &manifest("aside", "'web'", true)),
            ];
            let unclosed = r#"<templates><t t-name="smudge.Panel"><div>"#;
            modules[1].templates.push(file(unclosed));
            let index = Index::build(&modules, None);
            index.lost_templates(&index.scope(index.by_name[reader]))
        };
        assert!(lost("main", true));
        assert!(!lost("main", false));
        assert!(!lost("aside", true));
    }

    #[test]
    fn a_hook_is_named_by_any_file_of_any_module() {
        fn file<T: Default>(text: &str) -> SourceFile<T> {
            SourceFile {
                path: PathBuf::new(),
                rel: String::new(),
                rank: None,
                demo: false,
                facts: T::default(),
                suppressions: Default::default(),
                mentions: Mentions::scan(text),
            }
        }
        let manifest = "{'version': '18.0.1.0'}";
        let mut modules = [module("base", manifest), module("web", manifest)];
        modules[0]
            .python
            .push(file("compute='_compute_total'\ndef _compute_tax(self):"));
        modules[0].checked = false;
        modules[1]
            .js
            .push(file("orm.call(model, '_onchange_partner'); `_inverse_${name}`"));
        modules[1]
            .data
            .push(file("id,code\ncron_total,model._search_total_due()"));
        modules[1]
            .templates
            .push(file(r#"<button t-on-click="() => this.call('onchange_customer')"/>"#));
        let index = Index::build(&modules, None);
        let named = [
            "_compute_total",
            "_onchange_partner",
            "_inverse_amount",
            "_search_total_due",
            "onchange_customer",
        ];
        for name in named {
            assert!(index.names_hook(name), "{name}");
        }
        for name in [
            "_compute_tax",
            "_compute_total_tax",
            "_onchange_partner_id",
            "_search_total",
        ] {
            assert!(!index.names_hook(name), "{name}");
        }
    }

    /// `main` sees `owner`, which `sibling` extends and `stranger` does not see.
    fn hook_modules() -> Vec<Module> {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = vec![
            module("base", &manifest("")),
            module("owner", &manifest("'base'")),
            module("main", &manifest("'owner'")),
            module("heir", &manifest("'main'")),
            module("sibling", &manifest("'owner'")),
            module("stranger", &manifest("'base'")),
        ];
        for (mi, key, model) in [
            (1, "_name", "owned"),
            (4, "_inherit", "owned"),
            (5, "_name", "stranger"),
        ] {
            let source = format!("class Hook(models.Model):\n    {key} = '{model}.hook'\n");
            modules[mi].python.push(python_18(&source));
        }
        modules
    }

    #[test]
    fn an_unread_file_hides_the_hooks_of_the_scopes_it_is_in_and_of_the_models_it_reaches() {
        let mut modules = hook_modules();
        modules[2].unread_text = true;
        let index = Index::build(&modules, None);
        let unread: Vec<_> = (0..modules.len())
            .filter(|&mi| index.unread_in(&index.scope(mi)))
            .collect();
        assert_eq!(unread, [2, 3]);
        assert!(index.unread_beside("owned.hook"));
        assert!(!index.unread_beside("stranger.hook"));
    }

    #[test]
    fn an_unread_file_of_an_always_installed_module_is_in_every_scope() {
        let mut modules = hook_modules();
        let mut auto = module("auto", "{'depends': ['base'], 'auto_install': True}");
        auto.unread_text = true;
        modules.push(auto);
        let index = Index::build(&modules, None);
        assert!((0..modules.len()).all(|mi| index.unread_in(&index.scope(mi))));
        assert!(!index.unread_beside("stranger.hook"));
    }

    #[test]
    fn a_file_that_does_not_decode_hides_only_the_hooks_it_names() {
        let mut modules = hook_modules();
        modules[2].lost_text = true;
        modules[2].lost_mentions.push(Mentions::scan("x._compute_named()"));
        let index = Index::build(&modules, None);
        assert!(index.names_hook("_compute_named"));
        assert!(!index.names_hook("_compute_other"));
        assert!(!(0..modules.len()).any(|mi| index.unread_in(&index.scope(mi))));
        assert!(!index.unread_beside("owned.hook"));
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

    #[test]
    fn a_script_lists_the_bundles_that_load_it_by_name() {
        let mut a = module(
            "a",
            r#"{'version': '18.0.1.0', 'assets': {
                'z.pages': ['a/static/src/*.js', ('remove', 'a/static/src/cut.js')],
                'b.pages': ['a/static/src/both.js'],
            }}"#,
        );
        a.js = ["static/src/both.js", "static/src/cut.js"]
            .iter()
            .map(|rel| SourceFile {
                path: PathBuf::new(),
                rel: (*rel).to_owned(),
                rank: None,
                demo: false,
                facts: Default::default(),
                suppressions: Default::default(),
                mentions: Mentions::default(),
            })
            .collect();
        let modules = [a];
        let index = Index::build(&modules, None);
        assert_eq!(index.js_bundles(0, 0).collect::<Vec<_>>(), ["b.pages", "z.pages"]);
        assert_eq!(index.js_bundles(0, 1).count(), 0);
        assert!(index.js_loaded(0, 0) && !index.js_loaded(0, 1));
    }

    /// Whether `old.xml` and `both.xml` are loaded for a module of `version` (none for no version,
    /// whose series is then `default`), when `web.assets_qweb` lists both, `web.assets_backend` lists
    /// `both.xml`, and, when `included`, a third bundle includes `web.assets_qweb`.
    fn qweb_bundle_loaded(version: Option<&str>, default: Option<&str>, included: bool) -> [bool; 2] {
        let version = version.map_or(String::new(), |v| format!("'version': '{v}', "));
        let include = if included { "('include', 'web.assets_qweb')" } else { "" };
        let mut a = module(
            "a",
            &format!(
                "{{{version}'assets': {{
                    'web.assets_qweb': ['a/static/src/*.xml'],
                    'web.assets_backend': ['a/static/src/both.xml'],
                    'a.pages': [{include}],
                }}}}"
            ),
        );
        a.templates = ["static/src/old.xml", "static/src/both.xml"]
            .iter()
            .map(|rel| SourceFile {
                path: PathBuf::new(),
                rel: (*rel).to_owned(),
                rank: None,
                demo: false,
                facts: Default::default(),
                suppressions: Default::default(),
                mentions: Mentions::default(),
            })
            .collect();
        let modules = [a];
        let index = Index::build(&modules, default.and_then(Series::parse));
        [0, 1].map(|fi| index.template_loaded(0, fi))
    }

    #[test]
    fn web_assets_qweb_loads_templates_only_before_16() {
        for version in ["15.0.1.0.0", "15.4.1.0.0"] {
            assert_eq!(
                qweb_bundle_loaded(Some(version), None, false),
                [true, true],
                "{version}"
            );
        }
        assert_eq!(
            qweb_bundle_loaded(None, None, false),
            [true, true],
            "an unknown series reads as before 16.0"
        );
        for version in ["16.0.1.0.0", "18.0.1.0.0"] {
            assert_eq!(
                qweb_bundle_loaded(Some(version), None, false),
                [false, true],
                "{version}"
            );
        }
    }

    #[test]
    fn a_module_without_a_version_reads_web_assets_qweb_in_the_default_series() {
        assert_eq!(qweb_bundle_loaded(None, Some("18.0"), false), [false, true]);
        assert_eq!(qweb_bundle_loaded(None, Some("15.0"), false), [true, true]);
    }

    #[test]
    fn a_bundle_including_web_assets_qweb_loads_its_templates_from_16() {
        assert_eq!(qweb_bundle_loaded(Some("16.0.1.0.0"), None, true), [true, true]);
    }

    #[test]
    fn a_test_bundle_has_a_test_word_after_its_module() {
        for name in [
            "web.assets_tests",
            "web.tests_assets",
            "web.qunit_suite_tests",
            "web.__assets_tests_call__",
            "web_editor.mocha_tests",
        ] {
            assert!(test_bundle(name), "{name}");
        }
        for name in ["web.assets_backend", "x.latest", "test_x.assets_backend"] {
            assert!(!test_bundle(name), "{name}");
        }
    }

    #[test]
    fn a_script_is_a_test_by_its_path_or_when_only_test_bundles_load_it() {
        let mut a = module(
            "a",
            r#"{'version': '18.0.1.0', 'assets': {
                'web.assets_backend': ['a/static/src/both.js', 'a/static/tests/shipped.js'],
                'web.qunit_suite_tests': ['a/static/src/both.js', 'a/static/src/suite.js'],
                'a.mocha_tests': ['a/static/src/suite.js'],
            }}"#,
        );
        let rels = [
            "static/src/both.js",
            "static/src/none.js",
            "static/src/suite.js",
            "static/tests/shipped.js",
        ];
        a.js = rels.iter().map(|rel| script(rel, "")).collect();
        let modules = [a];
        let index = Index::build(&modules, None);
        assert!(!index.js_test(0, 0), "a shipped bundle loads it too");
        assert!(!index.js_test(0, 1), "no bundle loads it");
        assert!(index.js_test(0, 2), "only test bundles load it");
        assert!(index.js_test(0, 3), "its path makes it a test");
    }

    fn data_file(rel: &str, facts: crate::facts::DataFacts) -> SourceFile<crate::facts::DataFacts> {
        SourceFile {
            path: PathBuf::new(),
            rel: rel.to_owned(),
            rank: Some(0),
            demo: false,
            facts,
            suppressions: Default::default(),
            mentions: Mentions::default(),
        }
    }

    fn template_file(rel: &str, text: &str) -> SourceFile<crate::facts::TemplateFacts> {
        SourceFile {
            path: PathBuf::new(),
            rel: rel.to_owned(),
            rank: None,
            demo: false,
            facts: crate::owl::analyze(text),
            suppressions: Default::default(),
            mentions: Mentions::default(),
        }
    }

    fn script(rel: &str, text: &str) -> SourceFile<crate::facts::JsFacts> {
        SourceFile {
            path: PathBuf::new(),
            rel: rel.to_owned(),
            rank: None,
            demo: false,
            facts: crate::js::analyze(text),
            suppressions: Default::default(),
            mentions: Mentions::default(),
        }
    }

    #[test]
    fn a_registry_holds_the_keys_of_loaded_scripts_outside_tests() {
        let assets = |name: &str| format!("{{'version': '18.0.1.0', 'assets': {{'b': ['{name}/static/src/**/*']}}}}");
        let mut web = module("web", &assets("web"));
        web.js = vec![
            script(
                "static/src/fields.js",
                r#"registry.category("fields").add("list.tags", A).add("many2one", B);
registry.category("actions").add("reload", C);"#,
            ),
            script(
                "static/tests/mock.js",
                r#"registry.category("actions").add("mocked", D);"#,
            ),
        ];
        let mut side = module("side", &assets("side"));
        side.js = vec![script(
            "static/src/side.js",
            r#"registry.category("view_widgets").add("ribbon", E); registry.category("actions").add(name, F);"#,
        )];
        let mut sale = module(
            "sale",
            &assets("sale").replace("'version'", "'depends': ['web'], 'version'"),
        );
        sale.js = vec![script(
            "static/unbundled/x.js",
            r#"registry.category("actions").add("stray", G);"#,
        )];
        let modules = [web, side, sale];
        let index = Index::build(&modules, None);
        let scope = index.scope(2);
        let status = |category, key| match index.registry_status(category, key, &scope) {
            Status::Visible => "visible".to_owned(),
            Status::Hidden(m) => format!("hidden in {m}"),
            Status::Missing => "missing".to_owned(),
        };
        assert_eq!(status("actions", "reload"), "visible");
        assert_eq!(status("fields", "reload"), "missing");
        assert_eq!(status("actions", "mocked"), "missing");
        assert_eq!(status("actions", "stray"), "missing");
        assert_eq!(status("view_widgets", "ribbon"), "hidden in side");
        for widget in ["list.tags", "tags", "many2one"] {
            assert!(
                matches!(index.field_widget_status(widget, &scope), Status::Visible),
                "{widget}"
            );
        }
        assert!(matches!(index.field_widget_status("list", &scope), Status::Missing));
        assert!(!index.registry_open("actions", &scope));
        assert!(index.registry_open("actions", &index.scope(1)));
    }

    #[test]
    fn a_test_bundle_adds_only_its_tours() {
        let mut web = module(
            "web",
            "{'version': '18.0.1.0', 'assets': {'web.assets_tests': ['web/static/tests/t*.js', 'web/static/tests/o*.js'], \
             'web.assets_unit_tests': ['web/static/tests/unit/*']}}",
        );
        web.js = vec![
            script(
                "static/tests/tours.js",
                r#"registry.category("web_tour.tours").add("checkout", {}); registry.category("services").add("mock", {});"#,
            ),
            script(
                "static/tests/unit/tour.test.js",
                r#"registry.category("web_tour.tours").add("unit", {});"#,
            ),
            script(
                "static/tests/open.js",
                r#"const tours = registry.category("web_tour.tours"); tours.add(name, {});
registry.category("actions").add(tag, {});"#,
            ),
        ];
        let modules = [web];
        let index = Index::build(&modules, None);
        let scope = index.scope(0);
        let visible = |category, key| matches!(index.registry_status(category, key, &scope), Status::Visible);
        assert!(visible(super::TOUR_REGISTRY, "checkout"));
        assert!(matches!(
            index.registry_status(super::TOUR_REGISTRY, "unit", &scope),
            Status::Missing
        ));
        assert!(!visible(super::SERVICE_REGISTRY, "mock"));
        assert!(index.registry_open(super::TOUR_REGISTRY, &scope));
        assert!(!index.registry_open("actions", &scope));
    }

    #[test]
    fn a_test_script_is_named_from_the_source_folder() {
        assert_eq!(
            js_module_path("web", "static/tests/helpers/utils.js").as_deref(),
            Some("@web/../tests/helpers/utils")
        );
        assert_eq!(
            js_module_path("web", "static/src/core/hooks.js").as_deref(),
            Some("@web/core/hooks")
        );
        assert_eq!(js_module_path("web", "static/lib/hoot/hoot.js"), None);
    }

    #[test]
    fn specifiers_resolve_as_odoo_names_them() {
        let mut web = module("web", "{'version': '18.0.1.0'}");
        web.js = vec![
            script("static/src/core/hooks.js", "export const useService = 1;"),
            script("static/tests/helpers.js", "export function onRpc() {}"),
            script(
                "static/tests/legacy/utils.js",
                "/** @odoo-module alias=@web/../tests/utils default=false */\nexport const click = 1;",
            ),
            script(
                "static/tests/legacy/views.js",
                "/** @odoo-module alias=web.views */\nexport default {};",
            ),
        ];
        let mut sale = module("sale", "{'version': '18.0.1.0'}");
        sale.js = vec![script("static/src/order.js", "")];
        let modules = [web, sale];
        let index = Index::build(&modules, None);
        let resolve = |from: &str, specifier: &str| match index.js_resolve(from, specifier) {
            JsSpec::Found(t) => modules[t.module].js[t.file].rel.clone(),
            JsSpec::Missing(mi) => format!("missing in {}", modules[mi].name),
            JsSpec::Outside => "outside".to_owned(),
        };
        let test = "sale/static/tests/order.test.js";
        assert_eq!(resolve(test, "@web/../tests/helpers"), "static/tests/helpers.js");
        // Odoo looks an `@` name up as written.
        assert_eq!(resolve(test, "@web/core/../../tests/helpers"), "missing in web");
        assert_eq!(resolve(test, "@web/core/hooks.js"), "missing in web");
        assert_eq!(resolve(test, "@web/core/hooks"), "static/src/core/hooks.js");
        assert_eq!(
            resolve("web/static/tests/legacy/utils.js", "../helpers"),
            "static/tests/helpers.js"
        );
        assert_eq!(
            resolve("web/static/tests/helpers.js", "../src/core/hooks"),
            "static/src/core/hooks.js"
        );
        assert_eq!(resolve(test, "../src/order"), "static/src/order.js");
        assert_eq!(resolve(test, "../../../web/static/src/core/hooks"), "outside");
        assert_eq!(resolve(test, "@web/../lib/hoot/hoot"), "outside");
        assert_eq!(resolve(test, "@web/../../base/x"), "outside");
        assert_eq!(resolve(test, "@web/../tests/gone"), "missing in web");
        assert_eq!(resolve(test, "@sale/gone"), "missing in sale");
        // Sale has no test script, as in a core cached by an earlier odooscan or a sparse checkout
        // made without tests.
        assert_eq!(resolve(test, "@sale/../tests/gone"), "outside");
        assert_eq!(resolve(test, "@web/../tests/utils"), "static/tests/legacy/utils.js");
        assert_eq!(resolve(test, "web.views"), "static/tests/legacy/views.js");
        let export = |specifier: &str, name: &str| match index.js_resolve(test, specifier) {
            JsSpec::Found(t) => index.js_export(t, name, 0),
            _ => panic!("{specifier} resolves to no file"),
        };
        assert!(export("@web/../tests/utils", "click") == Export::Found);
        assert!(export("@web/../tests/utils", "gone") == Export::Missing);
        assert!(export("web.views", "gone") == Export::Unknown);
    }

    #[test]
    fn a_path_names_a_script_odoo_transpiles() {
        let found = |version: &str| {
            let manifest = format!("{{'version': '{version}'}}");
            let mut m = module("m", if version.is_empty() { "{}" } else { &manifest });
            m.js = vec![
                script("static/src/plain.js", "export const a = 1;"),
                script("static/src/tagged.js", "// @odoo-module\nexport const a = 1;"),
                script("static/src/ign.js", "// @odoo-module ignore\nexport const a = 1;"),
                script(
                    "static/src/defined.js",
                    "// @odoo-module ignore\nodoo.define(\"@m/defined\", [], function () { return {}; });",
                ),
            ];
            let modules = [m];
            let index = Index::build(&modules, None);
            let names = ["plain", "tagged", "ign", "defined"];
            let resolves = |n: &&str| {
                matches!(
                    index.js_resolve("m/static/src/x.js", &format!("@m/{n}")),
                    JsSpec::Found(_)
                )
            };
            names.iter().map(|n| (*n, resolves(n))).collect::<Vec<_>>()
        };
        let expect = |plain: bool, ign: bool| vec![("plain", plain), ("tagged", true), ("ign", ign), ("defined", true)];
        assert_eq!(found("16.0.1.0.0"), expect(false, true));
        assert_eq!(found("17.0.1.0.0"), expect(false, true));
        assert_eq!(found("saas~17.2.1.0.0"), expect(true, false));
        assert_eq!(found(""), expect(true, true));
    }

    #[test]
    fn an_alias_names_a_script_only_where_odoo_transpiles_it() {
        // Module b does not depend on a, so a claim of `a.x` would read as a missing dependency.
        let resolve = |version: &str| {
            let mut a = module("a", &format!("{{'version': '{version}'}}"));
            a.js = vec![script(
                "static/src/x.js",
                "// @odoo-module alias=a.x\nexport const x = 1;",
            )];
            let mut b = module("b", &format!("{{'version': '{version}'}}"));
            b.js = vec![script(
                "static/src/y.js",
                "odoo.define(\"b.y\", [\"a.x\"], function () {});",
            )];
            let modules = [a, b];
            let index = Index::build(&modules, None);
            match index.js_resolve("b/static/src/y.js", "a.x") {
                JsSpec::Found(t) => modules[t.module].name.clone(),
                JsSpec::Missing(_) => "missing".to_owned(),
                JsSpec::Outside => "outside".to_owned(),
            }
        };
        assert_eq!(resolve("14.0.1.0.0"), "outside");
        assert_eq!(resolve("saas~14.3.1.0.0"), "a");
    }

    #[test]
    fn an_untranspiled_script_is_told_apart_from_a_missing_one() {
        let untranspiled = |version: &str, rel: &str, text: &str, specifier: &str| {
            let mut m = module("m", &format!("{{'version': '{version}'}}"));
            m.js = vec![script(rel, text)];
            let modules = [m];
            let index = Index::build(&modules, None);
            let found = index.js_untranspiled(0, "m/static/src/x.js", specifier);
            found.map(|u| (u.rel.to_owned(), u.tag_fix, u.defined.map(str::to_owned)))
        };
        let expect = |rel: &str, fix: Option<TagFix>, defined: Option<&str>| {
            Some((rel.to_owned(), fix, defined.map(str::to_owned)))
        };
        let define = "odoo.define(\"m.x\", [], function () {});";
        let annotate = Some(TagFix::Annotate);
        assert_eq!(
            untranspiled("17.0.1.0.0", "static/src/plain.js", "", "@m/plain"),
            expect("static/src/plain.js", annotate, None)
        );
        assert_eq!(
            untranspiled("17.0.1.0.0", "static/src/dir/index.js", define, "./dir"),
            expect("static/src/dir/index.js", annotate, Some("m.x"))
        );
        assert_eq!(
            untranspiled("17.0.1.0.0", "static/tests/t.js", "", "@m/../tests/t"),
            expect("static/tests/t.js", annotate, None)
        );
        assert_eq!(
            untranspiled(
                "saas~17.2.1.0.0",
                "static/src/ign.js",
                "// @odoo-module ignore\n",
                "@m/ign"
            ),
            expect("static/src/ign.js", Some(TagFix::DropIgnore), None)
        );
        assert_eq!(
            untranspiled(
                "14.0.1.0.0",
                "static/src/old.js",
                &format!("// @odoo-module\n{define}"),
                "@m/old"
            ),
            expect("static/src/old.js", None, Some("m.x"))
        );
        // Transpiled, so the path resolves and no untranspiled script stands behind it.
        assert_eq!(
            untranspiled("saas~17.2.1.0.0", "static/src/plain.js", "", "@m/plain"),
            None
        );
        assert_eq!(untranspiled("17.0.1.0.0", "static/src/plain.js", "", "@m/gone"), None);
    }

    /// The modules of an Owl test: `web` with three components, and `sale` with `order.js` reading
    /// them.
    fn owl_modules(web_version: &str, version: &str, order: &str) -> [Module; 2] {
        let mut web = module("web", &format!("{{'version': '{web_version}'}}"));
        web.js = vec![
            script(
                "static/src/legacy/legacy_component.js",
                "// @odoo-module\nexport class LegacyComponent extends owl.Component { trigger() {} }",
            ),
            script(
                "static/src/form.js",
                "// @odoo-module\nimport { Component as C } from \"@odoo/owl\";\nexport class Form extends C {}",
            ),
            script(
                "static/src/panel.js",
                "// @odoo-module\nexport class Panel extends owl.Component {}",
            ),
        ];
        let mut sale = module("sale", &format!("{{'version': '{version}', 'depends': ['web']}}"));
        sale.js = vec![script("static/src/order.js", order)];
        [web, sale]
    }

    /// The sorted members `component_members` or `js_members` gives the class `name` of `order.js`.
    fn members_of(index: &Index, modules: &[Module], name: &str, component: bool) -> Option<String> {
        let class = modules[1].js[0].facts.classes.iter().find(|c| c.name == name).unwrap();
        let here = index.js_target(1, 0);
        let members = match component {
            true => index.component_members(here, class, modules[1].series)?,
            false => index.js_members(here, class, (9, 9, 9), modules[1].series)?,
        };
        let mut members: Vec<_> = members.into_iter().collect();
        members.sort_unstable();
        Some(members.join(","))
    }

    #[test]
    fn a_component_is_a_class_whose_chain_ends_at_owl_component() {
        let order = r#"import { LegacyComponent } from "@web/legacy/legacy_component";
import { Form } from "@web/form";
import { EventBus } from "@odoo/owl";
const { Component, Store: S } = owl;
class Bound extends Component {}
class Global extends owl.Component {}
class Legacy extends LegacyComponent {}
class Sub extends Form { save() {} }
class Patched extends Component {}
patch(Patched.prototype, "sale.patched", { trigger() {} });
class Plain {}
class Bus extends EventBus {}
class Owl1 extends owl.Store {}
class Mixed extends mixin(Form) {}
class Unbound extends S {}
"#;
        let modules = owl_modules("16.0.1.0.0", "16.0.1.0.0", order);
        let index = Index::build(&modules, None);
        let members = |name: &str| members_of(&index, &modules, name, true);
        assert_eq!(members("Bound").as_deref(), Some("render,setup"));
        assert_eq!(members("Global").as_deref(), Some("render,setup"));
        assert_eq!(members("Legacy").as_deref(), Some("render,setup,trigger"));
        assert_eq!(members("Sub").as_deref(), Some("render,save,setup"));
        assert_eq!(members("Patched").as_deref(), Some("render,setup,trigger"));
        for other in ["Plain", "Bus", "Owl1", "Mixed", "Unbound"] {
            assert_eq!(members(other), None, "{other}");
        }
        assert_eq!(members_of(&index, &modules, "Plain", false).as_deref(), Some(""));
    }

    #[test]
    fn owl_component_has_the_members_of_the_owl_of_the_series() {
        let order = r#"import { Form } from "@web/form";
import { Panel } from "@web/panel";
const { Component } = owl;
class Bound extends Component {}
class Global extends owl.Component {}
class Sub extends Form {}
class Core extends Panel {}
"#;
        let owl1 = OWL1_COMPONENT.iter().copied().collect::<FxHashSet<_>>();
        let owl2 = OWL_COMPONENT.iter().copied().collect::<FxHashSet<_>>();
        // The manifest versions of `web` and `sale`, `default_series`, the members expected of
        // `Bound` and `Global` (the `owl` global read in `sale`), and of `Core`, which extends
        // `web`'s `Panel` (the `owl` global read in `web`, which a `'1.0'` `web` takes from
        // `default_series`, else `sale`).
        for (web, sale, default, global, core) in [
            ("15.0.1.0.0", "15.0.1.0.0", None, &owl1, &owl1),
            ("1.0", "1.0", None, &owl1, &owl1),
            ("1.0", "1.0", Series::parse("saas~15.2"), &owl1, &owl1),
            ("15.3.1.0.0", "15.3.1.0.0", None, &owl2, &owl2),
            ("1.0", "1.0", Series::parse("16.0"), &owl2, &owl2),
            ("1.0", "17.0.1.0.0", None, &owl2, &owl2),
            ("1.0", "15.0.1.0.0", None, &owl1, &owl1),
            ("15.0.1.0.0", "17.0.1.0.0", None, &owl2, &owl1),
        ] {
            let modules = owl_modules(web, sale, order);
            let index = Index::build(&modules, default);
            let here = index.js_target(1, 0);
            let reader = modules[1].series.or(default);
            for (name, expected) in [("Bound", global), ("Global", global), ("Sub", &owl2), ("Core", core)] {
                let class = modules[1].js[0].facts.classes.iter().find(|c| c.name == name).unwrap();
                let members = index.js_members(here, class, (9, 9, 9), reader);
                assert_eq!(members.as_ref(), Some(expected), "{web} {sale} {default:?} {name}");
                assert_eq!(index.component_members(here, class, reader).as_ref(), Some(expected));
            }
        }
    }

    /// `web` with components that declare props, and `sale` with the owners of its templates.
    fn props_modules() -> [Module; 2] {
        let assets = |name: &str| format!("{{'version': '18.0.1.0', 'assets': {{'b': ['{name}/static/src/**/*']}}}}");
        let mut web = module("web", &assets("web"));
        web.js = vec![script(
            "static/src/card.js",
            r#"import { Component } from "@odoo/owl";
export class Card extends Component {
    static template = "web.Card";
    static props = { title: String, body: { type: String, optional: true } };
}
export class BigCard extends Card {
    static props = { ...Card.props, size: Number, body: String };
}
export class ListCard extends Component { static props = ["title", "body?"]; }
export class MixCard extends Component { static props = { ...ListCard.props }; }
export class Free extends Component { static props = ["*", ...ListCard.props]; }
export class Bare extends Component {}
export class Inherits extends Card {}
export class Written extends Component { static props = { x: String }; }
export class SuperCard extends Card { static props = { ...super.props, extra: Number }; }
export class Shared extends Component { static props = { a: String }; }
export class SharedSub extends Shared {}
export class Kept extends Component { static props = { a: String }; }
export class KeptSub extends Kept {}
export class Tile extends Component { static props = { title: String }; }
export class TileAlias extends Component { static props = Tile.props; }
export class Badge extends Component { static props = { title: String }; }
export class BadgeCopy extends Component { static props = { ...Badge.props }; }
export class Chip extends Component { static props = { title: String }; }
export class ChipCopy extends Component { static props = { ...Chip.props }; }
export class Link0 extends Component { static props = { title: String }; }
export class Link1 extends Component { static props = Link0.props; }
export class Link2 extends Component { static props = Link1.props; }
export class Link3 extends Component { static props = Link2.props; }
export class Link4 extends Component { static props = Link3.props; }
export class Link5 extends Component { static props = Link4.props; }
export class Link6 extends Component { static props = Link5.props; }
export class Link7 extends Component { static props = Link6.props; }
export class Link8 extends Component { static props = Link7.props; }
export class Link9 extends Component { static props = Link8.props; }
Link9.props.extra = {};
export class Loop extends Component { static props = Ring.props; }
export class Ring extends Component { static props = Loop.props; }
Loop.props.extra = {};
export class Knot extends Component { static props = { ...Bow.props, title: String }; }
export class Bow extends Component { static props = { ...Knot.props }; }
Knot.props.title.optional = true;
export class Tile2 extends Component { static props = { title: String }; }
export class Leaf extends Component { static props = { title: String }; }
export class Hub extends Component { static props = { title: String }; static components = { Leaf }; }
"#,
        )];
        let mut sale = module(
            "sale",
            &assets("sale").replace("'version'", "'depends': ['web'], 'version'"),
        );
        sale.js = vec![
            script(
                "static/src/order.js",
                r#"import { Component } from "@odoo/owl";
import { Card, BigCard, Written, SharedSub, KeptSub, TileAlias, BadgeCopy, ChipCopy, Tile2, Hub } from "@web/card";
export class Order extends Component {
    static template = "sale.Order";
    static components = { Card, Big: BigCard, Gone: missing() };
}
export class SubOrder extends Order {
    static components = { ...Order.components, Card: BigCard };
}
export class Lazy extends Order {}
export class Patched extends Component {
    static template = "sale.Patched";
    static components = { Card };
}
patch(Patched, { components: {} });
Written.props.y = String;
SharedSub.props.extra = { optional: true };
KeptSub.props = { b: String };
TileAlias.props.extra = {};
BadgeCopy.props.title.optional = true;
ChipCopy.props.extra = {};
export class Desk extends Component { static components = { Card }; }
export class DeskAlias extends Component { static components = Desk.components; }
DeskAlias.components.Extra = Card;
export class Shelf extends Component { static components = { Card }; }
export class ShelfCopy extends Component { static components = { ...Shelf.components }; }
ShelfCopy.components.Card.reset();
export class Called extends Component { static template = "sale.Called"; }
export class Parent extends Component { static template = "sale.Parent"; }
export class Rack extends Component { static props = { a: String }; static components = { Card, Hook: Tile2, Hub }; }
Rack.components.Hook.props.extra = {};
Rack.components.Hub.components.Leaf.props.extra = {};
Rack.components.Missing.props.extra = {};
"#,
            ),
            script(
                "static/tests/order_tests.js",
                r#"import { Order } from "@sale/order";
class TestOrder extends Order {}
Order.components.Card = null;
"#,
            ),
        ];
        sale.templates = vec![template_file(
            "static/src/order.xml",
            r#"<templates>
<t t-name="sale.Order"><Card title="1"/></t>
<t t-name="sale.Called"/>
<t t-name="sale.Caller"><t t-call="sale.Called"/></t>
<t t-name="sale.Parent"/>
<t t-name="sale.Child" t-inherit="sale.Parent"/>
<t t-inherit="sale.Order" t-inherit-mode="extension">
    <xpath expr="//Card" position="attributes"><attribute name="body">x</attribute></xpath>
</t>
</templates>"#,
        )];
        [web, sale]
    }

    #[test]
    fn a_tag_resolves_through_the_components_of_each_owner() {
        let modules = props_modules();
        let index = Index::build(&modules, None);
        let owners: Vec<_> = index
            .template_owners("sale.Order", 1)
            .unwrap()
            .iter()
            .map(|(_, c)| c.name.as_str())
            .collect();
        assert_eq!(owners, ["Order", "SubOrder", "Lazy"]);
        assert!(
            index.template_owners("sale.Called", 1).is_none(),
            "a template another calls"
        );
        assert!(
            index.template_owners("sale.Parent", 1).is_none(),
            "a template with a primary child"
        );
        assert!(
            index.template_owners("sale.Caller", 1).is_none(),
            "a template no class renders"
        );
        assert!(index.template_sets_attributes("sale.Order"));
        assert!(!index.template_sets_attributes("sale.Caller"));
        let class = |name: &str| modules[1].js[0].facts.classes.iter().find(|c| c.name == name).unwrap();
        let here = index.js_target(1, 0);
        let child = |owner: &str, tag: &str| {
            index
                .component_child(here, class(owner), tag)
                .map(|(_, c)| c.name.as_str())
        };
        assert_eq!(child("Order", "Card"), Some("Card"));
        assert_eq!(child("Order", "Big"), Some("BigCard"));
        assert_eq!(child("SubOrder", "Card"), Some("BigCard"));
        assert_eq!(child("SubOrder", "Big"), Some("BigCard"));
        assert_eq!(child("Lazy", "Card"), Some("Card"));
        // A spread copies the class references, so a write into one changes the class, not Shelf's.
        assert_eq!(child("Shelf", "Card"), Some("Card"));
        // A write past a key changes the class the key holds, not Rack's mapping, nor Rack's props
        // when the key holds no class it knows.
        assert_eq!(child("Rack", "Card"), Some("Card"));
        assert!(index.component_props(here, class("Rack")).is_some());
        // A write through a class that shares the object changes the source's too.
        for (owner, tag) in [
            ("Order", "Gone"),
            ("Order", "Nope"),
            ("Patched", "Card"),
            ("Desk", "Card"),
        ] {
            assert_eq!(child(owner, tag), None, "{owner} {tag}");
        }
    }

    #[test]
    fn an_unreadable_template_file_hides_the_owners() {
        let unclosed = r#"<templates><t t-inherit="sale.Order" t-inherit-mode="extension"><xpath expr="//Card">"#;
        // Whether `name`, read by `sale`, keeps its owners when `loser` loads the unclosed file, the
        // extra module depending on `depends` and bundling its own files or not.
        let owned = |name: &str, loser: &str, depends: &str, bundled: bool| {
            let [web, sale] = props_modules();
            let assets = if bundled {
                "'assets': {'b': ['extra/static/src/*']}, "
            } else {
                ""
            };
            let extra = module(
                "extra",
                &format!("{{'version': '18.0.1.0', {assets}'depends': ['{depends}']}}"),
            );
            let mut modules = [web, sale, extra];
            let at = modules.iter().position(|m| m.name == loser).unwrap();
            let lost = template_file("static/src/lost.xml", unclosed);
            modules[at].templates.push(lost);
            let index = Index::build(&modules, None);
            index.template_owners(name, 1).is_some()
        };
        assert!(!owned("sale.Order", "sale", "web", true), "the template's own module");
        assert!(!owned("sale.Order", "extra", "sale", true), "a module depending on it");
        assert!(owned("sale.Order", "extra", "sale", false), "an unbundled file");
        assert!(owned("sale.Order", "extra", "web", true), "outside its dependents");
        assert!(owned("sale.Order", "web", "web", true), "a dependency of its module");
        // `web.Card` has no readable definer, so the unread file of a dependency may define it.
        assert!(!owned("web.Card", "web", "web", true), "a dependency, no definer");
        assert!(owned("web.Card", "extra", "web", true), "outside the reader's scope");
    }

    #[test]
    fn a_component_has_the_props_it_declares_or_spreads() {
        let modules = props_modules();
        let index = Index::build(&modules, None);
        let here = index.js_target(0, 0);
        let props = |name: &str| {
            let class = modules[0].js[0].facts.classes.iter().find(|c| c.name == name).unwrap();
            let schema = index.component_props(here, class)?;
            let mut props: Vec<_> = schema
                .props
                .iter()
                .map(|(p, required)| format!("{p}{}", if *required { "" } else { "?" }))
                .collect();
            props.sort_unstable();
            Some(format!("{}{}", if schema.any { "*," } else { "" }, props.join(",")))
        };
        assert_eq!(props("Card").as_deref(), Some("body?,title"));
        assert_eq!(props("BigCard").as_deref(), Some("body,size,title"));
        assert_eq!(props("ListCard").as_deref(), Some("body?,title"));
        assert_eq!(props("Free").as_deref(), Some("*,body?,title"));
        assert_eq!(props("Inherits").as_deref(), Some("body?,title"));
        assert_eq!(props("SuperCard").as_deref(), Some("body?,extra,title"));
        // A subclass's own `props` replaces the inherited object, which a write into it changes.
        assert_eq!(props("Kept").as_deref(), Some("a"));
        // A spread copies the keys, so a key write into a copy leaves the source alone; a write
        // into a description, or any through the source's own object, reaches the source, however
        // long the chain of aliases and through a cycle.
        assert_eq!(props("Chip").as_deref(), Some("title"));
        // A write through two keys changes the class the last holds, not the one between.
        assert_eq!(props("Hub").as_deref(), Some("title"));
        for name in [
            "MixCard",
            "Bare",
            "Written",
            "Shared",
            "SharedSub",
            "KeptSub",
            "Tile",
            "TileAlias",
            "Badge",
            "BadgeCopy",
            "ChipCopy",
            "Link0",
            "Loop",
            "Ring",
            "Knot",
            "Bow",
            "Tile2",
            "Leaf",
        ] {
            assert_eq!(props(name), None, "{name}");
        }
    }

    #[test]
    fn a_name_two_modules_claim_goes_to_the_one_loaded_first() {
        let utils = "/** @odoo-module alias=@web/../tests/utils default=false */\nexport const click = 1;";
        // Named first, so only the load order puts web before it.
        let mut chat = module("chat", "{'version': '18.0.1.0', 'depends': ['web']}");
        chat.js = vec![script("static/tests/contains.js", utils)];
        let mut web = module("web", "{'version': '18.0.1.0'}");
        web.js = vec![script("static/tests/legacy/utils.js", utils)];
        let mut old = module("old", "{'version': '17.0.1.0.0'}");
        old.js = vec![script(
            "static/tests/utils.js",
            &utils.replace("@web/../tests/utils", "@old/../tests/kit"),
        )];
        let modules = [chat, web, old];
        let index = Index::build(&modules, None);
        let JsSpec::Found(t) = index.js_resolve("x/static/tests/a.js", "@web/../tests/utils") else {
            panic!("the alias resolves to no file");
        };
        assert_eq!(modules[t.module].name, "web");
        assert!(index.js_export(t, "gone", 0) == Export::Unknown);
        // 17.0 reads no alias of that form.
        assert!(matches!(
            index.js_resolve("x/static/tests/a.js", "@old/../tests/kit"),
            JsSpec::Missing(2)
        ));
    }

    #[test]
    fn a_class_of_base_above_in_the_file_is_a_read_parent() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = [module("base", &manifest("")), module("main", &manifest("'base'"))];
        modules[0]
            .python
            .push(python_18("class B(models.Model):\n    _name = 'base'\n"));
        modules[1].python.push(python_18(
            "from odoo.addons.other.models import Imported\n\n\
             class Base(models.AbstractModel):\n    _inherit = 'base'\n\n\
             class Base(Base):\n    _inherit = 'base'\n\n\
             class Local(models.Model, Base):\n    _name = 'main.local'\n\n\
             class Plain(models.Model):\n    _name = 'main.plain'\n\n\
             class Foreign(models.Model, Imported):\n    _name = 'main.foreign'\n",
        ));
        let index = Index::build(&modules, None);
        let main = index.by_name["main"];
        let scope = index.scope(main);
        for model in ["main.local", "main.plain"] {
            assert!(!index.hidden_field_parents(model, &scope), "{model}");
            assert!(!index.hidden_method_parents(model, &scope), "{model}");
        }
        assert!(index.hidden_field_parents("main.foreign", &scope));
        assert!(index.hidden_method_parents("main.foreign", &scope));
        assert!(index.holds_model(main, "main.local", &scope));
    }

    #[test]
    fn a_model_only_outside_scope_is_judged_on_all_its_classes() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = [
            module("base", &manifest("")),
            module("side", &manifest("'base'")),
            module("main", &manifest("'base'")),
            module("reader", &manifest("'side'")),
        ];
        modules[0]
            .python
            .push(python_18("class B(models.Model):\n    _name = 'base'\n"));
        modules[1].python.push(python_18(
            "from odoo.addons.other.models import Imported\n\n\
             class Hidden(models.Model, Imported):\n    _name = 'side.hidden'\n\n\
             class Plain(models.Model):\n    _name = 'side.plain'\n",
        ));
        let index = Index::build(&modules, None);
        let main = index.scope(index.by_name["main"]);
        let reader = index.scope(index.by_name["reader"]);
        assert!(index.hidden_outside("side.hidden", &main));
        assert!(!index.hidden_outside("side.plain", &main));
        assert!(!index.hidden_outside("side.hidden", &reader));
    }

    const REBOUND_BASE: &str = "class Base(models.AbstractModel):\n    _inherit = 'base'\n\n\
                                class Base(Base):\n    _inherit = 'base'\n";
    const OPAQUE_BASE: &str = "from mixin_kit import ImportedMixin\n\n\
                               class Base(models.AbstractModel, ImportedMixin):\n    _inherit = 'base'\n";

    /// `base`, `kit` with two models, which depends on `kit_depends`, and `extra`, which holds the
    /// classes of `base` in `source`.
    fn base_class_modules(kit_depends: &str, source: &str) -> [Module; 3] {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = [
            module("base", &manifest("")),
            module("kit", &manifest(kit_depends)),
            module("extra", &manifest("'base'")),
        ];
        modules[0]
            .python
            .push(python_18("class B(models.Model):\n    _name = 'base'\n"));
        modules[1].python.push(python_18(
            "class Thing(models.Model):\n    _name = 'kit.thing'\n\n\
             class Other(models.Model):\n    _name = 'kit.other'\n",
        ));
        modules[2].python.push(python_18(source));
        modules
    }

    #[test]
    fn a_plain_base_of_a_class_of_base_keeps_other_models_apart() {
        for source in [REBOUND_BASE, OPAQUE_BASE] {
            let modules = base_class_modules("'base'", source);
            let index = Index::build(&modules, None);
            let kit = index.scope(index.by_name["kit"]);
            assert_eq!(
                index.stands_for("kit.thing", "kit.other", &kit),
                Some(false),
                "{source}"
            );
        }
    }

    #[test]
    fn a_non_literal_inherit_of_a_class_of_base_leaves_other_models_unknown() {
        let source = "class Base(models.AbstractModel):\n    _inherit = ['base', PARENT]\n";
        let modules = base_class_modules("'base'", source);
        let index = Index::build(&modules, None);
        let kit = index.scope(index.by_name["kit"]);
        assert_eq!(index.stands_for("kit.thing", "kit.other", &kit), None);
    }

    #[test]
    fn a_class_of_base_hides_parents_only_through_an_unresolved_base() {
        for (source, hidden) in [(REBOUND_BASE, false), (OPAQUE_BASE, true)] {
            let modules = base_class_modules("'extra'", source);
            let index = Index::build(&modules, None);
            let kit = index.scope(index.by_name["kit"]);
            assert_eq!(index.hidden_parents("kit.thing", &kit), hidden, "{source}");
            assert_eq!(index.merged_hidden_parents("kit.thing", &kit), hidden, "{source}");
        }
    }

    #[test]
    fn only_an_unresolved_base_of_a_class_of_base_is_unread() {
        for (source, unread) in [(REBOUND_BASE, false), (OPAQUE_BASE, true)] {
            let modules = base_class_modules("'base'", source);
            let index = Index::build(&modules, None);
            let extra = index.by_name["extra"];
            let chain = index.chain_classes("kit.thing");
            let &(mi, class) = chain
                .iter()
                .find(|(mi, c)| *mi == extra && !c.plain_bases.is_empty())
                .unwrap();
            assert_eq!(
                index.unread_parents(mi, class, &chain, &class.plain_sources),
                unread,
                "{source}"
            );
        }
    }

    #[test]
    fn file_base_class_resolves_a_model_class_of_an_indexed_file() {
        let file = |rel: &str, source: &str| SourceFile {
            rel: rel.to_owned(),
            ..python_18(source)
        };
        let mut modules = [module("kit", "{'version': '18.0.1.0'}")];
        modules[0].python.push(file(
            "models/a.py",
            "class A(models.Model):\n    _name = 'kit.a'\n\nclass Plain:\n    pass\n",
        ));
        modules[0].python.push(file("models/b.py", ""));
        let index = Index::build(&modules, None);
        let kit = index.by_name["kit"];
        let [a, b] = [&modules[0].python[0], &modules[0].python[1]];
        let imported = |level: u32, path: &str| BaseSource::Imported {
            level,
            path: path.to_owned(),
        };
        let named = |base: &BaseSource, file| index.file_base_class(kit, file, base).map(|c| c.class_name.as_str());
        assert_eq!(named(&imported(1, "a.A"), b), Some("A"));
        assert_eq!(named(&imported(0, "odoo.addons.kit.models.a.A"), b), Some("A"));
        assert_eq!(named(&BaseSource::Local(a.facts.classes[0].pos), a), Some("A"));
        assert_eq!(named(&imported(1, "a.Plain"), b), None);
        assert_eq!(named(&imported(0, "odoo.addons.other.models.a.A"), b), None);
        assert_eq!(named(&BaseSource::Unread, b), None);
    }

    #[test]
    fn a_class_of_base_hides_parents_only_where_the_model_installs_it() {
        let hidden = |kit_depends: &str| {
            let modules = base_class_modules(kit_depends, OPAQUE_BASE);
            let index = Index::build(&modules, None);
            let reader = index.scope(index.by_name["base"]);
            index.hidden_outside("kit.thing", &reader)
        };
        assert!(!hidden("'base'"));
        assert!(hidden("'extra'"));
    }

    #[test]
    fn a_class_of_base_out_of_the_owner_closure_is_no_read_parent() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = [
            module("base", &manifest("")),
            module("kit", &manifest("'base'")),
            module("extra", &manifest("'kit'")),
        ];
        modules[0]
            .python
            .push(python_18("class B(models.Model):\n    _name = 'base'\n"));
        modules[1]
            .python
            .push(python_18("class Thing(models.Model):\n    _name = 'kit.thing'\n"));
        modules[2].python.push(python_18(
            "class Base(models.AbstractModel):\n    _inherit = 'base'\n    x_flag = fields.Char()\n\n\
             class Thing(Base):\n    _inherit = 'kit.thing'\n",
        ));
        let index = Index::build(&modules, None);
        let reader = index.scope(index.by_name["base"]);
        assert!(index.hidden_outside("kit.thing", &reader));
    }

    #[test]
    fn a_class_of_base_on_a_class_of_a_delegate_hides_parents() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = [
            module("base", &manifest("")),
            module("kit", &manifest("'base'")),
            module("extra", &manifest("'kit'")),
        ];
        modules[0]
            .python
            .push(python_18("class B(models.Model):\n    _name = 'base'\n"));
        modules[1].python.push(python_18(
            "class Parent(models.Model):\n    _name = 'kit.parent'\n\n\
             class Thing(models.Model):\n    _name = 'kit.thing'\n    _inherits = {'kit.parent': 'parent_id'}\n\
             \x20   parent_id = fields.Many2one('kit.parent')\n",
        ));
        modules[2].python.push(python_18(
            "class Parent(models.Model):\n    _inherit = 'kit.parent'\n\n\
             class Base(Parent):\n    _inherit = 'base'\n",
        ));
        let index = Index::build(&modules, None);
        let extra = index.scope(index.by_name["extra"]);
        assert!(index.hidden_parents("kit.thing", &extra));
        assert!(!index.hidden_parents("kit.parent", &extra));
    }

    #[test]
    fn an_imported_base_of_the_chain_is_a_read_parent() {
        let manifest = |depends: &str| format!("{{'version': '18.0.1.0', 'depends': [{depends}]}}");
        let mut modules = [module("base", &manifest("")), module("kit", &manifest("'base'"))];
        modules[0]
            .python
            .push(python_18("class B(models.Model):\n    _name = 'base'\n"));
        let at = |rel: &str, source: &str| SourceFile {
            rel: rel.to_owned(),
            ..python_18(source)
        };
        modules[1].python.push(at(
            "models/stamped.py",
            "class Stamped(models.Model):\n    _name = 'kit.stamped'\n\n\
             class Shadow(models.Model):\n    _name = 'kit.shadow'\n\n\
             Shadow = make(Shadow)\n",
        ));
        modules[1].python.push(at(
            "models/users.py",
            "from .stamped import Stamped, Shadow\n\
             from odoo.addons.kit.models import stamped\n\n\
             class Relative(Stamped):\n    _name = 'kit.relative'\n    _inherit = ['kit.stamped']\n\n\
             class Absolute(stamped.Stamped):\n    _name = 'kit.absolute'\n    _inherit = ['kit.stamped']\n\n\
             class Rebound(Shadow):\n    _name = 'kit.rebound'\n    _inherit = ['kit.shadow']\n",
        ));
        let index = Index::build(&modules, None);
        let kit = index.by_name["kit"];
        let scope = index.scope(kit);
        for model in ["kit.relative", "kit.absolute"] {
            assert!(index.holds_model(kit, model, &scope), "{model}");
            assert!(!index.hidden_field_parents(model, &scope), "{model}");
            assert!(!index.hidden_method_parents(model, &scope), "{model}");
        }
        assert!(index.hidden_field_parents("kit.rebound", &scope));
    }
}
