use std::path::PathBuf;

use odooscan_core::rules::{Kind, Severity, rule};
use odooscan_core::series::Series;
use odooscan_core::{ScanOptions, TaintTable, scan};

#[test]
fn fixtures() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    // As an odooscan.toml names them, for bad_sale/controllers/configured.py.
    let mut taint = TaintTable::default();
    taint.source("lib.read_form").unwrap();
    taint.sink("OD701", "lib.run_query").unwrap();
    taint.sanitizer("lib.clean_term").unwrap();
    let report = scan(&ScanOptions {
        paths: vec![root.join("custom")],
        addons_paths: vec![root.join("core")],
        default_series: None,
        target_series: None,
        taint,
    });
    let lines: Vec<String> = report
        .diagnostics
        .iter()
        .map(|d| {
            format!(
                "{}:{}:{}: {} {}",
                d.path.strip_prefix(&root).unwrap().display(),
                d.line,
                d.column,
                d.code,
                d.message
            )
        })
        .collect();
    insta::assert_snapshot!(lines.join("\n"));
}

#[test]
fn base_creates_none_of_its_ids() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let report = scan(&ScanOptions {
        paths: vec![root.join("bootstrap")],
        addons_paths: vec![],
        default_series: None,
        target_series: None,
        ..Default::default()
    });
    let lines: Vec<u32> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "OD324")
        .map(|d| d.line)
        .collect();
    assert_eq!(lines, [3]);
}

#[test]
fn series_notice_only_without_default() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let notices = |default_series| {
        scan(&ScanOptions {
            paths: vec![root.join("core")],
            addons_paths: vec![],
            default_series,
            target_series: None,
            ..Default::default()
        })
        .notices
        .into_iter()
        .filter(|n| n.contains("--odoo-version"))
        .count()
    };
    assert_eq!(notices(None), 3);
    assert_eq!(notices(Series::parse("18.0")), 0);
}

#[test]
fn python_2_files_of_checked_modules_get_a_notice() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let notices = |paths: Vec<PathBuf>, addons_paths: Vec<PathBuf>| {
        scan(&ScanOptions {
            paths,
            addons_paths,
            default_series: None,
            target_series: None,
            ..Default::default()
        })
        .notices
        .into_iter()
        .filter(|n| n.contains("Python 2 syntax"))
        .collect::<Vec<_>>()
    };
    let checked = notices(vec![root.join("custom")], vec![root.join("core")]);
    for file in [
        "legacy10/models/py2_forms.py:6:1: Python 2 syntax, which 10.0 runs",
        "legacy9/models/py2_tabs.py:7:1: Python 2 syntax, which 9.0 runs",
        "unparsed_code/models/ledger.py:25:15: Python 2 syntax, which 10.0 runs",
    ] {
        assert!(checked.iter().any(|n| n.starts_with(file)), "{file}: {checked:#?}");
    }
    for module in ["legacy15/", "inherited_ids_outside/", "legacy10/models/py2_broken.py"] {
        assert!(!checked.iter().any(|n| n.starts_with(module)), "{module}: {checked:#?}");
    }
    assert!(checked.is_sorted());
    assert_eq!(notices(vec![], vec![root.join("custom")]), [""; 0]);
}

#[test]
fn an_indexed_manifest_that_does_not_parse_gets_a_notice() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let report = scan(&ScanOptions {
        paths: vec![root.join("custom/manifest_unclosed_user")],
        addons_paths: vec![root.join("custom"), root.join("core")],
        ..Default::default()
    });
    let notice = "manifest_unclosed/__manifest__.py:5:1: unexpected EOF while parsing; this module is not indexed";
    assert!(report.notices.iter().any(|n| n == notice), "{:#?}", report.notices);
    let found: Vec<_> = report.diagnostics.iter().map(|d| (d.code, d.line, d.column)).collect();
    assert_eq!(found, [("OD503", 4, 25)]);
    assert_eq!(
        report.diagnostics[0].message,
        "Odoo cannot read the manifest of module 'manifest_unclosed'"
    );
}

