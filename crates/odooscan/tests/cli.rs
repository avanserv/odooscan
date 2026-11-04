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

/// `oscn check` in `dir`, with that directory as the home directory. A core it would fetch comes
/// from a remote that does not exist, so no test reaches the network, and the cache it would fill
/// gets no more than an empty `core/`.
fn check_in(dir: &Path, args: &[&str]) -> Command {
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_oscn"));
    command
        .arg("check")
        .args(args)
        .current_dir(dir)
        .env("HOME", dir)
        .env("USERPROFILE", dir)
        .env("ODOOSCAN_ODOO_REMOTE", file_url(&tmp.join("no-remote")))
        .env("ODOOSCAN_CACHE_DIR", tmp.join("no-cache"))
        .env_remove("XDG_CACHE_HOME")
        .env_remove("ODOOSCAN_ADDONS_PATH");
    command
}

fn file_url(path: &Path) -> String {
    let path = path.display().to_string().replace('\\', "/");
    if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    }
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

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=odooscan", "-c", "user.email=odooscan@example.com"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A scratch odoo/odoo: one commit on `branch` holding the fixture core and a `release.py`.
fn odoo_remote(dir: &Path, branch: &str, version_info: &str) -> PathBuf {
    let remote = dir.join("odoo.git");
    fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "-q", "-b", branch]);
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../odooscan_core/tests/fixtures");
    copy_dir(&fixtures.join("core"), &remote.join("odoo/addons"));
    // The sparse patterns leave tests out.
    fs::create_dir_all(remote.join("odoo/addons/base/tests")).unwrap();
    fs::write(remote.join("odoo/addons/base/tests/__init__.py"), "").unwrap();
    fs::write(
        remote.join("odoo/release.py"),
        format!("version_info = {version_info}\n"),
    )
    .unwrap();
    git(&remote, &["add", "-A"]);
    git(&remote, &["commit", "-q", "-m", "core"]);
    remote
}

/// `oscn check target/port17 --target-series <target>` with no core given, fetching from `remote`.
fn port_fetched(remote: &Path, cache: &Path, target: &str) -> Command {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../odooscan_core/tests/fixtures");
    let mut command = check_in(&fixtures, &["target/port17", "--target-series", target]);
    command
        .env("ODOOSCAN_ODOO_REMOTE", file_url(remote))
        .env("ODOOSCAN_CACHE_DIR", cache);
    command
}

/// The fetched commit holds the modules and `release.py`, without tests.
fn assert_fetched(cache: &Path, sha: &str) {
    let dir = cache.join("core").join(sha);
    assert!(dir.join("odoo/addons/base").is_dir() && dir.join("odoo/release.py").is_file());
    assert!(!dir.join("odoo/addons/base/tests").exists());
}

fn run(mut command: Command) -> (Option<i32>, String, String) {
    let out = command.output().unwrap();
    let text = |b: Vec<u8>| String::from_utf8(b).unwrap();
    (out.status.code(), text(out.stdout), text(out.stderr))
}

const RELEASE_18: &str = "(18, 0, 0, FINAL, 0, '')";

fn assert_summary(code: Option<i32>, stdout: &str, stderr: &str) {
    assert_eq!(code, Some(1), "{stderr}");
    let summary = stdout.split_once("\n\n").map_or(stdout, |(_, s)| s);
    assert!(summary.starts_with(PORT_SUMMARY), "{stdout}\n{stderr}");
}

#[test]
fn a_target_series_without_a_core_fetches_it_once() {
    let dir = scratch("target_fetch");
    let remote = odoo_remote(&dir, "18.0", RELEASE_18);
    let sha = git(&remote, &["rev-parse", "HEAD"]);
    let cache = dir.join("cache");
    let (code, stdout, stderr) = run(port_fetched(&remote, &cache, "18.0"));
    assert_summary(code, &stdout, &stderr);
    assert!(stderr.contains("notice: fetching the 18.0 core of file://"), "{stderr}");
    assert!(stderr.contains(&format!(" at {} into ", &sha[..12])), "{stderr}");
    assert!(!stderr.contains("the Odoo checkout is"), "{stderr}");
    assert_fetched(&cache, &sha);
    let (code, stdout, stderr) = run(port_fetched(&remote, &cache, "18.0"));
    assert_summary(code, &stdout, &stderr);
    assert!(!stderr.contains("fetching"), "{stderr}");
}

