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
}

/// Severities that replace a rule's own from some series on, as (code, series range, severity).
/// From 20.0, `ir.access` rows only grant access, so a model meant for the superuser alone has none.
const SEVERITY_BY_SERIES: &[(&str, &str, Severity)] = &[("OD401", ">=20.0", Severity::Info)];

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
    "OD201" "unknown-model" Correctness Error None, true, "The model is not defined by any indexed module.";
    "OD202" "model-not-in-depends" Correctness Error None, true, "The model comes from a module missing from depends.";
    "OD203" "unresolved-field-path" Correctness Error None, true, "A field path does not resolve on its model.";
    "OD204" "missing-method" Correctness Error None, true, "compute, inverse or search names a method the model does not have.";
    "OD205" "field-type-changed" Correctness Warning None, true, "An inherited field is redefined with another type.";
    "OD206" "unknown-env-model" Correctness Warning None, true, "env[...] names a model no indexed module defines.";
    "OD207" "related-type-mismatch" Correctness Error None, true, "A related field has another type than the field it relates to.";
    "OD208" "invalid-one2many-inverse" Correctness Error None, true, "A One2many's inverse is not a Many2one back to its model.";
    "OD209" "model-redefined" Correctness Error None, true, "A new _name replaces a model that a module in depends already defines.";
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
}

pub fn rule(code: &str) -> &'static Rule {
    RULES
        .iter()
        .find(|r| r.code == code)
        .unwrap_or_else(|| panic!("unregistered rule {code}"))
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
}
