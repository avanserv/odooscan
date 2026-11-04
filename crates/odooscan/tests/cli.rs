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

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../odooscan_core/tests/fixtures")
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
    let target = format!("cycles/{module}");
    let out = check_in(&fixtures(), &[&target, "--addons-path", "core"])
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
    let mut all = vec!["target/port17", "--addons-path", "core", "--target-series", "18.0"];
    all.extend(args);
    check_in(&fixtures(), &all).output().unwrap()
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
    copy_dir(&fixtures().join("core"), &remote.join("odoo/addons"));
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
    let mut command = check_in(&fixtures(), &["target/port17", "--target-series", target]);
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
    let mut command = check_in(&fixtures(), &["target/port17", "--target-series", "18.0"]);
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

/// An executable script at `path` that runs `body`.
#[cfg(unix)]
fn script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Git runs an fsmonitor hook on every index refresh, which `core.hooksPath` does not cover.
#[cfg(unix)]
#[test]
fn a_fetch_runs_no_fsmonitor_hook() {
    let dir = scratch("target_fetch_fsmonitor");
    let remote = odoo_remote(&dir, "18.0", RELEASE_18);
    let ran = dir.join("ran");
    let hook = dir.join("fsmonitor");
    script(&hook, &format!("touch '{}'\nexit 1", ran.display()));
    let config = dir.join("gitconfig");
    fs::write(&config, format!("[core]\n\tfsmonitor = {}\n", hook.display())).unwrap();
    let mut command = port_fetched(&remote, &dir.join("cache"), "18.0");
    command
        .env("GIT_CONFIG_GLOBAL", &config)
        .env("GIT_CONFIG_NOSYSTEM", "1");
    let (code, stdout, stderr) = run(command);
    assert_summary(code, &stdout, &stderr);
    assert!(!ran.exists(), "{stderr}");
}

/// The arguments the `ssh` first on `PATH` and the configured ssh command get for an ssh remote,
/// both fakes that record them and fail, so that nothing leaves the machine.
#[cfg(unix)]
fn ssh_arguments(name: &str, configured: bool) -> (Option<String>, Option<String>) {
    let dir = scratch(name);
    let (fake, chosen) = (dir.join("fake"), dir.join("chosen"));
    let record = |to: &Path| format!("printf '%s\\n' \"$*\" >> '{}'\nexit 255", to.display());
    script(&dir.join("bin/ssh"), &record(&fake));
    script(&dir.join("record-ssh"), &record(&chosen));
    let config = dir.join("gitconfig");
    let text = format!("[core]\n\tsshCommand = {}\n", dir.join("record-ssh").display());
    fs::write(&config, if configured { text } else { String::new() }).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths([dir.join("bin")].into_iter().chain(std::env::split_paths(&path))).unwrap();
    let mut command = port_fetched(&dir.join("unused"), &dir.join("cache"), "18.0");
    command
        .env("ODOOSCAN_ODOO_REMOTE", "ssh://git@odooscan.invalid/odoo.git")
        .env("GIT_CONFIG_GLOBAL", &config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("PATH", path);
    for var in ["GIT_SSH", "GIT_SSH_COMMAND", "GIT_SSH_VARIANT"] {
        command.env_remove(var);
    }
    let (code, _, stderr) = run(command);
    assert_eq!(code, Some(2), "{stderr}");
    (fs::read_to_string(fake).ok(), fs::read_to_string(chosen).ok())
}

/// A configured ssh command is the user's choice, which the batch options must not replace.
#[cfg(unix)]
#[test]
fn an_ssh_command_the_user_configured_runs_as_given() {
    let (fake, chosen) = ssh_arguments("target_fetch_ssh_chosen", true);
    assert_eq!(fake, None);
    let chosen = chosen.expect("the configured ssh command runs");
    assert!(!chosen.contains("BatchMode"), "{chosen}");
    let (fake, chosen) = ssh_arguments("target_fetch_ssh_default", false);
    assert_eq!(chosen, None);
    let fake = fake.expect("the ssh on PATH runs");
    assert!(fake.contains("BatchMode=yes"), "{fake}");
}

/// The findings `--fix` rewrites in the fixture, by message, but those of `GUARDED`.
const FIXED: &[&str] = &[
    "@api.multi no longer exists",
    "group_operator is renamed",
    "self._cr is deprecated",
    "self._uid is deprecated",
    "self._context is deprecated",
    "sudo(user)",
    "track_visibility=",
    "the <tree> view is renamed",
    "view_mode 'tree' is renamed",
    "the server no longer knows t-esc",
];

/// The findings of `FIXED` that a view of their tree keeps unfixed, by their path and position.
const GUARDED: &[&str] = &[
    "custom/modern20/views/pages.xml:5:13: OD325",
    "custom/modern20/views/pages.xml:18:21: OD325",
    "custom/modern20/views/pages.xml:42:9: OD325",
    "custom/modern20/views/pages.xml:52:9: OD325",
    "custom/tree_paths/views/partner.xml:8:21: OD306",
    "custom/tree_paths/views/partner.xml:29:13: OD306",
    "custom/tree_paths/views/partner.xml:48:13: OD306",
];

#[test]
fn a_diff_writes_nothing_and_a_second_fix_changes_nothing() {
    let dir = scratch("fix_fixtures");
    copy_dir(&fixtures().join("custom"), &dir.join("custom"));
    copy_dir(&fixtures().join("core"), &dir.join("core"));
    let checked = ["custom", "--addons-path", "core"];
    let before = contents(&dir);
    let (code, diff, stderr) = run(check_in(&dir, &[&checked[..], &["--diff"]].concat()));
    assert_eq!(code, Some(1), "{stderr}");
    assert!(contents(&dir) == before, "--diff wrote");
    insta::assert_snapshot!(diff);

    let (code, _, stderr) = run(check_in(&dir, &[&checked[..], &["--fix"]].concat()));
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("fixed 20 diagnostics in 9 files"), "{stderr}");
    let fixed = contents(&dir);
    let (_, _, stderr) = run(check_in(&dir, &[&checked[..], &["--fix"]].concat()));
    assert!(stderr.contains("fixed 0 diagnostics in 0 files"), "{stderr}");
    assert!(contents(&dir) == fixed, "the second --fix wrote");

    let (_, stdout, _) = run(check_in(&dir, &checked));
    let left = stdout.lines().filter(|l| FIXED.iter().any(|m| l.contains(m)));
    let left: Vec<&str> = left.filter(|l| !GUARDED.iter().any(|g| l.contains(g))).collect();
    assert!(left.is_empty(), "{left:?}");
    for guarded in GUARDED {
        assert!(stdout.contains(guarded), "{guarded}: {stdout}");
    }
}

