use std::path::PathBuf;

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