#[test]
fn an_ignored_manifest_error_is_silent() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let report = scan(&ScanOptions {
        paths: vec![root.join("custom/manifest_ignored")],
        addons_paths: vec![root.join("core")],
        ..Default::default()
    });
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(report.modules_checked, 1);
}

#[test]
fn default_series_reads_a_seriesless_auto_install_list() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    // Before saas~12.4 the list waits on every depend, and web_sticker also depends on web_glue.
    let reported = |default_series| {
        scan(&ScanOptions {
            paths: vec![root.join("custom")],
            addons_paths: vec![root.join("core")],
            default_series,
            target_series: None,
            ..Default::default()
        })
        .diagnostics
        .iter()
        .any(|d| d.code == "OD302" && d.message.contains("'web_sticker.sticker'"))
    };
    assert!(!reported(None));
    assert!(reported(Series::parse("12.0")));
}

#[test]
fn a_seriesless_auto_install_reads_as_the_module_checked() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let reported = |default_series, reader: &str| {
        scan(&ScanOptions {
            paths: vec![root.join("custom")],
            addons_paths: vec![root.join("core")],
            default_series,
            target_series: None,
            ..Default::default()
        })
        .diagnostics
        .iter()
        .filter(|d| d.path.ends_with(format!("auto_scope_reader_{reader}/thing.py")))
        .any(|d| d.code == "OD210" && d.message.contains("'country_any_stamp'"))
    };
    assert!(!reported(Series::parse("17.0"), "any"));
    assert!(reported(Series::parse("17.0"), "new"));
    assert!(!reported(Series::parse("18.0"), "old"));
    assert!(reported(Series::parse("18.0"), "any"));
}

#[test]
fn unassigned_compute_waits_for_a_known_series() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let reported = |default_series| {
        scan(&ScanOptions {
            paths: vec![root.join("custom")],
            addons_paths: vec![root.join("core")],
            default_series,
            target_series: None,
            ..Default::default()
        })
        .diagnostics
        .iter()
        .any(|d| d.code == "OD223" && d.path.ends_with("unversioned_logs/models/probes.py"))
    };
    assert!(!reported(None));
    assert!(reported(Series::parse("14.0")));
}

#[test]
fn a_lone_cr_waits_for_a_known_series() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let refused = |default_series| {
        let report = scan(&ScanOptions {
            paths: vec![root.join("custom")],
            addons_paths: vec![root.join("core")],
            default_series,
            target_series: None,
            ..Default::default()
        });
        let mut files: Vec<String> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "OD000" && d.path.to_string_lossy().contains("/csv_rows_any/"))
            .map(|d| d.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        files.sort();
        files
    };
    assert_eq!(refused(None), ["res.partner-short.csv"]);
    assert_eq!(
        refused(Series::parse("16.0")),
        ["res.partner-feed.csv", "res.partner-short.csv"]
    );
    assert_eq!(
        refused(Series::parse("18.0")),
        ["res.partner-cr.csv", "res.partner-short.csv"]
    );
}

#[test]
fn a_csv_feeds_other_rules_where_it_loads() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let codes = |default_series| {
        scan(&ScanOptions {
            paths: vec![root.join("custom")],
            addons_paths: vec![root.join("core")],
            default_series,
            target_series: None,
            ..Default::default()
        })
        .diagnostics
        .iter()
        .filter(|d| d.path.ends_with("csv_rows_any/data/res.partner-feed.csv"))
        .map(|d| d.code)
        .collect::<Vec<_>>()
    };
    assert_eq!(codes(None), ["OD301"]);
    assert_eq!(codes(Series::parse("16.0")), ["OD000"]);
}

#[test]
fn delegate_without_ondelete_waits_for_a_known_series() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let code = |default_series| {
        scan(&ScanOptions {
            paths: vec![root.join("custom")],
            addons_paths: vec![root.join("core")],
            default_series,
            target_series: None,
            ..Default::default()
        })
        .diagnostics
        .iter()
        .find(|d| d.path.ends_with("unversioned_logs/models/probes.py") && d.line == 59)
        .map(|d| d.code)
    };
    assert_eq!(code(None), Some("OD218"));
    assert_eq!(code(Series::parse("18.0")), Some("OD228"));
}