/// A module of `series` named `name` under `dir/mods`, holding `files` under `models/`.
fn module(dir: &Path, name: &str, series: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let root = dir.join("mods").join(name);
    fs::create_dir_all(root.join("models")).unwrap();
    let manifest = format!("{{\"name\": \"{name}\", \"version\": \"{series}.1.0.0\", \"depends\": [\"base\"]}}\n");
    fs::write(root.join("__manifest__.py"), manifest).unwrap();
    for (file, bytes) in files {
        fs::write(root.join("models").join(file), bytes).unwrap();
    }
    root.join("models")
}

/// Lists `files`, under `models/` of the module, in the manifest's data.
fn load(models: &Path, files: &[&str]) {
    let data: Vec<String> = files.iter().map(|f| format!("\"models/{f}\"")).collect();
    let manifest = models.parent().unwrap().join("__manifest__.py");
    let text = fs::read_to_string(&manifest).unwrap();
    let listed = format!("\"data\": [{}], \"depends\"", data.join(", "));
    fs::write(&manifest, text.replace("\"depends\"", &listed)).unwrap();
}

const AVERAGED: &str = "from odoo import fields, models\n\n\nclass X(models.Model):\n    _inherit = 'res.partner'\n\n    x = fields.Float(group_operator='avg')\n";

#[test]
fn a_fix_keeps_the_bom_and_the_crlf_line_ends() {
    let dir = scratch("fix_bytes");
    let src = "\u{feff}from odoo import api, fields, models\r\n\r\n\r\nclass X(models.Model):\r\n    _inherit = 'res.partner'\r\n\r\n    x = fields.Float(group_operator='avg')\r\n\r\n    @api.multi\r\n    def f(self):\r\n        return self\r\n";
    let models = module(&dir, "bytes", "18.0", &[("x.py", src.as_bytes())]);
    let (code, _, stderr) = run(check_in(&dir, &["mods", "--fix"]));
    assert_eq!(code, Some(0), "{stderr}");
    let expected = "\u{feff}from odoo import api, fields, models\r\n\r\n\r\nclass X(models.Model):\r\n    _inherit = 'res.partner'\r\n\r\n    x = fields.Float(aggregator='avg')\r\n\r\n    def f(self):\r\n        return self\r\n";
    assert_eq!(
        String::from_utf8(fs::read(models.join("x.py")).unwrap()).unwrap(),
        expected
    );
}

