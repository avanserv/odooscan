use std::path::PathBuf;

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
