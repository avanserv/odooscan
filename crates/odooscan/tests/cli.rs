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
