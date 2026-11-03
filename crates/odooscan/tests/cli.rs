use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn oscn(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oscn"))
        .args(args)
        .output()
        .expect("oscn runs")
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// `oscn check` in `dir`, with that directory as the home directory.
fn check_in(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_oscn"));
    command
        .arg("check")
        .args(args)
        .current_dir(dir)
        .env("HOME", dir)
        .env("USERPROFILE", dir)
        .env_remove("ODOOSCAN_ADDONS_PATH");
    command
}

fn assert_rejects(out: &Output, entry: &str) {
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("addons path") && stderr.contains(entry), "{stderr}");
}

#[test]
fn check_rejects_a_missing_addons_path() {
    let dir = scratch("addons_path_flag");
    let out = check_in(&dir, &["--addons-path", "missing"]).output().unwrap();
    assert_rejects(&out, "missing");
}

#[test]
fn check_rejects_a_missing_addons_path_from_the_environment() {
    let dir = scratch("addons_path_env");
    let out = check_in(&dir, &[])
        .env("ODOOSCAN_ADDONS_PATH", "missing")
        .output()
        .unwrap();
    assert_rejects(&out, "missing");
}

#[test]
fn check_rejects_a_missing_addons_path_from_the_file() {
    let dir = scratch("addons_path_file");
    fs::write(dir.join("odooscan.toml"), "addons-path = [\"missing\"]\n").unwrap();
    let out = check_in(&dir, &[]).output().unwrap();
    assert_rejects(&out, "missing");
}

#[test]
fn check_expands_a_tilde_after_a_comma() {
    let dir = scratch("addons_path_comma");
    fs::create_dir_all(dir.join("addons")).unwrap();
    let out = check_in(&dir, &["--addons-path", "addons,~/addons"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn check_expands_a_tilde_from_the_file() {
    let dir = scratch("addons_path_file_tilde");
    fs::create_dir_all(dir.join("addons")).unwrap();
    fs::write(dir.join("odooscan.toml"), "addons-path = [\"~/addons\"]\n").unwrap();
    let out = check_in(&dir, &[]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
}

/// The OD203 findings of a scan of `cycles/<module>`. A stack overflow aborts the process and would
/// take a test binary with it, so the scan is spawned: a signal gives no exit code.
fn cycle_paths(module: &str) -> Vec<String> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../odooscan_core/tests/fixtures");
    let target = format!("cycles/{module}");
    let out = check_in(&fixtures, &[&target, "--addons-path", "core"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| {
            line.split_once(": OD203 '")?
                .1
                .split_once('\'')
                .map(|(path, _)| path.to_owned())
        })
        .collect()
}

#[test]
fn a_shadowed_related_field_of_a_delegate_is_not_followed() {
    assert_eq!(cycle_paths("related_shadow"), ["size.name"]);
}

#[test]
fn two_fields_related_to_each_other_end() {
    assert!(cycle_paths("related_pair").is_empty());
}

#[test]
fn a_ring_of_related_fields_ends() {
    assert!(cycle_paths("related_ring").is_empty());
}

#[test]
fn a_related_field_through_a_delegation_ring_ends() {
    assert!(cycle_paths("delegate_ring").is_empty());
}

#[test]
fn a_long_related_chain_keeps_its_comodel() {
    assert_eq!(cycle_paths("related_chain"), ["end_id.missing", "twice_id.missing"]);
}

#[test]
fn a_ring_of_inherited_views_ends() {
    assert!(cycle_paths("views_ring").is_empty());
}

#[test]
fn explain_prints_the_page_of_a_code() {
    let out = oscn(&["explain", "OD203"]);
    assert_eq!(out.status.code(), Some(0));
    let page = String::from_utf8(out.stdout).unwrap();
    assert!(page.starts_with("# OD203 unresolved-field-path\n"), "{page}");
    assert!(page.contains("## Example"));
}

#[test]
fn explain_accepts_a_name() {
    let out = oscn(&["explain", "orphan-super"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .starts_with("# OD211 orphan-super\n")
    );
}

#[test]
fn explain_rejects_an_unknown_code() {
    let out = oscn(&["explain", "OD999"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("error: no rule is called 'OD999'")
    );
}

#[test]
fn explain_suggests_rules_sharing_a_prefix() {
    let out = oscn(&["explain", "od21"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("OD210") && stderr.contains("OD211"), "{stderr}");
}

/// `oscn check target/port17` against the fixture core, as of 18.0.
fn port(args: &[&str]) -> Output {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../odooscan_core/tests/fixtures");
    let mut all = vec!["target/port17", "--addons-path", "core", "--target-series", "18.0"];
    all.extend(args);
    check_in(&fixtures, &all).output().unwrap()
}

const PORT_SUMMARY: &str = "Findings by kind, checked as 18.0:
     1  removed API
     1  deprecated API
     1  missing upstream
     1  other
";

#[test]
fn a_target_series_summarizes_the_upgrade_after_the_findings() {
    let out = port(&[]);
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let (findings, summary) = stdout.split_once("Findings by kind").unwrap();
    assert!(
        findings.contains(": OD126 ") && findings.contains(": OD108 "),
        "{stdout}"
    );
    assert!(
        format!("Findings by kind{summary}").starts_with(PORT_SUMMARY),
        "{stdout}"
    );
    assert!(summary.contains("     1  OD126  python-api-removed\n"), "{stdout}");
}

#[test]
fn statistics_with_a_target_series_print_only_the_summary() {
    let out = port(&["--statistics"]);
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.starts_with(PORT_SUMMARY), "{stdout}");
    assert_eq!(stdout.matches("OD126").count(), 1, "{stdout}");
}

#[test]
fn a_target_series_summary_goes_to_stderr_beside_json() {
    let out = port(&["--format", "json"]);
    let findings: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(findings.as_array().map(Vec::len), Some(4));
    assert!(String::from_utf8(out.stderr).unwrap().contains(PORT_SUMMARY));
}

#[test]
fn a_target_series_conflicts_with_a_default_series() {
    let out = port(&["--odoo-version", "17.0"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("cannot be used with") && stderr.contains("--odoo-version"),
        "{stderr}"
    );
}

#[test]
fn a_checkout_of_another_series_gets_a_notice() {
    let dir = scratch("target_checkout");
    fs::create_dir_all(dir.join("odoo/odoo/addons/base")).unwrap();
    fs::write(
        dir.join("odoo/odoo/release.py"),
        "version_info = (17, 0, 0, FINAL, 0, '')\n",
    )
    .unwrap();
    let out = check_in(&dir, &["--addons-path", "odoo", "--target-series", "18.0"])
        .output()
        .unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("notice: odoo: the Odoo checkout is 17.0, not the target series 18.0"),
        "{stderr}"
    );
}