#[test]
fn a_moved_branch_keeps_the_previous_core_for_one_update() {
    let dir = scratch("target_fetch_moved");
    let remote = odoo_remote(&dir, "18.0", RELEASE_18);
    let cache = dir.join("cache");
    let mut commits = vec![git(&remote, &["rev-parse", "HEAD"])];
    run(port_fetched(&remote, &cache, "18.0"));
    for _ in 0..2 {
        git(&remote, &["commit", "-q", "--allow-empty", "-m", "moved"]);
        commits.push(git(&remote, &["rev-parse", "HEAD"]));
        let (code, stdout, stderr) = run(port_fetched(&remote, &cache, "18.0"));
        assert_summary(code, &stdout, &stderr);
        assert!(
            stderr.contains(&format!(" at {} into ", &commits[commits.len() - 1][..12])),
            "{stderr}"
        );
    }
    let core = cache.join("core");
    assert!(!core.join(&commits[0]).exists());
    assert!(core.join(&commits[1]).is_dir() && core.join(&commits[2]).is_dir());
    let record = fs::read_to_string(core.join("heads/18.0")).unwrap();
    assert_eq!(record, format!("{}\n{}\n", commits[2], commits[1]));
}

#[test]
fn an_unreachable_remote_falls_back_to_the_cached_core() {
    let dir = scratch("target_fetch_offline");
    let remote = odoo_remote(&dir, "18.0", RELEASE_18);
    let sha = git(&remote, &["rev-parse", "HEAD"]);
    let cache = dir.join("cache");
    run(port_fetched(&remote, &cache, "18.0"));
    let (code, stdout, stderr) = run(port_fetched(&dir.join("gone.git"), &cache, "18.0"));
    assert_summary(code, &stdout, &stderr);
    let fallback = format!("; using the cached 18.0 core at {}\n", &sha[..12]);
    assert!(
        stderr.contains("notice: could not reach file://") && stderr.contains(&fallback),
        "{stderr}"
    );
}

#[test]
fn a_remote_without_the_branch_falls_back_to_the_cached_core() {
    let dir = scratch("target_fetch_no_branch");
    let remote = odoo_remote(&dir, "18.0", RELEASE_18);
    let sha = git(&remote, &["rev-parse", "HEAD"]);
    let cache = dir.join("cache");
    run(port_fetched(&remote, &cache, "18.0"));
    git(&remote, &["branch", "-q", "-m", "other"]);
    let (code, stdout, stderr) = run(port_fetched(&remote, &cache, "18.0"));
    assert_summary(code, &stdout, &stderr);
    let notice = format!(" has no branch 18.0; using the cached 18.0 core at {}\n", &sha[..12]);
    assert!(stderr.contains(&notice), "{stderr}");
    assert!(!stderr.contains("could not reach"), "{stderr}");
}

#[test]
fn an_unreachable_remote_without_a_cached_core_is_an_error() {
    let dir = scratch("target_fetch_none");
    let (code, stdout, stderr) = run(port_fetched(&dir.join("gone.git"), &dir.join("cache"), "18.0"));
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.starts_with("error: could not fetch the 18.0 core of file://"),
        "{stderr}"
    );
    assert!(stderr.contains("pass --addons-path"), "{stderr}");
}

#[test]
fn a_saas_target_fetches_its_saas_branch() {
    let dir = scratch("target_fetch_saas");
    let remote = odoo_remote(&dir, "saas-18.1", "('saas~18', 1, 0, FINAL, 0, '')");
    let sha = git(&remote, &["rev-parse", "HEAD"]);
    let cache = dir.join("cache");
    let (code, _, stderr) = run(port_fetched(&remote, &cache, "saas~18.1"));
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("notice: fetching the saas~18.1 core of file://"),
        "{stderr}"
    );
    assert!(!stderr.contains("the Odoo checkout is"), "{stderr}");
    assert_fetched(&cache, &sha);
}