/// The severity each module of the fixtures reports `code` at, in scan order.
fn severities(code: &str) -> Vec<(String, Severity)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let report = scan(&ScanOptions {
        paths: vec![root.join("custom")],
        addons_paths: vec![root.join("core")],
        default_series: None,
        target_series: None,
        ..Default::default()
    });
    let mut found: Vec<(String, Severity)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == code)
        .map(|d| {
            let module = d.path.strip_prefix(root.join("custom")).unwrap().iter().next().unwrap();
            (module.to_string_lossy().into_owned(), d.severity)
        })
        .collect();
    found.dedup();
    found
}

#[test]
fn company_check_severity_follows_the_module_series() {
    let expected = [
        ("field_options", Severity::Error),
        ("legacy13", Severity::Warning),
        ("legacy16", Severity::Warning),
        ("legacy17", Severity::Error),
        ("saas125", Severity::Warning),
    ];
    assert_eq!(severities("OD222"), expected.map(|(m, s)| (m.to_owned(), s)));
}

#[test]
fn tree_severity_is_an_error() {
    let expected = [("bad_sale", Severity::Error), ("tree_paths", Severity::Error)];
    assert_eq!(severities("OD306"), expected.map(|(m, s)| (m.to_owned(), s)));
}

#[test]
fn raw_severity_is_an_error_from_saas_19_1() {
    let expected = [
        ("bad_sale", Severity::Warning),
        ("modern19", Severity::Warning),
        ("modern20", Severity::Error),
    ];
    assert_eq!(severities("OD307"), expected.map(|(m, s)| (m.to_owned(), s)));
}

#[test]
fn kanban_box_severity_follows_the_module_series() {
    let expected = [("bad_sale", Severity::Warning), ("modern19", Severity::Error)];
    assert_eq!(severities("OD326"), expected.map(|(m, s)| (m.to_owned(), s)));
}

#[test]
fn chatter_div_is_reported_from_saas_17_2() {
    assert_eq!(severities("OD327"), [("bad_sale".to_owned(), Severity::Warning)]);
}

#[test]
fn asset_declarations_are_reported_from_their_series() {
    let expected = [("legacy16", Severity::Error), ("saas143", Severity::Error)];
    assert_eq!(severities("OD328"), expected.map(|(m, s)| (m.to_owned(), s)));
    let expected = [
        ("legacy16", Severity::Warning),
        ("qweb_unindexed", Severity::Warning),
        ("saas143", Severity::Warning),
    ];
    assert_eq!(severities("OD505"), expected.map(|(m, s)| (m.to_owned(), s)));
    assert_eq!(severities("OD506"), [("legacy16".to_owned(), Severity::Warning)]);
}

#[test]
fn python_api_stages_keep_their_severity() {
    let deprecated = [
        "legacy14",
        "legacy154",
        "legacy17",
        "modern19",
        "saas125",
        "saas131",
        "saas132",
        "saas181",
        "saas182",
    ];
    assert_eq!(
        severities("OD125"),
        deprecated.map(|m| (m.to_owned(), Severity::Warning))
    );
    let removed = [
        "cursor_state",
        "legacy154",
        "legacy17",
        "modern20",
        "saas131",
        "saas181",
        "saas192",
    ];
    assert_eq!(severities("OD126"), removed.map(|m| (m.to_owned(), Severity::Error)));
}

#[test]
fn an_undecorated_route_override_is_an_error_before_saas_15_3() {
    let expected = [
        ("route_clash", Severity::Info),
        ("route_drop15", Severity::Error),
        ("route_unread", Severity::Info),
    ];
    assert_eq!(severities("OD232"), expected.map(|(m, s)| (m.to_owned(), s)));
}