/// A list view of `id` in an `<odoo>` document, its lines ended by `eol`.
fn tree_view(id: &str, eol: &str) -> String {
    [
        "<odoo>",
        &format!("    <record id=\"{id}\" model=\"ir.ui.view\">"),
        "        <field name=\"model\">res.partner</field>",
        "        <field name=\"arch\" type=\"xml\"><tree string=\"NAME\"><field name=\"name\"/></tree></field>",
        "    </record>",
        "</odoo>",
        "",
    ]
    .join(eol)
}

#[test]
fn an_xml_fix_keeps_its_bytes_and_skips_a_text_that_is_not_them() {
    let dir = scratch("fix_xml_bytes");
    let latin1 = |id: &str, name: &[u8]| {
        let text = format!(
            "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>\n{}",
            tree_view(id, "\n")
        );
        let (head, tail) = text.split_once("NAME").unwrap();
        [head.as_bytes(), name, tail.as_bytes()].concat()
    };
    let accented = latin1("accented", b"R\xe9sum\xe9");
    let ascii = latin1("ascii", b"Resume");
    let crlf = format!("\u{feff}<?xml version=\"1.0\"?>\r\n{}", tree_view("crlf", "\r\n"));
    let files: [(&str, &[u8]); 3] = [
        ("accented.xml", &accented),
        ("ascii.xml", &ascii),
        ("crlf.xml", crlf.as_bytes()),
    ];
    let models = module(&dir, "xml_bytes", "18.0", &files);
    load(&models, &["accented.xml", "ascii.xml", "crlf.xml"]);
    let (code, _, stderr) = run(check_in(&dir, &["mods", "--fix"]));
    assert_eq!(code, Some(1), "{stderr}");
    // The accented file has no fix to drop as stale, rather than one that no longer matches.
    assert!(
        stderr.contains("fixed 2 diagnostics in 2 files") && !stderr.contains("not fixed"),
        "{stderr}"
    );
    let listed = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace("tree", "list").into_bytes();
    let read = |file: &str| fs::read(models.join(file)).unwrap();
    assert_eq!(read("accented.xml"), accented);
    assert_eq!(read("ascii.xml"), listed(&ascii));
    assert_eq!(read("crlf.xml"), listed(crlf.as_bytes()));
}

#[test]
fn baselined_and_suppressed_findings_stay_unfixed() {
    let dir = scratch("fix_hidden");
    let models = module(&dir, "hidden", "18.0", &[("baselined.py", AVERAGED.as_bytes())]);
    let (code, _, stderr) = run(check_in(&dir, &["mods", "--write-baseline"]));
    assert_eq!(code, Some(0), "{stderr}");
    let suppressed = AVERAGED.replace("'avg')", "'avg')  # odooscan: ignore[OD108]");
    fs::write(models.join("suppressed.py"), &suppressed).unwrap();
    fs::write(models.join("reported.py"), AVERAGED).unwrap();
    let baseline = "--baseline=odooscan-baseline.json";
    let (code, _, stderr) = run(check_in(&dir, &["mods", baseline, "--fix"]));
    assert_eq!(code, Some(0), "{stderr}");
    let read = |file: &str| fs::read_to_string(models.join(file)).unwrap();
    assert_eq!(read("baselined.py"), AVERAGED);
    assert_eq!(read("suppressed.py"), suppressed);
    assert_eq!(read("reported.py"), AVERAGED.replace("group_operator", "aggregator"));
    assert_eq!(stderr.matches("the baseline hid").count(), 1, "{stderr}");
    // Nothing is left to write, so the findings come from the one scan.
    let (code, _, stderr) = run(check_in(&dir, &["mods", baseline, "--fix"]));
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(stderr.matches("the baseline hid").count(), 1, "{stderr}");
}

