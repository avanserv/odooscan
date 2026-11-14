//! Rules that need facts from more than one file.

use std::path::Path;
use std::rc::Rc;

use globset::GlobSet;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{
    AccessRow, ActionFact, ActionValue, ActionViewId, Annotation, ArchElement, ArchModifier, ArchSubview, ClassBase,
    ClassFact, DataFacts, DefinitionSource, DomainFact, FieldFact, FieldWrite, Flag, FunctionCall, GuardedRef, JsFacts,
    JsName, MethodDef, Migrate, ModelKind, OptionValue, OrmCall, Pos, PyFacts, RawDiag, RecordFact, RecordWrite,
    Refusal, ReportFact, RouteDef, RouteKey, RuleFact, ServerFact, Signature, Spanned, SqlColumn, SqlView, Subject,
    UNREAD_TYPE, ViewFact, WrittenValue, XmlDef, XmlRef,
};
use crate::grants::{self, AttachmentGrant, Grant, Grants};
use crate::imports::{AddonImport, Fault};
use crate::index::{
    BASE_MODEL_METHODS, Export, FieldDef, FieldInfo, Index, JsSpec, JsTarget, PathError, Placed, RELATIONAL_TYPES,
    Scope, Status, ViewTree, asset_glob_set, base_private_method, full_id, js_lookup, js_url, loaded_data, located,
};
use crate::mentions::HOOK_PREFIXES;
use crate::module_state::UPGRADE_DIRS;
use crate::project::{ASSET_EXTENSIONS, Module, SourceFile, code_loaded};
use crate::routing::{ClassAt, Endpoint, Origin};
use crate::rules::rule;
use crate::series::{self, Series};
use crate::xml::{LOCATOR, QWEB, UNKNOWN_CONTEXT};
use crate::{domain, flows, manifest, modifier, module_state, python};

/// The directory of IoT box drivers and interfaces. The `iot` module zips `iot_handlers/*/*` of
/// every installed module (18.0 enterprise `iot/controllers/main.py:61-65`) and the box loads each
/// file by its path (18.0 `hw_drivers/tools/helpers.py:532-547`), so no import reaches them.
const IOT_DIR: &str = "iot_handlers";

/// The first series that serves a route method whose override drops `@route()`, decorating the
/// override itself and logging a warning (347a3ccf763c, 18.0 `odoo/http.py:843-845`). Before it
/// Odoo skips the method (15.0 `odoo/http.py:977`, 9.0 `openerp/http.py:1007`).
const ROUTE_REDECORATED: Series = Series { major: 15, minor: 3 };

/// The decorators that leave a hook method to run only by its name. `api.onchange` runs it by the
/// fields it lists, and any other may register it elsewhere.
const NAMED_HOOK_DECORATORS: &[&str] = &[
    "api.depends",
    "api.depends_context",
    "api.model",
    "api.multi",
    "api.one",
    "api.returns",
];

/// Types a One2many's inverse may have: an Integer holds the id of a `res_model`/`res_id` pair, and
/// an unread type cannot be judged.
const INVERSE_TYPES: &[&str] = &["Many2one", "Many2oneReference", "Integer", UNREAD_TYPE];

/// The longest identifier PostgreSQL keeps, which Odoo checks a relation table name against.
const PG_NAME_MAX: usize = 63;

/// Directories whose XML and CSV files are not meant to be listed in the manifest.
const UNLISTED_DIRS: &[&str] = &["tests", "i18n", "migrations", "upgrades", "doc", "docs", "examples"];

/// Names 17.0 lets a modifier read with a deprecation warning, and saas~17.1 no longer does.
const ACTIVE_NAMES: &[&str] = &["active_id", "active_ids", "active_model"];
const ACTIVE_NAMES_UNTIL: Series = Series { major: 17, minor: 1 };

/// View types whose arch elements list every field the view has. The others also take fields from
/// attributes, as a calendar does from `date_start`.
const VIEW_FIELD_KINDS: &[&str] = &["form", "tree", "kanban"];

/// The one subview with fields its `<field>` elements do not list: Odoo checks a graph on those alone
/// (`odoo/addons/base/models/ir_ui_view.py:1407` on 17.0), and any tag it does not nest (`:1473`) on
/// the outer view.
const ATTRIBUTE_SUBVIEW: &str = "calendar";

/// View types Odoo generates for any model. `activity` is not one: `ACTIVITY_IN_MAIL` says why.
const GENERATED_VIEWS: &[&str] = &["form", "search", "list", "tree", "pivot", "kanban", "graph"];

/// Fields Odoo's generated calendar takes its start from, after the model's `_date_name`.
const CALENDAR_STARTS: &[&str] = &["date", "date_start", "x_date", "x_date_start"];

/// Fields Odoo's generated calendar takes its end or its duration from.
const CALENDAR_STOPS: &[&str] = &[
    "date_stop",
    "date_end",
    "x_date_stop",
    "x_date_end",
    "date_delay",
    "planned_hours",
    "x_date_delay",
    "x_planned_hours",
];

/// 19.0 refuses to generate a calendar without a stop (`odoo/addons/base/models/ir_ui_view.py:2898`),
/// saas~19.1 does not (`:2909`).
const CALENDAR_STOP_OPTIONAL: Series = Series { major: 19, minor: 1 };

/// The first series that loads a record nested in a one2many field (`2b80f3e5f1be`). Below it the
/// schema refuses the file (`odoo/tools/convert.py:797` on 15.0), so a row nested in `view_ids`
/// gives the action no view.
const NESTED_RECORDS: Series = Series { major: 15, minor: 1 };

/// The first series whose `activity` default comes with `mail` (`b2de618f9ff9`), so a model outside
/// it has none (`odoo/addons/base/models/ir_ui_view.py:2711` on 18.0), and whose client searches
/// `activity_ids` (`addons/mail/static/src/views/web/activity/activity_model.js:10` on 18.0). From
/// saas~11.5 up to here `BaseModel` gives the default (`odoo/models.py:1292` on 12.0), and before
/// saas~11.2 the client drops the type. saas~11.2 to 11.4 load it with no default
/// (`addons/web/static/src/js/chrome/action_manager_act_window.js:304` on saas~11.3), which the
/// rule leaves unreported.
const ACTIVITY_IN_MAIL: Series = Series { major: 12, minor: 4 };

/// The first series whose client asks the server for each view by the kind the action lists
/// (`addons/web/static/src/js/view_manager.js:135` on 10.0), which fails on a kind with no view
/// (`odoo/models.py:1417`). On 9.0 the view class asks for its own `view_type`
/// (`addons/web/static/src/js/framework/view.js:53`). `9.saas~9` to `9.saas~11` read as an unknown
/// series, so they report as this series does.
const VIEWS_BY_ACTION_KIND: Series = Series { major: 10, minor: 0 };

/// The first series that validates a record rule's domain when it writes one
/// (`odoo/addons/base/models/ir_rule.py:63` on 18.0). 14.0 evaluates it only when a query applies the
/// rule (`:149`), so only the last write can fail.
const RULE_DOMAIN_VALIDATED: Series = Series { major: 15, minor: 0 };

/// The first series that looks the fields of a view's domains and `group_by` up on their model
/// (`aae1d5782968`, `_get_server_domain_variables`), and those of a string `domain` on a field the
/// view lets a user edit. Before it a domain names only what the view must hold.
const VIEW_DOMAIN_VALIDATED: Series = Series { major: 13, minor: 1 };

/// The first series whose access check reads the rows of a transient model (`6d8688bb124d`; saas~13.1
/// lets every user through, `odoo/addons/base/models/ir_model.py:1500`).
const TRANSIENT_ACCESS: Series = Series { major: 13, minor: 2 };

/// The first series that gives a child action the model of its parent as it creates the child
/// (`45a5562fc41b`, `odoo/addons/base/models/ir_actions.py:645` on saas~18.2). `write` leaves the
/// model alone (`:714` on 20.0), so this says nothing of a child linked after its creation.
const CHILD_ACTION_MODEL: Series = Series { major: 18, minor: 2 };

/// The first series whose server action defaults to `object_write`, where 10.0 defaults to `code`
/// (`odoo/addons/base/ir/ir_actions.py:495`). `c56b74622141` changed it on saas-15, which no `Series`
/// names.
const OBJECT_WRITE_DEFAULT: Series = Series { major: 11, minor: 0 };

/// The first series with a constraint that compiles the `code` of a server action on each write
/// (`odoo/addons/base/ir/ir_actions.py:608-611` on 10.0). saas-12 adds it, which no `Series` names.
/// 9.0 has none (`openerp/addons/base/ir/ir_actions.py:706-712`): only running the action compiles
/// it (`:919`, reached at `:1086`).
const CODE_CONSTRAINT: Series = Series { major: 10, minor: 0 };

/// The first series whose cron has `code`, through its delegated server action
/// (`odoo/addons/base/ir/ir_cron.py:50-52` on 11.0). saas-14 delegates first, which no `Series`
/// names. Below it Odoo drops the field (`odoo/models.py:3876` on 10.0), so nothing compiles or runs.
const CRON_CODE: Series = Series { major: 11, minor: 0 };

/// The first series whose `Registry` has no `__call__` (`e6f99537f3aa`; saas~18.4 keeps it at
/// `odoo/orm/registry.py:311`), so calling `model.pool` fails.
const REGISTRY_UNCALLABLE: Series = Series { major: 19, minor: 0 };

/// The first series without `ir.model.access`: `odoo/addons/base/__manifest__.py:81` loads
/// `ir.access.csv` where saas~19.3 loads `ir.model.access.csv` (`:78`). The loader calls `env[model]`
/// on the model of the file name (`odoo/tools/convert.py:776` on saas~19.4), so loading an
/// `ir.model.access` row fails.
const IR_ACCESS: Series = Series { major: 19, minor: 4 };

/// The first series without `ir.property` (`de302c2d3630`, in no saas-17 branch).
const IR_PROPERTY_GONE: Series = Series { major: 18, minor: 0 };

/// The first series whose loader skips a `name@fr` of a record or a CSV column, which the load of a
/// language's terms reads as a translation (`b7f05277026a`; `odoo/tools/convert.py:396,704` on
/// saas~18.2).
/// saas~18.1 sends the name on to `create`, or the import refuses it.
const DATA_TRANSLATIONS: Series = Series { major: 18, minor: 2 };

/// The first series with `ir.actions.report`. Through 10.0 the model is `ir.actions.report.xml`, and
/// a `render_html` replaces the template (`addons/report/models/report.py:138` on 10.0).
const REPORT_MODEL: Series = Series { major: 11, minor: 0 };

/// The first series that asserts a `model_field` as it sets the field up (`37d701c153fe`, in no saas
/// branch). Before, only a run reads it.
const MODEL_FIELD_ASSERTED: Series = Series { major: 20, minor: 0 };

/// The first series whose import compares a Selection cell in any case
/// (`odoo/addons/base/models/ir_fields.py:319` on saas~14.3). saas~14.2 compares it exactly (`:266`).
const CSV_ANY_CASE: Series = Series { major: 14, minor: 3 };

/// The first series that keeps a selection's values in `_selection`, empty for a tuple, so a write
/// checks nothing (`odoo/fields.py:2763,2883` on saas~17.2). Before, a write reads the values from
/// the field, tuple included (`:2865` on 17.0).
const TUPLE_UNCHECKED: Series = Series { major: 17, minor: 2 };

/// The first series whose `selection_add` takes a tuple as its base (`odoo/orm/fields_selection.py:99`
/// on saas~18.1). Before, `odoo/fields.py:2932` on 18.0 leaves the tuple no values, so the
/// `selection_add` fails.
const TUPLE_LISTED: Series = Series { major: 18, minor: 1 };

/// The first series that reads a lone definition's tuple into `_selection`, so a write checks it
/// (`odoo/orm/fields_selection.py:101` on saas~19.3). From `TUPLE_LISTED`, `:95` on saas~19.2 returns
/// before that for a lone definition: only several definitions or a field from another model get
/// there, which `listed_values` cannot tell apart from a lone one.
const TUPLE_CHECKED: Series = Series { major: 19, minor: 3 };

/// The first series that loads the XML of a bundle with its scripts
/// (`odoo/addons/base/models/assetsbundle.py:420` on 16.0). It never loads `web.assets_qweb`,
/// which 15.0 fetches apart (`addons/web/models/ir_http.py:72`).
const XML_IN_BUNDLES: Series = Series { major: 16, minor: 0 };

/// The first series that serves assets from `static/` alone: it globs only under `<addon>/static/`
/// (`odoo/addons/base/models/ir_asset.py:332-334` on saas~16.3), where `:330-340` on saas~16.2
/// serves a script or a style from anywhere in the module and XML only from `static/`.
const STATIC_ASSETS: Series = Series { major: 16, minor: 3 };

/// The bundles that are templates up to saas~14.2 (`addons/web/views/webclient_templates.xml:56`),
/// and from saas~14.3 only the manifest's `assets` declares (`addons/web/__manifest__.py:61`).
const BUNDLE_TEMPLATES: [&str; 3] = ["web.assets_backend", "web.assets_common", "web.assets_frontend"];

/// The series whose loader reads `<report>` as a write to `ir.actions.report`. 18446f23f11e removes
/// the tag in saas~16.1 (`_tag_report`, `odoo/tools/convert.py:275` on 16.0).
const REPORT_TAG: &str = ">=11.0,<saas~16.1";

/// A name a modifier reads, with the view it sits in and the field it must find on `owner`.
struct ModifierName<'a> {
    model: &'a str,
    view: &'a ViewFact,
    kind: &'a str,
    attr: &'a str,
    name: &'a str,
    owner: String,
    field: &'a str,
    /// The subview steps left after each `parent.`: the level whose fields Odoo checks the name on.
    level: Vec<String>,
    /// The subviews the modifier's own arch shows, which take precedence over `ViewTree::subviews`.
    arch_subviews: &'a [ArchSubview],
    /// The field a spec locates to open the enclosing subview, which may match in any subview.
    locator: Option<&'a str>,
}

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
    manifest_keys(index, module, &mut out);
    upgrade_folders(module, series, &mut out);
    upgrade_scripts(module, series, &mut out);
    unmatched_assets(module, series, &mut out);
    unread_template_lists(module, series, &mut out);
    bundle_templates(module, &mut out);
    unloaded_files(module, series, &mut out);
    unimported_files(index, mi, &mut out);
    ids_repeated_in_a_file(module, series, &mut out);
    ids_repeated_across_files(module, series, &mut out);
    same_module_xmlids(index, mi, series, &mut out);
    shadowed_members(module, series, &mut out);
    legacy_hook_rollbacks(module, series, &mut out);
    cache_key_names(module, series, &mut out);
    import_patches(index, module, &mut out);
    public_writes(index, mi, series, &mut out);
    refused_server_code(index, mi, series, &mut out);
    refused_file_names(index, mi, series, &mut out);
    removed_escapes(index, mi, &mut out);
    renamed_lists(index, mi, &mut out);
    dropped_markup(index, mi, &mut out);
    for (_, file) in es_modules(index, mi, series) {
        if let Some(e) = &file.facts.syntax_error {
            out.push(&file.path, "OD000", e.pos, e.value.clone());
        }
        if !file.in_tests() {
            private_rpc(&mut out, file);
        }
    }
    let scope = index.scope(mi);
    // Odoo's own rows are needed to tell that none grants, so without `base` nothing does.
    let grants = index
        .has_base
        .then(|| Grants::of(index, &scope, series, reads_model_access(series)));
    let grant = grants.as_ref().map(AttachmentGrant::of).unwrap_or_default();
    granted_attachments(module, grant, &mut out);
    flows::check(index, mi, &scope, grant, &mut out);
    if let Some(grants) = grants {
        let setup = index.setup_scope(mi);
        let installs = index.install_scope(mi, series);
        let loaded = index.loaded_scope(mi, &scope, &installs);
        let built = BuiltModels::of(module);
        let ctx = Ctx {
            index,
            mi,
            module,
            scope: &scope,
            setup: &setup,
            installs: &installs,
            loaded: &loaded,
            dynamic: module.python.iter().any(|f| f.facts.dynamic_models),
            built: &built,
            partial: index.partial_scope(&scope),
            lost: index.lost_scope(&scope),
            series,
            grants: &grants,
        };
        ctx.dependencies(&mut out);
        ctx.addon_imports(&mut out);
        ctx.python(&mut out);
        ctx.routes(&mut out);
        ctx.registry(&mut out);
        ctx.data(&mut out);
        for (fi, file) in es_modules(index, mi, series) {
            ctx.js(&mut out, fi, file);
            // Only the imports of a test are checked: a unit test runs on a mock server whose models it
            // defines (18.0 `addons/web/static/tests/_framework/mock_server/mock_server.js:386`).
            if file.in_tests() {
                continue;
            }
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

/// Whether Odoo serves the controllers of a file of a module: those of `tests/` only run with the
/// tests, and upgrade scripts are no module code.
fn served(rel: &str) -> bool {
    let dir = rel.split('/').next().unwrap_or_default();
    dir != "tests" && !UPGRADE_DIRS.contains(&dir)
}

/// The paths a route spells out, none when it leaves them to the route it overrides or computes
/// one of them.
fn literal_urls(route: &RouteDef) -> &[String] {
    match &route.urls {
        RouteKey::Given(urls) => urls,
        RouteKey::Absent | RouteKey::Unknown => &[],
    }
}

/// The endpoints Odoo builds of the controllers a module sees, by each literal path, for OD231.
struct Paths<'t> {
    endpoints: Vec<Endpoint<'t>>,
    by_url: FxHashMap<&'t str, Vec<usize>>,
}

impl<'t> Paths<'t> {
    fn of(endpoints: Vec<Endpoint<'t>>) -> Self {
        let mut by_url: FxHashMap<&str, Vec<usize>> = FxHashMap::default();
        for (i, endpoint) in endpoints.iter().enumerate() {
            for url in endpoint.urls {
                by_url.entry(url).or_default().push(i);
            }
        }
        Self { endpoints, by_url }
    }

    /// OD231 on each path a route of `module` gives an endpoint that another endpoint serves too,
    /// as the route and the message. Each clash is reported once, where `outranks` leaves it.
    fn shadowed(&self, index: &Index, module: &str) -> Vec<(Origin<'t>, String)> {
        let mut found = vec![];
        let mut reported = FxHashSet::default();
        for ours in &self.endpoints {
            for &origin in ours.origins.iter().filter(|o| o.at.addon == module) {
                for url in ours.urls {
                    let mut others = self.by_url[url.as_str()].iter().map(|&i| &self.endpoints[i]);
                    let Some(theirs) = others.find(|theirs| clashes(index, (ours, origin), theirs)) else {
                        continue;
                    };
                    if reported.insert((std::ptr::from_ref(origin.route), url)) {
                        let other = theirs.origins[0];
                        let message = format!(
                            "path '{url}' is also routed by {}.{} in module {}, so one of the two hides the other",
                            other.at.class.name, other.route.method, other.at.addon
                        );
                        found.push((origin, message));
                    }
                }
            }
        }
        found
    }
}

/// Whether OD231 reports at `origin` of `ours` that `theirs` serves a path of it too: they share no
/// route, a request method may reach either, and no route of `theirs` reports the clash instead.
fn clashes(index: &Index, (ours, origin): (&Endpoint, Origin), theirs: &Endpoint) -> bool {
    let shared = ours
        .origins
        .iter()
        .any(|o| theirs.origins.iter().any(|t| std::ptr::eq(o.route, t.route)));
    !shared
        && overlap(ours.methods.as_ref(), theirs.methods.as_ref())
        && !theirs.origins.iter().any(|&other| outranks(index, other, origin))
}

/// Whether a clash between two routes is reported at `other` rather than at `origin`: of one
/// module, the later route, and of two, the one that sees the other and loads after it, so the
/// module that brings the clash in.
fn outranks(index: &Index, other: Origin, origin: Origin) -> bool {
    if other.at.addon == origin.at.addon {
        return (other.at.rel, other.route.pos) > (origin.at.rel, origin.route.pos);
    }
    let (Some(&theirs), Some(&ours)) = (index.by_name.get(other.at.addon), index.by_name.get(origin.at.addon)) else {
        return false;
    };
    index.sees(theirs, ours) && index.loads_before(ours, theirs)
}

/// Whether a request may take either of two lists of methods, `None` inside letting every method
/// through. Methods the scan does not read may be any, so they overlap none.
fn overlap(a: Option<&Option<Vec<String>>>, b: Option<&Option<Vec<String>>>) -> bool {
    match (a, b) {
        (Some(Some(a)), Some(Some(b))) => a.iter().any(|m| b.iter().any(|n| m.eq_ignore_ascii_case(n))),
        (Some(_), Some(_)) => true,
        _ => false,
    }
}

/// OD229's message: what the override of `method` refuses of the parent definition of `module`.
fn refusal_message(method: &str, module: &str, parent: &Signature, refusal: &Refusal) -> String {
    let parent = format!("{} in module {module}", parent.spelled(method));
    match refusal {
        Refusal::Lost(p) => format!(
            "this override takes no '{}' by position, which {parent} requires, so a call that passes it fails",
            p.name
        ),
        Refusal::Required(p) => format!(
            "this override requires '{}', which {parent} does not take, so a call made for the parent fails",
            p.name
        ),
        Refusal::Keyword(p) => format!(
            "this override takes no keyword '{}', which {parent} takes, so a call that passes it fails",
            p.name
        ),
    }
}

/// OD230: whether Odoo could run `name` as the hook its prefix says, by what it passes after the
/// records. A field's search method gets the operator and the value (18.0 `odoo/fields.py:1504`), a
/// compute, an inverse and an `@api.onchange` method nothing (`:1500`, 20.0
/// `odoo/orm/models.py:4453`), and a view's `on_change` attribute whatever it spells (10.0
/// `odoo/models.py:5460`). A `def` no such call fits is a helper that only shares the prefix.
fn runs_as_hook(name: &str, signature: Option<&Signature>) -> bool {
    let Some(prefix) = HOOK_PREFIXES.iter().find(|p| name.starts_with(**p)) else {
        return false;
    };
    let count = match *prefix {
        "onchange_" => return true,
        "_search_" => 2,
        _ => 0,
    };
    signature.is_some_and(|s| s.takes_positional(count))
}

/// Whether Odoo ignores the access rows of a model of this kind.
fn ignores_access(kind: Option<ModelKind>, series: Option<Series>) -> bool {
    kind == Some(ModelKind::Transient) && series.is_some_and(|s| s < TRANSIENT_ACCESS)
}

/// Whether a module of `series` can load a row of `ir.model.access`: an unknown series may.
fn reads_model_access(series: Option<Series>) -> bool {
    series.is_none_or(|s| s < IR_ACCESS)
}

/// OD401's message for `model`. An `ir.access` row without a group only restricts, so the message
/// asks for a group where the series reads `ir.access`.
fn no_access_message(model: &str, series: Option<Series>) -> String {
    if reads_model_access(series) {
        format!("model '{model}' has no ir.model.access rule, so only the superuser can use it")
    } else {
        format!("model '{model}' has no ir.access rule with a group, so only the superuser can use it")
    }
}

/// Whether a model's `name` holds a callable object in `series`, so calling it needs no method: an
/// `Environment` on every series, and a `Registry` before `REGISTRY_UNCALLABLE`, which an unknown
/// series may be. Only code running on records needs this: no series lets a button call them, and
/// a `<function>` that can, from saas~18.1, gains nothing by it, so `method_gap` still reports both.
fn callable_attr(name: &str, series: Option<Series>) -> bool {
    name == "env" || (name == "pool" && series.is_none_or(|s| s < REGISTRY_UNCALLABLE))
}

/// What OD402 says of a row of `module`, if it writes for public or portal users.
fn public_write_message(row: &AccessRow, module: &str, series: Option<Series>) -> Option<String> {
    if let Some(group) = &row.public_write {
        return Some(format!(
            "{group} gets write, create or unlink on {}; public and portal access should be read-only and go through controllers",
            row.model
        ));
    }
    let everyone = row.group.as_ref().is_some_and(|g| {
        let group = full_id(module, g);
        group == grants::EVERYONE && grants::public_group(&group, series)
    });
    if everyone && row.grants.writes() {
        return Some(format!(
            "base.group_everyone, public and portal users included, gets write, create or unlink on {}; public and portal access should be read-only and go through controllers",
            row.model
        ));
    }
    row.global_write.then(|| {
        format!(
            "no group limits this row, so every user, public and portal included, gets write, create or unlink on {}; name a group",
            row.model
        )
    })
}

/// What OD503 says of `dep`, a module the scan does not index, given how the Odoo of its series
/// reads its manifest when odooscan rejects it. A manifest that Odoo reads but odooscan does not
/// gets its own wording, since its notice alone says why.
fn unknown_dependency(dep: &str, rejected: Option<&manifest::OdooReads>) -> String {
    match rejected {
        Some(manifest::OdooReads::No) => format!("Odoo cannot read the manifest of module '{dep}'"),
        Some(manifest::OdooReads::AsPython2 | manifest::OdooReads::AsExpression) => {
            format!("odooscan cannot read the manifest of module '{dep}'")
        }
        None => format!("module '{dep}' is not on the addons path"),
    }
}

/// OD402 on the rows that write for public or portal users, in the files Odoo loads. A row with no
/// group of an id written twice may hold another value in the database. A plain `model_id` cell may
/// also hold the model's description, which finds no kind, so that row reports.
fn public_writes<'m>(index: &Index<'m>, mi: usize, series: Option<Series>, out: &mut Out<'m>) {
    let module = &index.modules[mi];
    let scope = index.scope(mi);
    let rewritten = |id: &String| index.xmlid_defs(&full_id(&module.name, id), series).len() > 1;
    let kind = |row: &AccessRow| {
        let id = full_id(&module.name, &row.model);
        let model = if row.by_name {
            Some(row.model.as_str())
        } else {
            index.model_of_xmlid(&id)
        };
        model.and_then(|m| index.kind(m, &scope))
    };
    let replaced = !reads_model_access(series);
    for file in loaded_data(module, series) {
        let rows = file.facts.access.iter().filter(|r| !(replaced && r.model_access));
        let rows = rows.filter(|r| !(r.global_write && r.id.as_ref().is_some_and(rewritten)));
        for row in rows.filter(|r| !ignores_access(kind(r), series)) {
            if let Some(message) = public_write_message(row, &module.name, series) {
                out.push(&file.path, "OD402", row.pos, message);
            }
        }
    }
}

/// OD319 on the code of what the manifest loads. Only a loaded record reaches what compiles it, and
/// compiling needs no index, so this runs without base.
fn refused_server_code<'m>(index: &Index<'m>, mi: usize, series: Option<Series>, out: &mut Out<'m>) {
    let module = &index.modules[mi];
    let scope = index.scope(mi);
    let demo_scope = scope.in_demo();
    for file in module.data.iter().filter(|f| f.rank.is_some()) {
        let scope = if file.demo { &demo_scope } else { &scope };
        for server in &file.facts.servers {
            let Some(refused) = &server.refused else { continue };
            let held = || runs_held_code(index, scope, series, server, &server_writes(index, mi, server));
            if let Some(when) = code_refusal(series, server.cron, held) {
                let message = format!("Odoo refuses this code {when}: {}", refused.value);
                out.push(&file.path, "OD319", refused.pos, message);
            }
        }
    }
}

/// When Odoo refuses broken code, if ever. From `CODE_CONSTRAINT` every write compiles, whatever the
/// `state`, or from `CRON_CODE` on a cron. Below that only running compiles, so `held` must say the
/// record runs that code. An unknown series may be on either side, so it takes every write.
fn code_refusal(series: Option<Series>, cron: bool, held: impl FnOnce() -> bool) -> Option<&'static str> {
    let gate = if cron { CRON_CODE } else { CODE_CONSTRAINT };
    match series {
        Some(series) if series >= gate => Some("when it loads the record"),
        Some(_) => held().then_some("when the action runs"),
        None => Some("when it loads the record from 10.0, or when the action runs below it"),
    }
}

/// The writes to the record of `server` in load order, `server` alone when it has no id.
fn server_writes<'m>(index: &Index<'m>, mi: usize, server: &'m ServerFact) -> Vec<Placed<&'m ServerFact>> {
    let id = server.id.as_ref().map(|id| full_id(&index.modules[mi].name, id));
    let writes = id.and_then(|id| index.server_writes.get(&id));
    let own = || {
        vec![Placed {
            module: mi,
            demo: false,
            fact: server,
        }]
    };
    writes.map_or_else(own, Vec::clone)
}

/// Whether the record of `server` still holds its code and runs it, each field holding the last value
/// `scope` gave it: what a module depending on this one replaces later does not mend a database
/// without that module. A cron runs code without saying so, from `CRON_CODE`, and so does a server
/// action `created_as_code`.
fn runs_held_code<'m>(
    index: &Index<'m>,
    scope: &Scope,
    series: Option<Series>,
    server: &'m ServerFact,
    writes: &[Placed<&'m ServerFact>],
) -> bool {
    if server.cron && series.is_some_and(|s| s < CRON_CODE) {
        return false;
    }
    let last = || writes.iter().rev().filter(|w| scope.reads(w));
    let holds = last()
        .find(|w| w.fact.writes_code)
        .is_some_and(|w| std::ptr::eq(w.fact, server));
    let runs = last().find_map(|w| w.fact.state);
    holds && runs.unwrap_or(server.cron || created_as_code(index, scope, series, writes))
}

/// Whether `writes` create the action on a series that fills an unset `state` with `code`. A first
/// write to another module's action creates nothing: the action may hold any state, and an update
/// leaves it. An unknown series may be on either side of `OBJECT_WRITE_DEFAULT`.
fn created_as_code<'m>(
    index: &Index<'m>,
    scope: &Scope,
    series: Option<Series>,
    writes: &[Placed<&'m ServerFact>],
) -> bool {
    let creates = |w: &Placed<&'m ServerFact>| {
        let module = &index.modules[w.module];
        w.fact.id.as_ref().is_none_or(|id| own_id(module, id))
    };
    series.is_some_and(|s| s < OBJECT_WRITE_DEFAULT) && writes.iter().find(|w| scope.reads(w)).is_some_and(creates)
}

/// Whether a module of `series` reads a write to a report: a series it cannot tell gets no `<report>`.
fn reads_report(series: Option<Series>, tag: bool) -> bool {
    match series {
        Some(series) if tag => series::matches(REPORT_TAG, series),
        Some(series) => series >= REPORT_MODEL,
        None => !tag,
    }
}

/// The writes that give the record of `report` its fields once `scope` has loaded, in load order.
/// What a module depending on this one replaces later does not mend a database without that module.
fn report_state<'m>(
    index: &Index<'m>,
    mi: usize,
    scope: &Scope,
    series: Option<Series>,
    report: &'m ReportFact,
) -> Vec<&'m ReportFact> {
    let own = [Placed {
        module: mi,
        demo: false,
        fact: report,
    }];
    let id = report.id.as_ref().map(|id| full_id(&index.modules[mi].name, id));
    let writes = id.and_then(|id| index.report_writes.get(&id));
    let writes = writes.map_or(&own[..], Vec::as_slice);
    let read = writes
        .iter()
        .filter(|w| scope.reads(w) && reads_report(series, w.fact.tag));
    read.map(|w| w.fact).collect()
}

/// OD319 on the `print_report_name` of what the manifest loads. Print compiles only the value the
/// record holds, unlike the constraint on `code`, and compiling needs no index, so this runs without
/// base.
fn refused_file_names<'m>(index: &Index<'m>, mi: usize, series: Option<Series>, out: &mut Out<'m>) {
    let module = &index.modules[mi];
    let scope = index.scope(mi);
    let demo_scope = scope.in_demo();
    for file in module.data.iter().filter(|f| f.rank.is_some()) {
        let scope = if file.demo { &demo_scope } else { &scope };
        for report in &file.facts.reports {
            let Some(refused) = &report.refused else { continue };
            let state = report_state(index, mi, scope, series, report);
            let holder = state.iter().rev().find(|r| r.writes_file_name);
            if holder.is_some_and(|r| std::ptr::eq(*r, report)) {
                let message = format!(
                    "Odoo refuses this expression when it prints the report: {}",
                    refused.value
                );
                out.push(&file.path, "OD319", refused.pos, message);
            }
        }
    }
}

/// OD325 on the `t-esc` of a view the server renders, which needs no index: a view whose tree is out
/// of reach falls back to its own type. The web client still reads `t-esc` in the archs it compiles,
/// such as a kanban's.
fn removed_escapes<'m>(index: &Index<'m>, mi: usize, out: &mut Out<'m>) {
    let module = &index.modules[mi];
    for file in module.data.iter().filter(|f| f.rank.is_some()) {
        for view in file.facts.views.iter().filter(|v| qweb_view(index, module, v)) {
            let guarded = || tree_holds(index, mi, view, |_, v| v.escape_specs);
            push_guarded(out, &file.path, &view.escapes, guarded);
        }
    }
}

/// OD306 on each `<tree>` of a view, unfixed while `tree_guarded` holds.
fn renamed_lists<'m>(index: &Index<'m>, mi: usize, out: &mut Out<'m>) {
    for file in &index.modules[mi].data {
        for view in &file.facts.views {
            push_guarded(out, &file.path, &view.renamed, || tree_guarded(index, mi, view));
        }
    }
}