#[test]
fn access_severity_follows_the_module_series() {
    let expected = [
        ("bad_sale", Severity::Warning),
        ("encoded_lost_stranger", Severity::Warning),
        ("modern20", Severity::Info),
        ("saas132", Severity::Warning),
        ("saas193", Severity::Warning),
        ("saas194", Severity::Info),
        ("sql_columns", Severity::Warning),
        ("unlisted_data", Severity::Warning),
    ];
    assert_eq!(severities("OD401"), expected.map(|(m, s)| (m.to_owned(), s)));
}

#[test]
fn a_row_with_no_group_needs_no_base() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let report = scan(&ScanOptions {
        paths: vec![root.join("custom/access_rows")],
        addons_paths: vec![],
        default_series: None,
        target_series: None,
        ..Default::default()
    });
    let lines: Vec<u32> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "OD402" && d.path.ends_with("security/ir.model.access.csv"))
        .map(|d| d.line)
        .collect();
    assert_eq!(lines, [2, 3, 4]);
}

#[test]
fn a_file_a_hook_loads_reports_its_rows() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/custom/access_rows");
    let report = scan(&ScanOptions {
        paths: vec![root.clone()],
        addons_paths: vec![],
        default_series: None,
        target_series: None,
        ..Default::default()
    });
    let lines = |file: &str| -> Vec<u32> {
        let od402 = report.diagnostics.iter().filter(|d| d.code == "OD402");
        od402.filter(|d| d.path == root.join(file)).map(|d| d.line).collect()
    };
    assert_eq!(lines("security/ir.model.access-hook.csv"), [2, 3]);
    assert!(lines("tests/ir.model.access.csv").is_empty());
}

#[test]
fn a_calendar_the_client_registers_still_needs_a_date_field() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    // Scanned alone, as the rest of the fixtures hold a calendar view and so keep `calendar` served.
    let report = scan(&ScanOptions {
        paths: vec![root.join("custom/legacy9")],
        addons_paths: vec![root.join("core")],
        default_series: None,
        target_series: None,
        ..Default::default()
    });
    let reported = |message: &str| {
        let od318 = report.diagnostics.iter().filter(|d| d.code == "OD318");
        od318.filter(|d| d.message.contains(message)).count()
    };
    assert_eq!(
        reported("legacy9.gauge has no primary calendar view, and no date field"),
        1
    );
    assert_eq!(reported("legacy9_compare"), 0);
}

#[test]
fn a_public_row_needs_no_base() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let report = scan(&ScanOptions {
        paths: vec![root.join("custom/saas131")],
        addons_paths: vec![],
        default_series: None,
        target_series: None,
        ..Default::default()
    });
    let lines: Vec<u32> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "OD402")
        .map(|d| d.line)
        .collect();
    assert_eq!(lines, [3, 5]);
}

#[test]
fn refused_server_code_needs_no_base() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let refused = |module: &str| {
        let report = scan(&ScanOptions {
            paths: vec![root.join(module)],
            addons_paths: vec![],
            default_series: None,
            target_series: None,
            ..Default::default()
        });
        report.diagnostics.iter().filter(|d| d.code == "OD319").count()
    };
    assert!(refused("custom/server_code") > 0);
    // Below 10.0 only the writes tell whether the code runs, and those need no base either.
    assert_eq!(refused("custom/legacy9"), 2);
}

#[test]
fn a_refused_file_name_needs_no_base() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let report = scan(&ScanOptions {
        paths: vec![root.join("custom/report_actions")],
        addons_paths: vec![],
        default_series: None,
        target_series: None,
        ..Default::default()
    });
    let in_file = |code: &str, name: &str| {
        let matching = report.diagnostics.iter().filter(|d| d.code == code);
        matching.filter(|d| d.path.ends_with(name)).count()
    };
    assert_eq!(in_file("OD319", "data/file_names.xml"), 11);
    assert_eq!(in_file("OD319", "demo/reports.xml"), 1);
    assert_eq!(in_file("OD311", "data/templates_named.xml"), 0);
}