#[test]
fn a_finding_of_the_target_series_only_is_fixed_under_it() {
    let dir = scratch("fix_target");
    let models = module(&dir, "port", "17.0", &[("x.py", AVERAGED.as_bytes())]);
    let core = fixtures().join("core");
    let core = core.to_str().unwrap();
    let (code, _, stderr) = run(check_in(&dir, &["mods", "--addons-path", core, "--fix"]));
    assert_eq!(
        (code, fs::read_to_string(models.join("x.py")).unwrap()),
        (Some(0), AVERAGED.to_owned()),
        "{stderr}"
    );
    let (code, _, stderr) = run(check_in(
        &dir,
        &["mods", "--addons-path", core, "--target-series", "18.0", "--fix"],
    ));
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        fs::read_to_string(models.join("x.py")).unwrap(),
        AVERAGED.replace("group_operator", "aggregator")
    );
}

#[test]
fn fix_and_diff_refuse_the_flags_they_cannot_honour() {
    let dir = scratch("fix_conflicts");
    module(&dir, "conflicts", "18.0", &[("x.py", AVERAGED.as_bytes())]);
    for flags in [
        &["--fix", "--diff"][..],
        &["--diff", "--format", "json"],
        &["--diff", "--statistics"],
        &["--fix", "--write-baseline"],
        &["--diff", "--write-baseline"],
    ] {
        let (code, _, stderr) = run(check_in(&dir, &[&["mods"][..], flags].concat()));
        assert_eq!(code, Some(2), "{flags:?}");
        assert!(stderr.contains("cannot be used with"), "{flags:?}: {stderr}");
    }
    assert_eq!(
        fs::read_to_string(dir.join("mods/conflicts/models/x.py")).unwrap(),
        AVERAGED
    );
    assert!(!dir.join("odooscan-baseline.json").exists());
}

/// Dropping `@api.multi` moves the lines below it, and `with_user` the columns after it.
const MOVED: &str = "from odoo import api, models\n\n\nclass X(models.Model):\n    _inherit = 'res.partner'\n\n    @api.multi\n    def f(self, q, user):\n        self._cr.execute('SELECT %s' % q)\n        return self.sudo(user).flush()\n";

#[test]
fn a_fix_reports_the_findings_left_where_they_now_are() {
    let dir = scratch("fix_positions");
    module(&dir, "moved", "19.0", &[("x.py", MOVED.as_bytes())]);
    let (code, fixed, stderr) = run(check_in(&dir, &["mods", "--fix"]));
    assert_eq!(code, Some(1), "{stderr}");
    assert!(fixed.contains("x.py:9:37: OD126 flush()"), "{fixed}");
    let (_, checked, _) = run(check_in(&dir, &["mods"]));
    assert_eq!(fixed, checked);
}

/// The names in `dir` but those of `kept`, to show that a fix left nothing beside the file.
fn left_beside(dir: &Path, kept: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| !kept.contains(&n.as_str()))
        .collect();
    names.sort();
    names
}

#[cfg(unix)]
#[test]
fn a_failed_write_exits_2_and_leaves_the_file_whole() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("fix_unwritable");
    let models = module(&dir, "unwritable", "18.0", &[("x.py", AVERAGED.as_bytes())]);
    // The file can be written, but nothing can be created beside it.
    fs::set_permissions(&models, fs::Permissions::from_mode(0o555)).unwrap();
    let (code, stdout, stderr) = run(check_in(&dir, &["mods", "--fix"]));
    fs::set_permissions(&models, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!((code, stdout.as_str()), (Some(2), ""), "{stderr}");
    assert!(stderr.contains("error: ") && stderr.contains("x.py: "), "{stderr}");
    assert_eq!(fs::read_to_string(models.join("x.py")).unwrap(), AVERAGED);
    assert_eq!(left_beside(&models, &["x.py"]), Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn a_fix_keeps_the_mode_and_leaves_nothing_beside_the_file() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("fix_mode");
    let models = module(&dir, "mode", "18.0", &[("x.py", AVERAGED.as_bytes())]);
    fs::set_permissions(models.join("x.py"), fs::Permissions::from_mode(0o640)).unwrap();
    let (code, _, stderr) = run(check_in(&dir, &["mods", "--fix"]));
    assert_eq!(code, Some(0), "{stderr}");
    let fixed = models.join("x.py");
    assert_eq!(
        fs::read_to_string(&fixed).unwrap(),
        AVERAGED.replace("group_operator", "aggregator")
    );
    assert_eq!(fs::metadata(&fixed).unwrap().permissions().mode() & 0o777, 0o640);
    assert_eq!(left_beside(&models, &["x.py"]), Vec::<String>::new());
}

/// A group of the user running the tests other than their primary one, if they are in one.
#[cfg(unix)]
fn other_group() -> Option<u32> {
    let id = |flag: &str| String::from_utf8(Command::new("id").arg(flag).output().unwrap().stdout).unwrap();
    let primary = id("-g");
    let other = id("-G").split_whitespace().find(|g| *g != primary.trim())?.to_owned();
    other.parse().ok()
}

#[cfg(unix)]
#[test]
fn a_fix_keeps_the_group_of_the_file() {
    use std::os::unix::fs::MetadataExt;
    let Some(gid) = other_group() else {
        eprintln!("skipped: the user running the tests is in no group but their primary one");
        return;
    };
    let dir = scratch("fix_group");
    let models = module(&dir, "group", "18.0", &[("x.py", AVERAGED.as_bytes())]);
    let file = models.join("x.py");
    std::os::unix::fs::chown(&file, None, Some(gid)).unwrap();
    let (code, _, stderr) = run(check_in(&dir, &["mods", "--fix"]));
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(fs::metadata(&file).unwrap().gid(), gid);
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        AVERAGED.replace("group_operator", "aggregator")
    );
    assert_eq!(left_beside(&models, &["x.py"]), Vec::<String>::new());
}

