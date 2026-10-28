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
/// From 20.0, `ir.access` rows only grant access, so a model meant for the superuser alone has none,
/// and Odoo refuses a model whose `_inherits` field is not required with ondelete 'cascade' or
/// 'restrict' instead of fixing it.
const SEVERITY_BY_SERIES: &[(&str, &str, Severity)] = &[
    ("OD401", ">=20.0", Severity::Info),
    ("OD218", ">=20.0", Severity::Error),
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
    "OD201" "unknown-model" Correctness Error None, true, "The model is not defined by any indexed module.";
    "OD202" "model-not-in-depends" Correctness Error None, true, "The model comes from a module missing from depends.";
    "OD203" "unresolved-field-path" Correctness Error None, true, "A field path does not resolve on its model.";
    "OD204" "missing-method" Correctness Error None, true, "compute, inverse, search, selection or group_expand names a method the model does not have.";
    "OD205" "field-type-changed" Correctness Warning None, true, "An inherited field is redefined with another type.";
    "OD206" "unknown-env-model" Correctness Warning None, true, "env[...] names a model no indexed module defines.";
    "OD207" "related-type-mismatch" Correctness Error None, true, "A related field has another type than the field it relates to.";
    "OD208" "invalid-one2many-inverse" Correctness Error None, true, "A One2many's inverse field is not a Many2one.";
    "OD209" "model-redefined" Correctness Error None, true, "A new _name replaces a model that a module in depends already defines.";
    "OD210" "unknown-domain-field" Correctness Error None, true, "A literal domain or group_by names a field its model does not have.";
    "OD211" "orphan-super" Correctness Error None, true, "super() calls a method no parent class defines.";
    "OD212" "monetary-without-currency" Correctness Error None, true, "A Monetary field's currency field is not a field of its model.";
    "OD213" "required-many2one-set-null" Correctness Error Some(">=13.0"), true, "A required Many2one declares ondelete='set null'.";
    "OD214" "many2many-table-clash" Correctness Error Some(">=13.0"), true, "Two Many2many fields of a model to one comodel get the same table.";
    "OD215" "many2many-table-too-long" Correctness Error None, true, "A Many2many's relation table name is longer than PostgreSQL allows.";
    "OD216" "selection-add-without-ondelete" Correctness Error Some(">=14.0"), true, "selection_add leaves a value of a required Selection with ondelete 'set null'.";
    "OD217" "parent-store-without-path" Correctness Error None, true, "A model sets _parent_store without an indexed parent_path field.";
    "OD218" "delegation-field-not-required" Correctness Warning None, true, "A field that delegates to a parent model is not required with ondelete cascade or restrict.";
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
    "OD401" "model-without-access" Security Warning None, true, "A new model has no ir.model.access rule.";
    "OD402" "public-write-access" Security Warning None, false, "Public or portal users get write, create or unlink access.";
    "OD501" "manifest-data-missing" Correctness Error None, false, "A file listed in the manifest does not exist.";
    "OD502" "data-file-not-loaded" Quality Warning None, false, "A data file is not listed in the manifest, so Odoo never loads it.";
    "OD503" "unknown-dependency" Correctness Error None, true, "A module in depends is not on the addons path.";
    "OD504" "circular-dependency" Correctness Error None, true, "A module in depends depends back on this module.";
    "OD601" "unresolved-js-import" Correctness Error None, true, "A JS import names a module or an export no indexed module provides.";
    "OD602" "orphan-patch-super" Correctness Error Some(">=17.0"), true, "A patch calls a super method the class it patches does not have.";
    "OD603" "unknown-rpc-model" Correctness Error None, true, "An ORM call from JS names a model no indexed module defines.";
    "OD604" "unknown-rpc-method" Correctness Error None, true, "orm.call() names a method its model does not have.";
    "OD605" "unknown-template" Correctness Error Some(">=17.0"), true, "A component or a t-inherit names a template no indexed bundle defines.";
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
    fn delegation_rule_is_an_error_from_20() {
        let od218 = rule("OD218");
        assert_eq!(od218.severity_for(Series::parse("18.0")), Severity::Warning);
        assert_eq!(od218.severity_for(Series::parse("19.0")), Severity::Warning);
        assert_eq!(od218.severity_for(Series::parse("20.0")), Severity::Error);
        assert_eq!(od218.severity_for(None), Severity::Warning);
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
        let series = r
            .series
            .map_or_else(|| "all".to_owned(), |s| format!("{} and later", since(s)));
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