#[test]
fn a_report_tag_waits_for_a_known_series() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let findings = |default_series| {
        let report = scan(&ScanOptions {
            paths: vec![root.join("custom")],
            addons_paths: vec![root.join("core")],
            default_series,
            target_series: None,
            ..Default::default()
        });
        let found = report.diagnostics.iter();
        found
            .filter(|d| d.path.ends_with("unversioned_logs/data/reports.xml"))
            .map(|d| d.line)
            .collect::<Vec<_>>()
    };
    assert_eq!(findings(None), [7, 8]);
    assert_eq!(findings(Series::parse("14.0")), [2, 2, 7, 8]);
    assert_eq!(findings(Series::parse("10.0")), [0; 0]);
}

#[test]
fn hierarchy_and_label_rules_wait_for_their_series_but_not_for_an_unknown_one() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let codes = |default_series| {
        let report = scan(&ScanOptions {
            paths: vec![root.join("custom")],
            addons_paths: vec![root.join("core")],
            default_series,
            target_series: None,
            ..Default::default()
        });
        let found = report.diagnostics.into_iter();
        found
            .filter(|d| d.path.ends_with("unversioned_logs/models/orchard.py"))
            .map(|d| d.code)
            .collect::<Vec<_>>()
    };
    assert_eq!(codes(None), ["OD217", "OD220"]);
    assert_eq!(codes(Series::parse("9.0")), [""; 0]);
    assert_eq!(codes(Series::parse("10.0")), [""; 0]);
    assert_eq!(codes(Series::parse("saas~11.1")), [""; 0]);
    assert_eq!(codes(Series::parse("saas~11.2")), ["OD220"]);
    assert_eq!(
        codes(Series::parse("saas~11.3")),
        ["OD217", "OD220", "OD217", "OD217", "OD217"]
    );
}

#[test]
fn import_patches_without_base_take_the_advice_of_the_path() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let report = scan(&ScanOptions {
        paths: vec![root.join("custom")],
        addons_paths: vec![],
        default_series: None,
        target_series: None,
        ..Default::default()
    });
    let advice: Vec<(u32, bool)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "OD123" && d.path.ends_with("class_patches.py"))
        .map(|d| (d.line, d.message.contains("; inherit the model")))
        .collect();
    // Unlike the scan with base, LabelFormat takes the advice of its wizard package, and
    // LabelPrinter and LabelQueue that of their services package.
    assert_eq!(
        advice,
        [(8, true), (9, true), (12, true), (13, false), (17, false), (18, false)]
    );
}

#[test]
fn a_version_of_another_series_is_refused_under_a_default_series_only() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let refused = |default_series| {
        let report = scan(&ScanOptions {
            paths: vec![
                root.join("custom/legacy17"),
                root.join("custom/manifest_version_refused"),
            ],
            addons_paths: vec![root.join("core")],
            default_series,
            ..Default::default()
        });
        let mut modules: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "OD509")
            .filter_map(|d| d.path.parent()?.file_name()?.to_str().map(str::to_owned))
            .collect();
        modules.sort();
        modules
    };
    assert_eq!(refused(None), ["manifest_version_refused"]);
    assert_eq!(refused(Series::parse("18.0")), ["legacy17", "manifest_version_refused"]);
}

#[test]
fn a_saas_series_wants_its_saas_prefix() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let report = scan(&ScanOptions {
        paths: ["manifest_version_plain181", "saas181", "manifest_assets_unmatched_162"]
            .into_iter()
            .map(|m| root.join("custom").join(m))
            .collect(),
        addons_paths: vec![root.join("core")],
        ..Default::default()
    });
    let refused: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "OD509")
        .filter_map(|d| d.path.parent()?.file_name()?.to_str())
        .collect();
    assert_eq!(refused, ["manifest_version_plain181"]);
}