#[test]
fn a_read_only_file_is_left_alone_with_a_notice() {
    let dir = scratch("fix_read_only");
    let models = module(&dir, "read_only", "18.0", &[("x.py", AVERAGED.as_bytes())]);
    let file = models.join("x.py");
    let writable = fs::metadata(&file).unwrap().permissions();
    let mut read_only = writable.clone();
    read_only.set_readonly(true);
    fs::set_permissions(&file, read_only).unwrap();
    let (code, _, stderr) = run(check_in(&dir, &["mods", "--fix"]));
    let diffed = run(check_in(&dir, &["mods", "--diff"]));
    // Windows refuses to remove a read-only file, so the next run's scratch could not.
    fs::set_permissions(&file, writable).unwrap();
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("x.py is not writable; not fixed"), "{stderr}");
    assert_skipped_by_diff(diffed, "x.py is not writable; not fixed");
    assert_eq!(fs::read_to_string(&file).unwrap(), AVERAGED);
    assert_eq!(left_beside(&models, &["x.py"]), Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn a_file_with_other_hard_links_is_left_alone_with_a_notice() {
    let dir = scratch("fix_hard_link");
    let models = module(&dir, "hard_link", "18.0", &[("x.py", AVERAGED.as_bytes())]);
    fs::hard_link(models.join("x.py"), dir.join("other.py")).unwrap();
    let (code, _, stderr) = run(check_in(&dir, &["mods", "--fix"]));
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("x.py has other hard links; not fixed"), "{stderr}");
    assert_eq!(fs::read_to_string(models.join("x.py")).unwrap(), AVERAGED);
    assert_skipped_by_diff(
        run(check_in(&dir, &["mods", "--diff"])),
        "x.py has other hard links; not fixed",
    );
}

/// `--diff` left out the one file it checked, with the `notice` that `--fix` gives.
fn assert_skipped_by_diff((code, stdout, stderr): (Option<i32>, String, String), notice: &str) {
    assert_eq!((code, stdout.as_str()), (Some(1), ""), "{stderr}");
    assert!(stderr.contains(notice), "{stderr}");
    assert!(
        stderr.contains("1 of 1 diagnostics have a fix, in 1 files, 1 left out"),
        "{stderr}"
    );
}

#[test]
fn a_diff_of_nothing_reported_exits_0() {
    let dir = scratch("fix_clean");
    let clean = AVERAGED.replace("group_operator", "aggregator");
    module(&dir, "clean", "18.0", &[("x.py", clean.as_bytes())]);
    let (code, stdout, stderr) = run(check_in(&dir, &["mods", "--diff"]));
    assert_eq!((code, stdout.as_str()), (Some(0), ""), "{stderr}");
}

/// A view of `res.partner` holding `arch`, inheriting `parent` when one is given.
fn partner_view(id: &str, parent: Option<&str>, arch: &str) -> String {
    let inherit = parent.map(|p| format!("<field name=\"inherit_id\" ref=\"{p}\"/>"));
    let fields = format!(
        "<field name=\"model\">res.partner</field>{}",
        inherit.unwrap_or_default()
    );
    format!(
        "<odoo><record id=\"{id}\" model=\"ir.ui.view\">{fields}<field name=\"arch\" type=\"xml\">{arch}</field></record></odoo>\n"
    )
}