/// Every file under `dir` with its contents, to show that nothing wrote there.
fn contents(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut all = vec![];
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            all.extend(contents(&path));
        } else {
            all.push((path.clone(), fs::read(&path).unwrap()));
        }
    }
    all.sort();
    all
}

/// A git hook exports `GIT_DIR` and `GIT_INDEX_FILE`, which would win over the cache's own repository.
#[test]
fn a_fetch_from_a_git_hook_leaves_the_callers_repository_alone() {
    let dir = scratch("target_fetch_hook");
    let remote = odoo_remote(&dir, "18.0", RELEASE_18);
    let decoy = dir.join("decoy");
    fs::create_dir_all(&decoy).unwrap();
    git(&decoy, &["init", "-q"]);
    let before = contents(&decoy);
    let index = decoy.join(".git/hook-index");
    let mut command = port_fetched(&remote, &dir.join("cache"), "18.0");
    command.env("GIT_DIR", decoy.join(".git")).env("GIT_INDEX_FILE", &index);
    let (code, stdout, stderr) = run(command);
    assert_summary(code, &stdout, &stderr);
    assert!(!index.exists(), "{stderr}");
    assert_eq!(contents(&decoy), before, "{stderr}");
}

/// Without a ceiling, `ls-remote` in the cache would read the enclosing repository's configuration.
#[test]
fn a_cache_inside_a_repository_ignores_its_configuration() {
    let dir = scratch("target_fetch_ceiling");
    let remote = odoo_remote(&dir, "18.0", RELEASE_18);
    let outer = dir.join("outer");
    fs::create_dir_all(&outer).unwrap();
    git(&outer, &["init", "-q"]);
    let rewrite = format!("url.{}.insteadOf", file_url(&dir.join("gone.git")));
    git(&outer, &["config", &rewrite, &file_url(&remote)]);
    let cache = outer.join("cache");
    let (code, stdout, stderr) = run(port_fetched(&remote, &cache, "18.0"));
    assert_summary(code, &stdout, &stderr);
    assert_fetched(&cache, &git(&remote, &["rev-parse", "HEAD"]));
}

#[test]
fn a_remote_starting_with_a_dash_is_an_error() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../odooscan_core/tests/fixtures");
    let mut command = check_in(&fixtures, &["target/port17", "--target-series", "18.0"]);
    command.env("ODOOSCAN_ODOO_REMOTE", "--upload-pack=touch x");
    let (code, _, stderr) = run(command);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("ODOOSCAN_ODOO_REMOTE starts with '-'"), "{stderr}");
    assert!(stderr.contains("pass --addons-path"), "{stderr}");
}

/// A user's template and global hooks would otherwise run on the cache's checkout.
#[cfg(unix)]
#[test]
fn a_fetch_runs_none_of_the_users_hooks() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("target_fetch_user_hooks");
    let remote = odoo_remote(&dir, "18.0", RELEASE_18);
    let ran = dir.join("ran");
    let global = dir.join("hooks");
    let template = dir.join("template");
    for hooks in [global.clone(), template.join("hooks")] {
        fs::create_dir_all(&hooks).unwrap();
        let hook = hooks.join("post-checkout");
        fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", ran.display())).unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let config = dir.join("gitconfig");
    let text = format!(
        "[core]\n\thooksPath = {}\n[init]\n\ttemplateDir = {}\n",
        global.display(),
        template.display()
    );
    fs::write(&config, text).unwrap();
    let mut command = port_fetched(&remote, &dir.join("cache"), "18.0");
    command.env("GIT_CONFIG_GLOBAL", &config);
    let (code, stdout, stderr) = run(command);
    assert_summary(code, &stdout, &stderr);
    assert!(!ran.exists(), "{stderr}");
}