#[test]
fn a_target_series_checks_a_module_as_of_that_series() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let findings = |target_series| {
        let report = scan(&ScanOptions {
            paths: vec![root.join("target/port17")],
            addons_paths: vec![root.join("core")],
            default_series: None,
            target_series,
            ..Default::default()
        });
        let found = report.diagnostics.iter();
        found
            .map(|d| (d.line, d.code, rule(d.code).kind(d.severity)))
            .collect::<Vec<_>>()
    };
    // The manifest still names 17.0, which 18.0 will not install.
    let ported = [
        (1, "OD509", Kind::Other),
        (7, "OD108", Kind::Deprecated),
        (10, "OD126", Kind::Removed),
        (13, "OD103", Kind::Other),
        (17, "OD201", Kind::Upstream),
    ];
    assert_eq!(findings(Series::parse("18.0")), ported);
    assert_eq!(
        findings(None),
        [(13, "OD103", Kind::Other), (17, "OD201", Kind::Upstream)]
    );
}

#[test]
fn an_indexed_module_keeps_its_own_series_under_a_target() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    // web_corkboard names 12.3, where its list-form auto_install waits on project too.
    let report = scan(&ScanOptions {
        paths: vec![root.join("custom/legacy12")],
        addons_paths: vec![root.join("core")],
        default_series: None,
        target_series: Series::parse("18.0"),
        ..Default::default()
    });
    let found = report.diagnostics.iter();
    assert!(
        found
            .filter(|d| d.code == "OD302")
            .any(|d| d.message.contains("'web_corkboard.corkboard'"))
    );
}

#[test]
fn a_seriesless_indexed_module_takes_the_target() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    // As 12.0 web_sticker's list-form auto_install waits on web_glue too, so base_only must depend on it.
    let reported = |target_series| {
        let report = scan(&ScanOptions {
            paths: vec![root.join("custom/base_only")],
            addons_paths: vec![root.join("core")],
            default_series: None,
            target_series,
            ..Default::default()
        });
        let found = report.diagnostics.iter();
        found
            .filter(|d| d.code == "OD302")
            .any(|d| d.message.contains("'web_sticker.sticker'"))
    };
    assert!(!reported(Series::parse("18.0")));
    assert!(reported(Series::parse("12.0")));
}

/// The `(code, folder)` of each OD509, OD514 and OD515 finding on `custom/migration_folders`, the
/// folder being the one holding the file the finding is on.
fn migration_findings(default_series: Option<Series>, target_series: Option<Series>) -> Vec<(&'static str, String)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let report = scan(&ScanOptions {
        paths: vec![root.join("custom/migration_folders")],
        addons_paths: vec![root.join("core")],
        default_series,
        target_series,
        ..Default::default()
    });
    let mut found: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| ["OD509", "OD514", "OD515"].contains(&d.code))
        .map(|d| {
            let folder = d.path.parent().and_then(|p| p.file_name()).unwrap_or_default();
            (d.code, folder.to_string_lossy().into_owned())
        })
        .collect();
    found.sort();
    found
}

#[test]
fn migration_folders_take_the_module_series() {
    // `v1.1` takes `migrate(env, version)`, yet Odoo never imports it, so it is OD514's alone.
    let own = ["1.3", "18.0.1.3.0", "19.0.1.0", "v1.1"].map(|f| ("OD514", f.to_owned()));
    assert_eq!(migration_findings(None, None), own);
    // The manifest names 18.0, so `--odoo-version 16.0` does not stand for its series.
    assert_eq!(migration_findings(Series::parse("16.0"), None), own);
}

#[test]
fn a_refused_version_leaves_folders_unread() {
    let found = migration_findings(None, Series::parse("20.0"));
    assert_eq!(found, [("OD509", "migration_folders".to_owned())]);
}

#[test]
fn a_seriesless_web_runs_the_owl_of_the_module_checked() {
    // Core's `web` gives `'version': '1.0'`, so only the checked module tells which Owl it runs.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/seriesless_web");
    let report = scan(&ScanOptions {
        paths: vec![root.join("custom")],
        addons_paths: vec![root.join("core")],
        ..Default::default()
    });
    let found: Vec<_> = report.diagnostics.iter().map(|d| (d.code, d.line)).collect();
    assert_eq!(found, [("OD613", 7), ("OD613", 13), ("OD602", 19), ("OD602", 25)]);
}