#[test]
fn a_tree_whose_inheritance_an_unchecked_module_shares_stays_unfixed() {
    let dir = scratch("fix_unchecked_tree");
    let list = partner_view("list", None, "<tree><field name=\"name\"/></tree>");
    let spec = partner_view(
        "spec",
        Some("other.list"),
        "<tree position=\"inside\"><field name=\"name\"/></tree>",
    );
    let own = module(
        &dir,
        "own",
        "18.0",
        &[("list.xml", list.as_bytes()), ("spec.xml", spec.as_bytes())],
    );
    load(&own, &["list.xml", "spec.xml"]);
    // Alone, the scan knows no other view of either tree.
    let (code, _, stderr) = run(check_in(&dir, &["mods", "--diff"]));
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("2 of 2 diagnostics have a fix"), "{stderr}");

    // Another module's spec finds own.list by its tag, and own.spec finds its <tree>.
    let other = module(&dir, "other", "18.0", &[]);
    let finder = partner_view(
        "finder",
        Some("own.list"),
        "<tree position=\"inside\"><field name=\"name\"/></tree>",
    );
    let found = partner_view("list", None, "<tree><field name=\"name\"/></tree>");
    fs::write(other.join("finder.xml"), finder).unwrap();
    fs::write(other.join("list.xml"), found).unwrap();
    load(&other, &["finder.xml", "list.xml"]);
    fs::create_dir_all(dir.join("addons")).unwrap();
    fs::rename(dir.join("mods/other"), dir.join("addons/other")).unwrap();
    let (code, _, stderr) = run(check_in(&dir, &["mods", "--addons-path", "addons", "--fix"]));
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("fixed 0 diagnostics in 0 files"), "{stderr}");
    assert_eq!(fs::read_to_string(own.join("list.xml")).unwrap(), list);
    assert_eq!(fs::read_to_string(own.join("spec.xml")).unwrap(), spec);
}

/// A `<tree>` spec on `own.list`.
fn tree_spec() -> String {
    partner_view(
        "spec",
        Some("own.list"),
        "<tree position=\"inside\"><field name=\"email\"/></tree>",
    )
}

/// A list view whose root is `root`, and `spec` on it, both in module `own` of 18.0.
fn tree_and_spec(dir: &Path, root: &str, spec: &[u8]) -> (PathBuf, String) {
    let list = partner_view("list", None, &format!("<{root}><field name=\"name\"/></{root}>"));
    let own = module(dir, "own", "18.0", &[("list.xml", list.as_bytes()), ("spec.xml", spec)]);
    load(&own, &["list.xml", "spec.xml"]);
    (own, list)
}

/// `--fix` in a fresh `tree_and_spec` changes neither file, `case` naming the run. With `baselined`,
/// `args` go to the `--write-baseline` run and `--fix` runs with `--baseline` only; without, `args`
/// go to `--fix`.
fn assert_tree_and_spec_unfixed(case: &str, spec: &[u8], args: &[&str], baselined: bool) {
    let dir = scratch(&format!("fix_tree_spec_{case}"));
    let (own, list) = tree_and_spec(&dir, "tree", spec);
    let mut fixed = vec!["mods", "--fix"];
    if baselined {
        let (code, _, stderr) = run(check_in(&dir, &[&["mods", "--write-baseline"], args].concat()));
        assert_eq!(code, Some(0), "{case}: {stderr}");
        fixed.push("--baseline=odooscan-baseline.json");
    } else {
        fixed.extend(args);
    }
    let (code, _, stderr) = run(check_in(&dir, &fixed));
    assert_eq!(code, Some(1), "{case}: {stderr}");
    assert!(stderr.contains("fixed 0 diagnostics in 0 files"), "{case}: {stderr}");
    assert_eq!(fs::read_to_string(own.join("list.xml")).unwrap(), list, "{case}");
    assert_eq!(fs::read(own.join("spec.xml")).unwrap(), spec, "{case}");
}