/// Whether a view in the inheritance tree of `view` finds a `<tree>` by its name: an xpath naming
/// `tree`, or an unchecked module, which the fix leaves as it is, holding a `<tree>` spec or element.
/// A `<tree>` spec of a checked module guards too while the tree adds a `<tree>` it may find, since
/// `--exclude`, the baseline or a write skip could apply one of the two fixes without the other.
fn tree_guarded(index: &Index, mi: usize, view: &ViewFact) -> bool {
    let finds_tree =
        |m: usize, v: &ViewFact| v.tree_paths || (!index.modules[m].checked && (v.tree_specs || !v.renamed.is_empty()));
    tree_holds(index, mi, view, finds_tree)
        || (tree_holds(index, mi, view, |_, v| v.tree_specs) && tree_holds(index, mi, view, |_, v| v.tree_elements))
}

/// Reports `diags`, without their fixes when `guarded` says the tree forbids them.
fn push_guarded<'m>(out: &mut Out<'m>, path: &'m Path, diags: &[RawDiag], guarded: impl FnOnce() -> bool) {
    let unfixed = diags.iter().any(|d| d.fix.is_some()) && guarded();
    out.diags.extend(diags.iter().map(|d| {
        let mut d = d.clone();
        if unfixed {
            d.fix = None;
        }
        (path, d)
    }));
}

/// Whether `view` of module `mi`, or a view in its inheritance tree as far as the index holds it,
/// satisfies `holds`, given its module. Views outside the scan are out of reach.
fn tree_holds(index: &Index, mi: usize, view: &ViewFact, holds: impl Fn(usize, &ViewFact) -> bool) -> bool {
    let start = view.inherit.as_deref().or(view.id.as_deref());
    let tree = |start: &str| {
        let top = index.views.top(full_id(&index.modules[mi].name, start));
        index.views.any(&top, &holds)
    };
    holds(mi, view) || start.is_some_and(tree)
}

/// OD326 and OD327 on the markup the web client stops reading, unless the view's tree is QWeb, as a
/// record inheriting a `<template>` is: the server renders its markup as it is.
fn dropped_markup<'m>(index: &Index<'m>, mi: usize, out: &mut Out<'m>) {
    let module = &index.modules[mi];
    for file in &module.data {
        for view in file.facts.views.iter().filter(|v| !qweb_view(index, module, v)) {
            out.diags
                .extend(view.dropped.iter().map(|d| (file.path.as_path(), d.clone())));
        }
    }
}

/// The type at the root of the view's inheritance. None when the view has neither a parent nor an
/// id, or the type of the root is not known.
fn root_kind<'m>(index: &Index<'m>, module: &Module, view: &ViewFact) -> Option<&'m str> {
    let start = view.inherit.as_deref().or(view.id.as_deref())?;
    index.view_kind(full_id(&module.name, start))
}

/// Whether the view renders on the server. The tree's root decides, since a kanban arch runs
/// `t-call` in the client whatever the type of a view extending it.
fn qweb_view(index: &Index, module: &Module, view: &ViewFact) -> bool {
    root_kind(index, module, view).or(view.kind.as_deref()) == Some(QWEB)
}

/// The module's scripts that a bundle loads as ES modules: annotated, or from 17.0 on, any script
/// not marked `ignore`, all of them under `static/src` or `static/tests`. A script that is not one
/// cannot use `import`.
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

/// The first series with Hoot, whose mock of `@web/core/utils/functions` adds `clearMemoizeCaches`
/// (18.0 `addons/web/static/tests/_framework/mock_functions.hoot.js:42`, `module_set.hoot.js:558`).
const HOOT: Series = Series { major: 18, minor: 0 };
/// The first series whose Hoot mocks `@web/core/utils/indexed_db`, adding `_OriginalIndexedDB`
/// (commit `f3d955b3235c`; 20.0 `addons/web/static/tests/_framework/mock_indexed_db.hoot.js:159`).
const INDEXED_DB_MOCKED: Series = Series { major: 18, minor: 3 };

/// The names a Hoot mock adds to a module in a unit test, with the first series that adds each. The
/// other mocks only replace what the file exports.
const HOOT_ADDED: &[(&str, &str, Series)] = &[
    ("@web/core/utils/functions", "clearMemoizeCaches", HOOT),
    ("@web/core/utils/indexed_db", "_OriginalIndexedDB", INDEXED_DB_MOCKED),
];

/// Whether a test script at the URL `from` gets `name` from a Hoot mock rather than from the file.
fn hoot_adds(series: Option<Series>, from: &str, name: &JsName) -> bool {
    let Some(lookup) = js_lookup(from, &name.specifier) else {
        return false;
    };
    HOOT_ADDED
        .iter()
        .any(|&(m, added, since)| m == lookup.name && added == name.name.value && series.is_none_or(|s| s >= since))
}

/// The bundles of each web client page. The backend page loads `web.assets_web`, which includes
/// `web.assets_backend`; the frontend page loads `web.assets_frontend_minimal` and
/// `web.assets_frontend_lazy`, which includes `web.assets_frontend` (18.0
/// `addons/web/views/webclient_templates.xml:54`, `:58`, `:304`, `addons/web/__manifest__.py:147-148`,
/// `:249-250`). A bundle that code loads lazily is on no page: it joins one that holds others.
const PAGES: &[&[&str]] = &[
    &["web.assets_backend", "web.assets_web"],
    &[
        "web.assets_frontend",
        "web.assets_frontend_minimal",
        "web.assets_frontend_lazy",
    ],
];

/// The first bundle of a page that loads the importer but none of the target's bundles. A target in
/// no bundle is left alone, since an `ir.asset` record may load it.
fn page_miss(importer: &[&str], target: &[&str]) -> Option<&'static str> {
    if target.is_empty() {
        return None;
    }
    PAGES.iter().find_map(|page| {
        let loads = page.iter().find(|b| importer.contains(b))?;
        (!target.iter().any(|b| page.contains(b))).then_some(*loads)
    })
}

/// The bundle names, the first three of them and how many more.
fn bundle_list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [init @ .., last] if names.len() <= 3 => format!("{} and {last}", init.join(", ")),
        _ => format!("{} and {} more", names[..3].join(", "), names.len() - 3),
    }
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

/// The record attributes `env`, `ids` and `pool`, and the class attribute `_table_query`.
const MODEL_ATTRS: &[&str] = &["env", "ids", "pool", "_table_query"];

/// Members of `BaseModel` that only some series have, by the range of series that has them. 18.0
/// deprecates `check_access_rights` and `check_access_rule` for `check_access` and `has_access`
/// (`odoo/models.py:4503-4530`). The others are in 18.0 `odoo/models.py` or 20.0 `odoo/orm`, not
/// both. Every series was read for `_table_query`, which came in saas~13.5, and for
/// `_condition_to_sql`, which came in saas~17.1 and left `BaseModel` for `Field`, where a model
/// field does not shadow it, in saas~18.4. For the other names 19.0 was not read, so it matches
/// `>=18.0` but neither `>=20.0` nor `<=18.0`.
const MEMBER_SERIES: &[(&str, &[&str])] = &[
    (">=18.0", &["check_access", "has_access"]),
    (
        ">=20.0",
        &[
            "_access_domain",
            "_additional_allowed_keys_properties_definition",
            "_can_return_content",
            "_has_field_access",
            "_make_access_error_message",
            "_post_model_setup__",
            "_read_grouping_sets",
        ],
    ),
    (
        "<=18.0",
        &[
            "toggle_active",
            "check_access_rights",
            "check_access_rule",
            "_filter_access_rules",
            "check_field_access_rights",
            "_where_calc",
            "_setup_complete",
            "copy_translations",
            "_check_recursion",
            "_check_m2m_recursion",
            "_add_inherited_fields",
        ],
    ),
    (">=13.5,<20.0", &["_table_query"]),
    (">=17.1,<18.4", &["_condition_to_sql"]),
];

/// The names Odoo's cache module binds at run time, by the range of series that has them, read on
/// every series from 10.0 to saas~19.4, so no row reaches before 10.0. Before 19.0 Odoo evaluates
/// an `ormcache` key with the globals of that module (18.0 `odoo/tools/cache.py:90,148`), so a key
/// may read them; from 19.0 it gives the key only `method` (19.0 `odoo/tools/cache.py:136-138`).
const CACHE_KEY_GLOBALS: &[(&str, &[&str])] = &[
    (
        ">=10.0,<19.0",
        &[
            "defaultdict",
            "decorator",
            "logging",
            "unsafe_eval",
            "_logger",
            "ormcache_counter",
            "STAT",
            "ormcache",
            "ormcache_context",
            "log_ormcache_stats",
            "get_cache_key_counter",
        ],
    ),
    (">=10.0,<13.1", &["formatargspec", "getargspec"]),
    (">=10.0,<16.4", &["ormcache_multi", "dummy_cache"]),
    (">=10.0,<18.2", &["cache"]),
    (">=11.0,<12.1", &["pycompat"]),
    (">=13.1,<19.0", &["signature"]),
    (">=13.4,<18.3", &["Counter"]),
    (">=16.2,<19.0", &["Parameter"]),
    (">=16.4,<18.1", &["warnings"]),
    (">=17.2,<19.0", &["time"]),
    (">=18.1,<19.0", &["annotations", "typing"]),
    // saas~18.1 drops the import of `warnings`, and saas~18.2 brings it back to deprecate `skiparg`.
    (">=18.2,<19.0", &["warnings"]),
    (
        ">=18.3,<19.0",
        &[
            "Mapping",
            "Collection",
            "chain",
            "signal",
            "sys",
            "threading",
            "OrmCacheStatsLogger",
            "get_cache_size",
        ],
    ),
    (">=19.0", &["method"]),
];

/// Methods of `BaseModel` that neither 18.0 nor 20.0 has, so the series that do are not known.
/// The other methods are in both, and when they came is not known either, so they count in every
/// series: `grouped`, `fetch` and `search_fetch` among them.
const UNPLACED_MEMBERS: &[&str] = &["copy_multi", "_read", "_name_search"];

/// Python finds a field before the member of `BaseModel` it is named like, so the model loses the
/// member. Odoo never checks for it.
fn shadowed_members<'m>(module: &'m Module, series: Option<Series>, out: &mut Out<'m>) {
    for file in &module.python {
        for field in file.facts.classes.iter().flat_map(|c| &c.fields) {
            if let Some(member) = shadowed_member(&field.name, series) {
                let message = format!(
                    "field '{}' replaces the {member} of BaseModel on this model, so callers get the field instead; rename it",
                    field.name
                );
                out.push(&file.path, "OD116", field.name_pos, message);
            }
        }
    }
}

/// From 16.0 Odoo no longer calls `ir.http._handle_exception()`, so its rollbacks are reported as
/// any other's.
fn legacy_hook_rollbacks<'m>(module: &'m Module, series: Option<Series>, out: &mut Out<'m>) {
    if series.is_some_and(|s| s >= Series { major: 16, minor: 0 }) {
        for file in &module.python {
            let found = file.facts.legacy_hook_rollbacks.iter().cloned();
            out.diags.extend(found.map(|d| (file.path.as_path(), d)));
        }
    }
}

/// OD717 on the public attachments a user creates with their own rights, where `grant` says
/// whether the scanned modules let the portal or public users who reach each function create one.
fn granted_attachments<'m>(module: &'m Module, grant: AttachmentGrant, out: &mut Out<'m>) {
    for file in &module.python {
        let found = file
            .facts
            .granted_attachments
            .iter()
            .filter(|(signed_in, _)| grant.reaches(*signed_in));
        out.diags.extend(found.map(|(_, d)| (file.path.as_path(), d.clone())));
    }
}

/// OD120 on the names an `ormcache` key reads that Odoo's cache module does not bind either.
fn cache_key_names<'m>(module: &'m Module, series: Option<Series>, out: &mut Out<'m>) {
    for file in &module.python {
        let found = file
            .facts
            .cache_key_names
            .iter()
            .filter(|(name, _)| !cache_global(name, series));
        out.diags.extend(found.map(|(_, d)| (file.path.as_path(), d.clone())));
    }
}

/// OD123 on each patch at import, with the advice for a model class when the index, or failing it
/// the path, tells one.
fn import_patches<'m>(index: &Index<'m>, module: &'m Module, out: &mut Out<'m>) {
    for file in &module.python {
        let found = file.facts.import_patches.iter();
        out.diags.extend(found.map(|p| {
            let diag = module_state::diag(p, index.model_class(&p.path));
            (file.path.as_path(), diag)
        }));
    }
}

/// Whether a key may read `name` as a global of Odoo's cache module in `series`. A module of
/// unknown series may run on any, so it may read the names of every series.
fn cache_global(name: &str, series: Option<Series>) -> bool {
    CACHE_KEY_GLOBALS
        .iter()
        .any(|(spec, names)| names.contains(&name) && series.is_none_or(|s| series::matches(spec, s)))
}

/// The member of `BaseModel` in `series` that a field called `name` replaces, as the message
/// names it. A module of unknown series has only the members of every series.
fn shadowed_member(name: &str, series: Option<Series>) -> Option<String> {
    let range = MEMBER_SERIES.iter().find(|(_, names)| names.contains(&name));
    if range.is_some_and(|(spec, _)| !series.is_some_and(|s| series::matches(spec, s))) {
        return None;
    }
    if MODEL_ATTRS.contains(&name) {
        Some(format!("attribute {name}"))
    } else if BASE_MODEL_METHODS.contains(&name) && !UNPLACED_MEMBERS.contains(&name) {
        Some(format!("method {name}()"))
    } else {
        None
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
}

/// The manifest's own findings, OD509 on a version the Odoo of the scan's series will not install,
/// and OD510 on a module in its own `depends`, which Odoo never loads since a module loads once all
/// of its depends have (18.0 `odoo/modules/graph.py:85-104`). The scan's series is the one
/// `--target-series` or `--odoo-version` names, else the module's own.
fn manifest_keys<'m>(index: &Index<'m>, module: &'m Module, out: &mut Out<'m>) {
    let path = module.manifest_path.as_path();
    out.diags
        .extend(module.manifest.diags.iter().map(|d| (path, d.clone())));
    if let Some(version) = &module.manifest.version
        && let Some(message) = manifest::version_refusal(&version.value, index.default_series.or(module.series))
    {
        out.push(path, "OD509", version.pos, message);
    }
    for dep in module.manifest.depends.iter().filter(|d| d.value == module.name) {
        let message = format!("module {} depends on itself, so Odoo never loads it", module.name);
        out.push(path, "OD510", dep.pos, message);
    }
}

/// The scripts Odoo runs from the upgrade folders of `module`, each with its folder as
/// `(dir, folder)`.
fn stage_scripts(module: &Module) -> impl Iterator<Item = ((&str, &str), &SourceFile<PyFacts>)> {
    module
        .python
        .iter()
        .filter_map(|file| Some((module_state::stage_script(&file.rel)?, file)))
}

/// OD514 on an upgrade folder whose scripts the Odoo of `series` never runs, once, at its first
/// script by name. `series` is the one the rule is gated on: `--target-series`, else the module's
/// own, else `--odoo-version`. A version that series refuses is OD509's alone.
fn upgrade_folders<'m>(module: &'m Module, series: Option<Series>, out: &mut Out<'m>) {
    let Some(series) = series.filter(|s| rule("OD514").applies_to(Some(*s))) else {
        return;
    };
    let Some(version) = module
        .manifest
        .version
        .as_ref()
        .filter(|v| manifest::version_refusal(&v.value, Some(series)).is_none())
    else {
        return;
    };
    let mut firsts: FxHashMap<(&str, &str), &'m SourceFile<PyFacts>> = FxHashMap::default();
    for (folder, file) in stage_scripts(module) {
        let first = firsts.entry(folder).or_insert(file);
        if file.rel < first.rel {
            *first = file;
        }
    }
    for ((_, folder), file) in firsts {
        if let Some(message) = manifest::upgrade_folder_refusal(folder, &version.value, series) {
            out.push(&file.path, "OD514", Pos { line: 1, col: 1 }, message);
        }
    }
}

/// OD515 on an upgrade script without the `migrate(cr, version)` Odoo calls, whose error fails the
/// update (18.0 `odoo/modules/migration.py:241-255`, `odoo/modules/loading.py:182`). Odoo never
/// imports a script of a folder whose name is not a version, so such a folder is skipped; one
/// above the module version is still judged, since Odoo runs it once the version rises.
fn upgrade_scripts<'m>(module: &'m Module, series: Option<Series>, out: &mut Out<'m>) {
    if !rule("OD515").applies_to(series) {
        return;
    }
    let scripts = stage_scripts(module).filter(|((_, folder), _)| manifest::upgrade_folder_named(folder));
    for (_, file) in scripts {
        let (pos, message) = match &file.facts.migrate {
            Migrate::Unread => continue,
            Migrate::Missing => (
                Pos { line: 1, col: 1 },
                "the script defines no migrate(cr, version), so Odoo raises and the update fails".to_owned(),
            ),
            Migrate::Refused { pos, params } => (
                *pos,
                format!("migrate{params} is not migrate(cr, version), so Odoo raises and the update fails"),
            ),
        };
        out.push(&file.path, "OD515", pos, message);
    }
}

/// OD513 on an `assets` path of the module's own that resolves to no file, which Odoo drops with a
/// warning when it is a glob and keeps as a URL that fails to load otherwise (18.0
/// `odoo/addons/base/models/ir_asset.py:330-374`). From saas~16.3 Odoo serves only files under
/// `static/`; before, a script or a style may sit anywhere in the module, so only an XML path
/// outside `static/` is reported there, since Odoo refuses templates elsewhere (saas~16.2 `:330-340`).
fn unmatched_assets<'m>(module: &'m Module, series: Option<Series>, out: &mut Out<'m>) {
    for path in module.manifest.bundles.iter().flat_map(|b| &b.paths) {
        if let Some(message) = unmatched_asset(module, &path.value, static_only(series)) {
            out.push(&module.manifest_path, "OD513", path.pos, message);
        }
    }
}

fn static_only(series: Option<Series>) -> bool {
    series.is_none_or(|s| s >= STATIC_ASSETS)
}

/// Why `path` of an `assets` bundle resolves to no file of `module`, none when it names another
/// module or a file Odoo serves. Odoo takes the module from the first part, then normalizes the
/// path, so `..` may lead out of `static/`.
fn unmatched_asset(module: &Module, path: &str, static_only: bool) -> Option<String> {
    let name = module.name.as_str();
    if path.split('/').find(|p| !p.is_empty()) != Some(name) {
        return None;
    }
    let parts = normal_parts(path);
    if !(parts.len() > 2 && parts[0] == name && parts[1] == "static") {
        return outside_static(name, path, &parts, static_only);
    }
    let rel = parts.join("/");
    if wildcard(path) {
        // Odoo globs with `glob.glob(recursive=True)`; a glob globset cannot read stays silent.
        let glob = asset_glob_set([&rel]);
        if glob.is_empty() {
            return None;
        }
        let matched = module.static_assets.iter().any(|f| glob.is_match(f));
        return (!matched).then(|| {
            format!(
                "'{path}' matches no js, css, scss, sass, less or xml file under {name}/static/, so Odoo adds nothing"
            )
        });
    }
    let extension = rel.rsplit('.').next().unwrap_or_default();
    (ASSET_EXTENSIONS.contains(&extension) && !module.static_assets.contains(&rel))
        .then(|| format!("'{path}' does not exist, so the bundle fails to load it"))
}

/// Why `path`, normalized to `parts` outside `name/static/`, is not served, none when it is.
fn outside_static(name: &str, path: &str, parts: &[&str], static_only: bool) -> Option<String> {
    if static_only {
        return Some(format!(
            "'{path}' is outside {name}/static/, the only files Odoo serves as assets"
        ));
    }
    // Odoo's leak check compares the extension case-sensitively, so it lets `a.XML` through (a
    // later lowercase filter drops it), and a wildcard first directory may glob into `static/`.
    let xml = parts
        .last()
        .and_then(|p| p.rsplit_once('.'))
        .is_some_and(|(_, ext)| ext == "xml");
    let may_reach_static = parts.len() > 2 && parts[0] == name && wildcard(parts[1]);
    (xml && !may_reach_static)
        .then(|| format!("'{path}' is outside {name}/static/, the only place Odoo serves XML assets from"))
}

/// Whether `path` holds one of the characters Odoo reads as a glob (`WILDCARD_CHARACTERS`).
pub(crate) fn wildcard(path: &str) -> bool {
    path.contains(['*', '?', '[', ']'])
}

/// The parts of `path` as `os.path.normpath` leaves them: without empty ones or `.`, and with each
/// `..` taking the part before it away.
fn normal_parts(path: &str) -> Vec<&str> {
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

/// OD505 on a `qweb` key, which from saas~14.3 nothing reads (`addons/web/controllers/main.py:557`),
/// and OD506 on a `web.assets_qweb` bundle, which nothing loads from `XML_IN_BUNDLES`, when a file
/// they name is in no bundle that gives templates in the series. A module moving to `assets` often
/// keeps the dead list beside the bundle that loads the same files.
fn unread_template_lists<'m>(module: &'m Module, series: Option<Series>, out: &mut Out<'m>) {
    let manifest = &module.manifest;
    let xml_in_bundles = series.is_some_and(|s| s >= XML_IN_BUNDLES);
    let loading = |assets_qweb: bool| {
        let bundles = manifest
            .bundles
            .iter()
            .filter(|b| (b.name.value == "web.assets_qweb") == assets_qweb);
        asset_glob_set(bundles.flat_map(|b| &b.globs))
    };
    if let Some(qweb) = &manifest.qweb {
        let listed: Vec<_> = qweb.value.iter().map(|f| format!("{}/{f}", module.name)).collect();
        if lost(module, &listed, &loading(!xml_in_bundles)) {
            let bundle = if xml_in_bundles {
                "web.assets_backend"
            } else {
                "web.assets_qweb"
            };
            let message = format!(
                "Odoo ignores the qweb key, so the templates it lists that no bundle loads are missing; list them under {bundle} in assets"
            );
            out.push(&module.manifest_path, "OD505", qweb.pos, message);
        }
    }
    let others = loading(false);
    for bundle in manifest.bundles.iter().filter(|b| b.name.value == "web.assets_qweb") {
        if lost(module, &bundle.globs, &others) {
            out.push(
                &module.manifest_path,
                "OD506",
                bundle.name.pos,
                "Odoo no longer loads web.assets_qweb, so the templates it lists that no other bundle loads are missing; list them under web.assets_backend",
            );
        }
    }
}

/// Whether a file of `listed`, globs relative to the addons directory, is one `loads` misses. A
/// glob that matches none of the module's templates stands for itself, so that a file the scan
/// does not index, such as one outside `static/src`, still counts.
fn lost(module: &Module, listed: &[String], loads: &GlobSet) -> bool {
    let templates: Vec<String> = module
        .templates
        .iter()
        .map(|t| format!("{}/{}", module.name, t.rel))
        .collect();
    listed.iter().any(|glob| {
        let set = asset_glob_set([glob]);
        let named: Vec<&str> = templates
            .iter()
            .map(String::as_str)
            .filter(|t| set.is_match(t))
            .collect();
        let named = if named.is_empty() { vec![glob.as_str()] } else { named };
        named.iter().any(|f| !loads.is_match(f))
    })
}

/// OD328 on a reference, `inherit_id` included, to a bundle that is no longer a template, in what
/// the manifest loads: the record holding it fails to install.
fn bundle_templates<'m>(module: &'m Module, out: &mut Out<'m>) {
    for file in module.data.iter().filter(|f| f.rank.is_some()) {
        for r in &file.facts.refs {
            let id = full_id(&module.name, &r.id);
            if BUNDLE_TEMPLATES.contains(&id.as_str()) {
                let message = format!(
                    "'{id}' is no longer a template but an asset bundle; list the files under it in the manifest's assets"
                );
                out.push(&file.path, "OD328", r.pos, message);
            }
        }
    }
}

fn unloaded_files<'m>(module: &'m Module, series: Option<Series>, out: &mut Out<'m>) {
    let mentions = module.code_mentions();
    for file in module.data.iter().filter(|f| f.rank.is_none() && f.facts.is_data) {
        let top = file.rel.split('/').next().unwrap_or_default();
        let ignored = module.manifest.ignored_key(&file.rel, series);
        // A file listed under a legacy key was meant to load, wherever it sits.
        let unlisted = ignored.is_none() && (!file.rel.contains('/') || UNLISTED_DIRS.contains(&top));
        if !code_loaded(&file.rel, &mentions, &module.manifest, series) && !unlisted {
            out.push(
                &file.path,
                "OD502",
                Pos { line: 1, col: 1 },
                unloaded(&file.rel, ignored, series),
            );
        }
    }
}

/// OD502's message for a data file Odoo never loads, listed under `ignored` or under no key.
fn unloaded(rel: &str, ignored: Option<&str>, series: Option<Series>) -> String {
    if let Some(key) = ignored {
        let to = if key == "demo_xml" { "demo" } else { "data" };
        return format!("'{rel}' is listed under {key}, which Odoo ignores in this series; move it to {to}");
    }
    let keys: Vec<_> = manifest::loaded_keys(series).collect();
    let keys = match keys.split_last() {
        Some((last, [])) => (*last).to_owned(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    };
    format!("'{rel}' is not in the manifest's {keys}, so Odoo never loads it")
}

/// OD516 on a file of models or controllers, and OD517 on a test file, that no import reaches from
/// the module's root, from `tests/__init__.py` for a test, or from another module. Silent when the
/// scan cannot tell what Odoo runs: without a root `__init__.py`, with a file it cannot decode, past
/// a file that does not parse or imports by a computed name, or when a module depending on this one
/// has such a file, as it may import any file of this one. A module outside the dependents is
/// OD518's when it imports one.
fn unimported_files<'m>(index: &Index<'m>, mi: usize, out: &mut Out<'m>) {
    let module = &index.modules[mi];
    if module.lost_text || !module.python.iter().any(|f| f.rel == "__init__.py") {
        return;
    }
    if index.has_unread_dependent(mi) {
        return;
    }
    let graph = &index.imports;
    let name = module.name.as_str();
    let mut starts: Vec<&str> = graph.elsewhere(name).collect();
    starts.push("__init__.py");
    let code = graph.reached(name, &starts);
    starts.push("tests/__init__.py");
    let tests = graph.reached(name, &starts);
    for file in &module.python {
        let finding = match file.rel.strip_prefix("tests/") {
            Some(test) => unrun_test(test, &file.rel, tests.as_ref()),
            None => unimported_code(module, file, code.as_ref()),
        };
        if let Some((rule, message)) = finding {
            out.push(&file.path, rule, Pos { line: 1, col: 1 }, message);
        }
    }
}

/// OD517 on a file directly under `tests/`, named `test_*`, that `reached` lacks.
fn unrun_test(test: &str, rel: &str, reached: Option<&FxHashSet<String>>) -> Option<(&'static str, String)> {
    let direct = test.starts_with("test_") && !test.contains('/');
    let unreached = reached.is_some_and(|r| !r.contains(rel));
    (direct && unreached).then(|| {
        let message = "no import from tests/__init__.py reaches this file, so Odoo never runs its tests";
        ("OD517", message.to_owned())
    })
}

/// OD516 on a file of models or controllers that `reached` lacks, upgrade scripts, IoT handlers and
/// the manifest aside.
fn unimported_code(
    module: &Module,
    file: &SourceFile<PyFacts>,
    reached: Option<&FxHashSet<String>>,
) -> Option<(&'static str, String)> {
    let dir = file.rel.split_once('/').map(|(dir, _)| dir);
    if dir.is_some_and(|d| UPGRADE_DIRS.contains(&d) || d == IOT_DIR)
        || file.path == module.manifest_path
        || reached.is_none_or(|r| r.contains(&file.rel))
    {
        return None;
    }
    let what = loaded_class(&file.facts)?;
    let message = format!("no import from the module's __init__.py reaches this file, so Odoo never loads {what}");
    Some(("OD516", message))
}

/// The first model or controller a file defines, as OD516 names it. A class extending a controller
/// of the file is one too, but the class it extends comes first.
fn loaded_class(facts: &PyFacts) -> Option<String> {
    let models = facts.classes.iter().map(|class| {
        let what = if class.new_model {
            "model"
        } else {
            "the extension of model"
        };
        (class.pos, format!("{what} '{}'", class.model))
    });
    let controllers = facts
        .controllers
        .iter()
        .filter(|c| !c.routes.is_empty() || c.bases.contains(&ClassBase::Root))
        .map(|c| (c.pos, format!("controller '{}'", c.name)));
    models
        .chain(controllers)
        .min_by_key(|(pos, _)| *pos)
        .map(|(_, what)| what)
}

/// OD519's position and message for what `import` lacks.
fn unresolved_import(import: &AddonImport, fault: &Fault) -> (Pos, String) {
    match fault {
        Fault::Missing => (
            import.pos,
            format!("no file of module {} provides '{}'", import.addon, import.text),
        ),
        Fault::Undefined(name) => (name.pos, format!("'{}' does not define '{}'", import.text, name.value)),
    }
}

fn same_module_xmlids<'m>(index: &Index<'m>, mi: usize, series: Option<Series>, out: &mut Out<'m>) {
    let module = &index.modules[mi];
    let scope = index.scope(mi);
    let partial = index.partial_scope(&scope);
    // base_data.sql creates base's bootstrap records before any data file loads.
    let bootstrapped = module.name == "base";
    let first_def = first_defs(module);
    for file in &module.data {
        let Some(rank) = file.rank else { continue };
        for r in &file.facts.refs {
            let id = full_id(&module.name, &r.id);
            if let Some(&(def_rank, def_line)) = first_def.get(&id)
                && (def_rank, def_line) > (rank, r.pos.line)
                && !bootstrapped
                && !registry_id(index, module, series, &scope, partial, &id)
            {
                let place = load_place(module, series, def_rank, rank);
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
fn first_defs(module: &Module) -> FxHashMap<String, (usize, u32)> {
    let mut first_def: FxHashMap<String, (usize, u32)> = FxHashMap::default();
    for file in &module.data {
        for def in &file.facts.defs {
            if let Some(rank) = file.rank.filter(|_| own_id(module, &def.id)) {
                first_def
                    .entry(full_id(&module.name, &def.id))
                    .or_insert((rank, def.pos.line));
            }
        }
    }
    first_def
}

/// A dotted id naming another module overrides that module's record, and defines nothing here.
fn own_id(module: &Module, id: &str) -> bool {
    id.split_once('.').is_none_or(|(m, _)| m == module.name)
}

/// OD303 on each definition that repeats an earlier one of its file, as `rewrites_in_a_file` tells.
fn ids_repeated_in_a_file<'m>(module: &'m Module, series: Option<Series>, out: &mut Out<'m>) {
    for (fi, file) in module.data.iter().enumerate() {
        let mut in_file: FxHashMap<&str, Vec<IdDef<'m>>> = FxHashMap::default();
        for (def, later) in id_defs(module, fi, file) {
            let seen = in_file.entry(&def.id).or_default();
            let earlier: Vec<&IdDef> = seen.iter().collect();
            if rewrites_in_a_file(&earlier, &later, series) {
                let message = format!("XML id '{}' is already defined earlier in this file", def.id);
                out.push(&file.path, "OD303", def.pos, message);
            }
            seen.push(later);
        }
    }
}

/// What one element defines an XML id as, for later definitions to compare with.
struct IdDef<'m> {
    file: usize,
    rel: &'m str,
    /// None when only a run tells it.
    model: Option<&'m str>,
    /// None for a tag such as `<template>` or `<menuitem>`.
    write: Option<&'m RecordWrite>,
    no_create: bool,
}

impl IdDef<'_> {
    fn writes(&self, name: &str) -> bool {
        self.write.is_some_and(|w| w.fields.iter().any(|f| f.name == name))
    }
}

/// Each definition of the data file at index `fi`, in file order, with what it defines its id as.
fn id_defs<'m>(
    module: &'m Module,
    fi: usize,
    file: &'m SourceFile<DataFacts>,
) -> impl Iterator<Item = (&'m XmlDef, IdDef<'m>)> {
    let writes: FxHashMap<Pos, &RecordWrite> = file.facts.writes.iter().map(|w| (w.pos, w)).collect();
    let themed = module.name.starts_with("theme_");
    file.facts.defs.iter().filter(|d| !d.assigned).map(move |def| {
        let id_def = IdDef {
            file: fi,
            rel: &file.rel,
            model: def.model.as_deref().filter(|_| !(def.themed && themed)),
            write: writes.get(&def.pos).copied(),
            no_create: def.no_create,
        };
        (def, id_def)
    })
}

/// Values are not compared, so a plain copy is reported too, and renaming it would duplicate the
/// record: the hint has to offer both fixes. A model clash cannot be a copy, so its message offers
/// only the rename.
const REPEATED_ID_HINT: &str =
    "remove the copy when the two say the same thing, or rename one when both are meant to exist";

/// The name a `<report>` and a `<record>` of the tag's model share. The parser, blind to the series,
/// calls the tag's model `ir.actions.report`, but through 10.0 the tag writes `ir.actions.report.xml`
/// (`odoo/tools/convert.py:323` on 10.0). No later series has that model, so a module of an unknown
/// series that names it counts as one through 10.0.
fn report_tag_model(model: &str, series: Option<Series>) -> &str {
    if model == "ir.actions.report.xml" && series.is_none_or(|s| s < REPORT_MODEL) {
        "ir.actions.report"
    } else {
        model
    }
}

