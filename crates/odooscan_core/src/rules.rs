use serde::Serialize;

use crate::series::{self, Series};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Correctness,
    Security,
    Deprecation,
    Quality,
}

/// Info-level security rules are hotspots: code to review, not a confirmed flaw.
pub struct Rule {
    pub code: &'static str,
    pub name: &'static str,
    pub category: Category,
    pub severity: Severity,
    /// Series range the rule applies to; `None` applies to every series.
    pub series: Option<&'static str>,
    /// Rules that need the whole dependency graph are skipped when `base` is not indexed.
    pub needs_index: bool,
    pub summary: &'static str,
    /// The rule's page in `docs/rules/`, embedded so that `oscn explain` and the site print the same text.
    pub doc: &'static str,
}

/// Rules that say less than another rule reporting at the same position, as (code, by).
/// A flow from a known source is the finding a pattern match on the same call only suspects.
pub const SUPERSEDED: &[(&str, &str)] = &[("OD101", "OD701")];

/// Severities that replace a rule's own from some series on, as (code, series range, severity).
/// From 20.0, `ir.access` rows only grant access, so a model meant for the superuser alone has
/// none. From saas~18.3, Odoo refuses a model whose `_inherits` field is not required with ondelete
/// 'cascade' or 'restrict' instead of fixing it. From saas~16.4, a `check_company` field gets the
/// domain its comodel's `_check_company_domain` gives, which by default names a `company_id` the
/// comodel may lack.
const SEVERITY_BY_SERIES: &[(&str, &str, Severity)] = &[
    ("OD401", ">=20.0", Severity::Info),
    ("OD218", ">=saas~18.3", Severity::Error),
    ("OD222", ">=saas~16.4", Severity::Error),
];

impl Rule {
    pub fn severity_for(&self, module_series: Option<Series>) -> Severity {
        SEVERITY_BY_SERIES
            .iter()
            .find(|(code, spec, _)| *code == self.code && module_series.is_some_and(|s| series::matches(spec, s)))
            .map_or(self.severity, |(_, _, severity)| *severity)
    }

    /// A module of unknown series gets only the rules that carry no range.
    pub fn applies_to(&self, module_series: Option<Series>) -> bool {
        match (self.series, module_series) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(spec), Some(s)) => series::matches(spec, s),
        }
    }
}

macro_rules! rules {
    ($($code:literal $name:literal $cat:ident $sev:ident $series:expr, $index:literal, $summary:literal;)*) => {
        pub static RULES: &[Rule] = &[$(Rule {
            code: $code,
            name: $name,
            category: Category::$cat,
            severity: Severity::$sev,
            series: $series,
            needs_index: $index,
            summary: $summary,
            doc: include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/rules/", $code, ".md")),
        }),*];
    };
}