#[test]
fn a_tree_and_its_spec_stay_unfixed_when_either_one_is_left_out() {
    let spec = tree_spec();
    for (case, file) in [
        ("list", "mods/own/models/list.xml"),
        ("spec", "mods/own/models/spec.xml"),
    ] {
        let excluded = ["--exclude", file];
        assert_tree_and_spec_unfixed(&format!("baselined_{case}"), spec.as_bytes(), &excluded, true);
        assert_tree_and_spec_unfixed(&format!("excluded_{case}"), spec.as_bytes(), &excluded, false);
    }
    let suppressed = spec.replace("</odoo>", "</odoo><!-- odooscan: ignore[OD306] -->");
    assert_tree_and_spec_unfixed("suppressed", suppressed.as_bytes(), &[], false);
    // A Latin-1 byte keeps the decoded text from being the file's bytes, so its fixes are dropped.
    let latin1 = format!("<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>\n{spec}<!-- R\u{e9}sum\u{e9} -->\n");
    let latin1: Vec<u8> = latin1.chars().map(|c| u8::try_from(u32::from(c)).unwrap()).collect();
    assert_tree_and_spec_unfixed("undecodable", &latin1, &[], false);
}

#[test]
fn a_tree_spec_over_a_list_is_fixed() {
    let dir = scratch("fix_tree_spec_list");
    let spec = tree_spec();
    let (own, list) = tree_and_spec(&dir, "list", spec.as_bytes());
    let (code, _, stderr) = run(check_in(&dir, &["mods", "--fix"]));
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(fs::read_to_string(own.join("list.xml")).unwrap(), list);
    assert_eq!(
        fs::read_to_string(own.join("spec.xml")).unwrap(),
        spec.replace("tree", "list")
    );
}

/// Whether a line of `stdout` reports `code` at `place`, a path and line.
fn reports(stdout: &str, place: &str, code: &str) -> bool {
    stdout.lines().any(|l| l.contains(place) && l.contains(code))
}

#[test]
fn a_fix_keeps_what_a_child_view_finds_or_sets_by_the_old_name() {
    let dir = scratch("fix_old_names");
    let parent =
        |id: &str| format!("<odoo><template id=\"{id}\"><span class=\"{id}\" t-esc=\"a\"/></template></odoo>\n");
    let child = |id: &str, parent: &str, spec: &str| {
        format!(
            "<odoo><template id=\"{id}\" inherit_id=\"{parent}\"><xpath expr=\"//span[hasclass('{parent}')]\" position=\"attributes\">{spec}</xpath></template></odoo>\n"
        )
    };
    let views = [
        ("label.xml", parent("label")),
        (
            "coded.xml",
            child("coded", "label", "<attribute name=\"t-esc\">b</attribute>"),
        ),
        ("badge.xml", parent("badge")),
        (
            "fielded.xml",
            child("fielded", "badge", "<attribute name=\"t-field\">b</attribute>"),
        ),
        ("tag.xml", parent("tag")),
        (
            "joined.xml",
            child("joined", "tag", "<attribute name=\"t-out\" add=\"b\"/>"),
        ),
    ];
    let files: Vec<(&str, &[u8])> = views.iter().map(|(f, v)| (*f, v.as_bytes())).collect();
    let pages = module(&dir, "pages", "20.0", &files);
    load(&pages, &views.each_ref().map(|(f, _)| *f));
    let list = partner_view("list", None, "<tree><field name=\"name\"/></tree>");
    let named = partner_view(
        "named",
        Some("list"),
        "<xpath expr=\"/*[local-name()='tree']\" position=\"inside\"><field name=\"email\"/></xpath>",
    );
    let rows = module(
        &dir,
        "rows",
        "18.0",
        &[("list.xml", list.as_bytes()), ("named.xml", named.as_bytes())],
    );
    load(&rows, &["list.xml", "named.xml"]);
    let (code, stdout, stderr) = run(check_in(&dir, &["mods", "--fix"]));
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("fixed 0 diagnostics in 0 files"), "{stderr}");
    for place in [
        "pages/models/label.xml:1:",
        "pages/models/badge.xml:1:",
        "pages/models/tag.xml:1:",
    ] {
        assert!(reports(&stdout, place, "OD325"), "{place}: {stdout}");
    }
    assert!(reports(&stdout, "rows/models/list.xml:1:", "OD306"), "{stdout}");
    let read = |models: &Path, file: &str| fs::read_to_string(models.join(file)).unwrap();
    for (file, view) in &views {
        assert_eq!(&read(&pages, file), view, "{file}");
    }
    assert_eq!((read(&rows, "list.xml"), read(&rows, "named.xml")), (list, named));
}