/// The models of two definitions of one id, when they are not the same one.
fn other_model<'m>(first: &IdDef<'m>, later: &IdDef<'m>, series: Option<Series>) -> Option<(&'m str, &'m str)> {
    let models = first.model.zip(later.model)?;
    (report_tag_model(models.0, series) != report_tag_model(models.1, series)).then_some(models)
}

/// Whether `field` replaces what one of the `earlier` definitions of its id wrote.
fn rewrites(field: &FieldWrite, earlier: &[&IdDef]) -> bool {
    !field.incremental && earlier.iter().any(|e| e.writes(&field.name))
}

/// Whether `later` undoes what the `earlier` definitions of its id in one file set up. Odoo writes a
/// definition field by field, so a `<record>` naming other fields is a second phase of the first.
/// This takes `any` against the `all` of OD323, and ignores `forcecreate`: a repeat within one file
/// is far more likely a copy than a deliberate update.
fn rewrites_in_a_file(earlier: &[&IdDef], later: &IdDef, series: Option<Series>) -> bool {
    let Some(first) = earlier.first() else { return false };
    other_model(first, later, series).is_some()
        || later
            .write
            .is_none_or(|write| write.fields.iter().any(|f| rewrites(f, earlier)))
}

/// The finding on `later` repeating what a file before it defines under `id`, given those definitions
/// in load order. A later `<record>` that adds a field, or adds to one, reads as a deliberate update,
/// as does one that writes nothing. A `<record>` or `<template>` declares with `forcecreate` that the
/// record exists, which says nothing of a record of another model.
fn repeated(id: &str, earlier: &[&IdDef], later: &IdDef, series: Option<Series>) -> Option<String> {
    let first = earlier.first()?;
    if let Some((model, other)) = other_model(first, later, series) {
        return Some(format!(
            "XML id '{id}' is already a record of {model} in {}, and this definition is one of {other}; rename one",
            first.rel
        ));
    }
    if later.no_create {
        return None;
    }
    let overwrites = match later.write {
        Some(write) if write.fields.is_empty() => false,
        Some(write) => earlier.iter().all(|e| e.write.is_some()) && write.fields.iter().all(|f| rewrites(f, earlier)),
        None => true,
    };
    overwrites.then(|| {
        format!(
            "XML id '{id}' is already defined in {}, and this definition overwrites the fields both name; {REPEATED_ID_HINT}",
            first.rel
        )
    })
}

/// OD323 on what loaded data files define under an id of the module that another of them defines.
/// Installing writes every definition of one model, so each field ends up as the last definition
/// naming it left it, except `groups` on a `<menuitem>`, and up to 16.0 on a `<report>` or
/// `<act_window>`, which links and unlinks instead of replacing. Demo data is meant to update what
/// the data created.
fn ids_repeated_across_files<'m>(module: &'m Module, series: Option<Series>, out: &mut Out<'m>) {
    let mut defs: FxHashMap<String, Vec<IdDef<'m>>> = FxHashMap::default();
    let files = module.data.iter().enumerate();
    for (fi, file) in files.filter(|(_, f)| f.rank.is_some() && !f.demo) {
        for (def, later) in id_defs(module, fi, file).filter(|(d, _)| own_id(module, &d.id)) {
            let seen = defs.entry(full_id(&module.name, &def.id)).or_default();
            let earlier: Vec<&IdDef> = seen.iter().filter(|e| e.file != fi).collect();
            if let Some(message) = repeated(&def.id, &earlier, &later, series) {
                out.push(&file.path, "OD323", def.pos, message);
            }
            seen.push(later);
        }
    }
}

/// An Image field is a Binary one to Odoo, which compares field types by that name.
fn odoo_type(ftype: &str) -> &str {
    if ftype == "Image" { "Binary" } else { ftype }
}

/// Whether Odoo sees two field types as different, which a type the scan cannot tell may not be.
fn types_differ(a: &str, b: &str) -> bool {
    a != UNREAD_TYPE && b != UNREAD_TYPE && odoo_type(a) != odoo_type(b)
}

/// The registry creates the `ir.model` and `ir.model.fields` ids of a module's models before any of
/// its data files load, so a record with such an id only updates it. On a `partial` `scope` the id
/// may be spelled from a table outside the index.
fn registry_id(index: &Index, module: &Module, series: Option<Series>, scope: &Scope, partial: bool, id: &str) -> bool {
    let local = id.split_once('.').map_or(id, |(_, l)| l);
    module.python.iter().flat_map(|f| &f.facts.classes).any(|c| {
        let mut prefixes = index.field_prefixes(&c.model, series).into_iter();
        local == format!("model_{}", c.model.replace('.', "_"))
            || prefixes.any(|p| local.starts_with(&p))
            || (partial && index.unseen_table_id(c, local, series, scope))
    })
}

fn load_place(module: &Module, series: Option<Series>, def_rank: usize, rank: usize) -> String {
    if def_rank == rank {
        return "further down this file".to_owned();
    }
    let file = module.manifest.loaded_files(series).nth(def_rank);
    format!("in {}", file.unwrap_or_default())
}

/// The names a module may build at runtime. A model another module spells out is still that
/// module's unless the name fits, so a name that fits nothing is a real gap in `depends`.
struct BuiltModels<'m> {
    starts: Vec<&'m str>,
    names: Vec<&'m str>,
}

impl<'m> BuiltModels<'m> {
    /// A class under `tests/` exists only in a test run, so it builds nothing the module's code or
    /// data can rely on.
    fn of(module: &'m Module) -> Self {
        let loaded = || {
            module
                .python
                .iter()
                .filter(|f| f.rel.split('/').next() != Some("tests"))
        };
        Self {
            starts: loaded()
                .flat_map(|f| &f.facts.built_model_starts)
                .map(String::as_str)
                .collect(),
            names: loaded()
                .flat_map(|f| &f.facts.built_model_names)
                .map(String::as_str)
                .collect(),
        }
    }

    fn fit(&self, model: &str) -> bool {
        self.names.contains(&model) || self.starts.iter().any(|start| model.starts_with(start))
    }
}

#[derive(Clone, Copy)]
struct Ctx<'a, 'm> {
    index: &'a Index<'m>,
    mi: usize,
    module: &'m Module,
    scope: &'a Scope,
    /// The scope of what Odoo reads as it sets up the module's Python fields and attributes, and of
    /// the queries of `init()` and `_auto_init()`.
    setup: &'a Scope,
    /// `scope` and what installing the module auto-installs with it.
    installs: &'a Scope,
    /// `installs`, short of what loads after the module.
    loaded: &'a Scope,
    /// The module builds model names at runtime, so its models and their ids cannot be listed.
    dynamic: bool,
    /// What the module may build, outside `tests/`.
    built: &'a BuiltModels<'m>,
    /// A module in scope depends on one that is not indexed, such as an Enterprise module, which
    /// may hold anything a rule misses.
    partial: bool,
    /// A module in scope loads a data file the scan cannot read, which may hold a model, a field, an
    /// XML id, a view element, an access row or a value of a record, but no Python or JS.
    lost: bool,
    series: Option<Series>,
    /// The access rows in scope that let public or portal users at a model.
    grants: &'a Grants<'m>,
}

