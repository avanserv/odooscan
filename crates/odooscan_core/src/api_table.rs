//! The Python APIs Odoo deprecates and later removes or ignores, with the series of each stage.
//! OD125 reports a form from the series Odoo warns about it, OD126 from the one where it fails or
//! has no effect; `python.rs` finds the forms.

use crate::series::Series;

/// Where a form of the API is written, as `python.rs` looks for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A call of the method on any receiver.
    Method(&'static str),
    /// A call of the method on a recordset the code shows: `self` in a model class, or a model it
    /// names. Files, cursors and browsers have methods of these names too.
    RecordsetMethod(&'static str),
    /// A call of the method given any of these keywords, reported at the first of them.
    Keywords(&'static str, &'static [&'static str]),
    /// `sudo()` given a user.
    SudoUser,
    /// A keyword of `with_context()`, or a key of the literal dict it is given.
    ContextKey(&'static str),
    /// A key of the literal dict, or of each literal dict of a list, that `create()` or `write()`
    /// writes on the model, with the field.
    WrittenField(&'static str, &'static str),
    /// A key of a literal dict an `@api.onchange` method returns.
    OnchangeKey(&'static str),
    /// A method of the name defined in a model class.
    Override(&'static str),
    /// A keyword of a field declaration.
    FieldOption(&'static str),
    /// An import of the package or of a module under it.
    Import(&'static str),
    /// An attribute of `request`.
    RequestAttr(&'static str),
    /// The `type` of an `@route()`.
    RouteType(&'static str),
}

pub(crate) struct Row {
    pub kind: Kind,
    /// The form as findings name it.
    form: &'static str,
    /// From which series Odoo warns about the form.
    warned: Option<Series>,
    /// From which series the form fails or has no effect.
    removed: Option<Series>,
    /// From which series the name stands for another API, which the row no longer describes.
    reused: Option<Series>,
    /// The advice that ends the message, such as `use tracking=`.
    instead: &'static str,
    /// The series from which `instead` exists, and the advice for the series before it.
    instead_before: Option<(Series, &'static str)>,
    /// The argument whose `False` changes what the call does, and the advice for such a call.
    pub if_false: Option<IfFalse>,
}

/// An argument of a call, by keyword or position, and the advice for a call passing `False` there.
pub(crate) struct IfFalse {
    pub keyword: &'static str,
    pub position: usize,
    instead: &'static str,
}

const fn at(major: u16, minor: u16) -> Option<Series> {
    Some(Series { major, minor })
}

const fn row(
    kind: Kind,
    form: &'static str,
    warned: Option<Series>,
    removed: Option<Series>,
    instead: &'static str,
) -> Row {
    Row {
        kind,
        form,
        warned,
        removed,
        reused: None,
        instead,
        instead_before: None,
        if_false: None,
    }
}

/// Each bound is the first series whose source shows the stage, and the comments cite that source.
pub(crate) static ROWS: &[Row] = &[
    // saas~12.5 warns that the parameter is no longer supported (`odoo/fields.py:453`).
    row(
        Kind::FieldOption("oldname"),
        "oldname=",
        None,
        at(12, 5),
        "use an upgrade script that renames the column",
    ),
    // saas~13.4 warns of an unknown parameter (`odoo/fields.py:357`), as mail.thread accepts only
    // `tracking` (`addons/mail/models/mail_thread.py:115`), and saas~18.1 mail.thread no longer
    // reads it where 18.0 does (`addons/mail/models/mail_thread.py:591`).
    row(
        Kind::FieldOption("track_visibility"),
        "track_visibility=",
        at(13, 4),
        at(18, 1),
        "use tracking=",
    ),
    // saas~12.5 warns (`odoo/models.py:4921`), saas~15.3 asserts a bool (`odoo/models.py:5327`).
    row(
        Kind::SudoUser,
        "sudo(user)",
        at(12, 5),
        at(15, 3),
        "use with_user(user)",
    ),
    // saas~13.1 raises on it (`addons/mail/models/mail_thread.py:1835`).
    row(
        Kind::Keywords("message_post", &["subtype"]),
        "message_post(subtype=)",
        None,
        at(13, 1),
        "use subtype_xmlid=",
    ),
    // saas~13.1 logs that the key is no longer supported (`odoo/models.py:5101`) and adds
    // with_company() (`:5047`), and nothing under `odoo/` reads the key any more.
    row(
        Kind::ContextKey("force_company"),
        "the context key force_company",
        None,
        at(13, 1),
        "use with_company(company)",
    ),
    // saas~13.2 logs a domain an onchange returns as deprecated (`odoo/models.py:5923`), and from
    // saas~16.3 `_onchange_eval` reads only its value and warning (`odoo/models.py:6507`).
    row(
        Kind::OnchangeKey("domain"),
        "an onchange returning a domain",
        at(13, 2),
        at(16, 3),
        "set the domain on the field or in the view",
    ),
    // saas~15.4 warns (`odoo/models.py:5790`, `:6331`, `:6151`), saas~15.2 for refresh()
    // (`odoo/models.py:6028`), and saas~16.2 has none of the four. Which replacement fits hangs on
    // the arguments: without any the call acts on the whole environment, with fields on the model,
    // with records or ids on those (saas~15.4 `odoo/models.py:5794`, `:6154`), so the advice names
    // each.
    row(
        Kind::RecordsetMethod("flush"),
        "flush()",
        at(15, 4),
        at(16, 2),
        "use flush_model(), flush_recordset() or self.env.flush_all()",
    ),
    row(
        Kind::RecordsetMethod("recompute"),
        "recompute()",
        at(15, 4),
        at(16, 2),
        "use flush_model(), flush_recordset() or self.env.flush_all()",
    ),
    row(
        Kind::RecordsetMethod("invalidate_cache"),
        "invalidate_cache()",
        at(15, 4),
        at(16, 2),
        "use invalidate_model(), invalidate_recordset() or self.env.invalidate_all()",
    ),
    // refresh() invalidates the whole cache: through invalidate_cache() in saas~15.2 and saas~15.3
    // (`odoo/models.py:6028`), through env.invalidate_all() from saas~15.4 (`odoo/models.py:6141`).
    Row {
        instead_before: Some((Series { major: 15, minor: 4 }, "use invalidate_cache()")),
        ..row(
            Kind::RecordsetMethod("refresh"),
            "refresh()",
            at(15, 2),
            at(16, 2),
            "use self.env.invalidate_all()",
        )
    },
    // saas~17.1 has it (`odoo/models.py:1563`), saas~17.2 not.
    row(
        Kind::Method("user_has_groups"),
        "user_has_groups()",
        None,
        at(17, 2),
        "use self.env.user.has_groups()",
    ),
    // saas~17.2 ir.cron has both fields (`odoo/addons/base/models/ir_cron.py:67`), and from
    // saas~17.3 create() and write() raise on a field the model lacks (`odoo/models.py:4437`,
    // `:4683`), while _trigger() schedules a run (`ir_cron.py:636`).
    row(
        Kind::WrittenField("ir.cron", "numbercall"),
        "writing numbercall on ir.cron",
        None,
        at(17, 3),
        "use active=False or _trigger()",
    ),
    row(
        Kind::WrittenField("ir.cron", "doall"),
        "writing doall on ir.cron",
        None,
        at(17, 3),
        "remove it",
    ),
    // saas~17.4 name_search() calls it (`odoo/models.py:1733`), and 18.0 searches through
    // _search_display_name() instead (`odoo/models.py:1885`, `:1823`).
    row(
        Kind::Override("_name_search"),
        "overriding _name_search()",
        None,
        at(18, 0),
        "override _search_display_name()",
    ),
    // 18.0 warns (`odoo/models.py:4512`, `:4527`), saas~19.1 has neither. With `raise_exception`
    // false, check_access_rights() returns has_access() (18.0 `odoo/models.py:4518`).
    Row {
        if_false: Some(IfFalse {
            keyword: "raise_exception",
            position: 1,
            instead: "use has_access()",
        }),
        ..row(
            Kind::Method("check_access_rights"),
            "check_access_rights()",
            at(18, 0),
            at(19, 1),
            "use check_access()",
        )
    },
    row(
        Kind::Method("check_access_rule"),
        "check_access_rule()",
        at(18, 0),
        at(19, 1),
        "use check_access()",
    ),
    // saas~18.2 warns (`odoo/orm/models.py:2410`), saas~19.1 drops it, and 20.0 brings back the
    // name with the signature of the next row (`odoo/orm/models.py:1932`).
    Row {
        reused: at(20, 0),
        ..row(
            Kind::Method("read_group"),
            "read_group()",
            at(18, 2),
            at(19, 1),
            "use _read_group()",
        )
    },
    row(
        Kind::Keywords("read_group", &["fields", "orderby", "lazy"]),
        "read_group(fields=, orderby=, lazy=)",
        None,
        at(20, 0),
        "use read_group(domain, groupby, aggregates, order=)",
    ),
    // 18.0 web reads groups through read_group() (`addons/web/models/models.py:311`), saas~18.2
    // through formatted_read_group() (`:300`, `:335`) as read_group() warns
    // (`odoo/orm/models.py:2410`), and 20.0 declares the new one final (`odoo/orm/models.py:1930`),
    // which its lint test holds every override to (`test_lint/tests/test_override_signatures.py:146`).
    row(
        Kind::Override("read_group"),
        "overriding read_group()",
        at(18, 2),
        at(19, 1),
        "override _read_group() or formatted_read_group()",
    ),
    // 19.0 warns on import (`odoo/osv/__init__.py:4`), saas~19.1 has no such package.
    row(
        Kind::Import("odoo.osv"),
        "importing odoo.osv",
        at(19, 0),
        at(19, 1),
        "use odoo.fields.Domain",
    ),
    // saas~15.3's Request has no such attribute (`odoo/http.py:964`) and keeps the body on its
    // dispatcher (`odoo/http.py:1598`), and saas~15.4 adds get_json_data() (`odoo/http.py:1287`).
    Row {
        instead_before: Some((Series { major: 15, minor: 4 }, "use request.dispatcher.jsonrequest")),
        ..row(
            Kind::RequestAttr("jsonrequest"),
            "request.jsonrequest",
            None,
            at(15, 3),
            "use request.get_json_data()",
        )
    },
    // saas~18.1 warns that the type is an alias (`odoo/http.py:746`).
    row(
        Kind::RouteType("json"),
        "type='json'",
        at(18, 1),
        None,
        "use type='jsonrpc'",
    ),
];

/// The row of a kind, which `ROWS` holds once.
pub(crate) fn row_of(kind: Kind) -> &'static Row {
    ROWS.iter()
        .find(|r| r.kind == kind)
        .unwrap_or_else(|| panic!("no API row for {kind:?}"))
}

/// The series as Odoo names its branch.
fn label(series: Series) -> String {
    if series.minor == 0 {
        series.to_string()
    } else {
        format!("saas~{series}")
    }
}

/// The code and the message of a use of the row's form in a module of `series`, or None while the
/// form works without a warning, and in a module of unknown series.
pub(crate) fn stage(row: &Row, series: Option<Series>) -> Option<(&'static str, String)> {
    staged(row, series, None)
}

/// `stage` for a call that passes `False` where `row.if_false` reads it.
pub(crate) fn stage_if_false(row: &Row, series: Option<Series>) -> Option<(&'static str, String)> {
    staged(row, series, row.if_false.as_ref().map(|f| f.instead))
}

fn staged(row: &Row, series: Option<Series>, advice: Option<&'static str>) -> Option<(&'static str, String)> {
    let series = series?;
    let reached = |bound: Option<Series>| bound.filter(|b| series >= *b);
    if reached(row.reused).is_some() {
        return None;
    }
    let instead = match row.instead_before {
        Some((bound, earlier)) if series < bound => earlier,
        _ => advice.unwrap_or(row.instead),
    };
    if let Some(from) = reached(row.removed) {
        let message = format!("{} no longer works from {}; {instead}", row.form, label(from));
        return Some(("OD126", message));
    }
    let from = reached(row.warned)?;
    let message = format!("{} is deprecated from {}; {instead}", row.form, label(from));
    Some(("OD125", message))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(kind: Kind, series: &str) -> Option<&'static str> {
        stage(row_of(kind), Series::parse(series)).map(|(code, _)| code)
    }

    #[test]
    fn each_row_is_silent_then_deprecated_then_removed() {
        // (kind, last silent, first warned, first removed), from the sources the comments in
        // `ROWS` cite.
        let cases: [(Kind, &str, Option<&str>, Option<&str>); 22] = [
            (Kind::FieldOption("oldname"), "saas~12.4", None, Some("saas~12.5")),
            (
                Kind::FieldOption("track_visibility"),
                "saas~13.3",
                Some("saas~13.4"),
                Some("saas~18.1"),
            ),
            (Kind::SudoUser, "saas~12.4", Some("saas~12.5"), Some("saas~15.3")),
            (
                Kind::Keywords("message_post", &["subtype"]),
                "13.0",
                None,
                Some("saas~13.1"),
            ),
            (
                Kind::RecordsetMethod("flush"),
                "saas~15.3",
                Some("saas~15.4"),
                Some("saas~16.2"),
            ),
            (
                Kind::RecordsetMethod("recompute"),
                "saas~15.3",
                Some("saas~15.4"),
                Some("saas~16.2"),
            ),
            (
                Kind::RecordsetMethod("invalidate_cache"),
                "saas~15.3",
                Some("saas~15.4"),
                Some("saas~16.2"),
            ),
            (
                Kind::RecordsetMethod("refresh"),
                "saas~15.1",
                Some("saas~15.2"),
                Some("saas~16.2"),
            ),
            (Kind::Method("user_has_groups"), "saas~17.1", None, Some("saas~17.2")),
            (
                Kind::Method("check_access_rights"),
                "saas~17.4",
                Some("18.0"),
                Some("saas~19.1"),
            ),
            (
                Kind::Method("check_access_rule"),
                "saas~17.4",
                Some("18.0"),
                Some("saas~19.1"),
            ),
            (
                Kind::Method("read_group"),
                "saas~18.1",
                Some("saas~18.2"),
                Some("saas~19.1"),
            ),
            (
                Kind::Keywords("read_group", &["fields", "orderby", "lazy"]),
                "saas~19.4",
                None,
                Some("20.0"),
            ),
            (Kind::Import("odoo.osv"), "saas~18.4", Some("19.0"), Some("saas~19.1")),
            (Kind::RequestAttr("jsonrequest"), "saas~15.2", None, Some("saas~15.3")),
            (Kind::RouteType("json"), "18.0", Some("saas~18.1"), None),
            (Kind::ContextKey("force_company"), "13.0", None, Some("saas~13.1")),
            (
                Kind::OnchangeKey("domain"),
                "saas~13.1",
                Some("saas~13.2"),
                Some("saas~16.3"),
            ),
            (
                Kind::WrittenField("ir.cron", "numbercall"),
                "saas~17.2",
                None,
                Some("saas~17.3"),
            ),
            (
                Kind::WrittenField("ir.cron", "doall"),
                "saas~17.2",
                None,
                Some("saas~17.3"),
            ),
            (Kind::Override("_name_search"), "saas~17.4", None, Some("18.0")),
            (
                Kind::Override("read_group"),
                "saas~18.1",
                Some("saas~18.2"),
                Some("saas~19.1"),
            ),
        ];
        for (kind, silent, warned, removed) in cases {
            assert_eq!(code(kind, silent), None, "{kind:?} on {silent}");
            if let Some(series) = warned {
                assert_eq!(code(kind, series), Some("OD125"), "{kind:?} on {series}");
            }
            if let Some(series) = removed {
                assert_eq!(code(kind, series), Some("OD126"), "{kind:?} on {series}");
            }
        }
        assert_eq!(ROWS.len(), cases.len());
        assert_eq!(stage(row_of(Kind::SudoUser), None), None, "unknown series");
    }

    #[test]
    fn the_rule_gates_start_at_the_earliest_bound() {
        let earliest = ROWS.iter().flat_map(|r| [r.warned, r.removed]).flatten().min();
        for code in ["OD125", "OD126"] {
            let gate = crate::rules::rule(code).series.and_then(|s| s.strip_prefix(">="));
            assert_eq!(gate.and_then(Series::parse), earliest, "{code}");
        }
    }

    #[test]
    fn the_old_read_group_is_silent_once_20_0_reuses_the_name() {
        assert_eq!(code(Kind::Method("read_group"), "saas~19.4"), Some("OD126"));
        assert_eq!(code(Kind::Method("read_group"), "20.0"), None);
    }

    #[test]
    fn an_override_of_read_group_stays_an_error_once_20_0_makes_it_final() {
        assert_eq!(code(Kind::Override("read_group"), "20.0"), Some("OD126"));
    }

    #[test]
    fn messages_name_the_series_and_the_replacement() {
        let (_, message) = stage(row_of(Kind::SudoUser), Series::parse("16.0")).unwrap();
        assert_eq!(
            message,
            "sudo(user) no longer works from saas~15.3; use with_user(user)"
        );
        let (_, message) = stage(row_of(Kind::Method("check_access_rule")), Series::parse("18.0")).unwrap();
        assert_eq!(
            message,
            "check_access_rule() is deprecated from 18.0; use check_access()"
        );
    }

    #[test]
    fn advice_names_a_replacement_the_series_has() {
        let message = |kind, series| stage(row_of(kind), Series::parse(series)).unwrap().1;
        let refresh = Kind::RecordsetMethod("refresh");
        assert_eq!(
            message(refresh, "saas~15.3"),
            "refresh() is deprecated from saas~15.2; use invalidate_cache()"
        );
        assert_eq!(
            message(refresh, "saas~15.4"),
            "refresh() is deprecated from saas~15.2; use self.env.invalidate_all()"
        );
        let jsonrequest = Kind::RequestAttr("jsonrequest");
        assert_eq!(
            message(jsonrequest, "saas~15.3"),
            "request.jsonrequest no longer works from saas~15.3; use request.dispatcher.jsonrequest"
        );
        assert_eq!(
            message(jsonrequest, "saas~15.4"),
            "request.jsonrequest no longer works from saas~15.3; use request.get_json_data()"
        );
    }

    #[test]
    fn advice_follows_an_argument_that_changes_the_call() {
        let row = row_of(Kind::Method("check_access_rights"));
        let (_, message) = stage_if_false(row, Series::parse("18.0")).unwrap();
        assert_eq!(
            message,
            "check_access_rights() is deprecated from 18.0; use has_access()"
        );
        let (_, message) = stage_if_false(row_of(Kind::Method("check_access_rule")), Series::parse("18.0")).unwrap();
        assert_eq!(
            message,
            "check_access_rule() is deprecated from 18.0; use check_access()"
        );
        let (_, message) = stage(row_of(Kind::WrittenField("ir.cron", "doall")), Series::parse("18.0")).unwrap();
        assert_eq!(
            message,
            "writing doall on ir.cron no longer works from saas~17.3; remove it"
        );
    }
}