rules! {
    "OD000" "syntax-error" Correctness Error None, false, "The file does not parse.";
    "OD101" "sql-injection" Security Error None, false, "cr.execute() receives a query built by string formatting.";
    "OD102" "translation-interpolated" Quality Warning None, false, "_() receives an already formatted string, so the source text cannot be translated.";
    "OD103" "cursor-commit" Correctness Warning None, false, "Business code commits the cursor, breaking the request's transaction.";
    "OD104" "default-evaluated-at-import" Correctness Error None, false, "A field default is called once at import instead of on each record.";
    "OD105" "missing-super" Correctness Error None, false, "An ORM override never calls super().";
    "OD106" "name-get-deprecated" Deprecation Warning Some(">=17.0"), false, "name_get() is ignored; override _compute_display_name().";
    "OD107" "api-multi-one-removed" Deprecation Error Some(">=13.0"), false, "@api.multi and @api.one no longer exist.";
    "OD108" "group-operator-renamed" Deprecation Warning Some(">=18.0"), false, "Field attribute group_operator is renamed aggregator.";
    "OD109" "private-env-shortcut" Deprecation Warning Some(">=19.0"), false, "self._cr, self._uid and self._context are deprecated; use self.env.";
    "OD110" "sql-constraints-unsupported" Deprecation Error Some(">=19.0"), false, "_sql_constraints is ignored; declare models.Constraint attributes.";
    "OD111" "public-route-sudo" Security Info None, false, "A public or auth='none' route escalates with sudo().";
    "OD112" "csrf-disabled" Security Info None, false, "An HTTP route disables CSRF protection.";
    "OD113" "fields-view-get-removed" Deprecation Error Some(">=16.0"), false, "fields_view_get() is removed; override _get_view() or get_views().";
    "OD114" "depends-on-id" Correctness Error None, false, "@api.depends names the field id, which Odoo refuses.";
    "OD115" "dotted-onchange-constrains" Correctness Warning None, false, "@api.onchange or @api.constrains names a dotted path, which Odoo ignores.";
    "OD116" "field-shadows-model-member" Correctness Error None, false, "A field is named like a method or an attribute every model has.";
    // saas-15.2 warns (`odoo/api.py:404`), saas-15.1 does not (`odoo/api.py:397`); saas-18.2 batches such a create
    // (`odoo/orm/decorators.py:286`), saas-18.1 does not (`odoo/orm/decorators.py:266`).
    "OD117" "create-not-batched" Deprecation Warning Some(">=saas~15.2,<saas~18.2"), false, "create() is decorated with @api.model, so Odoo calls it once per record.";
    // saas-18.2 hands such a create a list (`odoo/orm/decorators.py:286`), saas-18.1 one dict at a time
    // (`odoo/orm/decorators.py:266`).
    "OD118" "create-reads-dict" Correctness Error Some(">=saas~18.2"), false, "create() under @api.model reads its argument as one dict, but receives a list of them.";
    "OD119" "override-drops-result" Correctness Error None, false, "A create(), write() or copy() override calls super() but returns nothing.";
    "OD120" "ormcache-unknown-name" Correctness Error None, false, "An ormcache key names something that is not a parameter of the method.";
    "OD121" "cursor-rollback" Correctness Warning None, false, "Business code rolls back the cursor, discarding the caller's work with its own.";
    "OD122" "sql-write-without-invalidation" Correctness Warning None, false, "An SQL write runs without invalidating the cache, so the ORM keeps stale values.";
    "OD123" "patch-at-import" Correctness Warning None, false, "Code run at import patches Odoo, for every database the server runs.";
    "OD124" "module-level-cache" Correctness Warning None, false, "A module-level dict, list or set filled at runtime is shared by every database.";
    "OD201" "unknown-model" Correctness Error None, true, "The model is not defined by any indexed module.";
    "OD202" "model-not-in-depends" Correctness Error None, true, "The model comes from a module missing from depends.";
    "OD203" "unresolved-field-path" Correctness Error None, true, "A field path does not resolve on its model.";
    "OD204" "missing-method" Correctness Error None, true, "compute, inverse, search, selection or group_expand names a method the model does not have.";
    "OD205" "field-type-changed" Correctness Warning None, true, "An inherited field is redefined with another type.";
    "OD206" "unknown-env-model" Correctness Warning None, true, "env[...] names a model no indexed module defines.";
    "OD207" "related-type-mismatch" Correctness Error None, true, "A related field has another type than the field it relates to.";
    "OD208" "invalid-one2many-inverse" Correctness Error None, true, "A One2many's inverse field is not a Many2one.";
    "OD209" "model-redefined" Correctness Error None, true, "A class redefines a model that a module in depends already defines.";
    "OD210" "unknown-domain-field" Correctness Error None, true, "A literal domain or group_by names a field its model does not have.";
    "OD211" "orphan-super" Correctness Error None, true, "super() calls a method no parent class defines.";
    "OD212" "monetary-without-currency" Correctness Error None, true, "A Monetary field's currency field is not a field of its model.";
    "OD213" "required-many2one-set-null" Correctness Error Some(">=13.0"), true, "A required Many2one declares ondelete='set null'.";
    "OD214" "many2many-table-clash" Correctness Error Some(">=13.0"), true, "Two Many2many fields of a model to one comodel get the same table.";
    "OD215" "many2many-table-too-long" Correctness Error None, true, "A Many2many's relation table name is longer than PostgreSQL allows.";
    "OD216" "selection-add-without-ondelete" Correctness Error Some(">=14.0"), true, "selection_add leaves a value of a required Selection with ondelete 'set null'.";
    "OD217" "parent-store-without-path" Correctness Error None, true, "A model sets _parent_store without an indexed parent_path field.";
    "OD218" "delegation-field-not-required" Correctness Warning None, true, "A field that delegates to a parent model is not required with ondelete cascade or restrict.";
    // 14.0 validates the merged attributes and warns (`odoo/fields.py:381`, 231bdca3f26d). saas-13.5 validates
    // `self.args` (`odoo/fields.py:360`), which a registry field built by `field.new()` lacks (`odoo/models.py:2750`),
    // so it logs nothing.
    "OD219" "tracking-without-mail-thread" Correctness Warning Some(">=14.0"), true, "A field sets tracking on a model that does not inherit mail.thread, which Odoo ignores.";
    "OD220" "duplicate-field-label" Correctness Warning None, true, "Two fields of a model have the same label.";
    // saas-14.4 warns (`odoo/fields.py:713`), saas-14.3 sets `recursive` without a word (`odoo/fields.py:645`).
    "OD221" "self-dependency-not-recursive" Correctness Warning Some(">=saas~14.4"), true, "A computed field depends on itself through other records without recursive=True.";
    // saas-12.5 introduces `check_company` (`odoo/fields.py:2217`), saas-12.4 has no such attribute.
    "OD222" "check-company-without-company" Correctness Warning Some(">=saas~12.5"), true, "A field sets check_company to a comodel without company_id.";
    "OD223" "compute-leaves-field-unassigned" Correctness Error Some(">=saas~13.3"), true, "A compute method leaves a non-stored readonly field unassigned on some records.";
    "OD224" "sql-unknown-column" Correctness Error None, true, "A literal SQL query names a column its model's table does not have.";
    "OD225" "sql-view-missing-column" Correctness Error None, true, "The SQL view of a model selects no column for one of its stored fields.";
    "OD226" "sql-view-not-created" Correctness Error None, true, "A model sets _auto = False, and nothing creates its SQL view.";
    "OD227" "sql-definition-unknown-column" Correctness Error None, true, "An SQL constraint or index names a column its model's table does not have.";
    "OD228" "delegate-without-ondelete" Correctness Error Some(">=saas~14.4,<saas~18.3"), true, "A delegate=True Many2one has no ondelete, so Odoo fails when it sets up the registry.";
    "OD301" "unresolved-xmlid" Correctness Error None, true, "The XML id is not defined by its module.";
    "OD302" "xmlid-not-in-depends" Correctness Error None, true, "The XML id comes from a module missing from depends.";
    "OD303" "duplicate-xmlid" Correctness Warning None, false, "The same XML id is defined twice in one file.";
    "OD304" "unknown-view-field" Correctness Error None, true, "A view references a field its model does not have.";
    "OD305" "attrs-states-removed" Deprecation Error Some(">=17.0"), false, "attrs and states are removed; use invisible, readonly and required expressions.";
    "OD306" "tree-renamed-list" Deprecation Warning Some(">=18.0"), false, "The tree view type is renamed list.";
    "OD307" "t-raw-unsafe" Security Warning Some(">=15.0"), false, "t-raw is deprecated and renders unescaped HTML; use t-out with Markup.";
    "OD308" "xmlid-used-before-definition" Correctness Error None, false, "An XML id is referenced before the file that defines it is loaded.";
    "OD309" "unknown-record-field" Correctness Error None, true, "A data record sets a field its model does not have.";
    "OD310" "unmatched-view-locator" Correctness Error None, true, "An inherited view locates an element its parent views do not have.";
    "OD311" "unknown-qweb-template" Correctness Error None, true, "A QWeb view or a report names a template no module in scope defines.";
    "OD312" "unknown-button-method" Correctness Error None, true, "A button calls a method its model does not have.";
    "OD313" "private-button-method" Correctness Error None, true, "A button calls a private method, which Odoo refuses.";
    "OD314" "unknown-function-method" Correctness Error None, true, "A <function> of a data file calls a method its model does not have.";
    // saas-16.4 has no `VIEW_MODIFIERS`, and 17.0 validates them (`odoo/addons/base/models/ir_ui_view.py:1892`).
    "OD315" "invalid-modifier" Correctness Error Some(">=17.0"), true, "A view modifier is not an expression Odoo reads.";
    "OD316" "unknown-modifier-name" Correctness Error Some(">=17.0"), true, "A view modifier names a field its model does not have.";
    // saas-17.2 refuses a field the view lacks (`odoo/addons/base/models/ir_ui_view.py:2894`), saas-17.3 adds it
    // (`:1156`).
    "OD317" "modifier-field-not-in-view" Correctness Error Some(">=17.0,<saas~17.3"), true, "A view modifier reads a field the view does not show.";
    "OD318" "view-mode-without-view" Correctness Error None, true, "A window action lists a view type its model has no view for.";
    "OD319" "python-field-syntax-error" Correctness Error None, false, "The code of a server action or a cron, or the file name of a report, is not Python that Odoo compiles.";
    "OD320" "unknown-server-code-method" Correctness Error None, true, "The code of a server action or a cron calls a method its model does not have.";
    "OD321" "wrong-model-reference" Correctness Error None, true, "A data record gives a relational field a record of another model.";
    "OD322" "unknown-selection-value" Correctness Error None, true, "A data record gives a Selection field a value it does not list.";
    "OD323" "duplicate-xmlid-across-files" Correctness Warning None, false, "A data file defines an XML id another file of the module already defines, and writes the fields it names over it.";
    "OD324" "missing-required-field" Correctness Warning None, true, "A data record is created without a required field that has no default.";
    "OD401" "model-without-access" Security Warning None, true, "A new model has no ir.model.access rule.";
    "OD402" "public-write-access" Security Warning None, false, "An access row gives public or portal users write, create or unlink, through their group or no group.";
    "OD501" "manifest-data-missing" Correctness Error None, false, "A file listed in the manifest does not exist.";
    "OD502" "data-file-not-loaded" Quality Warning None, false, "A data file is listed under no manifest key Odoo reads, so Odoo never loads it.";
    "OD503" "unknown-dependency" Correctness Error None, true, "A module in depends is not on the addons path.";
    "OD504" "circular-dependency" Correctness Error None, true, "A module in depends depends back on this module.";
    "OD601" "unresolved-js-import" Correctness Error None, true, "A JS import names a module or an export no indexed module provides.";
    "OD602" "orphan-patch-super" Correctness Error Some(">=17.0"), true, "A patch calls a super method the class it patches does not have.";
    "OD603" "unknown-rpc-model" Correctness Error None, true, "An ORM call from JS names a model no indexed module defines.";
    "OD604" "unknown-rpc-method" Correctness Error None, true, "orm.call() names a method its model does not have.";
    "OD605" "unknown-template" Correctness Error Some(">=17.0"), true, "A component, a t-inherit or a t-call names a template no indexed bundle defines.";
    "OD606" "unmatched-template-locator" Correctness Error Some(">=17.0"), true, "A template locates an element its parent templates do not have.";
    "OD607" "private-rpc-method" Correctness Error None, false, "orm.call() names a private method, which Odoo refuses to call over RPC.";
    "OD701" "sql-injection-flow" Security Error None, false, "Data a user controls reaches the text of an SQL query.";
    "OD702" "code-injection-flow" Security Error None, false, "Data a user controls reaches safe_eval(), eval() or exec().";
    "OD703" "markup-injection-flow" Security Warning None, false, "Data a user controls is wrapped in Markup() without escaping.";
    "OD704" "sudo-mass-assignment" Security Error None, false, "A mapping a user controls is written as superuser, so the user picks the fields.";
}