impl<'m> Ctx<'_, 'm> {
    /// Whether a dependency outside the index may add a method to `model`. It loads before this
    /// module, and Odoo refuses to extend a model the registry does not hold yet, so it reaches a
    /// model of this module only through a class or a parent the module does not hold.
    fn may_gain(&self, model: &str) -> bool {
        self.partial && !self.index.holds_model(self.mi, model, self.scope)
    }

    /// Whether another module may add a field to `model`. As for `may_gain`, one that loads first
    /// reaches a model of this module only through a class or a parent the module does not hold.
    fn may_add_field(&self, model: &str) -> bool {
        !self.index.holds_fields(self.mi, model, self.scope)
    }

    /// Whether a dependency outside the index, or a file in scope that does not parse, may supply
    /// what a path lacks. Neither can change the type of a field found on the way.
    fn may_supply(&self, e: &PathError<'_>) -> bool {
        match e {
            PathError::Missing { model, .. } | PathError::Hidden { model, .. } => {
                (self.partial && self.may_add_field(model)) || self.index.unread_class(model, self.scope)
            }
            PathError::NotRelational { .. } => false,
        }
    }

    /// Whether this module builds `model` at runtime. A class in scope that introduces the model
    /// spells it out whatever the module builds: `base` itself builds models of any name, so `fit`
    /// alone would take all of its own models for built ones.
    fn built_here(&self, model: &str) -> bool {
        !self.index.introduced(model, self.scope) && self.built.fit(model)
    }

    /// Whether the model where a path fails may hold the field out of the scan's sight. That model
    /// is the one to ask about, as a later step can be the one that hides the field. A parent or a
    /// data file only explains a missing field: one `depends` does not reach is found on the model,
    /// so it stays a gap unless the model is built here or a file in scope that does not parse may
    /// add it. Nothing explains a field found there that is not relational.
    fn may_hide(&self, e: &PathError<'_>) -> bool {
        let unseen = match e {
            PathError::Missing { model, .. } => {
                self.built_here(model)
                    || self.index.hidden_field_parents(model, self.scope)
                    || self.index.unread_delegate(model, self.scope)
            }
            PathError::Hidden { model, .. } => self.built_here(model),
            PathError::NotRelational { .. } => false,
        };
        unseen || self.lost_field(e) || self.may_supply(e)
    }

    /// Whether a data file the scan cannot read may add the field a path lacks. A dependency's file
    /// loads before the module, so it cannot add to a model the module defines whole, while the
    /// module's own file can.
    fn lost_field(&self, e: &PathError<'_>) -> bool {
        matches!(e, PathError::Missing { model, .. }
            if self.lost && (self.module.lost_data() || self.may_add_field(model)))
    }

    /// A dependency outside the index may load a model the scope does not reach.
    fn model_ref(&self, out: &mut Out<'m>, path: &'m Path, model: &Spanned<String>, except: Option<&ClassFact>) {
        match self.index.model_status(&model.value, self.scope, except) {
            Status::Visible => {}
            Status::Hidden(_) if self.partial || self.built.fit(&model.value) => {}
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

    /// `model_ref` for a data file. An unread file in scope may hold the `ir.model` record of a model
    /// no class defines, but not the class of one a module outside `depends` defines.
    fn data_model_ref(&self, out: &mut Out<'m>, path: &'m Path, model: &Spanned<String>) {
        let status = self.index.model_status(&model.value, self.scope, None);
        if !(self.lost && matches!(status, Status::Missing)) {
            self.model_ref(out, path, model, None);
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
                    unknown_dependency(&dep.value, self.index.rejected.get(dep.value.as_str())),
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
        self.implied_dependencies(out);
        self.unused_dependencies(out);
    }

    /// OD520 on a `depends` entry another entry already brings, naming the top of the chain.
    fn implied_dependencies(&self, out: &mut Out<'m>) {
        let depends = &self.module.manifest.depends;
        for (d, e) in self.index.implied_depends(self.mi) {
            let message = format!(
                "module '{}' already comes with '{}', also in depends",
                depends[d].value, depends[e].value
            );
            out.push(&self.module.manifest_path, "OD520", depends[d].pos, message);
        }
    }

    /// OD521 on a `depends` entry from which the module's words and view specs take nothing it alone
    /// brings. A module with `auto_install` is a bridge that exists to pull its dependencies in, as
    /// is one with no file but its manifest and `__init__.py`, and a text the scan lost may hold the
    /// use.
    fn unused_dependencies(&self, out: &mut Out<'m>) {
        let module = self.module;
        if module.manifest.auto_install != manifest::AutoInstall::Off || module.lost_text || is_shell(module) {
            return;
        }
        let depends = &module.manifest.depends;
        for (d, set) in self.index.sole_brought(self.mi) {
            let used = self.index.takes_from(&module.words, &set, self.scope, self.mi)
                || self.index.locates_in(self.mi, &set, self.scope);
            if !used {
                let message = format!(
                    "nothing of module '{}' is used, nor of what only it brings",
                    depends[d].value
                );
                out.push(&module.manifest_path, "OD521", depends[d].pos, message);
            }
        }
    }

    /// OD518 on an import of a module outside `depends` that runs as the module loads, and OD519
    /// on what an import names that its module lacks. Upgrade scripts are no module code, an IoT
    /// handler runs on the box, and a failed import in a `try` catching it or under a `suppress()`
    /// that may changes nothing, nor does any import under `if TYPE_CHECKING:`, a function defined
    /// there included.
    fn addon_imports(&self, out: &mut Out<'m>) {
        for file in &self.module.python {
            let dir = file.rel.split_once('/').map(|(dir, _)| dir);
            let upgrade = dir.is_some_and(|d| UPGRADE_DIRS.contains(&d));
            let Some(imports) = file.facts.imports.as_ref().filter(|_| !upgrade) else {
                continue;
            };
            let loads = dir != Some(IOT_DIR);
            for fact in imports.list.iter().filter(|f| !f.guarded) {
                for import in self.index.imports.addon_imports(&file.rel, fact) {
                    if loads && !fact.deferred && self.outside_depends(&import.addon) {
                        let message = format!(
                            "'{}' comes from module {}; add it to depends",
                            import.text, import.addon
                        );
                        out.push(&file.path, "OD518", import.pos, message);
                    }
                    for fault in &import.faults {
                        let (pos, message) = unresolved_import(&import, fault);
                        out.push(&file.path, "OD519", pos, message);
                    }
                }
            }
        }
    }

    /// Whether `module` is another module than this one, outside its scope, and no dependency
    /// outside the index may bring it in.
    fn outside_depends(&self, module: &str) -> bool {
        let seen = self.index.by_name.get(module).is_some_and(|&mi| self.scope.sees(mi));
        module != self.module.name && !self.partial && !seen
    }

    fn path(&self, out: &mut Out<'m>, file: &'m Path, model: &str, path: &Spanned<String>) {
        if let Err(e) = self.index.resolve_path(model, &path.value, self.scope)
            && !self.may_supply(&e)
            && !self.lost_field(&e)
        {
            out.push(file, "OD203", path.pos, e.message(&path.value));
        }
    }

    /// What a domain finds where Odoo checks it as the module loads from `validated` on. Before that
    /// it runs once the install is done, with all of `installs` there.
    fn checked_reach(&self, validated: Series) -> &Scope {
        if self.series.is_some_and(|s| s < validated) {
            self.installs
        } else {
            self.loaded
        }
    }

    /// OD210 on a path that fails on `scope` and past it on `reach`, which holds what is there when
    /// the domain is read. `scope` keeps its say: a model built here on it may be spelled out by a
    /// module only `reach` holds, which says nothing of the model this module builds.
    fn domain_path(&self, out: &mut Out<'m>, file: &'m Path, model: &str, path: &Spanned<String>, reach: &Scope) {
        let Err(near) = self.index.resolve_path(model, &path.value, self.scope) else {
            return;
        };
        if self.may_hide(&near) {
            return;
        }
        let wider = Ctx { scope: reach, ..*self };
        if let Err(e) = self.index.resolve_path(model, &path.value, reach)
            && !wider.may_hide(&e)
        {
            out.push(file, "OD210", path.pos, e.message(&path.value));
        }
    }

    /// OD203 on a path Odoo resolves as it sets up the model, when a failure refuses the install.
    /// The module's own data files load after that, so only a dependency's unread file may add the
    /// field, and not to a model the module defines whole.
    fn setup_path(&self, out: &mut Out<'m>, file: &'m Path, model: &str, path: &Spanned<String>) {
        if let Err(e) = self.index.resolve_path(model, &path.value, self.setup)
            && !self.may_supply(&e)
            && !matches!(&e, PathError::Missing { model, .. }
                if self.index.lost_scope(self.setup) && self.may_add_field(model))
        {
            out.push(file, "OD203", path.pos, e.message(&path.value));
        }
    }

    fn python(&self, out: &mut Out<'m>) {
        for file in &self.module.python {
            let path = file.path.as_path();
            for class in file.facts.classes.iter().filter(|c| !c.unread) {
                self.class(out, path, class);
            }
            if !UPGRADE_DIRS.contains(&file.rel.split('/').next().unwrap_or_default()) {
                self.sql_columns(out, path, &file.facts.sql_columns);
                for r in &file.facts.xml_refs {
                    self.py_ref(out, path, r, true);
                }
                for r in &file.facts.guarded_refs {
                    self.guarded_ref(out, path, r);
                }
                for r in &file.facts.group_refs {
                    self.py_ref(out, path, r, false);
                }
                self.domains(out, path, &file.facts.domains);
            }
            // env['x'] for a model outside depends is usually guarded by `if 'x' in self.env`, a
            // dotless name is as likely a key of some other mapping called env, and a name that
            // fits what the module builds may exist once its code has run.
            for model in file
                .facts
                .env_models
                .iter()
                .filter(|m| m.value.contains('.') && !self.built.fit(&m.value))
            {
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

    /// OD231 and OD232 on the controllers of the module that Odoo serves.
    fn routes(&self, out: &mut Out<'m>) {
        let table = &self.index.routes;
        let name = self.module.name.as_str();
        let files: Vec<_> = self.module.python.iter().filter(|f| served(&f.rel)).collect();
        for file in &files {
            for class in table.module(name).filter(|c| c.rel == file.rel) {
                self.undecorated_overrides(out, &file.path, class);
            }
        }
        let spells = |c: &ClassAt| served(c.rel) && c.class.routes.iter().any(|r| !literal_urls(r).is_empty());
        if !table.module(name).any(|c| spells(&c)) {
            return;
        }
        let seen = |addon: &&str| self.index.by_name.get(addon).is_some_and(|&mi| self.scope.sees(mi));
        let mut classes: Vec<_> = table.addons().filter(seen).flat_map(|a| table.module(a)).collect();
        classes.retain(|c| served(c.rel));
        classes.sort_by_key(|c| (c.addon, c.rel, c.class.start));
        for (origin, message) in Paths::of(table.endpoints(&classes)).shadowed(self.index, name) {
            if let Some(file) = files.iter().find(|f| f.rel == origin.at.rel) {
                out.push(&file.path, "OD231", origin.route.pos, message);
            }
        }
    }

    /// OD232 on each method of `class` without a decorator that overrides a route method.
    fn undecorated_overrides(&self, out: &mut Out<'m>, path: &'m Path, class: ClassAt<'_>) {
        for def in &class.class.plain {
            let Some(routed) = self.index.routes.routed_ancestor(class, &def.value) else {
                continue;
            };
            let outcome = match self.series {
                Some(s) if s >= ROUTE_REDECORATED => "so Odoo logs a warning and routes it as if it had one",
                Some(_) => "so Odoo drops the route",
                None => "so Odoo drops the route before saas~15.3, and logs a warning from it",
            };
            let message = format!(
                "'{}' overrides the route of {} in module {} without @route(), {outcome}",
                def.value, routed.class.name, routed.addon
            );
            out.push(path, "OD232", def.pos, message);
        }
    }

    fn js(&self, out: &mut Out<'m>, fi: usize, file: &'m SourceFile<JsFacts>) {
        let from = js_url(&self.module.name, &file.rel);
        let mut unexported: Vec<&str> = vec![];
        let mocked = |name: &&JsName| file.in_tests() && hoot_adds(self.series, &from, name);
        for name in file.facts.names.iter().filter(|n| !mocked(n)) {
            if self.js_name(out, &file.path, &from, name) {
                unexported.push(&name.specifier);
            }
        }
        for specifier in &file.facts.specifiers {
            let check_page = !unexported.contains(&specifier.value.as_str());
            self.js_specifier(out, &file.path, fi, &from, specifier, check_page);
        }
    }

    /// A dependency outside the index may load the module a specifier names. It cannot add a file
    /// to that module, so a missing file is reported whatever the scope. `check_page` is false when
    /// OD601 already reports a name the specifier lacks, so the import gets one finding.
    fn js_specifier(
        &self,
        out: &mut Out<'m>,
        path: &'m Path,
        fi: usize,
        from: &str,
        specifier: &Spanned<String>,
        check_page: bool,
    ) {
        let module = |mi: usize| &self.index.modules[mi].name;
        // The import works when any file that claims the name is in scope and on the page.
        let claims = self.index.js_claims(from, &specifier.value);
        let message = match self.index.js_resolve(from, &specifier.value) {
            JsSpec::Missing(mi) => format!("no file of module {} provides '{}'", module(mi), specifier.value),
            JsSpec::Found(t) if !self.partial && !claims.iter().any(|c| self.scope.sees(c.module)) => format!(
                "'{}' comes from module {}, which is not in depends",
                specifier.value,
                module(t.module)
            ),
            JsSpec::Found(_) if check_page => return self.js_page(out, path, fi, claims, specifier),
            JsSpec::Found(_) | JsSpec::Outside => return,
        };
        out.push(path, "OD601", specifier.pos, message);
    }

    /// A dependency outside the index, such as an Enterprise one, may add the target, or a bundle
    /// that holds it, to the importer's page. Any of the `claims` on that page serves the import.
    fn js_page(&self, out: &mut Out<'m>, path: &'m Path, fi: usize, claims: &[JsTarget], specifier: &Spanned<String>) {
        if self.partial {
            return;
        }
        let importer: Vec<&str> = self.index.js_bundles(self.mi, fi).collect();
        let bundles = |t: &JsTarget| self.index.js_bundles(t.module, t.file).collect::<Vec<&str>>();
        let Some(first) = claims.first() else {
            return;
        };
        if claims.iter().any(|t| page_miss(&importer, &bundles(t)).is_none()) {
            return;
        }
        let bundles = bundles(first);
        let Some(page) = page_miss(&importer, &bundles) else {
            return;
        };
        let message = format!(
            "'{}' is in {}, not in {page}, which loads this file",
            specifier.value,
            bundle_list(&bundles)
        );
        out.push(path, "OD608", specifier.pos, message);
    }

    /// Whether OD601 reports the name as one its module does not export.
    fn js_name(&self, out: &mut Out<'m>, path: &'m Path, from: &str, name: &JsName) -> bool {
        let JsSpec::Found(target) = self.index.js_resolve(from, &name.specifier) else {
            return false;
        };
        if self.index.js_export(target, &name.name.value, 0) != Export::Missing {
            return false;
        }
        let message = match name.name.value.as_str() {
            "default" => format!("'{}' has no default export", name.specifier),
            n => format!("'{}' does not export '{n}'", name.specifier),
        };
        out.push(path, "OD601", name.name.pos, message);
        true
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

    /// A module outside the index, such as an Enterprise one, may load the model or be the one that
    /// defines the method called.
    fn orm_call(&self, out: &mut Out<'m>, path: &'m Path, call: &OrmCall) {
        if self.partial {
            return;
        }
        let Some(model) = &call.model else {
            return;
        };
        let message = match self.index.model_status(&model.value, self.scope, None) {
            Status::Visible => return self.orm_method(out, path, call, &model.value),
            Status::Hidden(m) if !self.built.fit(&model.value) => {
                format!("model '{}' comes from module {m}, which is not in depends", model.value)
            }
            Status::Missing if !self.dynamic => format!("no indexed module defines model '{}'", model.value),
            _ => return,
        };
        out.push(path, "OD603", model.pos, message);
    }

    /// A private method is OD607's, whether the model has it or not.
    fn orm_method(&self, out: &mut Out<'m>, path: &'m Path, call: &OrmCall, model: &str) {
        let Some(method) = call
            .method
            .as_ref()
            .filter(|m| !m.value.starts_with('_') && self.sees_methods(model))
        else {
            return;
        };
        let message = match self.index.rpc_method(model, &method.value, self.scope, self.series) {
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
                for call in &t.arch.calls {
                    self.template_ref(out, &file.path, call);
                }
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
        let paths = class
            .paths
            .iter()
            .chain(&class.order_fields)
            .chain(self.rec_names_search(class));
        for p in paths.filter(|_| class.kind != ModelKind::Abstract) {
            self.path(out, path, &class.model, p);
        }
        let setup_paths = class
            .depends
            .iter()
            .flat_map(|(_, paths)| paths)
            .chain(&class.setup_fields);
        for p in setup_paths.filter(|_| class.kind != ModelKind::Abstract) {
            self.setup_path(out, path, &class.model, p);
        }
        self.redefinition(out, path, class);
        self.super_calls(out, path, class);
        self.override_signatures(out, path, class);
        self.orphan_hooks(out, path, class);
        if self.lacks_access(class) {
            out.push(path, "OD401", class.pos, no_access_message(&class.model, self.series));
        }
    }

    /// Whether `class` makes a model that needs an access row and has none. A data file the scan
    /// cannot read may hold that row, in the scope or in a module that depends on this one.
    fn lacks_access(&self, class: &ClassFact) -> bool {
        class.new_model
            && class.kind != ModelKind::Abstract
            && class.registered()
            && !ignores_access(Some(class.kind), self.series)
            && !self.index.models_with_access.contains(class.model.as_str())
            && !self.index.lost_access(self.index.by_name[self.module.name.as_str()])
    }

    /// `_rec_names_search` is read from saas~15.3 on.
    fn rec_names_search<'c>(&self, class: &'c ClassFact) -> &'c [Spanned<String>] {
        if self.series.is_some_and(|s| s >= REC_NAMES_SEARCH) {
            &class.rec_names_search
        } else {
            &[]
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

    /// Whether a parent of `class` may be out of the index's sight: a plain Python base of this class,
    /// an `_inherit` element that is not a literal in any class of the model or of a model it
    /// inherits, a class in scope in a file that does not parse, a module that is not indexed, a
    /// model nobody indexes, and for a mixin, every model that inherits it.
    fn hides_parents(&self, class: &ClassFact) -> bool {
        !class.plain_bases.is_empty()
            || self.partial
            || class.kind == ModelKind::Abstract
            || !self.index.has_model(&class.model)
            || self
                .index
                .chain_read_from(&class.model, self.scope)
                .iter()
                .any(|(_, c)| c.inherit_unknown)
    }

    /// A `super()` call that no parent class answers, where `hides_parents` leaves every parent seen.
    fn super_calls(&self, out: &mut Out<'m>, path: &'m Path, class: &ClassFact) {
        if class.super_calls.is_empty() || self.hides_parents(class) {
            return;
        }
        for call in &class.super_calls {
            if !self
                .index
                .has_parent_method(&class.model, &call.value, class, self.series)
            {
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

    /// An override, in a class that extends its model in place, that refuses a call each definition it
    /// overrides takes. A new model's override may serve its own callers alone. A class that does
    /// not parse is read with an unknown `_inherit`, so `hides_parents` covers it.
    fn override_signatures(&self, out: &mut Out<'m>, path: &'m Path, class: &ClassFact) {
        if class.new_model
            || class.defs.is_empty()
            || self.hides_parents(class)
            || self.index.hidden_method_parents(&class.model, self.scope)
        {
            return;
        }
        let merged = self.index.merged_classes(&class.model, self.scope);
        for def in &class.defs {
            let name = def.name.value.as_str();
            if let Some((mi, parent, refusal)) = self.override_refusal(class, &merged, def) {
                let message = refusal_message(name, &self.index.modules[mi].name, parent, &refusal);
                out.push(path, "OD229", def.name.pos, message);
            }
        }
    }

    /// What `def` refuses of every definition it overrides, with the module and the signature of the
    /// one that loads first, ties kept in the chain order of `merged`. Only the last binding of a
    /// name in the class is the method, and only when every binding of it is a `def`.
    fn override_refusal<'c>(
        &self,
        class: &'c ClassFact,
        merged: &[(usize, &'m ClassFact)],
        def: &'c MethodDef,
    ) -> Option<(usize, &'m Signature, Refusal<'c>)>
    where
        'm: 'c,
    {
        let own = def.signature.as_ref()?;
        let signature = class.signature(&def.name.value).filter(|s| std::ptr::eq(*s, own))?;
        let parents = self.index.method_parents(self.mi, class, merged, &def.name.value)?;
        let refusals = parents
            .into_iter()
            .map(|(mi, parent)| Some((mi, (parent, signature.refuses(parent)?))))
            .collect::<Option<Vec<_>>>()?;
        let (mi, (parent, refusal)) = self.index.first_loaded(refusals)?;
        Some((mi, parent, refusal))
    }

    /// A hook method, in a class that extends its model in place, that Odoo never runs: the usual
    /// trace of a parent hook renamed upstream. A new model's hooks are its own to leave unused.
    fn orphan_hooks(&self, out: &mut Out<'m>, path: &'m Path, class: &ClassFact) {
        if class.new_model
            || class.defs.is_empty()
            || self.hides_parents(class)
            || self.index.hidden_method_parents(&class.model, self.scope)
            || self.index.lost_mentions(self.mi)
        {
            return;
        }
        for def in class.defs.iter().filter(|d| self.orphan_hook(class, d)) {
            let message = format!(
                "no parent class of {} defines '{}' and no file names it, so Odoo never runs it",
                class.model, def.name.value
            );
            out.push(path, "OD230", def.name.pos, message);
        }
    }

    /// Whether `def` is a hook method that only its name could run, bound by nothing else in the
    /// class, which no parent class defines and no file names, as far as the scan can read them.
    fn orphan_hook(&self, class: &ClassFact, def: &MethodDef) -> bool {
        let name = def.name.value.as_str();
        runs_as_hook(name, def.signature.as_ref())
            && def
                .decorators
                .iter()
                .all(|d| NAMED_HOOK_DECORATORS.contains(&d.as_str()))
            && class.methods.iter().filter(|m| *m == name).count() == 1
            && !self.index.has_parent_method(&class.model, name, class, self.series)
            && !self.index.names_hook(name)
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
            self.setup_path(out, path, &class.model, related);
            self.related_type(out, path, class, field, related);
        }
        self.model_field(out, path, class, field);
        if let (Some(comodel), Some(inverse)) = (&field.comodel, &field.inverse_name)
            && index.has_model(&comodel.value)
        {
            self.setup_path(out, path, &comodel.value, inverse);
            self.inverse(out, path, comodel, inverse);
        }
        if let Some(comodel) = &field.comodel {
            // A view validates a string, and leaves a list to the client.
            let reach = if field.domain_string {
                self.checked_reach(VIEW_DOMAIN_VALIDATED)
            } else {
                self.installs
            };
            for f in &field.domain {
                self.domain_path(out, path, &comodel.value, f, reach);
            }
        }
        self.missing_methods(out, path, class, field);
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

    /// Odoo replaces the compute and inverse of a related field with its own, and a dependency
    /// outside the index or a parent the scan cannot see may hold methods only Python knows.
    fn missing_methods(&self, out: &mut Out<'m>, path: &'m Path, class: &ClassFact, field: &FieldFact) {
        if field.related.is_some()
            || self.index.merged_hidden_parents(&class.model, self.scope)
            || self.may_gain(&class.model)
        {
            return;
        }
        for method in &field.methods {
            if !self.index.has_method(&class.model, &method.value, self.scope) {
                let message = format!("{} has no method '{}'", class.model, method.value);
                out.push(path, "OD204", method.pos, message);
            }
        }
    }

    /// A Many2oneReference reads its record's model from the Char field `model_field` names, which
    /// in a mixin is a field of the models that inherit it. A related one keeps the `model_field`
    /// it sets; one it inherits from its target is not checked.
    fn model_field(&self, out: &mut Out<'m>, path: &'m Path, class: &ClassFact, field: &FieldFact) {
        if field.ftype != "Many2oneReference" || class.kind == ModelKind::Abstract {
            return;
        }
        if let Some(Spanned {
            value: OptionValue::Str(name),
            pos,
        }) = field.option("model_field")
        {
            let name = Spanned {
                value: name.clone(),
                pos: *pos,
            };
            if self.series.is_some_and(|s| s >= MODEL_FIELD_ASSERTED) {
                self.setup_path(out, path, &class.model, &name);
            } else {
                self.path(out, path, &class.model, &name);
            }
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
        let Some(targets) = self.index.path_target(&class.model, &related.value, self.setup) else {
            return;
        };
        if targets.iter().all(|t| types_differ(&t.ftype, &field.ftype)) {
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
        if field.ftype == UNREAD_TYPE {
            return None;
        }
        self.index
            .field_defs(&class.model, &field.name, Some(self.scope))
            .into_iter()
            .filter(|(_, c, _)| !std::ptr::eq(*c, class) && c.model == class.model)
            .map(|(_, _, f)| f.ftype.as_str())
            .find(|t| *t != field.ftype && *t != UNREAD_TYPE)
    }

    fn data(&self, out: &mut Out<'m>) {
        let mut trees: FxHashMap<String, ViewTree<'m>> = FxHashMap::default();
        let loaded = Loaded::of(self.module);
        let mut required = FxHashMap::default();
        let demo_scope = self.scope.in_demo();
        let demo_installs = self.installs.in_demo();
        let demo_loaded = self.loaded.in_demo();
        let demo = Ctx {
            scope: &demo_scope,
            installs: &demo_installs,
            loaded: &demo_loaded,
            ..*self
        };
        // Files the manifest does not load, such as chart template CSVs read by Python code, are skipped.
        for file in self.module.data.iter().filter(|f| f.rank.is_some()) {
            let ctx = if file.demo { &demo } else { self };
            ctx.data_file(out, file, &mut trees, &loaded, &mut required);
        }
    }

    fn data_file(
        &self,
        out: &mut Out<'m>,
        file: &'m SourceFile<DataFacts>,
        trees: &mut FxHashMap<String, ViewTree<'m>>,
        loaded: &Loaded,
        required: &mut RequiredFields<'m>,
    ) {
        let path = file.path.as_path();
        for r in &file.facts.refs {
            self.xml_ref(out, path, r);
        }
        for view in &file.facts.views {
            self.view(out, path, view, trees);
            self.locators(out, path, view, trees);
            self.qweb_calls(out, path, view);
        }
        for record in &file.facts.records {
            self.record(out, path, record);
        }
        for write in &file.facts.writes {
            self.record_write(out, path, write);
        }
        self.created_records(out, file, loaded, required);
        for call in &file.facts.functions {
            self.function_call(out, path, call);
        }
        for rule in &file.facts.rules {
            self.rule_domain(out, path, rule);
            self.public_rule(out, path, rule);
            self.rule_without_permission(out, path, rule);
        }
        for server in &file.facts.servers {
            self.server_code(out, path, server);
        }
        for report in &file.facts.reports {
            self.report_template(out, path, report);
        }
        self.domains(out, path, &file.facts.action_domains);
        self.actions(out, path, &file.facts.actions);
        self.exposing_rows(out, file);
    }

    /// OD324 on the records of a loaded `file` that Odoo creates.
    fn created_records(
        &self,
        out: &mut Out<'m>,
        file: &'m SourceFile<DataFacts>,
        loaded: &Loaded,
        memo: &mut RequiredFields<'m>,
    ) {
        for write in file.facts.writes.iter().filter(|w| self.creates(w, file, loaded)) {
            self.missing_required(out, &file.path, write, file.demo, memo);
        }
    }

    /// Whether `write` of a loaded `file` reaches `create`: it has no id, or Odoo finds no record under
    /// its id. Anything before it may have made that record: a definition, a write the scope reads,
    /// the registry, a parent record, `base_data.sql` or a `<function>` of the module. An id of
    /// another module is none of `first_defs`.
    fn creates(&self, write: &RecordWrite, file: &SourceFile<DataFacts>, loaded: &Loaded) -> bool {
        if write.no_create || write.fields.iter().any(database_id) {
            return false;
        }
        let Some(id) = &write.id else { return true };
        let full = full_id(&self.module.name, id);
        let at = file.rank.map(|rank| (rank, write.pos.line));
        let writes = self.index.value_writes.get(&full).map_or(&[][..], Vec::as_slice);
        let first = writes.iter().find(|w| self.scope.reads(w));
        self.module.name != "base"
            && at.is_some_and(|at| loaded.ids.get(&full) == Some(&at) && loaded.first_call.is_none_or(|c| at < c))
            && first.is_some_and(|w| std::ptr::eq(w.fact, write))
            && !registry_id(self.index, self.module, self.series, self.scope, self.partial, &full)
            && !self.index.derived.contains(&full)
    }

    /// OD324 on each required field a create of `write.model` leaves without a value.
    fn missing_required(
        &self,
        out: &mut Out<'m>,
        path: &'m Path,
        write: &'m RecordWrite,
        demo: bool,
        memo: &mut RequiredFields<'m>,
    ) {
        let given = Given {
            write,
            demo,
            models: vec![],
        };
        let mut seen = FxHashSet::default();
        let mut gaps = vec![];
        self.required_gaps(&write.model, given, memo, &mut seen, &mut gaps);
        for (model, field) in gaps {
            let message = format!(
                "{model}.{field} is required and has no default, so creating this record fails if the column is NOT NULL; give the field a value"
            );
            out.push(path, "OD324", write.pos, message);
        }
    }

    /// The required fields a create of `model` leaves empty, and those of each parent it creates. A
    /// parent takes what the child's values and defaults give the field; a delegate field that gets
    /// a value leaves that parent alone. `_inherits` cycles end at the seen model.
    fn required_gaps(
        &self,
        model: &'m str,
        mut given: Given<'m>,
        memo: &mut RequiredFields<'m>,
        seen: &mut FxHashSet<&'m str>,
        gaps: &mut Vec<(&'m str, &'m str)>,
    ) {
        if !seen.insert(model) {
            return;
        }
        given.models.push(model);
        let required = self.required_of(model, memo);
        let left = required.fields.iter().filter(|f| !self.gives(&given, f));
        gaps.extend(left.map(|f| (model, *f)));
        for (parent, via) in &required.delegates {
            if !self.gives(&given, via) {
                self.required_gaps(parent, given.clone(), memo, seen, gaps);
            }
        }
    }

    /// Whether the record has a value for `name` without a default of the field: it writes it, its
    /// context, an `ir.default` of the model or a child, or a default of a child gives it, or the
    /// record it sits in does.
    fn gives(&self, given: &Given<'m>, name: &str) -> bool {
        let write = given.write;
        let children = given.models.split_last().map_or(&[][..], |(_, c)| c);
        write.fields.iter().any(|f| f.name == name)
            || write.context_defaults.iter().any(|d| d == name)
            || given
                .models
                .iter()
                .any(|m| self.index.ir_default(m, name, self.scope, given.demo))
            || self.parent_decides(write, name)
            || children.iter().any(|m| self.hands_down_default(m, name))
    }

    /// Whether `child` redeclares the delegated field `name` with a default: `default_get` reads it
    /// and `create` passes it to the parent. Without `inherited` the field only shadows the parent's,
    /// so its value never reaches the parent (`odoo/models.py:5021` on 18.0).
    fn hands_down_default(&self, child: &'m str, name: &str) -> bool {
        self.merged(child, name).is_some_and(|f| {
            f.get("default") != Merged::Unset
                && f.get("inherited") != Merged::Unset
                && !f.is("inherited", &OptionValue::Bool(false))
        })
    }

    /// `read_required`, once per model.
    fn required_of(&self, model: &'m str, memo: &mut RequiredFields<'m>) -> Rc<Required<'m>> {
        let known = memo.get(model).cloned();
        known.unwrap_or_else(|| {
            let required = Rc::new(self.read_required(model));
            memo.insert(model, required.clone());
            required
        })
    }

    /// What nothing fills on a create of `model`. Empty when only a run tells: a class hides from the
    /// scan, the table has no constraints of Odoo's, or the model itself has a hook, which may fill
    /// any field, its parents' included.
    fn read_required(&self, model: &'m str) -> Required<'m> {
        let classes = self.index.merged_classes(model, self.scope);
        let no_table = |c: &ClassFact| c.attr("_auto").is_some_and(|a| a.value != OptionValue::Bool(true));
        let unread = |c: &ClassFact| c.inherits_unknown || no_table(c) || (c.model == model && fills_on_create(c));
        if !self.sees_whole_model(model) || classes.iter().any(|(_, c)| unread(c)) {
            return Required::default();
        }
        let delegates: Vec<(&'m str, &'m str)> = classes
            .iter()
            .flat_map(|(_, c)| &c.inherits)
            .map(|(parent, via)| (parent.as_str(), via.as_str()))
            .collect();
        let creates_parent = |via: &str| {
            self.merged(model, via)
                .is_none_or(|f| f.get("default") == Merged::Unset)
        };
        Required {
            fields: self.fields_without_default(model, &classes, &delegates),
            delegates: delegates
                .iter()
                .copied()
                .filter(|(_, via)| creates_parent(via))
                .collect(),
        }
    }

    /// The required fields of `model` that no default fills, sorted, but the delegating ones, which
    /// Odoo fills with the parent it creates.
    fn fields_without_default(
        &self,
        model: &'m str,
        classes: &[(usize, &'m ClassFact)],
        delegates: &[(&'m str, &'m str)],
    ) -> Vec<&'m str> {
        let mut by_name: FxHashMap<&'m str, Vec<FieldDef<'m>>> = FxHashMap::default();
        for def in self.index.merged_fields(model, self.scope) {
            by_name.entry(def.2.name.as_str()).or_default().push(def);
        }
        by_name.retain(|name, _| !LOADER_FILLS.contains(name) && !delegates.iter().any(|(_, via)| via == name));
        by_name.retain(|_, defs| defs.iter().any(|d| d.2.splat || d.2.option("required").is_some()));
        let mut fields: Vec<&'m str> = by_name
            .into_iter()
            .filter_map(|(name, defs)| Some((name, self.merge(model, defs)?)))
            .filter(|(name, field)| takes_no_default(field) && !self.parent_fills(classes, name))
            .map(|(name, _)| name)
            .collect();
        fields.sort_unstable();
        fields
    }

    /// Whether a class has a default for `name` that no `Field` tells: a hook of a parent class whose
    /// model has the field, or the `_defaults` of a series that reads them.
    fn parent_fills(&self, classes: &[(usize, &'m ClassFact)], name: &str) -> bool {
        let hooked = |c: &ClassFact| fills_on_create(c) && self.index.defines_field(&c.model, name, self.scope);
        let legacy = self.series.is_none_or(|s| s < DEFAULTS_GONE);
        classes
            .iter()
            .any(|(_, c)| hooked(c) || (legacy && legacy_default(c, name)))
    }

    /// OD321 and OD322 on what `write` gives the fields the record still holds once its scope loaded.
    fn record_write(&self, out: &mut Out<'m>, path: &'m Path, write: &'m RecordWrite) {
        // A dependency's unread file loads first and adds no Selection value, as a Python field wins
        // over a manual one. The module's own file may replace what this write gives.
        if self.module.lost_data() || !self.sees_all_classes(&write.model) {
            return;
        }
        for field in &write.fields {
            let known = !matches!(field.value, WrittenValue::Opaque | WrittenValue::Model(_));
            if !known || !self.holds(write, &field.name) || self.parent_decides(write, &field.name) {
                continue;
            }
            let Some(target) = self.written_field(&write.model, &field.name) else {
                continue;
            };
            match &field.value {
                WrittenValue::Ids(ids) => self.written_ids(out, path, write, field.name.as_str(), &target, ids),
                WrittenValue::Text(text) => self.written_text(out, path, write, field.name.as_str(), &target, text),
                WrittenValue::Model(_) | WrittenValue::Opaque => {}
            }
        }
    }

    /// Whether `write` makes the last write of `name` its scope reads: what a module depending on this
    /// one replaces later does not mend a database without that module, nor a demo write one without
    /// demo data.
    fn holds(&self, write: &'m RecordWrite, name: &str) -> bool {
        let id = write.id.as_ref().map(|id| full_id(&self.module.name, id));
        let Some(writes) = id.and_then(|id| self.index.value_writes.get(&id)) else {
            return true;
        };
        let writes_it = |w: &RecordWrite| w.fields.iter().any(|f| f.name == name);
        let last = writes.iter().rev().find(|w| self.scope.reads(w) && writes_it(w.fact));
        last.is_none_or(|w| std::ptr::eq(w.fact, write))
    }

    /// Whether the record enclosing `write` overrides or drops `name`. Odoo gives a record nested in
    /// a one2many its inverse field whatever it says, and creates none nested in any other field.
    fn parent_decides(&self, write: &'m RecordWrite, name: &str) -> bool {
        let Some((model, field)) = &write.nested_in else {
            return false;
        };
        let enclosing = self.written_field(model, field);
        let Some(enclosing) = enclosing.filter(|f| f.defs[0].2.ftype == "One2many") else {
            return true;
        };
        !matches!(enclosing.get("inverse_name"), Merged::Known(OptionValue::Str(i)) if i != name)
    }

    /// The field `name` of `model` as a record writes it: its own, or that of the model it delegates
    /// to, which an own field of that name shadows. `_inherits` cycles end at the seen model.
    fn written_field(&self, model: &'m str, name: &str) -> Option<Field<'m>> {
        let mut seen = FxHashSet::default();
        let mut model = model;
        while seen.insert(model) {
            let mut defs = self.index.merged_fields(model, self.scope);
            defs.retain(|d| d.2.name == name);
            if !defs.is_empty() {
                return self.merge(model, defs);
            }
            model = self.delegated_field(model, name)?.0;
        }
        None
    }

    /// OD321 on each id of `ids` none of whose records fits the comodel of `field`.
    fn written_ids(
        &self,
        out: &mut Out<'m>,
        path: &'m Path,
        write: &RecordWrite,
        name: &str,
        field: &Field<'m>,
        ids: &[Spanned<String>],
    ) {
        if !RELATIONAL_TYPES.contains(&field.defs[0].2.ftype.as_str()) || field.stored() != Some(true) {
            return;
        }
        let Merged::Known(OptionValue::Str(comodel)) = field.get("comodel_name") else {
            return;
        };
        if !self.sees_classes(comodel) {
            return;
        }
        let fits = |m: &&str| self.index.stands_for(m, comodel, self.scope) != Some(false);
        for id in ids {
            let full = full_id(&self.module.name, &id.value);
            let models = self.index.record_models(&full, self.series).unwrap_or_default();
            if let Some(other) = models.first().filter(|_| !models.iter().any(&fits)) {
                let message = format!(
                    "'{full}' is a record of {other}, but {}.{name} takes {comodel}",
                    write.model
                );
                out.push(path, "OD321", id.pos, message);
            }
        }
    }

    /// OD322 on a value no indexed definition of the field lists: one only a module outside the
    /// scope adds may still load first, as an auto-installed bridge does.
    fn written_text(
        &self,
        out: &mut Out<'m>,
        path: &'m Path,
        write: &RecordWrite,
        name: &str,
        field: &Field<'m>,
        text: &Spanned<String>,
    ) {
        let validated = matches!(
            field.get("validate"),
            Merged::Unset | Merged::Known(OptionValue::Bool(true))
        );
        if field.defs[0].2.ftype != "Selection" || !validated {
            return;
        }
        if field.get("related") != Merged::Unset || field.stored() != Some(true) {
            return;
        }
        let defs = self.index.field_defs(&write.model, name, None);
        let Some(values) = self.listed_values(&defs, write.csv) else {
            return;
        };
        if values.contains(text.value.as_str()) || (write.csv && csv_label(&defs, &values, &text.value, self.series)) {
            return;
        }
        let message = format!("{}.{name} has no value '{}'", write.model, text.value.escape_debug());
        out.push(path, "OD322", text.pos, message);
    }

    /// The values `defs` list between them. None when one of them only tells a run, or none sets
    /// `selection`, which leaves the base of a `selection_add` out of the index.
    fn listed_values(&self, defs: &[FieldDef<'m>], csv: bool) -> Option<FxHashSet<&'m str>> {
        let tuples = csv || tuple_checked(self.series);
        let mut values = FxHashSet::default();
        for (_, _, def) in defs {
            if def.splat {
                return None;
            }
            selection_values(def, tuples, &mut values)?;
        }
        defs.iter().any(|d| d.2.option("selection").is_some()).then_some(values)
    }

    /// Checks the domain `rule` writes, if the record still holds it when Odoo reads it.
    fn rule_domain(&self, out: &mut Out<'m>, path: &'m Path, rule: &'m RuleFact) {
        if rule.fields.is_empty() {
            return;
        }
        let writes = self.rule_state(rule);
        let holder = |w: &&Placed<&'m RuleFact>| !matches!(w.fact.domain, ActionValue::Unset);
        let holds = writes
            .iter()
            .rev()
            .find(holder)
            .is_some_and(|w| std::ptr::eq(w.fact, rule));
        let active = writes.iter().rev().find_map(|w| w.fact.active);
        let Some(model) = self.rule_model(&writes).filter(|_| holds && active != Some(false)) else {
            return;
        };
        if self.sees_classes(model) {
            let reach = self.checked_reach(RULE_DOMAIN_VALIDATED);
            for field in &rule.fields {
                self.domain_path(out, path, model, field, reach);
            }
        }
    }

    /// `fact` as the only write of its record, in a file of this module. The reader is the file that
    /// holds it, so `demo` false loads it in either scope.
    fn own<F>(&self, fact: F) -> Placed<F> {
        Placed {
            module: self.index.by_name[self.module.name.as_str()],
            demo: false,
            fact,
        }
    }

    /// The writes that decide what Odoo checks. From 15.0, and in an unknown series, it validates each
    /// write as it loads, on what the modules in scope gave so far that the reader's file is sure of,
    /// so no demo write for a data file. Before, the query reads what the whole load left, without
    /// the demo writes a data file cannot count on.
    fn rule_state(&self, rule: &'m RuleFact) -> Vec<Placed<&'m RuleFact>> {
        match self.record_rule_writes(rule) {
            Some(writes) if self.series.is_some_and(|s| s < RULE_DOMAIN_VALIDATED) => {
                writes.iter().filter(|w| self.scope.loads(w)).copied().collect()
            }
            _ => self.rule_upto(rule),
        }
    }

    /// The writes of the record of `rule` that the scope reads, up to `rule`: what the record holds
    /// once `rule` loads.
    fn rule_upto(&self, rule: &'m RuleFact) -> Vec<Placed<&'m RuleFact>> {
        let Some(writes) = self.record_rule_writes(rule) else {
            return vec![self.own(rule)];
        };
        let upto = writes.iter().position(|w| std::ptr::eq(w.fact, rule));
        let upto = upto.map_or(writes.len(), |at| at + 1);
        writes[..upto].iter().filter(|w| self.scope.reads(w)).copied().collect()
    }

    /// Every write to the record of `rule`, in load order, when it has an id.
    fn record_rule_writes(&self, rule: &'m RuleFact) -> Option<&[Placed<&'m RuleFact>]> {
        let id = full_id(&self.module.name, rule.id.as_ref()?);
        self.index.rule_writes.get(&id).map(Vec::as_slice)
    }

    /// The model of the last `model_id` of `writes`, whose XML id the module that wrote it resolves.
    fn rule_model(&self, writes: &[Placed<&'m RuleFact>]) -> Option<&'m str> {
        let write = writes
            .iter()
            .rev()
            .find(|w| !matches!(w.fact.model, ActionValue::Unset))?;
        self.written_model(write.module, &write.fact.model, write.fact.by_name)
    }

    /// The model a `model_id` written by the module `mi` names.
    fn written_model(&self, mi: usize, model: &'m ActionValue, by_name: bool) -> Option<&'m str> {
        let ActionValue::Literal(model) = model else {
            return None;
        };
        if by_name {
            return Some(&model.value);
        }
        self.index
            .model_of_xmlid(&full_id(&self.index.modules[mi].name, &model.value))
    }

    /// Checks the calls `server` makes, if `runs_held_code` says they run. `server_model` gives a
    /// created child's model.
    fn server_code(&self, out: &mut Out<'m>, path: &'m Path, server: &'m ServerFact) {
        if server.calls.is_empty() {
            return;
        }
        let writes = server_writes(self.index, self.mi, server);
        if !runs_held_code(self.index, self.scope, self.series, server, &writes) {
            return;
        }
        let id = server.id.as_ref().map(|id| full_id(&self.module.name, id));
        let Some(model) = self.server_model(&writes, &mut id.into_iter().collect()) else {
            return;
        };
        let calls = server.calls.iter().filter(|c| !callable_attr(&c.value, self.series));
        for call in calls {
            if let Some(message) = self.method_gap(model, &call.value) {
                out.push(path, "OD320", call.pos, message);
            }
        }
    }

    /// The model `writes` leave a server action on: that of the last write after the one creating
    /// the record that gives `model_id`, or else the one it was created on. `seen` holds the actions
    /// already followed, whose `parent_id` may form a cycle.
    fn server_model(&self, writes: &[Placed<&'m ServerFact>], seen: &mut FxHashSet<String>) -> Option<&'m str> {
        let mut read = writes.iter().filter(|w| self.scope.reads(w));
        let creator = read.next()?;
        match read.rfind(|w| !matches!(w.fact.model, ActionValue::Unset)) {
            Some(w) => self.written_model(w.module, &w.fact.model, w.fact.by_name),
            None => self.created_model(creator, seen),
        }
    }

    /// The model `creator` creates a server action on: from `CHILD_ACTION_MODEL` that of its parent,
    /// whatever it gives `model_id`. An unknown series may be on either side of that bound.
    fn created_model(&self, creator: &Placed<&'m ServerFact>, seen: &mut FxHashSet<String>) -> Option<&'m str> {
        let own = self.written_model(creator.module, &creator.fact.model, creator.fact.by_name);
        let orphan = matches!(creator.fact.parent, ActionValue::Unset);
        if orphan || self.series.is_some_and(|s| s < CHILD_ACTION_MODEL) {
            return own;
        }
        let parent = self.parent_model(creator, seen);
        parent.filter(|_| self.series.is_some() || parent == own)
    }

    /// The model the parent of `child` holds when Odoo creates `child`, if the scan can tell: only
    /// a parent all of whose writes agree says, as a child keeps the model of the moment it was
    /// created. `parent_id` points at `ir.actions.server`, so a cron gives no model.
    fn parent_model(&self, child: &Placed<&'m ServerFact>, seen: &mut FxHashSet<String>) -> Option<&'m str> {
        let ActionValue::Literal(parent) = &child.fact.parent else {
            return None;
        };
        let id = full_id(&self.index.modules[child.module].name, &parent.value);
        let writes = self.index.server_writes.get(&id)?;
        if !seen.insert(id) {
            return None;
        }
        let mut read = writes.iter().filter(|w| self.scope.reads(w));
        if read.clone().any(|w| w.fact.cron) {
            return None;
        }
        let model = self.created_model(read.next()?, seen)?;
        let agrees = |w: &Placed<&'m ServerFact>| {
            matches!(w.fact.model, ActionValue::Unset)
                || self.written_model(w.module, &w.fact.model, w.fact.by_name) == Some(model)
        };
        read.all(agrees).then_some(model)
    }

    /// OD311 on the template `report` names, if the record still holds it and renders one. A Python
    /// report model only gives the values: the type alone tells whether a template is needed.
    fn report_template(&self, out: &mut Out<'m>, path: &'m Path, report: &'m ReportFact) {
        let ActionValue::Literal(template) = &report.template else {
            return;
        };
        let mi = self.index.by_name[self.module.name.as_str()];
        let state = report_state(self.index, mi, self.scope, self.series, report);
        let holder = state.iter().rev().find(|r| !matches!(r.template, ActionValue::Unset));
        let qweb = state.iter().rev().find_map(|r| r.qweb).unwrap_or(true);
        if qweb && holder.is_some_and(|r| std::ptr::eq(*r, report)) {
            self.qweb_call(out, path, template);
        }
    }

    fn actions(&self, out: &mut Out<'m>, path: &'m Path, actions: &'m [ActionFact]) {
        for action in actions {
            if let ActionValue::Literal(model) = &action.model {
                self.data_model_ref(out, path, model);
            }
            if let ActionValue::Literal(mode) = &action.view_mode {
                self.action_view_mode(out, path, action, mode);
            }
        }
    }

    /// Checks `mode` when Odoo keeps it. Each field holds the last value it was given, so a `view_mode`
    /// counts only if nothing replaces it, and an update takes its model from what the load left.
    /// Only `scope` loads ahead of this module: what a module depending on it replaces later does not
    /// mend a database without that module. A model that stays unknown is out of the scan or an `eval`.
    /// A delegate's hidden parent does not stop the check: Odoo looks a view up by the model's `_name`
    /// and calls the default on the model, so only a class merged into it can give a type.
    fn action_view_mode(&self, out: &mut Out<'m>, path: &'m Path, action: &'m ActionFact, mode: &Spanned<String>) {
        let own = [self.own(action)];
        let id = action.id.as_ref().map(|id| full_id(&self.module.name, id));
        let writes = id.and_then(|id| self.index.action_writes.get(&id));
        let writes = writes.map_or(&own[..], Vec::as_slice);
        let last = |value: fn(&'m ActionFact) -> &'m ActionValue| {
            let loaded = writes.iter().filter(|w| self.scope.reads(w));
            loaded
                .rev()
                .map(|w| value(w.fact))
                .find(|v| !matches!(v, ActionValue::Unset))
        };
        if !last(|a| &a.view_mode).is_some_and(|v| std::ptr::eq(v, &action.view_mode)) {
            return;
        }
        if let Some(ActionValue::Literal(model)) = last(|a| &a.model)
            && self.sees_classes(&model.value)
            && !self.index.merged_hidden_parents(&model.value, self.scope)
        {
            self.view_mode(out, path, &model.value, mode, writes);
        }
    }

    /// An unread data file may hold a view, but not `activity_ids`: a field it adds is named `x_`.
    fn view_mode(
        &self,
        out: &mut Out<'m>,
        path: &'m Path,
        model: &str,
        mode: &Spanned<String>,
        writes: &[Placed<&ActionFact>],
    ) {
        let mut seen = FxHashSet::default();
        let kinds = mode.value.split(',').map(str::trim).filter(|k| !k.is_empty());
        for kind in kinds.filter(|k| seen.insert(*k)) {
            let gap = self.activity_gap(model, kind);
            let held = |_: &String| !self.lost && !self.action_names(writes, kind);
            let gap = gap.or_else(|| self.view_gap(model, kind).filter(held));
            if let Some(message) = gap {
                out.push(path, "OD318", mode.pos, message);
            }
        }
    }

    /// Whether `writes` give the action a view of `kind`. A view counts from any indexed module, as one
    /// in `view_types` does, as long as the scope loads that write. A module of unknown series may run
    /// from `NESTED_RECORDS`.
    fn action_names(&self, writes: &[Placed<&ActionFact>], kind: &str) -> bool {
        let nested = self.series.is_none_or(|s| s >= NESTED_RECORDS);
        let of_kind = |kinds: &[String]| kinds.iter().any(|k| k == kind);
        writes
            .iter()
            .any(|w| self.scope.loads(w) && w.fact.row_names_view(kind, nested))
            || self
                .held(writes, |a| !matches!(a.view_id, ActionViewId::Unset))
                .any(|w| self.view_id_names(w, kind))
            || self
                .held(writes, |a| a.context_kinds.is_some())
                .any(|w| w.fact.context_kinds.as_deref().is_some_and(of_kind))
    }

    /// The writes of a field that may be what the action holds: only a write in scope undoes those
    /// before it, and one from outside the scope after it counts as that module may be installed.
    fn held<'w, 'a>(
        &self,
        writes: &'w [Placed<&'a ActionFact>],
        writes_field: fn(&ActionFact) -> bool,
    ) -> impl Iterator<Item = &'w Placed<&'a ActionFact>> {
        let start = writes.iter().rposition(|w| self.scope.reads(w) && writes_field(w.fact));
        let from = writes[start.unwrap_or(0)..].iter();
        from.filter(move |w| self.scope.loads(w) && writes_field(w.fact))
    }

    /// Whether the `view_id` of `write` serves `kind`: Odoo gives it to its view's type alone. A view
    /// the scan does not reach may be of any type.
    fn view_id_names(&self, write: &Placed<&ActionFact>, kind: &str) -> bool {
        match &write.fact.view_id {
            ActionViewId::Ref(id) => {
                let id = full_id(&self.index.modules[write.module].name, id);
                self.index.view_kind(id).is_none_or(|k| k == kind)
            }
            ActionViewId::Unread => true,
            ActionViewId::Unset | ActionViewId::Cleared => false,
        }
    }

    /// Why the client cannot open `model` as `kind` whatever view it gets: from `ACTIVITY_IN_MAIL`
    /// its domain names `activity_ids`, and the server refuses a field the model lacks. A field only
    /// a module outside `depends` adds counts, as that module may be installed, and so may one that
    /// a delegate the scan cannot read gives, or a hidden parent of a delegate.
    fn activity_gap(&self, model: &str, kind: &str) -> Option<String> {
        let searched = kind == "activity" && self.series.is_some_and(|s| s >= ACTIVITY_IN_MAIL);
        let absent = || {
            !self.index.unread_delegate(model, self.scope)
                && !self.index.hidden_parents(model, self.scope)
                && matches!(self.index.field(model, "activity_ids", self.scope), Err(None))
        };
        (searched && absent()).then(|| {
            format!("{model} has no field activity_ids, which the activity view searches; inherit mail.activity.mixin")
        })
    }

    /// Whether this module's series needs no view of `kind` from the model. `activity` is reported
    /// from `ACTIVITY_IN_MAIL` only, and a module of unknown series may run below it.
    fn generated(&self, kind: &str) -> bool {
        GENERATED_VIEWS.contains(&kind) || (kind == "activity" && self.series.is_none_or(|s| s < ACTIVITY_IN_MAIL))
    }

    /// Why opening `model` as `kind` fails. A view counts from any indexed module, in `depends` or
    /// not, as a bridge module that depends on this one may be what adds it. A default counts in
    /// scope only: a fresh database of this module lacks one from a module outside `depends`.
    fn view_gap(&self, model: &str, kind: &str) -> Option<String> {
        let types = &self.index.view_types;
        if self.generated(kind)
            || types.contains(&(model, Some(kind)))
            || types.contains(&(model, None))
            || self.index.default_view(model, kind, self.scope)
        {
            return None;
        }
        if kind == "calendar" {
            return self.calendar_gap(model);
        }
        if fetches_own_view(self.series) && self.index.client_views.contains(kind) {
            return None;
        }
        Some(format!("{model} has no primary {kind} view, and nothing generates one"))
    }

    /// What Odoo misses to generate a calendar for `model`. A field or a `_date_name` only a module
    /// outside `depends` gives counts, as that module may be installed. A `_date_name` that is no
    /// literal may name any, and a hidden parent of a delegate may give any field.
    fn calendar_gap(&self, model: &str) -> Option<String> {
        if self.index.hidden_parents(model, self.scope) {
            return None;
        }
        let mut starts = CALENDAR_STARTS.to_vec();
        for (_, class) in self.index.chain_classes(model) {
            match class.attr("_date_name").map(|a| &a.value) {
                Some(OptionValue::Str(name)) => starts.push(name),
                Some(_) => return None,
                None => {}
            }
        }
        let has = |field: &&str| !matches!(self.index.field(model, field, self.scope), Err(None));
        if !starts.iter().any(has) {
            return Some(format!(
                "{model} has no primary calendar view, and no date field to generate one from"
            ));
        }
        let needs_stop = self.series.is_some_and(|s| s < CALENDAR_STOP_OPTIONAL);
        (needs_stop && !CALENDAR_STOPS.iter().any(has)).then(|| {
            format!("{model} has no primary calendar view, and no stop or duration field to generate one from")
        })
    }

    /// A dependency outside the index may load the module that defines an id out of scope.
    fn xml_ref(&self, out: &mut Out<'m>, path: &'m Path, r: &XmlRef) {
        let index = self.index;
        let id = full_id(&self.module.name, &r.id);
        match index.xmlid_defs(&id, self.series).as_slice() {
            [] => self.undefined_ref(out, path, r.pos, &id),
            defs if defs.iter().any(|d| self.scope.sees(d.module)) => {}
            _ if self.partial => {}
            defs => {
                let m = &index.modules[defs[0].module].name;
                out.push(
                    path,
                    "OD302",
                    r.pos,
                    format!("'{id}' is defined by module {m}; add it to depends"),
                );
            }
        }
    }

    /// An `ir.model` or `ir.model.fields` id of a model this module names at runtime.
    fn runtime_id(&self, owner: &str, local: &str) -> bool {
        self.dynamic && owner == self.module.name && (local.starts_with("model_") || local.starts_with("field_"))
    }

    /// Whether `owner` is a module in scope that loads a data file the scan cannot read, so the XML
    /// ids under its name are not all known. On a complete scan a module out of scope is a missing
    /// dependency either way.
    fn lost_owner(&self, owner: &str) -> bool {
        let module = self.index.by_name.get(owner).filter(|&&mi| self.scope.sees(mi));
        module.is_some_and(|&mi| self.index.modules[mi].lost_data())
    }

    /// Whether `owner` may hold ids the index lacks: the module is not indexed and a dependency
    /// outside the index may be or load it, or it loads a data file the scan cannot read and is in
    /// scope or may be loaded the same way.
    fn open_owner(&self, owner: &str) -> bool {
        match self.index.by_name.get(owner) {
            Some(&mi) => self.lost_owner(owner) || (self.partial && self.index.modules[mi].lost_data()),
            None => self.partial,
        }
    }

    /// Whether `local` may name a field of an unread class of `owner`, a module in scope, or one that
    /// a class of `owner` gets from a parent the scan cannot read: the id may exist, but the scan has
    /// no field to build it from. On a partial scope it may also be spelled from a table outside the
    /// index, so that the scan has no prefix to match it with.
    fn unread_field_id(&self, owner: &str, local: &str) -> bool {
        let Some(&mi) = self.index.by_name.get(owner).filter(|&&mi| self.scope.sees(mi)) else {
            return false;
        };
        let mut classes = self.index.modules[mi].python.iter().flat_map(|f| &f.facts.classes);
        let series = self.series.or(self.index.modules[mi].series);
        classes.any(|c| {
            let mut prefixes = self.index.field_prefixes(&c.model, series).into_iter();
            (self.index.unread_fields(c, self.scope) && prefixes.any(|p| local.starts_with(&p)))
                || (self.partial && self.index.unseen_table_id(c, local, series, self.scope))
        })
    }

    fn undefined_ref(&self, out: &mut Out<'m>, path: &'m Path, pos: Pos, id: &str) {
        let (owner, local) = id.split_once('.').unwrap_or_default();
        if self.runtime_id(owner, local) || self.unread_field_id(owner, local) || self.open_owner(owner) {
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
            None => out.push(
                path,
                "OD302",
                pos,
                format!("'{id}' belongs to module {owner}, which is neither in depends nor on the addons path"),
            ),
        }
    }

    /// Python code may name ids of modules outside the index, such as Enterprise ones, behind a
    /// runtime check, so only ids of indexed modules are checked; `hidden`, for a reference no such
    /// check covers, also reports those of modules missing from depends, unless a dependency outside
    /// the index may load one that holds the id.
    fn py_ref(&self, out: &mut Out<'m>, path: &'m Path, r: &XmlRef, hidden: bool) {
        let (owner, local) = r.id.split_once('.').unwrap_or_default();
        let Some(&mi) = self.index.by_name.get(owner) else {
            return;
        };
        let defs = self.index.xmlid_defs(&r.id, self.series);
        if !self.scope.sees(mi) {
            let loadable = self.partial && !defs.is_empty();
            if hidden && !loadable && !self.open_owner(owner) {
                let message = format!("'{}' belongs to module {owner}; add it to depends", r.id);
                out.push(path, "OD302", r.pos, message);
            }
        } else if defs.is_empty() && !self.lost_owner(owner) && !self.unread_field_id(owner, local) {
            out.push(
                path,
                "OD301",
                r.pos,
                format!("module {owner} defines no XML id '{}'", r.id),
            );
        }
    }

    /// A reference behind runtime tests to a module outside scope reports as an unguarded one would,
    /// unless what the tests prove installed brings that module in. A test that names nothing the
    /// scan reads, or what it does not index, may prove the module of any id.
    fn guarded_ref(&self, out: &mut Out<'m>, path: &'m Path, r: &GuardedRef) {
        let owner = r.xml_ref.id.split_once('.').unwrap_or_default().0;
        let Some(&mi) = self.index.by_name.get(owner).filter(|&&mi| !self.scope.sees(mi)) else {
            return self.py_ref(out, path, &r.xml_ref, false);
        };
        let Some(guard) = r.proven.as_deref().and_then(|proven| self.guard_scope(proven)) else {
            return;
        };
        if !guard.sees(mi) {
            let partial = self.index.partial_scope(&guard);
            Ctx {
                scope: &guard,
                partial,
                ..*self
            }
            .py_ref(out, path, &r.xml_ref, true);
        }
    }

    /// The scope of the module once each of `proven` is installed, none when one is not indexed.
    /// A model stands for every module that introduces it.
    fn guard_scope(&self, proven: &[Subject]) -> Option<Scope> {
        let mut named = vec![];
        for subject in proven {
            match subject {
                Subject::Module(name) => named.push(*self.index.by_name.get(name.as_str())?),
                Subject::Model(model) => {
                    let introducers = self.index.classes(model).iter().filter(|(_, c)| c.new_model);
                    let before = named.len();
                    named.extend(introducers.map(|(mi, _)| *mi));
                    if named.len() == before {
                        return None;
                    }
                }
            }
        }
        Some(self.index.guard_scope(self.mi, named, self.series))
    }

    fn view(&self, out: &mut Out<'m>, path: &'m Path, view: &ViewFact, trees: &mut FxHashMap<String, ViewTree<'m>>) {
        let model = match (&view.model, &view.inherit) {
            (Some(m), _) => {
                self.data_model_ref(out, path, m);
                Some(m.value.as_str())
            }
            (None, Some(parent)) => self.index.view_model(full_id(&self.module.name, parent)),
            (None, None) => None,
        };
        let Some(model) = model else { return };
        self.view_domains(out, path, model, view, trees);
        self.buttons(out, path, model, view, trees);
        self.modifiers(out, path, model, view, trees);
        for field in view
            .fields
            .iter()
            .filter(|f| !f.path.iter().any(|s| s == UNKNOWN_CONTEXT))
        {
            if self.unsure_spec_subview(model, view, &field.path, trees) {
                continue;
            }
            let Some(joined) = self.arch_path(model, &field.path) else {
                continue;
            };
            if let Err(e) = self.index.resolve_path(model, &joined, self.scope)
                && !self.may_hide(&e)
            {
                out.push(path, "OD304", field.pos, e.message(&joined));
            }
        }
    }

    fn domains(&self, out: &mut Out<'m>, path: &'m Path, domains: &[DomainFact]) {
        for d in domains {
            for f in &d.fields {
                self.domain_path(out, path, &d.model, f, self.installs);
            }
        }
    }

    /// Domains in an arch filter the model at their position: the view's, or a subview's comodel.
    fn view_domains(
        &self,
        out: &mut Out<'m>,
        path: &'m Path,
        model: &str,
        view: &ViewFact,
        trees: &mut FxHashMap<String, ViewTree<'m>>,
    ) {
        let validated = self.checked_reach(VIEW_DOMAIN_VALIDATED);
        for d in &view.domains {
            let unsure = self.unsure_spec_subview(model, view, &d.path, trees);
            let target = self.arch_model(model, &d.path).filter(|_| !unsure);
            let reach = if d.validated { validated } else { self.installs };
            for f in target.iter().flat_map(|t| d.fields.iter().map(move |f| (t, f))) {
                self.domain_path(out, path, f.0, f.1, reach);
            }
        }
    }

    /// The model of an arch position: the view's, or the comodel of the subviews that enclose it.
    fn arch_model(&self, model: &str, segments: &[String]) -> Option<String> {
        if segments.iter().any(|s| s == UNKNOWN_CONTEXT) {
            return None;
        }
        match self.arch_path(model, segments)?.as_str() {
            "" => Some(model.to_owned()),
            joined => self.index.path_comodel(model, joined, self.scope),
        }
    }

    /// The type of the tree a view belongs to, unless it is QWeb, whose attributes are plain HTML. A
    /// root the index cannot read may be QWeb.
    fn arch_kind<'a>(&'a self, view: &'a ViewFact) -> Option<&'a str> {
        let kind = if view.inherit.is_some() {
            root_kind(self.index, self.module, view)
        } else {
            view.kind.as_deref()
        };
        kind.filter(|k| *k != QWEB)
    }

    /// Odoo checks a button on the model at its arch position.
    fn buttons(
        &self,
        out: &mut Out<'m>,
        path: &'m Path,
        model: &str,
        view: &ViewFact,
        trees: &mut FxHashMap<String, ViewTree<'m>>,
    ) {
        if self.arch_kind(view).is_none() {
            return;
        }
        for button in &view.buttons {
            if self.unsure_spec_subview(model, view, &button.path, trees) {
                continue;
            }
            if let Some(target) = self.arch_model(model, &button.path) {
                self.button(out, path, &target, &button.method);
            }
        }
    }

    /// A leading `_` or a private method of `BaseModel` is private whatever the model defines, so it
    /// needs no lookup, even on a model the scan cannot see whole. `@api.private` counts on any
    /// definition, since Odoo walks the whole MRO.
    fn button(&self, out: &mut Out<'m>, path: &'m Path, model: &str, method: &Spanned<String>) {
        let name = &method.value;
        if name.starts_with('_')
            || base_private_method(name, self.series)
            || self.index.private_method(model, name, self.scope)
        {
            let message = format!("method '{name}' of {model} is private, so Odoo refuses to call it from a button");
            out.push(path, "OD313", method.pos, message);
        } else if let Some(message) = self.method_gap(model, name) {
            out.push(path, "OD312", method.pos, message);
        }
    }

    fn function_call(&self, out: &mut Out<'m>, path: &'m Path, call: &FunctionCall) {
        if let Some(message) = self.method_gap(&call.model, &call.method.value) {
            out.push(path, "OD314", call.method.pos, message);
        }
    }

    /// Whether the scan sees every class and data file that could add to `model`. A caller stays
    /// silent on a model that is not visible: only the places that call `model_ref` raise OD201 or
    /// OD202.
    fn sees_whole_model(&self, model: &str) -> bool {
        !self.lost && self.sees_all_classes(model)
    }

    /// What `sees_whole_model` asks, short of the data files.
    fn sees_all_classes(&self, model: &str) -> bool {
        self.sees_classes(model) && !self.index.hidden_parents(model, self.scope)
    }

    /// Whether the scan reaches every class of `model`, short of the parents a class hides from it.
    fn sees_classes(&self, model: &str) -> bool {
        !self.partial
            && !self.dynamic
            && matches!(self.index.model_status(model, self.scope, None), Status::Visible)
            && !self.index.unindexed_parent(model, self.scope)
    }

    /// Whether the scan sees every class that could define a method of `model`: no data file adds
    /// one, and a delegate's hidden parent cannot, since `_inherits` passes on fields only.
    fn sees_methods(&self, model: &str) -> bool {
        self.sees_classes(model) && !self.index.merged_hidden_parents(model, self.scope)
    }

    /// Why `model` lacks the method a data file calls, when the scan can tell.
    fn method_gap(&self, model: &str, method: &str) -> Option<String> {
        if !self.sees_methods(model) {
            return None;
        }
        match self.index.data_method(model, method, self.scope, self.series) {
            Status::Visible => None,
            Status::Hidden(m) => Some(format!(
                "method '{method}' of {model} comes from module {m}, which is not in depends"
            )),
            Status::Missing => Some(format!("{model} has no method '{method}'")),
        }
    }

    /// Specs of a view must each find their element in the tree of views it extends, which is
    /// built once per root and module. A dependency outside the index loads first, so it cannot add
    /// a view to a tree whose root the module defines.
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
        // A parent outside depends is OD302's to report.
        if !self.index.views.visible(&parent, |m| self.scope.sees(m)) {
            return;
        }
        let Some(tree) = self.view_tree(parent, trees) else {
            return;
        };
        if self.may_extend(&tree.root) {
            return;
        }
        for locator in &view.locators {
            let e = located(&locator.value, &self.module.name);
            if tree.visible.contains(&e) {
                continue;
            }
            let element = format!("<{} {}=\"{}\">", e.tag, e.attr, e.value);
            let message = match tree.hidden.get(&e) {
                Some(m) => format!("{element} is added by module {m}, which is not in depends"),
                None if self.lost => continue,
                None => format!("no {element} in view {} or the views that extend it", tree.root),
            };
            out.push(path, "OD310", locator.pos, message);
        }
    }

    /// Whether a dependency outside the index may add a view to the tree under `root`.
    fn may_extend(&self, root: &str) -> bool {
        self.partial && self.index.views.definers(root).any(|m| m != self.mi)
    }

    /// Whether a dependency outside the index may define template `name` by loading a module that
    /// does. It cannot load one that depends on this module: Odoo leaves a circular `depends` out.
    fn may_load_template(&self, name: &str) -> bool {
        self.partial && self.index.qweb_definers(name).any(|m| !self.index.reaches(m, self.mi))
    }

    /// The tree of views `id` belongs to, or None when the elements of some view on the way to its
    /// root are unknown.
    fn view_tree<'t>(&self, id: String, trees: &'t mut FxHashMap<String, ViewTree<'m>>) -> Option<&'t ViewTree<'m>> {
        let root = self.index.views.root(id)?;
        let tree = trees
            .entry(root.clone())
            .or_insert_with(|| self.index.views.tree(root, self.index.modules, |m| self.scope.sees(m)));
        Some(tree)
    }

    /// Odoo validates modifiers on every view but a QWeb one. The gate comes first so that a series
    /// without them, whose attributes mean something else, parses nothing.
    fn modifiers(
        &self,
        out: &mut Out<'m>,
        path: &'m Path,
        model: &str,
        view: &ViewFact,
        trees: &mut FxHashMap<String, ViewTree<'m>>,
    ) {
        if !rule("OD315").applies_to(self.series) {
            return;
        }
        let Some(kind) = self.arch_kind(view) else { return };
        let read: Vec<&ArchModifier> = view
            .modifiers
            .iter()
            .filter(|m| !self.lone_constant(m, view, trees))
            .collect();
        for m in read {
            match modifier::names(&m.value.value) {
                Ok(names) => {
                    for name in &names {
                        let Some(name) = self.modifier_name(model, view, kind, m, name) else {
                            continue;
                        };
                        if let Some((code, message)) = self.modifier_finding(&name, trees) {
                            out.push(path, code, m.value.pos, message);
                        }
                    }
                }
                Err(why) => out.push(
                    path,
                    "OD315",
                    m.value.pos,
                    format!("invalid {} expression: {why}", m.attr),
                ),
            }
        }
    }

    /// Whether an `add` of a constant may stand alone, which Odoo takes unparsed: it joins a value
    /// only when every element the spec may locate holds one, as far as the tree shows. The order
    /// of the tree's extensions is out of reach, so a spec that may empty the modifier on one of
    /// those elements counts, whichever of the element's names it uses.
    fn lone_constant(&self, m: &ArchModifier, view: &ViewFact, trees: &mut FxHashMap<String, ViewTree<'m>>) -> bool {
        let Some(joins) = m.joins.as_ref().filter(|_| modifier::constant(&m.value.value)) else {
            return false;
        };
        let parent = view.inherit.as_deref().map(|p| full_id(&self.module.name, p));
        let Some(tree) = parent.and_then(|p| self.view_tree(p, trees)) else {
            return true;
        };
        let joins = located(joins, &self.module.name);
        let matches: Vec<_> = tree.valued.iter().filter(|v| v.elements.contains(&joins)).collect();
        let reaches = |target: &ArchElement| matches.iter().any(|v| v.elements.contains(target));
        let emptied = tree
            .emptied
            .iter()
            .any(|e| e.attr == m.attr && e.target.as_ref().is_none_or(reaches));
        emptied || matches.is_empty() || !matches.iter().all(|v| v.attrs.contains(&m.attr))
    }

    /// Resolves a name to the field it reads, unless that field is out of the scan's reach. Each
    /// `parent.` leaves a subview; Odoo ignores what leaves the outermost one.
    fn modifier_name<'a>(
        &self,
        model: &'a str,
        view: &'a ViewFact,
        kind: &'a str,
        m: &'a ArchModifier,
        name: &'a str,
    ) -> Option<ModifierName<'a>> {
        self.arch_model(model, &m.path)?;
        let mut steps: Vec<String> = subview_steps(&m.path).cloned().collect();
        let mut field = name;
        while let Some(rest) = field.strip_prefix("parent.") {
            steps.pop()?;
            field = rest;
        }
        let owner = self.arch_model(model, &steps)?;
        let active = ACTIVE_NAMES.contains(&field) && self.series.is_some_and(|s| s < ACTIVE_NAMES_UNTIL);
        (field != "id" && !active && self.sees_whole_model(&owner)).then_some(ModifierName {
            model,
            view,
            kind,
            attr: m.attr,
            name,
            owner,
            field,
            level: steps,
            arch_subviews: &m.subviews,
            locator: spec_locator(&m.path),
        })
    }

    /// A field outside `depends` stays silent: an `auto_install` module may bring it wherever the
    /// view is installed.
    fn modifier_finding(
        &self,
        n: &ModifierName,
        trees: &mut FxHashMap<String, ViewTree<'m>>,
    ) -> Option<(&'static str, String)> {
        match self.index.field(&n.owner, n.field, self.scope) {
            Ok(_) => self.absent_field(n, trees).map(|message| ("OD317", message)),
            Err(None) => self.unknown_name(n, trees).map(|message| ("OD316", message)),
            Err(Some(_)) => None,
        }
    }

    /// Only a dotted name after `parent.` gets here with a dot, and Odoo refuses those whatever the
    /// path would reach.
    fn unknown_name(&self, n: &ModifierName, trees: &mut FxHashMap<String, ViewTree<'m>>) -> Option<String> {
        if self.anchored_in_subview(n, trees)? {
            return None;
        }
        let why = if n.field.contains('.') {
            "Odoo refuses a composed field".to_owned()
        } else {
            format!("{} has no field '{}'", n.owner, n.field)
        };
        Some(format!("{} names '{}': {why}", n.attr, n.name))
    }

    /// Whether an extension's element may have landed in a subview of the tree, where the name
    /// would be a field of the comodel: a spec on a plain element can match inside one. A field
    /// spec takes the first match anywhere, so a subview on a model with the locator leaves the
    /// owner unknown too. None when the tree is unknown.
    fn anchored_in_subview(&self, n: &ModifierName, trees: &mut FxHashMap<String, ViewTree<'m>>) -> Option<bool> {
        let names = [n.locator.unwrap_or(n.field), n.field];
        self.subview_may_have(n.model, n.view, &names, trees)
    }

    /// Whether the spec subview at `path` may sit in another field than the one its path resolves:
    /// a field spec takes the first match anywhere, so the comodel is then unknown.
    fn unsure_spec_subview(
        &self,
        model: &str,
        view: &ViewFact,
        path: &[String],
        trees: &mut FxHashMap<String, ViewTree<'m>>,
    ) -> bool {
        spec_locator(path).is_some_and(|locator| self.subview_may_have(model, view, &[locator], trees) != Some(false))
    }

    /// Whether a subview of the view's tree is on a model that may have one of `names`, or on an
    /// unknown model. None when the tree is unknown.
    fn subview_may_have(
        &self,
        model: &str,
        view: &ViewFact,
        names: &[&str],
        trees: &mut FxHashMap<String, ViewTree<'m>>,
    ) -> Option<bool> {
        let Some(parent) = view.inherit.as_deref() else {
            return Some(false);
        };
        let tree = self.view_tree(full_id(&self.module.name, parent), trees)?;
        let has = |comodel: String| names.iter().any(|name| self.may_have(&comodel, name));
        Some(
            tree.subviews
                .iter()
                .any(|subview| self.arch_model(model, &subview.path).is_none_or(has)),
        )
    }

    /// Whether `field` may exist on `model`, even if only outside `depends` or the scan's reach.
    fn may_have(&self, model: &str, field: &str) -> bool {
        !self.sees_all_classes(model) || !matches!(self.index.field(model, field, self.scope), Err(None))
    }

    /// Before saas~17.3 Odoo refuses a modifier on a field the view lacks. Any element of the tree
    /// counts, in a module outside the scope too, since the check cannot tell what is installed.
    fn absent_field(&self, n: &ModifierName, trees: &mut FxHashMap<String, ViewTree<'m>>) -> Option<String> {
        if !rule("OD317").applies_to(self.series) || !VIEW_FIELD_KINDS.contains(&n.kind) {
            return None;
        }
        let start = n.view.inherit.as_deref().or(n.view.id.as_deref())?;
        let tree = self.view_tree(full_id(&self.module.name, start), trees)?;
        let shown = ["field", "groupby"].into_iter().any(|tag| {
            let element = ArchElement {
                tag: tag.to_owned(),
                attr: "name".to_owned(),
                value: n.field.to_owned(),
            };
            tree.visible.contains(&element) || tree.hidden.contains_key(&element)
        });
        (!shown && !takes_attribute_field(tree, n))
            .then(|| format!("{} reads field '{}', which the view does not show", n.attr, n.field))
    }

    fn qweb_calls(&self, out: &mut Out<'m>, path: &'m Path, view: &ViewFact) {
        if !qweb_view(self.index, self.module, view) {
            return;
        }
        for call in &view.calls {
            self.qweb_call(out, path, call);
        }
    }

    /// OD311 on `name`, the target of a server render, unless a view answers to it once the install
    /// is done: Odoo resolves the target at render, so a view of an auto-installed module counts.
    fn qweb_call(&self, out: &mut Out<'m>, path: &'m Path, name: &Spanned<String>) {
        // A module outside the index, such as an Enterprise one, may define the target.
        let owner = name.value.split_once('.').map(|(m, _)| m);
        if self.partial && !owner.is_some_and(|m| self.index.by_name.contains_key(m)) {
            return;
        }
        let message = match self.index.qweb_status(&name.value, self.installs) {
            Status::Visible => return,
            Status::Hidden(_) if self.may_load_template(&name.value) => return,
            Status::Hidden(m) => format!(
                "template '{}' comes from module {m}, which is not in depends",
                name.value
            ),
            // A key without a module prefix may be in any lost file.
            Status::Missing if owner.map_or(self.lost, |m| self.lost_owner(m)) => return,
            Status::Missing => format!("no indexed module defines template '{}'", name.value),
        };
        out.push(path, "OD311", name.pos, message);
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
        // A dependency outside the index may add the model or the field.
        if self.partial {
            return;
        }
        if !matches!(
            self.index.model_status(&record.model.value, self.scope, None),
            Status::Visible
        ) {
            self.data_model_ref(out, path, &record.model);
            return;
        }
        if self.lost || self.index.unread_class(&record.model.value, self.scope) {
            return;
        }
        for field in record
            .fields
            .iter()
            .filter(|f| !popped_value(record, &f.value, self.series))
        {
            let fname = field_name(&field.value, self.series);
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

/// Whether `name` is the `value` that `create` and `write` of `ir.property` pop into the column of
/// the property's type (`odoo/addons/base/models/ir_property.py:79,118,139` on 17.0), where the XML
/// loader sends a record. An import refuses the column before `create` (`odoo/models.py:1353` on
/// 17.0), and `ir.property` in the index of an unknown series means a base that pops it. A name
/// with a language stays apart: nothing pops `value@fr`.
fn popped_value(record: &RecordFact, name: &str, series: Option<Series>) -> bool {
    !record.csv && record.model.value == "ir.property" && name == "value" && series.is_none_or(|s| s < IR_PROPERTY_GONE)
}

/// The field a data file's `name` sets. `name@fr` is `name` where the loader leaves it to the load of
/// a language's terms, and an unknown series may be one of those, so reporting it there could be false.
fn field_name(name: &str, series: Option<Series>) -> &str {
    match name.split_once('@') {
        Some((field, _)) if series.is_none_or(|s| s >= DATA_TRANSLATIONS) => field,
        _ => name,
    }
}

/// The x2many fields of an arch position, without the locators a spec leaves in it.
fn subview_steps(path: &[String]) -> impl Iterator<Item = &String> {
    path.iter().filter(|s| !s.starts_with(LOCATOR))
}

/// Whether the subview the name lands on loads that field itself. Odoo checks each subview apart
/// (`odoo/addons/base/models/ir_ui_view.py:1473` on 17.0), so this counts only on its own level. The
/// subview the arch shows there decides, or for a calendar an xpath only names, the calendars of the
/// tree. A spec that reaches in unnamed may land in any subview of the level, so all must load it.
fn takes_attribute_field(tree: &ViewTree, n: &ModifierName) -> bool {
    let at_level = |s: &&ArchSubview| subview_steps(&s.path).eq(&n.level);
    let loads = |s: &ArchSubview| s.attribute_fields.iter().any(|field| field == n.field);
    let mut subviews = tree.subviews.iter().copied().filter(at_level).peekable();
    match n.arch_subviews.iter().rev().find(at_level) {
        Some(shown) if shown.tag == ATTRIBUTE_SUBVIEW && shown.attribute_fields.is_empty() => subviews.any(loads),
        Some(shown) => loads(shown),
        None => subviews.peek().is_some() && subviews.all(loads),
    }
}

/// The field a spec locates to open the subview `path` runs through. A locator followed by another
/// name is a field the spec adds beside it, whose subview reads that field as any other.
fn spec_locator(path: &[String]) -> Option<&str> {
    match path {
        [first, opened, ..] => first.strip_prefix(LOCATOR).filter(|locator| locator == opened),
        _ => None,
    }
}

/// Fields of a record the XML loader fills itself (`odoo/tools/convert.py:472,476` on 20.0).
const LOADER_FILLS: [&str; 2] = ["sequence", "xmlid"];

/// Methods that may give a record the values its data does not, on the way to `INSERT`.
const CREATE_HOOKS: [&str; 7] = [
    "default_get",
    "create",
    "load",
    "_add_missing_default_values",
    "_prepare_create_values",
    "_load_records",
    "_load_records_create",
];

/// The first series whose ORM ignores `_defaults` (10.0; 9.0 merges it in `openerp/models.py:3012`).
const DEFAULTS_GONE: Series = Series { major: 10, minor: 0 };

/// Where the module may have made a record before its data asks for it.
struct Loaded {
    /// Where each of the module's own XML ids is first defined, as `first_defs` says.
    ids: FxHashMap<String, (usize, u32)>,
    /// The first `<function>` of the files the manifest loads, as (load rank, line).
    first_call: Option<(usize, u32)>,
}

impl Loaded {
    fn of(module: &Module) -> Self {
        let calls = module.data.iter().flat_map(|f| {
            let rank = f.rank.into_iter();
            rank.flat_map(|r| f.facts.functions.iter().map(move |c| (r, c.method.pos.line)))
        });
        Self {
            ids: first_defs(module),
            first_call: calls.min(),
        }
    }
}

/// What a create of a model must get from elsewhere than its fields.
#[derive(Default)]
struct Required<'m> {
    /// The required fields with a column and no default, sorted.
    fields: Vec<&'m str>,
    /// The `_inherits` parents Odoo creates for a record, with the field that names each.
    delegates: Vec<(&'m str, &'m str)>,
}

type RequiredFields<'m> = FxHashMap<&'m str, Rc<Required<'m>>>;

/// What gives the fields of a record, in `Ctx::required_gaps`.
#[derive(Clone)]
struct Given<'m> {
    write: &'m RecordWrite,
    /// It sits in a demo file.
    demo: bool,
    /// The model of the record, then each parent down to the one read.
    models: Vec<&'m str>,
}

/// A value for `.id` makes Odoo update that row (`odoo/models.py:5489` on 18.0).
fn database_id(field: &FieldWrite) -> bool {
    field.name == "id" || (field.name == ".id" && matches!(field.value, WrittenValue::Text(_)))
}

fn fills_on_create(class: &ClassFact) -> bool {
    CREATE_HOOKS.iter().any(|h| class.methods.iter().any(|m| m == h))
}

/// Whether the `_defaults` of `class` may hold `name`. An expression may hold anything.
fn legacy_default(class: &ClassFact, name: &str) -> bool {
    class.attr("_defaults").is_some_and(|d| match &d.value {
        OptionValue::Dict(keys) => keys.iter().any(|(key, _)| key == name),
        _ => true,
    })
}

/// Whether a create leaves the column of `field` to the record, to fail on NOT NULL. Odoo computes a
/// `compute` or `related` field after the `INSERT` or with it, which only a run tells.
fn takes_no_default(field: &Field) -> bool {
    field.is("required", &OptionValue::Bool(true))
        && field.get("default") == Merged::Unset
        && field.get("compute") == Merged::Unset
        && field.get("related") == Merged::Unset
        && has_certain_column(field)
}

/// What the registry makes of an attribute of a field, as far as literals tell.
#[derive(Clone, Copy, PartialEq)]
enum Merged<'m> {
    Unset,
    Known(&'m OptionValue),
    Unknown,
}

/// The first series whose Many2one setup gives an empty `ondelete` its default, and refuses a
/// required field's 'set null', only after `_inherits_check` (`saas~14.4`). The check can then meet
/// a delegating field with no `ondelete`, and has replaced its 'set null' before the refusal.
const LATE_MANY2ONE_SETUP: Series = Series { major: 14, minor: 4 };

/// The first series whose Many2one setup fills an empty `ondelete` before `_inherits_check` reads
/// it (`saas~12.2`).
const EARLY_ONDELETE_DEFAULT: Series = Series { major: 12, minor: 2 };

/// The first series that refuses ondelete 'restrict' on a Many2one to one of
/// `NO_RESTRICT_COMODELS` (`saas~13.1`).
const REFUSED_IR_RESTRICT: Series = Series { major: 13, minor: 1 };

/// The comodels Odoo keeps ondelete 'restrict' from, since uninstalling a module deletes their
/// records.
const NO_RESTRICT_COMODELS: [&str; 7] = [
    "ir.model",
    "ir.model.data",
    "ir.model.fields",
    "ir.model.fields.selection",
    "ir.model.relation",
    "ir.model.constraint",
    "ir.module.module",
];

/// The first series whose field setup no longer merges a definition with the field of a model
/// class in the registry (`saas~12.5`).
const SETUP_MAGIC: Series = Series { major: 12, minor: 5 };

/// The first series whose model metaclass adds the magic fields to the class that introduces a
/// model, rather than the setup of every model (`saas~14.4`).
const METACLASS_MAGIC: Series = Series { major: 14, minor: 4 };

/// The first series without the `__last_update` field (`saas~16.1`).
const NO_LAST_UPDATE: Series = Series { major: 16, minor: 1 };

/// The first series whose `BaseModel` defines `id` and `display_name` (`saas~18.1`).
const BASE_MAGIC: Series = Series { major: 18, minor: 1 };

/// The first series that refuses an `_inherits` field that is not required with ondelete 'cascade'
/// or 'restrict', and a `delegate=True` field no entry names (`saas~18.3`).
const REFUSED_DELEGATION: Series = Series { major: 18, minor: 3 };

/// The first series that builds no company domain for a `check_company` field of a model without
/// a company field, logging that it cannot (`18.0`, `odoo/fields.py:3177`; `saas~17.4` builds it for
/// every model, `odoo/fields.py:3030`).
const COMPANY_FIELD_DOMAIN: Series = Series { major: 18, minor: 0 };

/// The first series whose name search reads `_rec_names_search` (`saas~15.3`, `odoo/models.py:1924`;
/// `saas~15.2` knows `_rec_name` alone, `odoo/models.py:1932`).
const REC_NAMES_SEARCH: Series = Series { major: 15, minor: 3 };

/// The first series that moves the acceptance of `tracking` to `mail.track.mixin`, so an abstract
/// model loses it (`saas~19.3`, `addons/mail/models/mail_track_mixin.py:34`; `saas~19.2` still
/// accepts it, `addons/mail/models/models.py:28`). No lower bound applies: `mail` accepts it from
/// `saas~13.4`, before OD219 starts, in `addons/mail/models/ir_model.py:11` until `saas~14.3`.
const MIXIN_TRACKING: Series = Series { major: 19, minor: 3 };

/// The first series that keeps a hierarchy in `parent_path` (`saas~11.3`, `odoo/models.py:2308`;
/// `saas~11.2` asks for `parent_left` and `parent_right`, `odoo/models.py:2236`).
const PARENT_PATH: Series = Series { major: 11, minor: 3 };

/// The first series that warns of a repeated label as it reflects a model (`saas~11.2`,
/// `odoo/addons/base/models/ir_model.py:807`; `saas~11.1` says nothing).
const LABEL_WARNING: Series = Series { major: 11, minor: 2 };

/// An `_inherits` entry of a class: its module, the class, the parent model and the field.
type Entry<'m> = (usize, &'m ClassFact, &'m str, &'m Spanned<String>);

/// The models the classes of one model list in `_inherit`, for `Ctx::entry_before_in`.
struct Listed<'m> {
    parents: Vec<Parent<'m>>,
}

/// An entry of `_inherit`: the class listing it, its position, the parent model and the classes
/// that model merges.
struct Parent<'m> {
    class: &'m ClassFact,
    at: usize,
    model: &'m str,
    merges: Vec<(usize, &'m ClassFact)>,
}

impl<'m> Listed<'m> {
    /// The one entry whose parent merges `class`. None when no entry or several merge it, as for a
    /// class reached through two parents, or one of `base` under two, since every model merges it.
    fn through(&self, class: &ClassFact) -> Option<&Parent<'m>> {
        let has = |classes: &[(usize, &ClassFact)]| classes.iter().any(|(_, c)| std::ptr::eq(*c, class));
        let mut hits = self.parents.iter().filter(|p| has(&p.merges));
        let first = hits.next()?;
        hits.next().is_none().then_some(first)
    }
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

    /// The value the last definitions that set the option agree on. A definition that unpacks
    /// `**kwargs` without naming the option may set it, and an option given as an expression that
    /// only a run can evaluate is unknown, so either makes the value unknown.
    fn get(&self, name: &str) -> Merged<'m> {
        self.get_where(name, |_| true)
    }

    /// The value of `get` among the definitions `keep` picks.
    fn get_where(&self, name: &str, keep: impl Fn(&FieldDef<'m>) -> bool) -> Merged<'m> {
        let mut merged = Merged::Unset;
        for i in self.last(|d| keep(d) && (d.2.splat || d.2.option(name).is_some())) {
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
    /// none unless `store=True`. Odoo reads `related` and `compute` for their truth, so
    /// `compute=None` leaves the field stored. A company-dependent field with no `store=False`,
    /// `compute` or `related` reads as stored because its value is written as a column's would be.
    fn stored(&self) -> Option<bool> {
        match self.get("store") {
            Merged::Known(OptionValue::Bool(b)) => Some(*b),
            Merged::Unset => match (truth(self.get("related")), truth(self.get("compute"))) {
                (Some(false), Some(false)) => Some(true),
                (None, _) | (_, None) => None,
                _ => Some(false),
            },
            _ => None,
        }
    }

    fn position(&self, own: &FieldFact) -> Option<usize> {
        self.defs.iter().position(|d| std::ptr::eq(d.2, own))
    }
}

/// The fields of a model as the registry builds them in scope, by name, and their labels.
struct ModelFields<'m> {
    model: &'m str,
    fields: FxHashMap<&'m str, Field<'m>>,
    labels: Vec<Label<'m>>,
    /// The view `init()` creates for the model, when its text spells the names of the columns.
    view: Option<&'m SqlView>,
}

/// The label Odoo gives a field of a model, and the definition of this module that decides it with
/// its load key, if one does.
struct Label<'m> {
    text: String,
    name: &'m str,
    here: Option<(&'m FieldFact, (usize, Pos))>,
}

/// The fields Odoo adds to a model, with their labels: the first two, then `__last_update` in the
/// series that have it, then the rest only when the model logs access. See `Ctx::magic_labels`
/// for the models that get them.
const MAGIC_LABELS: &[(&str, &str)] = &[
    ("id", "ID"),
    ("display_name", "Display Name"),
    ("__last_update", "Last Modified on"),
    ("create_uid", "Created by"),
    ("create_date", "Created on"),
    ("write_uid", "Last Updated by"),
    ("write_date", "Last Updated on"),
];

/// The label Odoo derives from a field's name: without `_ids` or `_id`, underscores made spaces,
/// in Python's title case, which capitalizes each letter that follows no other letter.
fn derived_label(name: &str) -> String {
    let stem = name
        .strip_suffix("_ids")
        .or_else(|| name.strip_suffix("_id"))
        .unwrap_or(name);
    let mut label = String::with_capacity(stem.len());
    let mut after_letter = false;
    for c in stem.chars() {
        let cased = c.is_lowercase() || c.is_uppercase();
        match (c, cased, after_letter) {
            ('_', ..) => label.push(' '),
            (_, true, true) => label.extend(c.to_lowercase()),
            (_, true, false) => label.extend(c.to_uppercase()),
            _ => label.push(c),
        }
        after_letter = cased;
    }
    label
}

/// How Odoo adds the fields of `MAGIC_LABELS`, which decides the models that get them and what a
/// definition of the same name makes of them.
#[derive(Clone, Copy, Debug, PartialEq)]
enum MagicEra {
    /// Setup adds them to every model, and a definition merges with the field of the same name on
    /// the registry class of each model it inherits.
    RegistryMerge,
    /// Setup adds them to every model, but not one that a class of the model or of a model it
    /// inherits defines.
    Setup,
    /// The metaclass adds them to the class that introduces a model that is not abstract.
    Metaclass,
    /// `BaseModel` defines `id` and `display_name`, and the metaclass adds the others.
    Base,
}

/// The era of `series`. An unknown series takes the metaclass era, which every series from
/// saas~14.4 through 18.0 shares.
fn magic_era(series: Option<Series>) -> MagicEra {
    match series {
        Some(s) if s < SETUP_MAGIC => MagicEra::RegistryMerge,
        Some(s) if s < METACLASS_MAGIC => MagicEra::Setup,
        Some(s) if s >= BASE_MAGIC => MagicEra::Base,
        _ => MagicEra::Metaclass,
    }
}

/// Whether a client view of `series` fetches the type of its class, whatever kind it registers as.
fn fetches_own_view(series: Option<Series>) -> bool {
    series.is_some_and(|s| s < VIEWS_BY_ACTION_KIND)
}

/// Whether Odoo adds `__last_update`. Not for an unknown series, as the newest series lack it.
fn has_last_update(series: Option<Series>) -> bool {
    series.is_some_and(|s| s < NO_LAST_UPDATE)
}

/// Whether a hierarchy of `series` needs `parent_path`: an unknown series may.
fn stores_parent_path(series: Option<Series>) -> bool {
    series.is_none_or(|s| s >= PARENT_PATH)
}

/// Whether Odoo warns of a repeated label in `series`: an unknown series may.
fn warns_on_labels(series: Option<Series>) -> bool {
    series.is_none_or(|s| s >= LABEL_WARNING)
}

fn unlogged(class: &ClassFact) -> bool {
    class
        .attr("_log_access")
        .is_some_and(|a| a.value == OptionValue::Bool(false))
}

/// What becomes of the field Odoo adds to a model under a name that indexed classes define too.
enum MagicBase<'a> {
    /// The definitions `After` picks merge into it, so its label stands unless one sets `string`.
    Kept(&'a str, After),
    /// Odoo does not add its own field, or from saas~18.1 merges `display_name` first, so the
    /// definitions decide the label.
    Replaced,
    /// Odoo adds its own field over every definition and merges none, so its label stands.
    Fixed(&'a str),
    /// A model it inherits defines the field too, and where Odoo merges that one is not modelled.
    Unknown,
}

/// The definitions that merge after the field Odoo adds.
#[derive(Clone, Copy)]
enum After {
    /// All of them: Odoo's field merges before every class.
    Every,
    /// Those of the classes of the model that do not introduce it: Odoo adds the field to the
    /// introducing class, which merges after the models it inherits.
    Extensions,
}

impl After {
    fn keeps(self, field: &Field, def: &FieldDef) -> bool {
        match self {
            After::Every => true,
            After::Extensions => def.1.model == field.model && !def.1.new_model,
        }
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

/// Whether Odoo raises on a record that `method` leaves without a value of the field: it does for
/// a field it neither stores nor lets users write, as every definition merged into it sets, and
/// gives the others False. A related field gets Odoo's own compute method.
fn raises_unassigned(field: &Field, method: &str) -> bool {
    let readonly = match field.get("readonly") {
        Merged::Unset => field.get("inverse") == Merged::Unset,
        merged => merged == Merged::Known(&OptionValue::Bool(true)),
    };
    readonly
        && field.stored() == Some(false)
        && field.get("related") == Merged::Unset
        && field.is("compute", &OptionValue::Str(method.to_owned()))
}

/// The methods, by model, that a class of the module overrides with a `super()` call. The order
/// of the files that import them is not indexed, so any class may come after the one it extends.
fn super_overrides(module: &Module) -> FxHashSet<(&str, &str)> {
    let classes = module.python.iter().flat_map(|f| &f.facts.classes);
    let overrides = classes.flat_map(|c| c.extended.iter().map(|m| (c.model.as_str(), m.as_str())));
    overrides.collect()
}

/// OD223 at each compute method of the class that can leave a field unassigned, naming the fields
/// Odoo raises on. A method that a class of the module overrides with a `super()` call is skipped,
/// since the override may assign the rest.
fn unassigned_computes<'m>(
    out: &mut Out<'m>,
    path: &'m Path,
    class: &ClassFact,
    fields: &ModelFields<'m>,
    overrides: &FxHashSet<(&str, &str)>,
) {
    let extended = |(method, _): &&(Spanned<String>, _)| overrides.contains(&(&*class.model, &*method.value));
    for (method, names) in class.unassigned_computes.iter().filter(|c| !extended(c)) {
        let raises = |name: &&str| {
            fields
                .fields
                .get(name)
                .is_some_and(|f| raises_unassigned(f, &method.value))
        };
        let raising: Vec<&str> = names.iter().map(String::as_str).filter(raises).collect();
        if !raising.is_empty() {
            out.push(path, "OD223", method.pos, unassigned_message(&method.value, &raising));
        }
    }
}

fn unassigned_message(method: &str, fields: &[&str]) -> String {
    let (names, it) = match fields {
        [field] => (format!("'{field}'"), "it"),
        _ => (format!("'{}'", fields.join("', '")), "them"),
    };
    format!(
        "{method}() can leave {names} unassigned on a record, so reading {it} raises ValueError; assign {it} on every path through the loop, False when there is no value"
    )
}

/// Model and field definitions Odoo refuses, ignores or logs as wrong when it loads the registry,
/// or that fail once used. Each rule reads every definition merged into the model, so none runs
/// where a module in scope depends on a module that is not indexed. None runs on a model whose
/// class with `_name` is out of scope, or where a class of it, or of a model it inherits or
/// delegates to, has a plain Python base class or an `_inherit` element that is not a literal, a
/// parent whose fields the scan cannot see.
impl<'m> Ctx<'_, 'm> {
    fn registry(&self, out: &mut Out<'m>) {
        if self.partial {
            return;
        }
        let checked = |c: &&ClassFact| {
            self.index.introduced(&c.model, self.scope) && !self.index.hidden_parents(&c.model, self.scope)
        };
        let mut models: FxHashMap<&'m str, ModelFields<'m>> = FxHashMap::default();
        let mut reported_self_dependencies = FxHashSet::default();
        let overrides = super_overrides(self.module);
        for file in &self.module.python {
            for class in file.facts.classes.iter().filter(checked) {
                let model = class.model.as_str();
                let fields = models.entry(model).or_insert_with(|| self.model_fields(model));
                for own in &class.fields {
                    self.registry_field(out, &file.path, fields, own);
                }
                self.self_dependencies(out, &file.path, class, fields, &mut reported_self_dependencies);
                unassigned_computes(out, &file.path, class, fields, &overrides);
                self.parent_store(out, &file.path, class);
                self.view_not_created(out, &file.path, class);
                self.view_id(out, &file.path, class, fields);
                self.definitions(out, &file.path, class);
                for (_, entry) in &class.delegations {
                    self.delegation(out, &file.path, class, entry);
                }
            }
        }
    }

    fn registry_field(&self, out: &mut Out<'m>, path: &'m Path, fields: &ModelFields<'m>, own: &'m FieldFact) {
        let Some(field) = fields.fields.get(own.name.as_str()) else {
            return;
        };
        let Some(at) = field.position(own) else {
            return;
        };
        match own.ftype.as_str() {
            "Monetary" => self.monetary(out, path, field, own),
            "Many2one" => {
                self.required_set_null(out, path, field, own);
                self.delegate_field(out, path, field, own);
            }
            "Many2many" if field.stored() == Some(true) => {
                self.long_relation(out, path, field, own);
                self.relation_clash(out, path, field, own);
            }
            "Selection" => self.selection_add(out, path, field, at),
            _ => {}
        }
        self.tracking(out, path, field, own);
        self.check_company(out, path, field, own);
        self.duplicate_label(out, path, fields, own);
        self.view_field(out, path, fields, own);
        self.exposure(out, path, field, own);
    }

    /// The fields of `model` as the registry builds them in scope, merged once for every rule.
    fn model_fields(&self, model: &'m str) -> ModelFields<'m> {
        let mut by_name: FxHashMap<&'m str, Vec<FieldDef<'m>>> = FxHashMap::default();
        for def in self.index.merged_fields(model, self.scope) {
            by_name.entry(def.2.name.as_str()).or_default().push(def);
        }
        let names: FxHashSet<&str> = by_name.keys().copied().collect();
        let fields: FxHashMap<&'m str, Field<'m>> = by_name
            .into_iter()
            .filter_map(|(name, defs)| Some((name, self.merge(model, defs)?)))
            .collect();
        let magic = self.magic_labels(model);
        let magic_of = |name: &str| magic.iter().find(|(n, _)| *n == name).map(|(_, text)| *text);
        let mut labels: Vec<Label<'m>> = fields
            .iter()
            .filter_map(|(name, f)| self.label(name, f, magic_of(name)))
            .collect();
        labels.extend(
            magic
                .iter()
                .filter(|(name, _)| !names.contains(name))
                .map(|&(name, text)| Label {
                    text: text.to_owned(),
                    name,
                    here: None,
                }),
        );
        ModelFields {
            model,
            fields,
            labels,
            view: self.view_of(model),
        }
    }

    /// The field `name` of `model` as the registry builds it in scope.
    fn merged(&self, model: &'m str, name: &str) -> Option<Field<'m>> {
        let mut defs = self.index.merged_fields(model, self.scope);
        defs.retain(|d| d.2.name == name);
        self.merge(model, defs)
    }

    /// A field from its definitions in scope. None when the order of a definition that changes its
    /// type against another one is unknown, or the type is not that of the last definitions.
    fn merge(&self, model: &'m str, defs: Vec<FieldDef<'m>>) -> Option<Field<'m>> {
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

    /// Whether the registry certainly merges definition `a` of a field of `model` before `b`.
    fn before(&self, model: &str, a: &FieldDef<'m>, b: &FieldDef<'m>) -> bool {
        self.merges_before(model, (a.0, a.1, a.2.pos), (b.0, b.1, b.2.pos))
    }

    /// Whether the registry certainly merges what module `a.0`'s class `a.1` gives `model` at `a.2`
    /// before what `b` gives. The classes of `model` come after those of the models they inherit,
    /// unless one of them lists another model before its own in `_inherit`, which puts that model
    /// after the classes loaded before it. Among them, a dependency's come first, then in each
    /// module the class that introduces the model, then the others of one file in order; files load
    /// in the order of the imports, which is not indexed.
    fn merges_before(&self, model: &str, a: (usize, &ClassFact, Pos), b: (usize, &ClassFact, Pos)) -> bool {
        match (a.1.model == model, b.1.model == model) {
            (_, false) => false,
            (false, true) => !self.inherits_ahead(model),
            _ if a.0 != b.0 => self.index.reaches(b.0, a.0) && !self.index.reaches(a.0, b.0),
            _ if a.1.new_model != b.1.new_model => a.1.new_model,
            _ => self.in_one_file(a.0, a.1, b.1) && a.2 < b.2,
        }
    }

    /// `merges_before` for the `_inherits` entries of `model`, which also orders two classes of
    /// models it inherits. Odoo merges the `_inherits` of each model one class lists in `_inherit`
    /// whole, in reverse, so the first listed parent's entries come last. Field definitions do not
    /// share this order: they merge over Python's method resolution order.
    fn entry_before(&self, model: &str, a: (usize, &ClassFact, Pos), b: (usize, &ClassFact, Pos)) -> bool {
        self.entry_before_in(model, a, b, &mut vec![model])
    }

    /// `entry_before`, having passed through the models in `seen`, `model` among them. Unknown
    /// between two classes of other models when either has no single parent, or the parents are
    /// listed by different classes of `model`, whose order in the registry's bases is not indexed.
    fn entry_before_in<'s>(
        &self,
        model: &'s str,
        a: (usize, &ClassFact, Pos),
        b: (usize, &ClassFact, Pos),
        seen: &mut Vec<&'s str>,
    ) -> bool
    where
        'm: 's,
    {
        if std::ptr::eq(a.1, b.1) {
            // One class keeps its own order whichever path merges it.
            return self.merges_before(&a.1.model, a, b);
        }
        if a.1.model == model || b.1.model == model {
            return self.merges_before(model, a, b);
        }
        let listed = self.listed(model);
        let (Some(pa), Some(pb)) = (listed.through(a.1), listed.through(b.1)) else {
            return false;
        };
        if pa.model == pb.model {
            // Defence only: on an `_inherit` cycle, `through` finds two paths first.
            if seen.contains(&pa.model) {
                return false;
            }
            seen.push(pa.model);
            return self.entry_before_in(pa.model, a, b, seen);
        }
        std::ptr::eq(pa.class, pb.class) && pa.at > pb.at
    }

    /// The models the in-scope classes of `model` list in `_inherit`, with what each merges.
    fn listed(&self, model: &str) -> Listed<'m> {
        let classes = self.index.classes(model).iter().filter(|(mi, _)| self.scope.sees(*mi));
        let parents = classes.flat_map(|&(_, class)| {
            let listed = class.inherit.iter().enumerate().filter(|(_, p)| p.value != model);
            listed.map(move |(at, p)| Parent {
                class,
                at,
                model: p.value.as_str(),
                merges: self.index.merged_classes(&p.value, self.scope),
            })
        });
        Listed {
            parents: parents.collect(),
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
        let missing = |name: &str| matches!(self.index.field(model, name, self.setup), Err(None));
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
        // The rewrite test walks the inherit chain, so it runs only on a field that needs it.
        if field.is("required", &OptionValue::Bool(true))
            && field.is("ondelete", &OptionValue::Str("set null".to_owned()))
            && !self.ondelete_rewritten(field, &own.name)
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

    /// Whether Odoo may replace the field's `ondelete` with 'cascade' before it holds 'set null'
    /// against `required`, as it does when the field delegates by `delegate` or an `_inherits`
    /// entry. Until saas~14.3 it holds it first, in `setup_base`; from saas~14.4 it rewrites it in
    /// `_inherits_check`, until saas~18.3 refuses the field.
    fn ondelete_rewritten(&self, field: &Field<'m>, name: &str) -> bool {
        self.late_ondelete() && (may_delegate(field) || self.inherits_entry(field.model, name) != Some(false))
    }

    /// Whether `_inherits_check` reads a Many2one's `ondelete` before its setup gives the default:
    /// from saas~14.4, until saas~18.3 refuses such a field outright.
    fn late_ondelete(&self) -> bool {
        self.series
            .is_some_and(|s| LATE_MANY2ONE_SETUP <= s && s < REFUSED_DELEGATION)
    }

    /// Whether the field's setup fills an empty `ondelete`, by the truth of the value, with one
    /// Odoo accepts before `_inherits_check` reads it, so that `required=True` is all the field
    /// lacks: saas~12.2 until saas~14.3.
    fn ondelete_defaulted(&self, field: &Field<'m>) -> bool {
        self.series
            .is_some_and(|s| EARLY_ONDELETE_DEFAULT <= s && s < LATE_MANY2ONE_SETUP)
            && !self.default_refused(field)
    }

    /// Whether the 'restrict' a required field would get by default is refused for its comodel.
    /// A transient model's required field to such a comodel gets 'cascade' instead.
    fn default_refused(&self, field: &Field<'m>) -> bool {
        let refusing = |c: &&str| NO_RESTRICT_COMODELS.contains(c);
        self.series.is_some_and(|s| s >= REFUSED_IR_RESTRICT)
            && matches!(field.get("comodel_name"), Merged::Known(OptionValue::Str(c)) if refusing(&c.as_str()))
            && self
                .index
                .kind(field.model, self.scope)
                .is_some_and(|kind| kind != ModelKind::Transient)
    }

    /// A `delegate=True` field that no `_inherits` entry names. Before saas~18.3, Odoo forces it
    /// required with a warning, and from saas~14.4 fails on a field with no ondelete, reading it
    /// before giving it a default: OD228 reports that, and OD218 the rest. From saas~18.3 Odoo
    /// refuses such a field whatever its attributes, which neither rule reports.
    fn delegate_field(&self, out: &mut Out<'m>, path: &'m Path, field: &Field<'m>, own: &FieldFact) {
        const OPTIONS: [&str; 3] = ["delegate", "required", "ondelete"];
        let sets = |f: &FieldFact| OPTIONS.iter().any(|o| f.option(o).is_some());
        let refused = self.series.is_some_and(|s| s >= REFUSED_DELEGATION);
        if refused || !self.lone_delegate(field, &own.name) || !self.reported_at(field, own, sets) {
            return;
        }
        let Some(fix) = delegation_fix(field, self.ondelete_defaulted(field)) else {
            return;
        };
        let at = OPTIONS.iter().find_map(|o| own.option(o)).map_or(own.pos, |o| o.pos);
        let name = &own.name;
        if self.late_ondelete() && no_ondelete(field) {
            let message = format!(
                "'{name}' has delegate=True and no ondelete, so Odoo fails with AttributeError when it sets up the registry; give it {fix}"
            );
            out.push(path, "OD228", at, message);
        } else {
            let message = format!(
                "'{name}' has delegate=True, so it must be required with ondelete 'cascade' or 'restrict'; give it {fix}"
            );
            out.push(path, "OD218", at, message);
        }
    }

    /// Whether the field delegates with `delegate=True` alone: not related, and certainly named by
    /// no `_inherits` entry of its model.
    fn lone_delegate(&self, field: &Field<'m>, name: &str) -> bool {
        field.is("delegate", &OptionValue::Bool(true))
            && field.get("related") == Merged::Unset
            && self.inherits_entry(field.model, name) == Some(false)
    }

    /// Whether the `_inherits` of `model` names the field `name`, None when unknown: a class gives
    /// an `_inherits` or lists an `_inherit` that is not literal, whose unseen parent may give an
    /// entry, or the last classes giving a parent model disagree on whether its field is `name`.
    /// Odoo merges the `_inherits` of the classes the model merges, those of the models it
    /// inherits included, key by key, a later class replacing the field of a parent.
    fn inherits_entry(&self, model: &str, name: &str) -> Option<bool> {
        let classes = self.index.merged_classes(model, self.scope);
        if classes.iter().any(|(_, c)| c.inherits_unknown || c.inherit_unknown) {
            return None;
        }
        let entries: Vec<Entry<'m>> = classes
            .iter()
            .flat_map(|&(mi, c)| c.delegations.iter().map(move |(parent, f)| (mi, c, parent.as_str(), f)))
            .collect();
        let parents = entries.iter().filter(|e| e.3.value == name).map(|e| e.2);
        let named: Vec<Option<bool>> = parents.map(|p| self.names_parent(model, &entries, p, name)).collect();
        if named.contains(&Some(true)) {
            Some(true)
        } else {
            (!named.contains(&None)).then_some(false)
        }
    }

    /// Whether the last entries for `parent` name the field `name`, None when only some do.
    fn names_parent(&self, model: &str, entries: &[Entry<'m>], parent: &str, name: &str) -> Option<bool> {
        let at = |e: &Entry<'m>| (e.0, e.1, e.3.pos);
        let keyed: Vec<&Entry<'m>> = entries.iter().filter(|e| e.2 == parent).collect();
        let last = keyed
            .iter()
            .filter(|a| !keyed.iter().any(|b| self.entry_before(model, at(a), at(b))));
        let names: Vec<bool> = last.map(|e| e.3.value == name).collect();
        match (names.contains(&true), names.contains(&false)) {
            (true, false) => Some(true),
            (true, true) => None,
            (false, _) => Some(false),
        }
    }

    /// Odoo wants the Many2one an `_inherits` entry names required, with ondelete 'cascade' or
    /// 'restrict'. 18.0 warns and forces both, an unset ondelete included, which saas~12.2 until
    /// saas~14.3 fill first; saas~18.3 refuses the model. A missing field, or one of another type,
    /// is left alone, and an entry repeated in another class of the model in this module is
    /// reported at the first.
    fn delegation(&self, out: &mut Out<'m>, path: &'m Path, class: &'m ClassFact, entry: &Spanned<String>) {
        let names = |c: &ClassFact| c.delegations.iter().any(|(_, d)| d.value == entry.value);
        if !self.first_class(class, names) {
            return;
        }
        let Some(field) = self.merged(&class.model, &entry.value) else {
            return;
        };
        if field.defs[0].2.ftype != "Many2one" {
            return;
        }
        if let Some(fix) = delegation_fix(&field, self.ondelete_defaulted(&field)) {
            let message = format!(
                "_inherits delegates to '{}', which must be required with ondelete 'cascade' or 'restrict'; give it {fix}",
                entry.value
            );
            out.push(path, "OD218", entry.pos, message);
        }
    }

    /// Whether `class` is the first class of its model in this module that `picks`, by the module's
    /// file list and then by position, so that a model is reported once per module.
    fn first_class(&self, class: &ClassFact, picks: impl Fn(&ClassFact) -> bool) -> bool {
        let me = self.index.by_name[self.module.name.as_str()];
        let file = |c: &ClassFact| {
            let has = |x: &ClassFact| std::ptr::eq(x, c);
            self.module.python.iter().position(|f| f.facts.classes.iter().any(has))
        };
        self.index
            .classes(&class.model)
            .iter()
            .filter(|(mi, c)| *mi == me && picks(c))
            .min_by_key(|(_, c)| (file(c), c.pos))
            .is_some_and(|(_, c)| std::ptr::eq(*c, class))
    }

    /// Odoo logs an error when it creates the table of a model with `_parent_store` and no indexed
    /// `parent_path` field. A model without a table of its own, or with a class that sets either
    /// attribute otherwise, is left alone, and one set in several classes of this module is reported
    /// at the first. Only series below `PARENT_PATH` ask for `parent_left`, so a module of unknown
    /// series whose model holds that field is taken to be on one of them.
    fn parent_store(&self, out: &mut Out<'m>, path: &'m Path, class: &'m ClassFact) {
        let sets = |c: &ClassFact| c.attr("_parent_store").is_some();
        let Some(at) = class
            .attr("_parent_store")
            .filter(|_| stores_parent_path(self.series) && !self.older_hierarchy(&class.model))
            .filter(|_| self.first_class(class, sets) && self.parent_store_table(&class.model))
        else {
            return;
        };
        let model = class.model.as_str();
        let message = match self.index.field(model, "parent_path", self.scope) {
            Err(_) => format!(
                "{model} sets _parent_store but has no parent_path field, which Odoo logs as an error; add parent_path = fields.Char(index=True)"
            ),
            Ok(_) => match self.merged(model, "parent_path").map(|f| f.get("index")) {
                Some(Merged::Unset | Merged::Known(OptionValue::Bool(false))) => format!(
                    "{model} sets _parent_store but its parent_path field has no index, which Odoo logs as an error; add index=True to it"
                ),
                _ => return,
            },
        };
        out.push(path, "OD217", at.pos, message);
    }

    /// Whether `model`, in a module of unknown series, holds the `parent_left` field of a hierarchy
    /// from before `PARENT_PATH`. A module outside `depends` does not count, as for `parent_path`.
    fn older_hierarchy(&self, model: &str) -> bool {
        self.series.is_none() && self.index.field(model, "parent_left", self.scope).is_ok()
    }

    /// Whether every class of `model` in scope leaves it a table with `_parent_store`: it has a
    /// table, and none sets `_parent_store` to anything but True.
    fn parent_store_table(&self, model: &str) -> bool {
        self.has_table(model) && self.every_class(model, |c| true_or_unset(c, "_parent_store"))
    }

    /// Whether every class of `model` in scope leaves it a table Odoo builds from its fields: none
    /// is abstract, or sets `_auto` to anything but True.
    fn has_table(&self, model: &str) -> bool {
        self.every_class(model, |c| c.kind != ModelKind::Abstract && true_or_unset(c, "_auto"))
    }

    fn every_class(&self, model: &str, keeps: impl Fn(&ClassFact) -> bool) -> bool {
        self.index
            .classes(model)
            .iter()
            .all(|(mi, c)| !self.scope.sees(*mi) || keeps(c))
    }

    fn some_class(&self, model: &str, picks: impl Fn(&ClassFact) -> bool) -> bool {
        !self.every_class(model, |c| !picks(c))
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
        let (relation, at) = match field.get("relation") {
            Merged::Known(OptionValue::Str(r)) if self.reported_at(field, own, |f| f.option("relation").is_some()) => {
                (r.clone(), pos_of(own, "relation"))
            }
            Merged::Unset if self.introduces(field, own) => match self.implicit_relation(field) {
                Some(r) => (r, own.pos),
                None => return,
            },
            _ => return,
        };
        if relation.len() <= PG_NAME_MAX {
            return;
        }
        let message = format!(
            "relation table '{relation}' of '{}' is {} characters long, and PostgreSQL allows {PG_NAME_MAX}; name a shorter one with relation=",
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
            value: OptionValue::Strs(added) | OptionValue::Tuple(added),
            pos,
        }) = own.option("selection_add")
        else {
            return;
        };
        if field.get("related") != Merged::Unset || !field.is("required", &OptionValue::Bool(true)) {
            return;
        }
        let Some(known) = earlier_values(field, at, tuple_listed(self.series)) else {
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

    /// Odoo hands each attribute a field does not know to the model's `_valid_field_parameter`, and
    /// logs a warning when it refuses it. `BaseModel` only accepts `related_sudo`; `mail.thread` and
    /// the other models that override the method may accept `tracking`, and before saas~19.3 so
    /// does `mail` for an abstract model.
    fn tracking(&self, out: &mut Out<'m>, path: &'m Path, field: &Field<'m>, own: &FieldFact) {
        let Some(at) = own.option("tracking") else {
            return;
        };
        let model = field.model;
        let abstract_accepted = self.series.is_none_or(|s| s < MIXIN_TRACKING)
            && self.index.kind(model, self.scope) == Some(ModelKind::Abstract);
        if abstract_accepted
            || self.index.defines_method(model, "_valid_field_parameter", false)
            || !self.reported_at(field, own, |f| f.option("tracking").is_some())
        {
            return;
        }
        let message = format!(
            "'{}' sets tracking, but {model} does not inherit mail.thread, so Odoo ignores it and logs a warning; inherit mail.thread or remove tracking",
            own.name
        );
        out.push(path, "OD219", at.pos, message);
    }

    /// Odoo logs a warning for each field of a model whose label another field already has. The
    /// definition of this module that decides a label is reported, against a label decided by
    /// another module or one this module loads earlier.
    fn duplicate_label(&self, out: &mut Out<'m>, path: &'m Path, fields: &ModelFields<'m>, own: &FieldFact) {
        if !warns_on_labels(self.series) {
            return;
        }
        let Some((label, key)) = fields.labels.iter().find_map(|l| match l.here {
            Some((def, key)) if std::ptr::eq(def, own) => Some((l, key)),
            _ => None,
        }) else {
            return;
        };
        let earlier = |o: &&Label<'m>| o.name != label.name && o.text == label.text && o.here.map(|h| h.1) < Some(key);
        let Some(other) = fields
            .labels
            .iter()
            .filter(earlier)
            .min_by_key(|o| (o.here.map(|h| h.1), o.name))
        else {
            return;
        };
        let message = format!(
            "'{}' has the label '{}' of field '{}' of {}, so Odoo logs a warning and users cannot tell them apart; give it another string",
            own.name, label.text, other.name, fields.model
        );
        out.push(path, "OD220", pos_of(own, "string"), message);
    }

    /// A field's label: a non-empty literal `string`, else `magic`, the label of the field Odoo
    /// adds under that name, else one derived from its name unless the field is related. It comes
    /// with the last definition of this module that decides it, if any. A `Fixed` magic field
    /// keeps its label even over a literal `string`, a `Kept` one reads `string` only from the
    /// definitions merged after it, and an `Unknown` one has none.
    fn label(&self, name: &'m str, field: &Field<'m>, magic: Option<&str>) -> Option<Label<'m>> {
        let (magic, after) = match magic.map(|text| self.magic_base(name, field, text)) {
            Some(MagicBase::Unknown) => return None,
            Some(MagicBase::Fixed(text)) => {
                return Some(Label {
                    text: text.to_owned(),
                    name,
                    here: None,
                });
            }
            Some(MagicBase::Kept(text, after)) => (Some(text), after),
            Some(MagicBase::Replaced) | None => (None, After::Every),
        };
        let keep = |d: &FieldDef<'m>| after.keeps(field, d);
        let (text, deciders) = match (field.get_where("string", keep), magic) {
            (Merged::Known(OptionValue::Str(s)), _) if !s.is_empty() => {
                (s.clone(), field.last(|d| keep(d) && d.2.option("string").is_some()))
            }
            (Merged::Unset, Some(text)) => (text.to_owned(), vec![]),
            (Merged::Unset, None) if field.get("related") == Merged::Unset => {
                (derived_label(name), field.first(|_| true))
            }
            _ => return None,
        };
        let here = self.decided_here(field, deciders);
        Some(Label { text, name, here })
    }

    /// The last of `deciders`, definitions of `field`, that this module gives the field's model,
    /// with its load key.
    fn decided_here(&self, field: &Field<'m>, deciders: Vec<usize>) -> Option<(&'m FieldFact, (usize, Pos))> {
        let me = self.index.by_name[self.module.name.as_str()];
        deciders
            .into_iter()
            .map(|i| &field.defs[i])
            .filter(|d| d.0 == me && d.1.model == field.model)
            .map(|d| (d.2, self.load_key(d)))
            .max_by_key(|(_, key)| *key)
    }

    /// What becomes of the field Odoo adds as `name`, labelled `text`, given the definitions of
    /// `field`. Before saas~14.4 setup adds `id` over every definition, and from saas~12.5 no
    /// other field that a class defines. Python mangles `__last_update` in a class body, so what
    /// the index records under that name is another field and never changes Odoo's.
    fn magic_base<'a>(&self, name: &str, field: &Field, text: &'a str) -> MagicBase<'a> {
        let era = magic_era(self.series);
        let early = matches!(era, MagicEra::RegistryMerge | MagicEra::Setup);
        match era {
            _ if name == "__last_update" || (early && name == "id") => MagicBase::Fixed(text),
            MagicEra::Setup => MagicBase::Replaced,
            MagicEra::RegistryMerge => self.registry_magic(name, field, text),
            MagicEra::Metaclass | MagicEra::Base => self.introduced_magic(era, name, field, text),
        }
    }

    /// What becomes of the field Odoo adds as `name` before saas~12.5, where a definition merges
    /// with the field of the registry class of each model the model inherits: `base` has
    /// `display_name`, and a model that logs access has the log fields. Where Odoo merges a
    /// definition of another model's class is not modelled.
    fn registry_magic<'a>(&self, name: &str, field: &Field, text: &'a str) -> MagicBase<'a> {
        let model = field.model;
        let logs = |(_, c): &(usize, &ClassFact)| c.model != model && c.model != "base" && self.log_access(&c.model);
        if field.defs.iter().any(|d| d.1.model != model) {
            MagicBase::Unknown
        } else if name == "display_name" || self.index.merged_classes(model, self.scope).iter().any(logs) {
            MagicBase::Kept(text, After::Every)
        } else {
            MagicBase::Replaced
        }
    }

    /// What becomes of the field Odoo adds as `name`, labelled `text`, given the definitions of
    /// `field`, from saas~14.4. The field Odoo adds as `id` sets no `string`, and from saas~18.1
    /// `BaseModel` defines `id` and `display_name` before every class, so any definition's `string`
    /// stands. The metaclass adds any other field to the introducing class only when that class
    /// does not define it, and a model listed before the model's own in `_inherit` merges after it.
    fn introduced_magic<'a>(&self, era: MagicEra, name: &str, field: &Field, text: &'a str) -> MagicBase<'a> {
        let model = field.model;
        let introducer = |d: &FieldDef| d.1.model == model && d.1.new_model;
        if name == "id" {
            MagicBase::Kept(text, After::Every)
        } else if field.defs.iter().any(introducer) {
            // From saas~18.1 this covers `display_name` too: `BaseModel`'s merges first, and the
            // introducer's definition gives the same label unless it is related without `string`,
            // which then stays unknown; deciding it there reports a pair with a mixin's field.
            MagicBase::Replaced
        } else if name == "display_name" && era == MagicEra::Base {
            MagicBase::Kept(text, After::Every)
        } else if field.defs.iter().any(|d| d.1.model != model) && self.inherits_ahead(model) {
            MagicBase::Unknown
        } else {
            MagicBase::Kept(text, After::Extensions)
        }
    }

    /// The fields Odoo adds to `model`, with their labels. Before saas~14.4 every model gets `id`,
    /// `display_name` and `__last_update`, from saas~14.4 only a model that is not abstract, and
    /// from saas~18.1, where `BaseModel` defines the first two, every model gets those again.
    /// `__last_update` stops after 16.0 (`has_last_update`).
    fn magic_labels(&self, model: &str) -> Vec<(&'static str, &'static str)> {
        let era = magic_era(self.series);
        if era == MagicEra::Metaclass && self.index.kind(model, self.scope) == Some(ModelKind::Abstract) {
            return vec![];
        }
        let (last_update, logs) = (has_last_update(self.series), self.log_access(model));
        let added = |name: &str| match name {
            "id" | "display_name" => true,
            "__last_update" => last_update,
            _ => logs,
        };
        MAGIC_LABELS.iter().copied().filter(|(name, _)| added(name)).collect()
    }

    /// Whether Odoo adds the fields that log access to `model`: it has a table of its own, and no
    /// class of it in scope sets `_auto` or `_log_access` to anything but True.
    fn log_access(&self, model: &str) -> bool {
        let on = |c: &ClassFact, name: &str| c.attr(name).is_none_or(|a| a.value == OptionValue::Bool(true));
        self.index.kind(model, self.scope) != Some(ModelKind::Abstract)
            && self
                .index
                .classes(model)
                .iter()
                .all(|(mi, c)| !self.scope.sees(*mi) || (on(c, "_auto") && on(c, "_log_access")))
    }

    /// Odoo warns about a computed field whose dependency leads back to the field itself on
    /// another record, and makes it recursive. Each field is reported once per module, at the first
    /// such path. Abstract models get no dependencies, and from a transient model Odoo does not
    /// follow a path into a regular one.
    fn self_dependencies(
        &self,
        out: &mut Out<'m>,
        path: &'m Path,
        class: &ClassFact,
        fields: &ModelFields<'m>,
        reported: &mut FxHashSet<(&'m str, &'m str)>,
    ) {
        let kind = self.index.kind(&class.model, self.scope);
        if kind == Some(ModelKind::Abstract) {
            return;
        }
        let transient = kind == Some(ModelKind::Transient);
        for (method, paths) in &class.depends {
            for field in fields.fields.values().filter(|f| non_recursive_compute_of(f, method)) {
                let name = field.defs[0].2.name.as_str();
                if let Some(dep) = paths.iter().find(|p| self.leads_back(field, &p.value, transient))
                    && reported.insert((field.model, name))
                {
                    let message = format!(
                        "'{}' makes '{name}' depend on itself on other records, so Odoo logs a warning; declare '{name}' with recursive=True",
                        dep.value
                    );
                    out.push(path, "OD221", dep.pos, message);
                }
            }
        }
    }

    /// Whether a later segment of the dotted path names the field on its own model.
    fn leads_back(&self, field: &Field<'m>, path: &str, transient: bool) -> bool {
        let name = field.defs[0].2.name.as_str();
        let mut model = field.model.to_owned();
        for (i, segment) in path.split('.').enumerate() {
            if i > 0 && model == field.model && segment == name {
                return true;
            }
            let Ok(Some(FieldInfo {
                comodel: Some(next), ..
            })) = self.index.field(&model, segment, self.setup)
            else {
                return false;
            };
            if transient && self.index.kind(&next, self.scope) != Some(ModelKind::Transient) {
                return false;
            }
            model = next;
        }
        false
    }

    /// From saas~16.4 Odoo gives a relational field with `check_company` a domain on the comodel's
    /// `company_id`, through the comodel's `_check_company_domain`, which fails on a comodel without
    /// one. From 18.0 it builds that domain only for a model that has a company field, is
    /// `res.company`, or for a company-dependent field.
    fn check_company(&self, out: &mut Out<'m>, path: &'m Path, field: &Field<'m>, own: &FieldFact) {
        if !RELATIONAL_TYPES.contains(&own.ftype.as_str())
            || !field.is("check_company", &OptionValue::Bool(true))
            || field.get("related") != Merged::Unset
            || !self.reported_at(field, own, |f| f.option("check_company").is_some())
        {
            return;
        }
        let Merged::Known(OptionValue::Str(comodel)) = field.get("comodel_name") else {
            return;
        };
        if !self.lacks_company(comodel)
            || (self.series.is_some_and(|s| s >= COMPANY_FIELD_DOMAIN) && !self.company_domain(field))
        {
            return;
        }
        let message = format!(
            "'{}' has check_company=True, but {comodel} has no company_id field to check; remove check_company",
            own.name
        );
        out.push(path, "OD222", pos_of(own, "check_company"), message);
    }

    /// Whether the comodel is introduced in scope with no `company_id` anywhere, and nothing but
    /// `BaseModel` gives its `_check_company_domain`.
    fn lacks_company(&self, comodel: &str) -> bool {
        self.index.introduced(comodel, self.scope)
            && !self.index.hidden_parents(comodel, self.scope)
            && matches!(self.index.field(comodel, "company_id", self.scope), Err(None))
            && !self.index.defines_method(comodel, "_check_company_domain", true)
    }

    /// Whether Odoo, from 18.0, builds a company domain for the field rather than logging that it
    /// cannot.
    fn company_domain(&self, field: &Field<'m>) -> bool {
        field.model == "res.company"
            || field.is("company_dependent", &OptionValue::Bool(true))
            || ["company_id", "company_ids"]
                .iter()
                .any(|name| self.index.field(field.model, name, self.scope).is_ok())
    }
}

/// Whether the field is computed by `method` with dependencies it takes from the method, and is
/// not declared recursive.
fn non_recursive_compute_of(field: &Field, method: &str) -> bool {
    matches!(field.get("compute"), Merged::Known(OptionValue::Str(m)) if m == method)
        && field.get("related") == Merged::Unset
        && field.get("depends") == Merged::Unset
        && matches!(
            field.get("recursive"),
            Merged::Unset | Merged::Known(OptionValue::Bool(false))
        )
}

/// Whether the field is required, when a literal tells.
fn required(field: &Field) -> Option<bool> {
    match field.get("required") {
        Merged::Unset => Some(false),
        Merged::Known(OptionValue::Bool(b)) => Some(*b),
        _ => None,
    }
}

/// Whether the field gives no ondelete: unset, or a false literal other than `""`, which unlike the
/// others has the `.lower()` Odoo calls on the value.
fn no_ondelete(field: &Field) -> bool {
    matches!(
        field.get("ondelete"),
        Merged::Unset | Merged::Known(OptionValue::Falsy | OptionValue::Bool(false))
    )
}

/// Whether the field's ondelete keeps the records it points to from going away under it.
fn kept_on_delete(field: &Field) -> Option<bool> {
    if no_ondelete(field) {
        return Some(false);
    }
    match field.get("ondelete") {
        Merged::Known(OptionValue::Str(policy)) => {
            Some(matches!(policy.to_lowercase().as_str(), "cascade" | "restrict"))
        }
        _ => None,
    }
}

/// Whether the field may delegate by `delegate`, which Odoo reads for its truth: a literal tells,
/// and any other value may.
fn may_delegate(field: &Field) -> bool {
    truth(field.get("delegate")) != Some(false)
}

/// The truth Python gives an attribute, for one whose default is false; None when only a run
/// tells.
fn truth(merged: Merged) -> Option<bool> {
    match merged {
        Merged::Unset | Merged::Known(OptionValue::Falsy) => Some(false),
        Merged::Known(OptionValue::Bool(b)) => Some(*b),
        Merged::Known(OptionValue::Str(s)) => Some(!s.is_empty()),
        Merged::Known(OptionValue::Strs(v) | OptionValue::Tuple(v)) => Some(!v.is_empty()),
        Merged::Known(OptionValue::Dict(d)) => Some(!d.is_empty()),
        Merged::Known(OptionValue::Other) | Merged::Unknown => None,
    }
}

/// What a delegating field lacks of `required=True` and ondelete 'cascade' or 'restrict', or None
/// when it lacks neither. An attribute that is not a literal is not named, nor is an ondelete that
/// is false when `defaulted`: Odoo then picks one from `required`.
fn delegation_fix(field: &Field, defaulted: bool) -> Option<String> {
    let mut fix = vec![];
    if required(field) == Some(false) {
        fix.push("required=True");
    }
    if kept_on_delete(field) == Some(false) && !(defaulted && truth(field.get("ondelete")) == Some(false)) {
        fix.push("ondelete=\"cascade\"");
    }
    (!fix.is_empty()).then(|| fix.join(", "))
}

/// The values the field has before its definition `at` applies its `selection_add`, or None when a
/// definition computes them, or one that sets them may come before or after it or the last
/// `selection` list. Odoo replaces the values with each `selection` list it meets, the definition's
/// own included, and ignores the `selection_add` of a lone definition from 15.0. `tuples` tells
/// whether the series takes a tuple as a `selection` list.
fn earlier_values<'m>(field: &Field<'m>, at: usize, tuples: bool) -> Option<FxHashSet<&'m str>> {
    if let Some(own) = field.defs[at].2.option("selection") {
        let own = own.value.listed(tuples)?;
        return (field.defs.len() > 1).then(|| own.iter().map(String::as_str).collect());
    }
    let setters = earlier_setters(field, at)?;
    let base = last_list(field, &setters)?;
    let mut values = FxHashSet::default();
    for i in setters.into_iter().filter(|&i| i == base || field.before[base][i]) {
        selection_values(field.defs[i].2, tuples, &mut values)?;
    }
    Some(values)
}

/// The definitions that set the field's values before its definition `at`, or None when one may
/// also come after it, or one unpacks `**kwargs`.
fn earlier_setters(field: &Field, at: usize) -> Option<Vec<usize>> {
    let mut setters = vec![];
    for i in (0..field.defs.len()).filter(|&i| i != at && !field.before[at][i]) {
        let def = field.defs[i].2;
        let sets = def.option("selection").is_some() || def.option("selection_add").is_some();
        if def.splat || (sets && !field.before[i][at]) {
            return None;
        }
        if sets {
            setters.push(i);
        }
    }
    Some(setters)
}

/// The setter whose `selection` list Odoo applies last, so that only it and the `selection_add` of
/// the setters after it count. None when no setter has a list, or no setter with a list is known
/// to come after every other setter.
fn last_list(field: &Field, setters: &[usize]) -> Option<usize> {
    let lists = |i: usize| field.defs[i].2.option("selection").is_some();
    let applied_last = |b: usize| {
        let placed = |s: usize| s == b || field.before[s][b] || (field.before[b][s] && !lists(s));
        setters.iter().all(|&s| placed(s))
    };
    setters.iter().copied().find(|&b| lists(b) && applied_last(b))
}

/// Adds the values of a definition's `selection` and `selection_add`. None when one is not a
/// literal list, or `selection` is a tuple and not `tuples`.
fn selection_values<'m>(def: &'m FieldFact, tuples: bool, values: &mut FxHashSet<&'m str>) -> Option<()> {
    for (name, tuples) in [("selection", tuples), ("selection_add", true)] {
        if let Some(option) = def.option(name) {
            values.extend(option.value.listed(tuples)?.iter().map(String::as_str));
        }
    }
    Some(())
}

/// Whether a write outside a CSV checks its value against a tuple `selection`. The import never
/// reads `_selection`: it asks the field for its values
/// (`odoo/addons/base/models/ir_fields.py:473` on 18.0), which a tuple gives in every series.
fn tuple_checked(series: Option<Series>) -> bool {
    series.is_some_and(|s| s < TUPLE_UNCHECKED || s >= TUPLE_CHECKED)
}

fn tuple_listed(series: Option<Series>) -> bool {
    series.is_some_and(|s| s >= TUPLE_LISTED)
}

/// Whether Odoo's import takes `cell` for a value of `defs`: it compares each value and each label,
/// from `CSV_ANY_CASE` in any case. A label only a run tells may be the cell.
fn csv_label(defs: &[FieldDef], values: &FxHashSet<&str>, cell: &str, series: Option<Series>) -> bool {
    let any_case = series.is_none_or(|s| s >= CSV_ANY_CASE);
    let same = |known: &str| known == cell || (any_case && known.to_lowercase() == cell.to_lowercase());
    let mut labels = defs.iter().flat_map(|d| &d.2.labels);
    values.iter().any(|v| same(v)) || labels.any(|l| l.as_deref().is_none_or(same))
}

/// Whether `module` ships no file but its manifest and `__init__.py`.
fn is_shell(module: &Module) -> bool {
    module.data.is_empty()
        && module.js.iter().all(SourceFile::in_tests)
        && module.templates.is_empty()
        && module
            .python
            .iter()
            .all(|f| f.path == module.manifest_path || f.rel == "__init__.py")
}

fn sets_parent_store(class: &ClassFact) -> bool {
    class
        .attr("_parent_store")
        .is_some_and(|a| a.value != OptionValue::Bool(false))
}

fn true_or_unset(class: &ClassFact, attr: &str) -> bool {
    class.attr(attr).is_none_or(|a| a.value == OptionValue::Bool(true))
}

/// What the table of a model holds for a name, as far as its stored fields tell.
enum Column<'m> {
    /// A column, or a field whose column the rule cannot tell.
    Present,
    Missing,
    NotStored,
    /// A field every model has, which the ORM computes, such as `display_name`.
    Computed,
    /// A log column of a model that does not log access.
    Unlogged,
    X2many(&'m str),
}

/// Why the table of a model has no column of a name.
enum Lack<'m> {
    NotStored,
    Computed,
    Unlogged,
    X2many(&'m str),
    /// A field of the parent model it names first, which the model reaches through `_inherits` by
    /// the field it names second.
    Delegated(&'m str, &'m str),
    /// No field of the name; the `<name>_id` field the model stores, when it has one.
    Undefined(Option<String>),
}

/// Methods and attributes that create the table or the view of a model with `_auto = False`.
const VIEW_MAKERS: &[&str] = &["init", "_auto_init", "_table_query", "_table_sql"];

/// OD224 on a qualified column a literal query names on the table of a model, which no stored
/// field of the model provides, or on a view whose columns its `init()` spells, which selects no
/// column of that name. The rule does not run for a module whose scope is partial, nor in upgrade
/// scripts, and reads only tables `column_model` or `view_model` accepts.
impl<'m> Ctx<'_, 'm> {
    fn sql_columns(&self, out: &mut Out<'m>, path: &'m Path, columns: &[SqlColumn]) {
        if self.partial {
            return;
        }
        let setup = Ctx {
            scope: self.setup,
            ..*self
        };
        for column in columns {
            let ctx = if column.before_data { &setup } else { self };
            if let Some(message) = ctx.unknown_column(column) {
                out.push(path, "OD224", column.pos, message);
            }
        }
    }

    /// Whether `model` is the one model whose table may be `table`, introduced in scope, which
    /// takes that table there and has no parent the scan cannot see.
    fn owns_table(&self, model: &str, table: &str) -> bool {
        self.index.table_models(table) == [model]
            && self.index.table(model, self.scope).as_deref() == Some(table)
            && self.index.introduced(model, self.scope)
            && !self.index.hidden_parents(model, self.scope)
    }

    /// The model whose stored fields make up `table`: the one `owns_table` accepts, when Odoo builds
    /// the table from its fields rather than an SQL view or a table query.
    fn column_model(&self, table: &str) -> Option<&'m str> {
        let &[model] = self.index.table_models(table) else {
            return None;
        };
        let readable = self.owns_table(model, table) && self.has_table(model) && !self.table_query(model);
        readable.then_some(model)
    }

    /// The model whose view is `table`, the one `owns_table` accepts, with the view `view_of` reads.
    fn view_model(&self, table: &str) -> Option<(&'m str, &'m SqlView)> {
        let &[model] = self.index.table_models(table) else {
            return None;
        };
        let view = self.owns_table(model, table).then(|| self.view_of(model))??;
        Some((model, view))
    }

    /// Whether a class of `model` in scope gives it a `_table_query` or, from 20.0, a `_table_sql`.
    fn table_query(&self, model: &str) -> bool {
        self.some_class(model, |c| {
            c.methods.iter().any(|m| m == "_table_query" || m == "_table_sql")
        })
    }

    /// The message on a column no field gives, unless a module adds it by hand.
    fn unknown_column(&self, column: &SqlColumn) -> Option<String> {
        let head = format!("{}: {} has no such column", column.text, column.table);
        if let Some((model, view)) = self.view_model(&column.table) {
            let names = view.names.as_deref().unwrap_or_default();
            return (!names.contains(&column.column)).then(|| {
                format!(
                    "{head}, since the view init() creates for {model} selects none; name a column the view selects"
                )
            });
        }
        let model = self.column_model(&column.table)?;
        let name = column.column.as_str();
        let reason = match self.lack(model, &column.table, name)? {
            Lack::NotStored => {
                format!("{model}.{name} is not stored; set store=True on the field, or read it through the ORM")
            }
            Lack::Computed => format!("{model}.{name} is computed by the ORM, not stored; read it through the ORM"),
            Lack::Unlogged => format!(
                "{model}.{name} is not created, as {model} sets _log_access to False; drop it from the query, or log access on the model"
            ),
            Lack::X2many("One2many") => {
                format!("{model}.{name} is a One2many; join the table of its comodel on its inverse field")
            }
            Lack::X2many(ftype) => format!("{model}.{name} is a {ftype}; join its relation table"),
            Lack::Delegated(parent, via) => format!(
                "{name} is a field of {parent}, which {model} reaches through _inherits; join {} on {via}",
                self.parent_table(parent)
            ),
            Lack::Undefined(suggested) => format!("{model} has no field {name}; {}", fix_name(model, suggested)),
        };
        Some(format!("{head}, since {reason}"))
    }

    fn parent_table(&self, parent: &str) -> String {
        self.index
            .table(parent, self.scope)
            .unwrap_or_else(|| parent.replace('.', "_"))
    }

    /// Why the `table` of `model` has no column `name`, None when it has one or may have one: a
    /// module adds it by hand, one outside the scope defines the field, or an `ir.model.fields`
    /// record adds it.
    fn lack(&self, model: &'m str, table: &str, name: &str) -> Option<Lack<'m>> {
        if self.index.hand_made_column(model, table, name, self.scope) {
            return None;
        }
        match self.column(model, name) {
            Column::Present => None,
            Column::NotStored => Some(Lack::NotStored),
            Column::Computed => Some(Lack::Computed),
            Column::Unlogged => Some(Lack::Unlogged),
            Column::X2many(ftype) => Some(Lack::X2many(ftype)),
            Column::Missing => self.undefined_column(model, name),
        }
    }

    /// What a table lacks for a name no field of `model` in scope has, unless a module outside the
    /// scope defines it or an `ir.model.fields` record adds it.
    fn undefined_column(&self, model: &'m str, name: &str) -> Option<Lack<'m>> {
        if self.index.defines_field(model, name, self.scope) {
            return None;
        }
        if let Some((parent, via)) = self.delegated_field(model, name) {
            return Some(Lack::Delegated(parent, via));
        }
        let suggested = format!("{name}_id");
        let stored = matches!(self.column(model, &suggested), Column::Present)
            && self.index.defines_field(model, &suggested, self.scope);
        Some(Lack::Undefined(stored.then_some(suggested)))
    }

    /// What the table of `model` holds for `name`. Of the fields Odoo adds without a definition,
    /// `id`, the log columns where `logs_access` says so and `parent_path` under `_parent_store`
    /// are present, and `display_name`, and `__last_update` up to 16.0, are not stored. Present too
    /// are fields whose column depends on the series or on `attachment`, and an undefined name the
    /// merged `_inherits` may name, which Odoo creates or refuses the model over.
    fn column(&self, model: &'m str, name: &str) -> Column<'m> {
        if self.index.implicit_column(model, name)
            || self.index.log_column(name) && self.logs_access(model)
            || name == "parent_path" && self.some_class(model, sets_parent_store)
        {
            return Column::Present;
        }
        let mut defs = self.index.merged_fields(model, self.scope);
        defs.retain(|d| d.2.name == name);
        if defs.is_empty() {
            return match self.undefined(name) {
                Column::Missing if self.inherits_entry(model, name) != Some(false) => Column::Present,
                lack => lack,
            };
        }
        self.merge(model, defs)
            .map_or(Column::Present, |field| stored_column(&field))
    }

    /// What a name no field of the model defines is: a log column the model leaves out, a field the
    /// ORM computes, or nothing, which `__last_update` is after 16.0 or in an unknown series.
    fn undefined(&self, name: &str) -> Column<'m> {
        if self.index.log_column(name) {
            Column::Unlogged
        } else if self.index.magic_field(name) && (name != "__last_update" || has_last_update(self.series)) {
            Column::Computed
        } else {
            Column::Missing
        }
    }

    /// Whether Odoo gives the table of `model` its log columns. From saas~14.4 the metaclass adds
    /// them only to the class that introduces the model, so they are there unless every such class
    /// in scope sets `_log_access` to False. Before, classes merge into one whose last setting wins,
    /// and Odoo never drops a column, so they are missing only when the module introducing the
    /// model sets False and no class sets anything else. An unknown series misses them only when
    /// both rules agree, so no era reports a column its series has.
    fn logs_access(&self, model: &str) -> bool {
        if self.series.is_none() {
            return self.introduced_logs_access(model) || self.merged_logs_access(model);
        }
        match magic_era(self.series) {
            MagicEra::RegistryMerge | MagicEra::Setup => self.merged_logs_access(model),
            MagicEra::Metaclass | MagicEra::Base => self.introduced_logs_access(model),
        }
    }

    fn introduced_logs_access(&self, model: &str) -> bool {
        !self.some_class(model, |c| c.new_model) || self.some_class(model, |c| c.new_model && !unlogged(c))
    }

    fn merged_logs_access(&self, model: &str) -> bool {
        let classes = self.index.classes(model);
        let introducing = |mi: usize| classes.iter().any(|(m, c)| *m == mi && c.new_model);
        let unlogged_at_introduction = classes
            .iter()
            .any(|(mi, c)| self.scope.sees(*mi) && introducing(*mi) && unlogged(c));
        !unlogged_at_introduction || self.some_class(model, |c| c.attr("_log_access").is_some() && !unlogged(c))
    }

    /// The model `model` delegates to that has the field `name`, with the field that delegates to
    /// it, among the `_inherits` entries and `delegate=True` fields of the classes it merges: the
    /// first whose field the merged `_inherits` may still name, else the first.
    fn delegated_field(&self, model: &str, name: &str) -> Option<(&'m str, &'m str)> {
        let classes = self.index.merged_classes(model, self.scope);
        let pairs: Vec<(&'m str, &'m str)> = classes
            .iter()
            .flat_map(|(_, c)| &c.inherits)
            .filter(|(parent, _)| !self.index.field_defs(parent, name, None).is_empty())
            .map(|(parent, via)| (parent.as_str(), via.as_str()))
            .collect();
        let holds = |&&(_, via): &&(&str, &str)| self.inherits_entry(model, via) != Some(false);
        pairs.iter().find(holds).or(pairs.first()).copied()
    }
}

/// The fix for a name no field of `model` has: its `_id` field, or else any stored field.
fn fix_name(model: &str, suggested: Option<String>) -> String {
    suggested.map_or_else(
        || format!("name the column of a stored field of {model}"),
        |field| format!("use {field}"),
    )
}

/// OD225 and OD226 on models built on an SQL view, and OD227 on the columns of constraint and index
/// definitions. They run where `registry` does.
impl<'m> Ctx<'_, 'm> {
    /// OD226 at the `_auto = False` of the class that introduces a model with a literal table that
    /// `view_made` finds nothing to create, unless the class is kept out of the registry.
    fn view_not_created(&self, out: &mut Out<'m>, path: &'m Path, class: &ClassFact) {
        let auto = class.attr("_auto").filter(|a| a.value == OptionValue::Bool(false));
        let Some(auto) = auto.filter(|_| class.new_model && class.registered()) else {
            return;
        };
        let model = class.model.as_str();
        let Some(table) = self.index.table(model, self.scope) else {
            return;
        };
        if self.view_made(model, &table) {
            return;
        }
        out.push(
            path,
            "OD226",
            auto.pos,
            format!(
                "_auto = False leaves {model} without a table, and nothing creates the view {table}; create it in init(), or drop _auto = False"
            ),
        );
    }

    /// Whether `model` needs no relation or something may give it `table`: no class of it or of a
    /// model it inherits has a field, `base` apart, so it stores nothing; a class of those, in any
    /// indexed module but for one that does not parse outside the scope, defines `init()` or
    /// `_auto_init()`, sets a table query, creates a view whose whole name is `self._table` or has
    /// a parent the scan cannot see; another model, or a Many2many in scope, has that table; an
    /// indexed module creates a view of that name in Python or SQL; or this module, not a
    /// dependency, creates a view it names at runtime. `table_models` leaves out an abstract model.
    fn view_made(&self, model: &str, table: &str) -> bool {
        let creates = |c: &ClassFact| {
            c.hidden_parents() || c.own_view || c.methods.iter().any(|m| VIEW_MAKERS.contains(&m.as_str()))
        };
        let chain = self.index.chain_read_from(model, self.scope);
        let own = || chain.iter().filter(|(_, c)| c.model != "base");
        own().all(|(_, c)| c.fields.is_empty())
            || chain.iter().any(|(_, c)| creates(c))
            || self.index.table_models(table) != [model]
            || self.index.relation_table(table, self.scope)
            || self.index.creates_view(table)
            || self.module.python.iter().any(|f| f.facts.creates_any_view)
    }

    /// The view the `init()` of `model` creates, when its text spells the names of the columns. The
    /// model has no table nor table query, and the one class of it or of a model it inherits, in any
    /// indexed module, that defines `init()` is one of `model` in scope, which creates one view, on
    /// the table of the model or on a placeholder; none defines `_auto_init()`.
    fn view_of(&self, model: &'m str) -> Option<&'m SqlView> {
        if !self.viewed(model) {
            return None;
        }
        let chain = self.index.chain_classes(model);
        let defining = |method: &str| -> Vec<(usize, &'m ClassFact)> {
            chain
                .iter()
                .filter(|(_, c)| c.methods.iter().any(|m| m == method))
                .copied()
                .collect()
        };
        let [(module, class)] = defining("init")[..] else {
            return None;
        };
        let [view] = class.views.as_slice() else {
            return None;
        };
        let table = self.index.table(model, self.scope)?;
        let own = class.model == model && self.scope.sees(module) && defining("_auto_init").is_empty();
        let on_table = view.table.as_ref().is_none_or(|t| *t == table);
        (own && on_table && view.names.is_some()).then_some(view)
    }

    /// Whether `model` reads from a view: no class of it in scope is abstract, one sets
    /// `_auto = False`, and none gives it a table query.
    fn viewed(&self, model: &str) -> bool {
        let auto_false = |c: &ClassFact| c.attr("_auto").is_some_and(|a| a.value == OptionValue::Bool(false));
        self.every_class(model, |c| c.kind != ModelKind::Abstract)
            && self.some_class(model, auto_false)
            && !self.table_query(model)
    }

    /// OD225 on a field with a column, at its first definition, when the view of its model selects
    /// no column of its name.
    fn view_field(&self, out: &mut Out<'m>, path: &'m Path, fields: &ModelFields<'m>, own: &'m FieldFact) {
        let (Some(view), Some(field)) = (fields.view, fields.fields.get(own.name.as_str())) else {
            return;
        };
        let names = view.names.as_deref().unwrap_or_default();
        let first = self.first_def(field).is_some_and(|d| std::ptr::eq(d.2, own));
        if names.contains(&own.name) || !first || !has_certain_column(field) {
            return;
        }
        let (model, name) = (fields.model, own.name.as_str());
        out.push(
            path,
            "OD225",
            own.name_pos,
            format!(
                "{model}.{name} is stored, but the view {} selects no column {name} for it; select one AS {name} in init()",
                self.view_table(model, view)
            ),
        );
    }

    /// OD225 at the view `class` creates for its model, when the view selects no `id`, or no column
    /// for a field with a column that another module defines first, which `view_field` reports
    /// nowhere in this one.
    fn view_id(&self, out: &mut Out<'m>, path: &'m Path, class: &ClassFact, fields: &ModelFields<'m>) {
        let Some(view) = fields
            .view
            .filter(|v| class.views.iter().any(|own| std::ptr::eq(own, *v)))
        else {
            return;
        };
        let (model, table) = (fields.model, self.view_table(fields.model, view));
        let names = view.names.as_deref().unwrap_or_default();
        if !names.iter().any(|n| n == "id") {
            let message = format!(
                "the view {table} of {model} selects no id column, which every read of its records needs; select one AS id"
            );
            out.push(path, "OD225", view.pos, message);
        }
        for (name, module) in self.foreign_fields(fields, names) {
            let message = format!(
                "the view {table} of {model} selects no column {name}, which the stored field {model}.{name} of {module} needs; select one AS {name}"
            );
            out.push(path, "OD225", view.pos, message);
        }
    }

    /// The fields with a column of a model with a view that the view selects no column for and
    /// another module defines first, by name, with that module.
    fn foreign_fields(&self, fields: &ModelFields<'m>, names: &[String]) -> Vec<(&'m str, &'m str)> {
        let me = self.index.by_name[self.module.name.as_str()];
        let mut found: Vec<(&str, &str)> = fields
            .fields
            .iter()
            .filter(|(name, field)| !names.iter().any(|n| n == *name) && has_certain_column(field))
            .filter_map(|(&name, field)| {
                let (module, _, _) = self.first_def(field)?;
                (*module != me).then(|| (name, self.index.modules[*module].name.as_str()))
            })
            .collect();
        found.sort_unstable();
        found
    }

    fn view_table(&self, model: &str, view: &SqlView) -> String {
        view.table
            .clone()
            .or_else(|| self.index.table(model, self.scope))
            .unwrap_or_else(|| model.replace('.', "_"))
    }

    /// OD227 on each column a constraint or an index definition of `class` names that the table of
    /// its model lacks. Odoo reads `_sql_constraints` before 19.0 and the table objects from 19.0,
    /// and applies neither to a model without a table of its fields.
    fn definitions(&self, out: &mut Out<'m>, path: &'m Path, class: &'m ClassFact) {
        let model = class.model.as_str();
        if class.definitions.is_empty() || !self.has_table(model) || self.table_query(model) {
            return;
        }
        let Some(table) = self.index.table(model, self.scope) else {
            return;
        };
        for (_, column) in class
            .definitions
            .iter()
            .filter(|(source, _)| applies(*source, self.series))
        {
            if let Some(reason) = self.definition_reason(model, &table, &column.value) {
                let message = format!("{}: {table} has no such column, since {reason}", column.value);
                out.push(path, "OD227", column.pos, message);
            }
        }
    }

    /// Why a definition cannot name `name` on the table of `model`, with the fix.
    fn definition_reason(&self, model: &'m str, table: &str, name: &str) -> Option<String> {
        Some(match self.lack(model, table, name)? {
            Lack::NotStored => {
                format!("{model}.{name} is not stored; set store=True on the field, or drop it from the definition")
            }
            Lack::Computed => format!("{model}.{name} is computed by the ORM, not stored; drop it from the definition"),
            Lack::Unlogged => format!(
                "{model}.{name} is not created, as {model} sets _log_access to False; drop it from the definition, or log access on the model"
            ),
            Lack::X2many(ftype) => {
                format!("{model}.{name} is a {ftype}, which has no column; drop it from the definition")
            }
            Lack::Delegated(parent, _) => format!(
                "{name} is a field of {parent}, which {model} reaches through _inherits; declare the definition on {parent}"
            ),
            Lack::Undefined(suggested) => format!("{model} has no field {name}; {}", fix_name(model, suggested)),
        })
    }
}

/// Whether Odoo applies the definitions of `source` in `series`, or may when it is unknown.
fn applies(source: DefinitionSource, series: Option<Series>) -> bool {
    let modern = series.map(|s| s >= Series { major: 19, minor: 0 });
    match source {
        DefinitionSource::SqlConstraints => modern != Some(true),
        DefinitionSource::TableObject => modern != Some(false),
    }
}

fn company_dependent(field: &Field) -> bool {
    !matches!(
        field.get("company_dependent"),
        Merged::Unset | Merged::Known(OptionValue::Bool(false))
    )
}

/// Whether a merged field has a column: x2many fields and fields not stored have none, and the
/// column of a Binary or a company-dependent field depends on `attachment` or on the series.
fn stored_column<'m>(field: &Field<'m>) -> Column<'m> {
    let ftype = field.defs.first().map_or("", |d| d.2.ftype.as_str());
    match ftype {
        "One2many" | "Many2many" => Column::X2many(ftype),
        "Binary" | "Image" => Column::Present,
        _ if company_dependent(field) => Column::Present,
        _ if field.stored() == Some(false) => Column::NotStored,
        _ => Column::Present,
    }
}

/// Whether a merged field certainly has a column: it is stored, its column does not depend on
/// `attachment` or the series, as `stored_column` says, and its type is read, as an unread one may
/// be an x2many.
fn has_certain_column(field: &Field) -> bool {
    let ftype = field.defs.first().map_or("", |d| d.2.ftype.as_str());
    let typed = !matches!(ftype, "One2many" | "Many2many" | "Binary" | "Image" | UNREAD_TYPE);
    typed && !company_dependent(field) && field.stored() == Some(true)
}

/// What a public or portal user reaches of a field through an access row: OD403 stores into an
/// unsanitized Html field, OD404 reads a secret.
#[derive(Clone, Copy)]
enum Exposure {
    Html,
    Secret,
}

impl Exposure {
    const ALL: [Self; 2] = [Self::Html, Self::Secret];

    fn code(self) -> &'static str {
        match self {
            Self::Html => "OD403",
            Self::Secret => "OD404",
        }
    }

    fn kind(self) -> grants::Kind {
        match self {
            Self::Html => grants::stores,
            Self::Secret => grants::reads,
        }
    }

    /// What an access row lets its users do to the model, as the message says it.
    fn verb(self) -> &'static str {
        match self {
            Self::Html => "write",
            Self::Secret => "read",
        }
    }
}

/// The options that turn the sanitizer of an Html field off, all on by default.
const SANITIZE_OPTIONS: [&str; 3] = ["sanitize", "sanitize_tags", "sanitize_attributes"];

/// The last words of a field name that say it holds a secret. `signature`, which OD131 counts, is
/// mostly the text of an email signature here.
const SECRET_WORDS: [&str; 3] = ["password", "token", "secret"];

fn sets_sanitize(own: &FieldFact) -> bool {
    SANITIZE_OPTIONS.iter().any(|o| own.option(o).is_some())
}

/// Whether a field name says it holds a secret. `access_token` is the one portal links carry, which
/// the portal user who follows them may read.
fn secret_name(name: &str) -> bool {
    let word = python::last_word(name).unwrap_or_default();
    name != "access_token"
        && (SECRET_WORDS.iter().any(|w| word.eq_ignore_ascii_case(w)) || name.to_ascii_lowercase().ends_with("api_key"))
}

/// The option, and how it is spelled, that leaves an Html field unsanitized. `'email_outgoing'`
/// turns the tags and the attributes off unless the field sets them (18.0 `odoo/fields.py:2187`,
/// 20.0 `odoo/orm/fields_textual.py:628`).
fn unsanitized(field: &Field) -> Option<(&'static str, &'static str)> {
    let off = |name: &str| field.get(name) != Merged::Unset && truth(field.get(name)) == Some(false);
    let on = |name: &str| truth(field.get(name)) == Some(true);
    let outgoing = matches!(field.get("sanitize"), Merged::Known(OptionValue::Str(s)) if s == "email_outgoing");
    if off("sanitize") {
        Some(("sanitize", "sanitize=False"))
    } else if outgoing && !(on("sanitize_tags") && on("sanitize_attributes")) {
        Some(("sanitize", "sanitize='email_outgoing'"))
    } else if off("sanitize_tags") {
        Some(("sanitize_tags", "sanitize_tags=False"))
    } else {
        off("sanitize_attributes").then_some(("sanitize_attributes", "sanitize_attributes=False"))
    }
}

/// OD403 and OD404: a field public or portal users reach through an access row. The module that
/// decides the field reports it at the field, given a row in scope; else the module whose row first
/// lets them at the model reports it at the row.
impl<'m> Ctx<'_, 'm> {
    fn exposure(&self, out: &mut Out<'m>, path: &'m Path, field: &Field<'m>, own: &'m FieldFact) {
        for exposure in Exposure::ALL {
            let Some((option, why)) = self.exposed(field, exposure) else {
                continue;
            };
            let decides = match exposure {
                Exposure::Html => self.reported_at(field, own, sets_sanitize),
                Exposure::Secret => self.introduces(field, own),
            };
            let grant = self.grants.public(field.model, exposure.kind()).next();
            if let Some(grant) = grant.filter(|_| decides) {
                let reach = format!("{} can {} {}", grant.whom(), exposure.verb(), field.model);
                let message = exposure_message(exposure, field, &own.name, why, &reach);
                out.push(
                    path,
                    exposure.code(),
                    option.map_or(own.pos, |o| pos_of(own, o)),
                    message,
                );
            }
        }
    }

    /// What exposes `field` to a user who can reach its model, as the option at fault and its
    /// spelling. None for a field `groups` limits, a related one, or one of a transient model.
    fn exposed(&self, field: &Field<'m>, exposure: Exposure) -> Option<(Option<&'static str>, &'static str)> {
        let transient = self.index.kind(field.model, self.scope) == Some(ModelKind::Transient);
        if transient || field.get("groups") != Merged::Unset || field.get("related") != Merged::Unset {
            return None;
        }
        let ftype = field.defs.first().map_or("", |d| d.2.ftype.as_str());
        match exposure {
            Exposure::Html if ftype == "Html" && self.writable(field) => {
                unsanitized(field).map(|(option, why)| (Some(option), why))
            }
            Exposure::Secret if self.secret(field, ftype) => Some((None, "")),
            _ => None,
        }
    }

    /// Whether a write reaches the field's value: a compute without an inverse drops it.
    fn writable(&self, field: &Field<'m>) -> bool {
        truth(field.get("compute")) == Some(false) || field.get("inverse") != Merged::Unset
    }

    /// Whether the field stores a secret where its model's readers see it. `res.users` hides its
    /// own from other users (18.0 `odoo/addons/base/models/res_users.py:580-585`).
    fn secret(&self, field: &Field<'m>, ftype: &str) -> bool {
        let name = field.defs.first().map_or("", |d| d.2.name.as_str());
        matches!(ftype, "Char" | "Text")
            && field.model != "res.users"
            && secret_name(name)
            && field.stored() == Some(true)
    }

    /// OD403 and OD404 at the public rows of `file` that first let public or portal users at a model
    /// whose field another module decides.
    fn exposing_rows(&self, out: &mut Out<'m>, file: &'m SourceFile<DataFacts>) {
        if self.partial {
            return;
        }
        for exposure in Exposure::ALL {
            let mut granted: Vec<(&'m str, &Grant<'m>)> = self.grants.introduced_in(self.mi, exposure.kind()).collect();
            granted.retain(|(_, g)| g.path == file.path);
            granted.sort_by_key(|(model, _)| *model);
            for (model, grant) in granted {
                self.exposing_row(out, exposure, model, grant);
            }
        }
    }

    fn exposing_row(&self, out: &mut Out<'m>, exposure: Exposure, model: &'m str, grant: &Grant<'m>) {
        if !self.index.introduced(model, self.scope) || self.index.hidden_parents(model, self.scope) {
            return;
        }
        let fields = self.model_fields(model);
        let mut names: Vec<&&'m str> = fields.fields.keys().collect();
        names.sort();
        for name in names {
            let field = &fields.fields[*name];
            let Some((_, why)) = self
                .exposed(field, exposure)
                .filter(|_| self.decided_elsewhere(field, exposure))
            else {
                continue;
            };
            let reach = format!("this row lets {} {} {model}", grant.whom(), exposure.verb());
            let message = exposure_message(exposure, field, name, why, &reach);
            out.push(grant.path, exposure.code(), grant.row.pos, message);
        }
    }

    /// Whether another module decides what exposes `field`, a field of the model's own classes: a
    /// field only a mixin defines is left out on either side.
    fn decided_elsewhere(&self, field: &Field<'m>, exposure: Exposure) -> bool {
        let me = self.index.by_name[self.module.name.as_str()];
        let decided_here = match exposure {
            Exposure::Html => field.defs.iter().any(|d| d.0 == me && sets_sanitize(d.2)),
            Exposure::Secret => field.defs.iter().all(|d| d.0 == me),
        };
        !decided_here && field.defs.iter().any(|d| d.1.model == field.model)
    }
}

/// The message of OD403 or OD404, where `reach` says who reaches the model and how.
fn exposure_message(exposure: Exposure, field: &Field, name: &str, why: &str, reach: &str) -> String {
    let model = field.model;
    match exposure {
        Exposure::Html => format!(
            "Html field '{name}' of {model} is not sanitized ({why}) and {reach}, so they can store {} for whoever opens the record; keep the sanitizer on, or give the field groups",
            unsanitized_markup(why)
        ),
        Exposure::Secret => format!(
            "'{name}' of {model} holds a secret and has no groups, and {reach}, so they can read it over RPC; give the field groups, such as base.group_system"
        ),
    }
}

/// What an Html field stores that its sanitizer would remove, by the option at fault. Only
/// `sanitize=False` skips the cleaner, whose defaults strip script and event handlers whatever the
/// other options say (18.0 `odoo/tools/mail.py:328-365`).
fn unsanitized_markup(why: &str) -> &'static str {
    match why {
        "sanitize=False" => "script that runs",
        "sanitize_attributes=False" => "attributes the sanitizer would strip (not event handlers)",
        _ => "style, base and SVG markup that restyles the page or redirects its links",
    }
}

/// What the writes of a rule leave it holding, folded in load order.
struct RuleState<'m> {
    /// The last domain written, None when no write gives one.
    domain: Option<&'m ActionValue>,
    /// The groups, as full XML ids, the rule certainly holds.
    groups: Vec<String>,
    perms: [Flag; 4],
    active: Option<bool>,
}

impl<'m> RuleState<'m> {
    fn of(index: &Index<'m>, writes: &[Placed<&'m RuleFact>]) -> Self {
        let mut state = Self {
            domain: None,
            groups: vec![],
            perms: [Flag::Unset; 4],
            active: None,
        };
        for write in writes {
            let fact = write.fact;
            if !matches!(fact.domain, ActionValue::Unset) {
                state.domain = Some(&fact.domain);
            }
            // A write only a run tells may remove any group.
            if fact.groups.opaque || !fact.groups.keeps {
                state.groups.clear();
            }
            let module = &index.modules[write.module].name;
            state
                .groups
                .extend(fact.groups.added.iter().map(|g| full_id(module, g)));
            for (held, flag) in state.perms.iter_mut().zip(fact.perms) {
                if flag != Flag::Unset {
                    *held = flag;
                }
            }
            state.active = fact.active.or(state.active);
        }
        state
    }

    /// How the domain holds every record, if it does. `ir.rule` reads a domain it lacks or that is
    /// false as none (18.0 `odoo/addons/base/models/ir_rule.py:160,170`), which the parsers give as
    /// an empty literal. An empty one is how an `ir.access` row grants without a restriction, so only
    /// a written `TRUE_LEAF` counts there.
    fn every_record(&self, access: bool) -> Option<&'static str> {
        match self.domain {
            None => (!access).then_some("is empty"),
            Some(ActionValue::Literal(text)) => holds_every_record(&text.value, access),
            _ => None,
        }
    }
}

/// How a domain written as `text` holds every record, if it does.
fn holds_every_record(text: &str, access: bool) -> Option<&'static str> {
    match domain::always_true(text) {
        Some(leaves) if leaves > 0 => Some("is always true"),
        _ if access => None,
        Some(_) => Some("is empty"),
        None => text.is_empty().then_some("is empty"),
    }
}

/// Whether a write to a rule sets what decides whom it lets at which records.
fn sets_reach(fact: &RuleFact) -> bool {
    let groups = &fact.groups;
    !matches!(fact.domain, ActionValue::Unset)
        || !matches!(fact.model, ActionValue::Unset)
        || groups.opaque
        || !groups.keeps
        || !groups.added.is_empty()
        || fact.perms.iter().any(|p| *p != Flag::Unset)
        || fact.active.is_some()
}

/// The operations of `Perms` in their order, as messages name them, and the grant of each.
const OPERATIONS: [(&str, grants::Kind); 4] = [
    ("read", |p| p.read),
    ("write", |p| p.write),
    ("create", |p| p.create),
    ("unlink", |p| p.unlink),
];

/// OD405 and OD406, on what the records of `ir.rule` and `ir.access` hold.
impl<'m> Ctx<'_, 'm> {
    /// OD405 on the last write of this module to a rule for public or portal users whose domain
    /// holds every record, naming the first of its public groups that reaches an operation. A later
    /// write of the module may narrow it, and one of a module that depends on it is not this
    /// module's to report.
    fn public_rule(&self, out: &mut Out<'m>, path: &'m Path, rule: &'m RuleFact) {
        let read = if rule.access {
            self.series.is_none_or(|s| s >= IR_ACCESS)
        } else {
            reads_model_access(self.series)
        };
        if !read || !self.last_write_here(rule) {
            return;
        }
        let writes = self.rule_state(rule);
        let state = RuleState::of(self.index, &writes);
        let Some(model) = self.rule_model(&writes).filter(|_| state.active != Some(false)) else {
            return;
        };
        let Some(why) = state.every_record(rule.access) else {
            return;
        };
        let reached = state
            .groups
            .iter()
            .filter(|g| grants::public_group(g, self.series))
            .find_map(|g| {
                let ops = self.reached_operations(&state, model, g, rule.access);
                (!ops.is_empty()).then_some((g, ops))
            });
        let Some((group, ops)) = reached else {
            return;
        };
        let what = if rule.access { "ir.access record" } else { "rule" };
        let message = format!(
            "this {what} gives {group} {} on every record of {model}, as its domain {why}; narrow the domain to the records those users own",
            ops.join(", ")
        );
        let pos = match &rule.domain {
            ActionValue::Literal(text) => text.pos,
            _ => rule.pos,
        };
        out.push(path, "OD405", pos, message);
    }

    /// Whether `rule` is the last write of this module to its record that the scope loads, of a
    /// module that writes what OD405 reads: one that only renames the rule leaves it to the others.
    fn last_write_here(&self, rule: &'m RuleFact) -> bool {
        let Some(writes) = self.record_rule_writes(rule) else {
            return true;
        };
        let here: Vec<_> = writes
            .iter()
            .filter(|w| w.module == self.mi && self.scope.loads(w))
            .collect();
        here.last()
            .is_none_or(|w| std::ptr::eq(w.fact, rule) && here.iter().any(|w| sets_reach(w.fact)))
    }

    /// The operations a rule for `group` opens on every record. An `ir.access` record grants them
    /// itself; an `ir.rule` only lets through what an access row grants the group, all of them when
    /// it names none. Every public grant reaches a rule for everyone, whose group holds them all.
    fn reached_operations(&self, state: &RuleState<'m>, model: &str, group: &str, access: bool) -> Vec<&'static str> {
        let granted = |flag: Flag| flag == Flag::Set(true) || (!access && flag == Flag::Unset);
        let to_group = |g: &Grant| {
            group == grants::EVERYONE || g.group.as_deref().is_none_or(|g| g == group || g == grants::EVERYONE)
        };
        let reached = |kind| access || self.grants.public(model, kind).any(to_group);
        let ops = OPERATIONS.iter().zip(state.perms);
        ops.filter(|((_, kind), flag)| granted(*flag) && reached(*kind))
            .map(|((name, _), _)| *name)
            .collect()
    }

    /// OD406 on a write that leaves a rule with every `perm_*` false, which the CHECK of `ir.rule`
    /// refuses as it writes the row, whatever the writes after it give.
    fn rule_without_permission(&self, out: &mut Out<'m>, path: &'m Path, rule: &'m RuleFact) {
        if rule.access || rule.perms.iter().all(|p| *p == Flag::Unset) {
            return;
        }
        let state = RuleState::of(self.index, &self.rule_upto(rule));
        if state.perms == [Flag::Set(false); 4] {
            let message = "perm_read, perm_write, perm_create and perm_unlink are all false here, which the CHECK of ir.rule refuses, so the file fails to load; leave the operations the rule restricts true";
            out.push(path, "OD406", rule.pos, message);
        }
    }
}

#[cfg(test)]
mod tests {
    use rustc_hash::FxHashSet;

    use super::{
        MagicEra, applies, bundle_list, cache_global, callable_attr, csv_label, derived_label, fetches_own_view,
        field_name, has_last_update, hoot_adds, magic_era, no_access_message, page_miss, public_write_message,
        reads_model_access, shadowed_member, static_only, stores_parent_path, tuple_checked, tuple_listed,
        unknown_dependency, unmatched_asset, warns_on_labels,
    };
    use crate::facts::{AccessRow, DefinitionSource, JsName, Perms, Pos, Spanned};
    use crate::manifest::OdooReads;
    use crate::series::Series;

    fn everyone_row(group: &str, grants: Perms) -> AccessRow {
        AccessRow {
            id: Some("access_desk_everyone".to_owned()),
            model: "model_desk".to_owned(),
            by_name: false,
            pos: Pos { line: 2, col: 1 },
            group: Some(group.to_owned()),
            grants,
            global_write: false,
            public_write: None,
            model_access: true,
            domain: None,
        }
    }

    #[test]
    fn everyone_writes_report_from_saas_19_1_and_on_an_unknown_series() {
        let write = Perms {
            read: true,
            write: true,
            ..Perms::default()
        };
        let read = Perms {
            read: true,
            ..Perms::default()
        };
        let qualified = everyone_row("base.group_everyone", write);
        let message =
            |row: &AccessRow, module: &str, series: &str| public_write_message(row, module, Series::parse(series));
        assert!(message(&qualified, "desk", "20.0").is_some());
        assert!(message(&qualified, "desk", "saas~19.1").is_some());
        assert!(public_write_message(&qualified, "desk", None).is_some());
        assert!(message(&qualified, "desk", "18.0").is_none());
        assert!(message(&qualified, "desk", "19.0").is_none());
        assert!(message(&everyone_row("base.group_everyone", read), "desk", "20.0").is_none());
        let unqualified = everyone_row("group_everyone", write);
        assert!(message(&unqualified, "desk", "20.0").is_none());
        assert!(message(&unqualified, "base", "20.0").is_some());
    }

    #[test]
    fn language_suffix_is_cut_from_saas_18_2_and_on_an_unknown_series() {
        let name = |series: &str| field_name("name@fr", Series::parse(series));
        assert_eq!(field_name("name@fr", None), "name");
        assert_eq!(name("saas~18.1"), "name@fr");
        assert_eq!(name("saas~18.2"), "name");
        assert_eq!(name("19.0"), "name");
        assert_eq!(field_name("name", None), "name");
    }

    #[test]
    fn model_access_rows_load_below_saas_19_4() {
        let reads = |series: &str| reads_model_access(Series::parse(series));
        assert!(reads_model_access(None));
        assert!(reads("18.0") && reads("saas~19.3"));
        assert!(!reads("saas~19.4") && !reads("20.0"));
    }

    #[test]
    fn parent_path_counts_from_saas_11_3_and_on_an_unknown_series() {
        let stores = |series: &str| stores_parent_path(Series::parse(series));
        assert!(stores_parent_path(None));
        assert!(!stores("9.0") && !stores("10.0") && !stores("11.0") && !stores("saas~11.2"));
        assert!(stores("saas~11.3") && stores("saas~11.4") && stores("12.0"));
    }

    #[test]
    fn repeated_labels_warn_from_saas_11_2_and_on_an_unknown_series() {
        let warns = |series: &str| warns_on_labels(Series::parse(series));
        assert!(warns_on_labels(None));
        assert!(!warns("9.0") && !warns("10.0") && !warns("11.0") && !warns("saas~11.1"));
        assert!(warns("saas~11.2") && warns("saas~11.3") && warns("12.0"));
    }

    #[test]
    fn only_9_0_fetches_the_view_type_of_the_class() {
        let fetches = |series: &str| fetches_own_view(Series::parse(series));
        assert!(fetches("9.0"));
        assert!(!fetches("10.0") && !fetches("12.0") && !fetches("18.0"));
        assert!(!fetches_own_view(None));
    }

    #[test]
    fn the_no_access_message_follows_the_series() {
        let message = |series: Option<&str>| no_access_message("x.y", series.and_then(Series::parse));
        let model_access = "model 'x.y' has no ir.model.access rule, so only the superuser can use it";
        let ir_access = "model 'x.y' has no ir.access rule with a group, so only the superuser can use it";
        for series in [Some("9.0"), Some("10.0"), Some("saas~19.3"), None] {
            assert_eq!(message(series), model_access, "{series:?}");
        }
        for series in [Some("saas~19.4"), Some("20.0")] {
            assert_eq!(message(series), ir_access, "{series:?}");
        }
    }

    #[test]
    fn the_unknown_dependency_message_says_whether_odoo_reads_the_manifest() {
        assert_eq!(
            unknown_dependency("x", Some(&OdooReads::No)),
            "Odoo cannot read the manifest of module 'x'"
        );
        for reads in [OdooReads::AsPython2, OdooReads::AsExpression] {
            assert_eq!(
                unknown_dependency("x", Some(&reads)),
                "odooscan cannot read the manifest of module 'x'",
                "{reads:?}"
            );
        }
        assert_eq!(unknown_dependency("x", None), "module 'x' is not on the addons path");
    }

    #[test]
    fn a_write_checks_a_tuple_outside_saas_17_2_to_19_2() {
        let checked = |series: &str| tuple_checked(Series::parse(series));
        assert!(checked("9.0") && checked("saas~17.1"));
        assert!(!checked("saas~17.2") && !checked("18.0"));
        assert!(!checked("saas~18.1") && !checked("19.0") && !checked("saas~19.2"));
        assert!(checked("saas~19.3") && checked("20.0"));
        assert!(!tuple_checked(None));
    }

    #[test]
    fn a_tuple_takes_a_selection_add_from_saas_18_1() {
        let takes = |series: &str| tuple_listed(Series::parse(series));
        assert!(!takes("saas~17.1") && !takes("18.0"));
        assert!(takes("saas~18.1") && takes("20.0"));
        assert!(!tuple_listed(None));
    }

    #[test]
    fn a_csv_cell_passes_in_any_case_from_saas_14_3() {
        let values = FxHashSet::from_iter(["draft"]);
        let passes = |cell: &str, series: Option<Series>| csv_label(&[], &values, cell, series);
        assert!(!passes("DRAFT", Series::parse("9.0")) && !passes("DRAFT", Series::parse("saas~14.2")));
        assert!(passes("draft", Series::parse("saas~14.2")));
        assert!(passes("DRAFT", Series::parse("saas~14.3")) && passes("DRAFT", Series::parse("18.0")));
        assert!(passes("DRAFT", None));
        assert!(!passes("done", None));
    }

    #[test]
    fn applies_sql_constraints_before_19_and_table_objects_from_19() {
        let applied = |source, series: &str| applies(source, Series::parse(series));
        assert!(
            applied(DefinitionSource::SqlConstraints, "18.0") && !applied(DefinitionSource::SqlConstraints, "19.0")
        );
        assert!(!applied(DefinitionSource::TableObject, "18.0") && applied(DefinitionSource::TableObject, "19.0"));
        assert!(applies(DefinitionSource::SqlConstraints, None) && applies(DefinitionSource::TableObject, None));
    }

    #[test]
    fn derives_labels_as_python_title_case() {
        assert_eq!(derived_label("partner_id"), "Partner");
        assert_eq!(derived_label("tag_ids"), "Tag");
        assert_eq!(derived_label("parent_id_ids"), "Parent Id");
        assert_eq!(derived_label("x_studio_9a_date"), "X Studio 9A Date");
        assert_eq!(derived_label("is_VAT"), "Is Vat");
        assert_eq!(derived_label("id"), "Id");
    }

    #[test]
    fn adds_magic_fields_by_series() {
        let era = |series: &str| magic_era(Series::parse(series));
        assert_eq!(era("12.0"), MagicEra::RegistryMerge);
        assert_eq!(era("saas~12.4"), MagicEra::RegistryMerge);
        assert_eq!(era("saas~12.5"), MagicEra::Setup);
        assert_eq!(era("saas~14.3"), MagicEra::Setup);
        assert_eq!(era("saas~14.4"), MagicEra::Metaclass);
        assert_eq!(era("18.0"), MagicEra::Metaclass);
        assert_eq!(era("saas~18.1"), MagicEra::Base);
        assert_eq!(magic_era(None), MagicEra::Metaclass);
        assert!(has_last_update(Series::parse("16.0")) && !has_last_update(Series::parse("saas~16.1")));
        assert!(!has_last_update(None));
    }

    /// Each bound of `CACHE_KEY_GLOBALS` with the series on either side of it.
    #[test]
    fn cache_keys_read_the_globals_of_the_series() {
        let global = |name: &str, series: &str| cache_global(name, Series::parse(series));
        assert!(!global("time", "saas~17.1") && global("time", "saas~17.2") && !global("time", "19.0"));
        assert!(global("warnings", "18.0") && !global("warnings", "saas~18.1") && global("warnings", "saas~18.2"));
        assert!(!global("warnings", "saas~16.3") && global("warnings", "saas~16.4") && !global("warnings", "19.0"));
        assert!(!global("Counter", "saas~13.3") && global("Counter", "saas~13.4"));
        assert!(global("Counter", "saas~18.2") && !global("Counter", "saas~18.3"));
        assert!(!global("method", "saas~18.4") && global("method", "19.0") && global("method", "20.0"));
        assert!(global("ormcache", "saas~18.4") && !global("ormcache", "19.0"));
        assert!(global("getargspec", "13.0") && !global("getargspec", "saas~13.1"));
        assert!(!global("signature", "13.0") && global("signature", "saas~13.1") && !global("signature", "19.0"));
        assert!(!global("pycompat", "10.0") && global("pycompat", "11.0"));
        assert!(global("pycompat", "12.0") && !global("pycompat", "saas~12.1"));
        assert!(global("dummy_cache", "saas~16.3") && !global("dummy_cache", "saas~16.4"));
        assert!(global("cache", "saas~18.1") && !global("cache", "saas~18.2"));
        assert!(!global("Parameter", "saas~16.1") && global("Parameter", "saas~16.2"));
        assert!(global("Parameter", "saas~18.4") && !global("Parameter", "19.0"));
        assert!(!global("typing", "18.0") && global("typing", "saas~18.1") && !global("typing", "19.0"));
        assert!(!global("threading", "saas~18.2") && global("threading", "saas~18.3"));
        assert!(global("threading", "saas~18.4") && !global("threading", "19.0"));
        assert!(!global("_logger", "9.0") && global("_logger", "10.0") && !global("getargspec", "9.0"));
        assert!(cache_global("time", None) && cache_global("method", None) && !cache_global("lang", None));
    }

    #[test]
    fn shadows_only_the_members_the_series_has() {
        let shadows = |name: &str, series: &str| shadowed_member(name, Series::parse(series)).is_some();
        assert!(shadows("copy", "12.0") && shadows("env", "20.0") && shadows("grouped", "16.0"));
        assert!(!shadows("has_access", "17.0") && shadows("has_access", "18.0"));
        assert!(!shadows("_read_grouping_sets", "19.0") && shadows("_read_grouping_sets", "20.0"));
        assert!(shadows("toggle_active", "18.0") && !shadows("toggle_active", "20.0"));
        assert!(!shadows("_table_query", "saas~13.4") && shadows("_table_query", "saas~13.5"));
        assert!(shadows("_table_query", "19.0") && shadows("_table_query", "saas~19.4"));
        assert!(!shadows("_table_query", "20.0"));
        assert!(!shadows("_condition_to_sql", "17.0") && shadows("_condition_to_sql", "saas~17.1"));
        assert!(shadows("_condition_to_sql", "saas~18.3") && !shadows("_condition_to_sql", "saas~18.4"));
        assert!(!shadows("_condition_to_sql", "19.0") && !shadows("_condition_to_sql", "20.0"));
        assert!(!shadows("_name_search", "16.0") && !shadows("copied", "18.0"));
        assert!(shadowed_member("has_access", None).is_none() && shadowed_member("copy", None).is_some());
        let eighteen = Series::parse("18.0");
        assert_eq!(
            shadowed_member("_table_query", eighteen).as_deref(),
            Some("attribute _table_query")
        );
        assert_eq!(shadowed_member("copy", eighteen).as_deref(), Some("method copy()"));
    }

    #[test]
    fn an_asset_path_of_the_module_must_resolve_to_a_file_it_serves() {
        let mut module = crate::project::module("loan", r#"{"name": "x"}"#);
        module.static_assets = vec![
            "loan/static/src/js/a.js".to_owned(),
            "loan/static/lib/b.scss".to_owned(),
        ];
        let unmatched = |path: &str, static_only: bool| unmatched_asset(&module, path, static_only).is_some();
        for path in [
            "loan/static/src/**/*",
            "/loan/static/src/js/*.js",
            "loan/static/lib/b.scss",
            "loan/./static/src/js/a.js",
            "loan/static/src/js/../../src/js/a.js",
            "loan/static/fonts/c.woff2",
            "loan/static/src/js",
            "other/static/src/missing.js",
            "web/static/src/**/*",
            "loan/static/src/[js/*.js",
        ] {
            assert!(!unmatched(path, true), "{path}");
        }
        for path in [
            "loan/static/src/jss/**/*",
            "loan/static/src/js/*.css",
            "loan/static/src/js/missing.js",
            "loan/static/src/xml/missing.xml",
            "loan/views/assets.xml",
            "loan/**/*.js",
            "loan/static/../models/a.js",
        ] {
            assert!(unmatched(path, true), "{path}");
        }
        for path in ["loan/views/assets.xml", "loan/views/*.xml", "loan/../other/views/a.xml"] {
            assert!(unmatched(path, false), "{path}");
        }
        for path in [
            "loan/**/*.js",
            "loan/**/*.xml",
            "loan/*/src/a.xml",
            "loan/views/a.XML",
            "loan/views/a.js",
        ] {
            assert!(!unmatched(path, false), "{path}");
        }
        assert!(unmatched("loan/static/src/jss/**/*", false));
    }

    #[test]
    fn assets_come_from_static_alone_from_saas_16_3_and_on_an_unknown_series() {
        let only = |series: &str| static_only(Series::parse(series));
        assert!(static_only(None));
        assert!(!only("saas~16.2"));
        assert!(only("saas~16.3") && only("20.0"));
    }

    #[test]
    fn calls_env_on_every_series_and_pool_while_the_registry_is_callable() {
        let callable = |name: &str, series: &str| callable_attr(name, Series::parse(series));
        assert!(callable_attr("env", None) && callable_attr("pool", None));
        assert!(callable("env", "9.0") && callable("env", "10.0") && callable("env", "20.0"));
        assert!(callable("pool", "9.0") && callable("pool", "10.0") && callable("pool", "saas~18.4"));
        assert!(!callable("pool", "19.0") && !callable("pool", "saas~19.1") && !callable("pool", "20.0"));
        assert!(!callable_attr("ids", None) && !callable_attr("_cr", None));
        assert!(!callable("ids", "18.0") && !callable("_cr", "18.0"));
    }

    #[test]
    fn an_import_misses_a_page_that_loads_the_importer_but_not_the_target() {
        let backend = ["web.assets_backend", "web.assets_web"];
        assert_eq!(
            page_miss(&backend, &["web.assets_frontend"]),
            Some("web.assets_backend")
        );
        assert_eq!(page_miss(&backend, &["web.assets_web"]), None);
        assert_eq!(page_miss(&backend, &[]), None);
        assert_eq!(page_miss(&["shop.lazy"], &["web.assets_frontend"]), None);
        let lazy = ["web.assets_frontend_lazy"];
        assert_eq!(page_miss(&lazy, &["web.assets_frontend_minimal"]), None);
        let both = ["web.assets_backend", "web.assets_frontend"];
        assert_eq!(page_miss(&both, &["web.assets_backend"]), Some("web.assets_frontend"));
    }

    #[test]
    fn a_hoot_mock_adds_its_names_from_the_series_that_mocks_the_module() {
        let adds = |series: &str, from: &str, specifier: &str, name: &str| {
            let name = JsName {
                specifier: specifier.to_owned(),
                name: Spanned {
                    value: name.to_owned(),
                    pos: Pos { line: 1, col: 1 },
                },
                local: None,
            };
            hoot_adds(Series::parse(series), from, &name)
        };
        let test = "sale/static/tests/order.test.js";
        assert!(adds("18.0", test, "@web/core/utils/functions", "clearMemoizeCaches"));
        assert!(!adds("18.0", test, "@web/core/utils/functions", "debounceGone"));
        assert!(!adds("18.0", test, "@web/core/user", "clearMemoizeCaches"));
        assert!(!adds("18.0", test, "@web/core/utils/indexed_db", "_OriginalIndexedDB"));
        assert!(adds(
            "saas~18.3",
            test,
            "@web/core/utils/indexed_db",
            "_OriginalIndexedDB"
        ));
        // Hoot mocks the module by its name, which a relative specifier resolves to.
        let web = "web/static/tests/helpers.js";
        assert!(adds("18.0", web, "../src/core/utils/functions", "clearMemoizeCaches"));
    }

    #[test]
    fn a_bundle_list_names_three_then_counts_the_rest() {
        assert_eq!(bundle_list(&["a"]), "a");
        assert_eq!(bundle_list(&["a", "b", "c"]), "a, b and c");
        assert_eq!(bundle_list(&["a", "b", "c", "d", "e"]), "a, b, c and 2 more");
    }
}
