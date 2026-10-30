use std::path::PathBuf;

use odooscan_core::rules::Severity;
use odooscan_core::series::Series;
use odooscan_core::{ScanOptions, scan};

#[test]
fn fixtures() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let report = scan(&ScanOptions {
        paths: vec![root.join("custom")],
        addons_paths: vec![root.join("core")],
        default_series: None,
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
fn series_notice_only_without_default() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let notices = |default_series| {
        scan(&ScanOptions {
            paths: vec![root.join("core")],
            addons_paths: vec![],
            default_series,
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
fn default_series_reads_a_seriesless_auto_install_list() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    // Before saas~12.4 the list waits on every depend, and web_sticker also depends on web_glue.
    let reported = |default_series| {
        scan(&ScanOptions {
            paths: vec![root.join("custom")],
            addons_paths: vec![root.join("core")],
            default_series,
        })
        .diagnostics
        .iter()
        .any(|d| d.code == "OD302" && d.message.contains("'web_sticker.sticker'"))
    };
    assert!(!reported(None));
    assert!(reported(Series::parse("12.0")));
}

#[test]
fn unassigned_compute_waits_for_a_known_series() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let reported = |default_series| {
        scan(&ScanOptions {
            paths: vec![root.join("custom")],
            addons_paths: vec![root.join("core")],
            default_series,
        })
        .diagnostics
        .iter()
        .any(|d| d.code == "OD223" && d.path.ends_with("unversioned_logs/models/probes.py"))
    };
    assert!(!reported(None));
    assert!(reported(Series::parse("14.0")));
}

#[test]
fn delegate_without_ondelete_waits_for_a_known_series() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let code = |default_series| {
        scan(&ScanOptions {
            paths: vec![root.join("custom")],
            addons_paths: vec![root.join("core")],
            default_series,
        })
        .diagnostics
        .iter()
        .find(|d| d.path.ends_with("unversioned_logs/models/probes.py") && d.line == 59)
        .map(|d| d.code)
    };
    assert_eq!(code(None), Some("OD218"));
    assert_eq!(code(Series::parse("18.0")), Some("OD228"));
}

#[test]
fn company_check_severity_follows_the_module_series() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let report = scan(&ScanOptions {
        paths: vec![root.join("custom")],
        addons_paths: vec![root.join("core")],
        default_series: None,
    });
    let mut found: Vec<(String, Severity)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "OD222")
        .map(|d| {
            let module = d.path.strip_prefix(root.join("custom")).unwrap().iter().next().unwrap();
            (module.to_string_lossy().into_owned(), d.severity)
        })
        .collect();
    found.dedup();
    let expected = [
        ("field_options", Severity::Error),
        ("legacy13", Severity::Warning),
        ("legacy16", Severity::Warning),
        ("legacy17", Severity::Error),
        ("saas125", Severity::Warning),
    ];
    assert_eq!(found, expected.map(|(m, s)| (m.to_owned(), s)));
}

#[test]
fn a_row_with_no_group_needs_no_base() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let report = scan(&ScanOptions {
        paths: vec![root.join("custom/access_rows")],
        addons_paths: vec![],
        default_series: None,
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
fn refused_server_code_needs_no_base() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let report = scan(&ScanOptions {
        paths: vec![root.join("custom/server_code")],
        addons_paths: vec![],
        default_series: None,
    });
    assert!(report.diagnostics.iter().any(|d| d.code == "OD319"));
}

#[test]
fn import_patches_without_base_take_the_advice_of_the_path() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let report = scan(&ScanOptions {
        paths: vec![root.join("custom")],
        addons_paths: vec![],
        default_series: None,
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