pub fn rule(code: &str) -> &'static Rule {
    RULES
        .iter()
        .find(|r| r.code == code)
        .unwrap_or_else(|| panic!("unregistered rule {code}"))
}

/// The rule a code or a name designates, in any case.
pub fn find(query: &str) -> Option<&'static Rule> {
    RULES
        .iter()
        .find(|r| r.code.eq_ignore_ascii_case(query) || r.name.eq_ignore_ascii_case(query))
}

/// The rules whose code or name shares the longest prefix with the query, at most five. A prefix
/// shorter than three characters, such as the `OD` every code has, says nothing, so it yields none.
pub fn nearest(query: &str) -> Vec<&'static Rule> {
    let query = query.to_ascii_lowercase();
    let shared = |s: &str| {
        s.to_ascii_lowercase()
            .bytes()
            .zip(query.bytes())
            .take_while(|(a, b)| a == b)
            .count()
    };
    let score = |r: &Rule| shared(r.code).max(shared(r.name));
    let best = RULES.iter().map(score).max().unwrap_or_default();
    if best < 3 {
        return vec![];
    }
    RULES.iter().filter(|r| score(r) == best).take(5).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_rule_is_info_from_20() {
        let od401 = rule("OD401");
        assert_eq!(od401.severity_for(Series::parse("18.0")), Severity::Warning);
        assert_eq!(od401.severity_for(Series::parse("20.0")), Severity::Info);
        assert_eq!(od401.severity_for(None), Severity::Warning);
    }

    #[test]
    fn delegation_rule_is_an_error_from_saas_18_3() {
        let od218 = rule("OD218");
        assert_eq!(od218.severity_for(Series::parse("18.0")), Severity::Warning);
        assert_eq!(od218.severity_for(Series::parse("saas~18.2")), Severity::Warning);
        assert_eq!(od218.severity_for(Series::parse("saas~18.3")), Severity::Error);
        assert_eq!(od218.severity_for(Series::parse("19.0")), Severity::Error);
        assert_eq!(od218.severity_for(Series::parse("20.0")), Severity::Error);
        assert_eq!(od218.severity_for(None), Severity::Warning);
    }

    #[test]
    fn unassigned_compute_rule_runs_from_saas_13_3() {
        let od223 = rule("OD223");
        assert!(!od223.applies_to(Series::parse("saas~13.2")));
        assert!(!od223.applies_to(None));
        assert!(od223.applies_to(Series::parse("saas~13.3")));
        assert!(od223.applies_to(Series::parse("14.0")));
        assert!(od223.applies_to(Series::parse("20.0")));
    }

    #[test]
    fn company_check_rule_is_an_error_from_saas_16_4() {
        let od222 = rule("OD222");
        assert_eq!(od222.severity_for(Series::parse("saas~16.3")), Severity::Warning);
        assert_eq!(od222.severity_for(Series::parse("saas~16.4")), Severity::Error);
        assert_eq!(od222.severity_for(Series::parse("17.0")), Severity::Error);
        assert_eq!(od222.severity_for(Series::parse("18.0")), Severity::Error);
        assert_eq!(od222.severity_for(Series::parse("20.0")), Severity::Error);
        assert_eq!(od222.severity_for(None), Severity::Warning);
    }

    #[test]
    fn delegate_crash_rule_runs_from_saas_14_4_before_saas_18_3() {
        let od228 = rule("OD228");
        assert!(!od228.applies_to(Series::parse("saas~14.3")));
        assert!(!od228.applies_to(Series::parse("saas~18.3")));
        assert!(!od228.applies_to(None));
        assert!(od228.applies_to(Series::parse("saas~14.4")));
        assert!(od228.applies_to(Series::parse("18.0")));
        assert!(od228.applies_to(Series::parse("saas~18.2")));
    }

    #[test]
    fn series_gates_follow_source() {
        let gates: [(&str, &[&str], &[&str]); 8] = [
            ("OD117", &["saas~15.1", "saas~18.2"], &["saas~15.2", "saas~18.1"]),
            ("OD118", &["saas~18.1"], &["saas~18.2"]),
            ("OD219", &["saas~13.5"], &["14.0"]),
            ("OD221", &["saas~14.3"], &["saas~14.4"]),
            ("OD222", &["saas~12.4"], &["saas~12.5"]),
            ("OD315", &["saas~16.4"], &["17.0", "20.0"]),
            ("OD316", &["saas~16.4"], &["17.0", "20.0"]),
            ("OD317", &["saas~16.4", "saas~17.3"], &["17.0", "saas~17.2"]),
        ];
        for (code, silent, reported) in gates {
            let r = rule(code);
            for series in silent {
                assert!(!r.applies_to(Series::parse(series)), "{code} runs on {series}");
            }
            for series in reported {
                assert!(r.applies_to(Series::parse(series)), "{code} skips {series}");
            }
        }
    }

    #[test]
    fn finds_by_code_or_name_and_suggests_by_prefix() {
        assert_eq!(find("od211").map(|r| r.code), Some("OD211"));
        assert_eq!(find("orphan-super").map(|r| r.code), Some("OD211"));
        assert!(find("OD999").is_none());
        let codes = |q| nearest(q).iter().map(|r| r.code).collect::<Vec<_>>();
        assert_eq!(codes("od21"), ["OD210", "OD211", "OD212", "OD213", "OD214"]);
        assert_eq!(codes("orphan-supper"), ["OD211"]);
        assert!(codes("OD999").is_empty());
    }

    fn pages() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/rules")
    }

    /// The row a rule's page must carry under its heading, rendered from the registry.
    fn facts_row(r: &Rule) -> String {
        let since = |spec: &str| spec.strip_prefix(">=").unwrap_or(spec).to_owned();
        let mut severity = format!("{:?}", r.severity);
        for (_, spec, s) in SEVERITY_BY_SERIES.iter().filter(|(code, ..)| *code == r.code) {
            severity += &format!(", {s:?} from {}", since(spec));
        }
        let range = |spec: &str| match spec.split_once(",<") {
            Some((from, before)) => format!("{} and later, before {before}", since(from)),
            None => format!("{} and later", since(spec)),
        };
        let series = r.series.map_or_else(|| "all".to_owned(), range);
        let index = if r.needs_index { "yes" } else { "no" };
        format!("| {:?} | {severity} | {series} | {index} |", r.category)
    }

    fn page_errors(r: &Rule, text: &str) -> Vec<String> {
        let mut errors = vec![];
        let heading = format!("# {} {}", r.code, r.name);
        if text.lines().next() != Some(heading.as_str()) {
            errors.push(format!("{}: the page must open with `{heading}`", r.code));
        }
        let row = text.lines().skip_while(|l| !l.starts_with("| Category |")).nth(2);
        let expected = facts_row(r);
        if row != Some(expected.as_str()) {
            errors.push(format!("{}: the facts row must read `{expected}`, not {row:?}", r.code));
        }
        errors
    }

    #[test]
    fn every_rule_has_a_page_that_matches_it() {
        let index = std::fs::read_to_string(pages().join("index.md")).expect("docs/rules/index.md");
        let mut errors = vec![];
        for r in RULES {
            match std::fs::read_to_string(pages().join(format!("{}.md", r.code))) {
                Ok(text) => errors.extend(page_errors(r, &text)),
                Err(_) => errors.push(format!("{}: docs/rules/{}.md is missing", r.code, r.code)),
            }
            if !index.contains(&format!("[{0}]({0}.md)", r.code)) {
                errors.push(format!("{}: docs/rules/index.md does not link to the page", r.code));
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    #[test]
    fn every_page_names_a_registered_rule() {
        let stray: Vec<_> = std::fs::read_dir(pages())
            .expect("docs/rules")
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter_map(|name| name.strip_suffix(".md").map(str::to_owned))
            .filter(|stem| stem != "index" && !RULES.iter().any(|r| r.code == stem))
            .collect();
        assert!(stray.is_empty(), "pages for no registered rule: {stray:?}");
    }
}
