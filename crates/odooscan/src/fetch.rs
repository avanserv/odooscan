//! The Odoo core of a target series, fetched from odoo/odoo with git when the scan is given none.
//!
//! The cache holds `core/<commit>`, a sparse checkout of the modules and `odoo/release.py`, and
//! `core/heads/<branch>`, the commit last fetched for the branch and the one before it. Each run
//! resolves the branch head, so a moved branch is fetched again. The commit before stays for one
//! more update, since a run that resolved it earlier may still be reading it.

use std::hash::{BuildHasher, Hasher};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use odooscan_core::series::Series;

const DEFAULT_REMOTE: &str = "https://github.com/odoo/odoo";

/// What `make oracle-fetch` checks out but the assets it adds for OD513, which checks only the
/// checked modules' own, plus the file that names the checkout's series.
const SPARSE: &[&str] = &[
    "/addons/",
    "/odoo/addons/",
    "!**/i18n/",
    "!**/static/**",
    "**/static/src/**/*.js",
    "**/static/src/**/*.xml",
    "!**/tests/",
    "**/static/tests/**/*.js",
    "/odoo/release.py",
];

/// The variables `git rev-parse --local-env-vars` lists. A git hook exports some of them, and they
/// override `-C`, so left set they would make every step act on the caller's repository.
const LOCAL_ENV: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
];

/// An HTTP transfer below 1 kB/s for 30 s is given up, so that a network dropping packets falls
/// back to the cached core rather than hanging the scan.
const STALL: &[&str] = &["-c", "http.lowSpeedLimit=1000", "-c", "http.lowSpeedTime=30"];

/// BatchMode fails on a passphrase or host key prompt that nobody may see. A host that does not
/// answer is given up after 30 s, and a connection silent for about 45 s is dropped, so that a
/// stalled ssh transfer ends as a stalled HTTP one does.
const SSH: &str = "ssh -o BatchMode=yes -o ConnectTimeout=30 -o ServerAliveInterval=15 -o ServerAliveCountMax=2";

/// How long a step may run before it is killed. The settings above bound a stalled transfer, not
/// every wait: curl gives a connection 300 s, and a proxy may hold one open for good.
const ANSWER: Duration = Duration::from_secs(60);

/// The deadline of the fetch and the checkout, which download the core and so may run long on a
/// slow but live connection.
const TRANSFER: Duration = Duration::from_secs(30 * 60);

const POLL: Duration = Duration::from_millis(10);

/// How long the pipes of a step that exited are still read: a helper it started may hold them open
/// for as long as it runs.
const LINGER: Duration = Duration::from_secs(2);

/// How long a temporary or unrecorded entry stays in the cache: far longer than the deadlines let a
/// fetch run, so that a sweep never takes one from under a concurrent run.
const STALE: Duration = Duration::from_secs(24 * 60 * 60);

/// The odoo/odoo branch of a series: `18.0`, or `saas-18.1` for a SaaS release.
pub fn branch(series: Series) -> String {
    if series.minor == 0 {
        series.to_string()
    } else {
        format!("saas-{series}")
    }
}

/// The series as a user names it: `18.0`, or `saas~18.1`.
fn name(series: Series) -> String {
    if series.minor == 0 {
        series.to_string()
    } else {
        format!("saas~{series}")
    }
}

/// The odooscan cache directory, from the environment `var` reads and the home directory.
pub fn cache_root_from(var: impl Fn(&str) -> Option<String>, home: Option<PathBuf>) -> Option<PathBuf> {
    let set = |name: &str| var(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(dir) = set("ODOOSCAN_CACHE_DIR") {
        return Some(dir);
    }
    // The XDG specification has a relative value ignored.
    if let Some(dir) = set("XDG_CACHE_HOME").filter(|d| d.is_absolute()) {
        return Some(dir.join("odooscan"));
    }
    if cfg!(windows)
        && let Some(dir) = set("LOCALAPPDATA")
    {
        return Some(dir.join("odooscan"));
    }
    home.map(|h| h.join(".cache/odooscan"))
}

/// The remote `ODOOSCAN_ODOO_REMOTE` names, or why it cannot be one. A path is made absolute,
/// since `ls-remote` and `fetch` run in different directories.
fn remote_from(var: Option<String>) -> Result<String, String> {
    let Some(remote) = var.filter(|v| !v.is_empty()) else {
        return Ok(DEFAULT_REMOTE.to_owned());
    };
    if remote.starts_with('-') {
        return Err("ODOOSCAN_ODOO_REMOTE starts with '-'".to_owned());
    }
    let path = Path::new(&remote);
    Ok(match std::path::absolute(path) {
        Ok(absolute) if path.exists() => absolute.display().to_string(),
        _ => remote,
    })
}

/// The core checkout of `series`, fetched unless the cache holds the branch head. Offline, the
/// commit last fetched for the branch stands in for the head.
pub fn core(series: Series) -> Result<PathBuf, String> {
    let var = std::env::var("ODOOSCAN_ODOO_REMOTE").ok();
    let branch = branch(series);
    let series = name(series);
    let fail = |remote: &str, why: &str| {
        format!(
            "could not fetch the {series} core of {remote} ({why}); pass --addons-path with an Odoo checkout of {series}"
        )
    };
    let remote = remote_from(var.clone()).map_err(|why| fail(var.as_deref().unwrap_or_default(), &why))?;
    let root = cache_root_from(|n| std::env::var(n).ok(), std::env::home_dir())
        .ok_or_else(|| fail(&remote, "no cache directory; set ODOOSCAN_CACHE_DIR"))?;
    let cache = Cache {
        core: std::path::absolute(&root).unwrap_or(root).join("core"),
        branch,
        series: &series,
        remote: &remote,
    };
    let recorded = cache.recorded();
    let failure = match cache.update(recorded.as_deref()) {
        Ok(dir) => return Ok(dir),
        Err(failure) => failure,
    };
    match recorded.filter(|sha| cache.core.join(sha).is_dir()) {
        Some(sha) => {
            eprintln!(
                "notice: {}; using the cached {series} core at {}",
                failure.cause(&remote),
                &sha[..12]
            );
            Ok(cache.core.join(sha))
        }
        None => Err(fail(&remote, failure.why())),
    }
}

/// Why the cache does not hold the branch head: the remote did not answer, or anything else.
#[derive(Debug)]
enum Failure {
    Unreachable(String),
    Other(String),
}

impl Failure {
    fn why(&self) -> &str {
        match self {
            Self::Unreachable(why) | Self::Other(why) => why,
        }
    }

    fn cause(&self, remote: &str) -> String {
        match self {
            Self::Unreachable(why) => format!("could not reach {remote} ({why})"),
            Self::Other(why) => why.clone(),
        }
    }

    fn io(path: &Path) -> impl Fn(std::io::Error) -> Self {
        move |e| Self::Other(format!("{}: {e}", path.display()))
    }
}

struct Cache<'a> {
    core: PathBuf,
    branch: String,
    series: &'a str,
    remote: &'a str,
}

impl Cache<'_> {
    fn heads(&self) -> PathBuf {
        self.core.join("heads")
    }

    fn head_file(&self) -> PathBuf {
        self.heads().join(&self.branch)
    }

    fn recorded(&self) -> Option<String> {
        commits_in(&self.head_file()).into_iter().next()
    }

    /// The checkout of the branch head, fetched when missing, then recorded as the branch's.
    fn update(&self, recorded: Option<&str>) -> Result<PathBuf, Failure> {
        std::fs::create_dir_all(&self.core).map_err(Failure::io(&self.core))?;
        let sha = self.head()?;
        let dir = self.core.join(&sha);
        if dir.is_dir() {
            // Until `record` names it, a concurrent sweep spares the directory only for being new.
            touch(&dir);
        } else {
            self.fetch(&sha)?;
        }
        if recorded != Some(sha.as_str()) {
            let dropped = commits_in(&self.head_file()).into_iter().nth(1);
            self.record(&sha, recorded)?;
            if let Some(old) = dropped.filter(|old| *old != sha && !self.named().contains(old)) {
                let _ = std::fs::remove_dir_all(self.core.join(old));
            }
        }
        self.sweep(&sha, SystemTime::now());
        Ok(dir)
    }

    fn head(&self) -> Result<String, Failure> {
        let refname = format!("refs/heads/{}", self.branch);
        let out = git(&self.core, &["ls-remote", "--", self.remote, &refname])?;
        let sha = out.split_whitespace().next().unwrap_or_default();
        if sha.is_empty() {
            return Err(Failure::Other(format!("{} has no branch {}", self.remote, self.branch)));
        }
        // The commit names a directory, so nothing but a hash may come from the remote.
        is_commit(sha)
            .then(|| sha.to_owned())
            .ok_or_else(|| Failure::Other(format!("unexpected ls-remote output '{sha}'")))
    }

    /// Fetches `sha` beside its directory and moves it in once complete, so that an interrupted or
    /// concurrent run never leaves a partial checkout under the commit's name.
    fn fetch(&self, sha: &str) -> Result<(), Failure> {
        let dir = self.core.join(sha);
        eprintln!(
            "notice: fetching the {} core of {} at {} into {}",
            self.series,
            self.remote,
            &sha[..12],
            dir.display()
        );
        let partial = claim(&self.core, (0..8).map(|_| temporary("")))?;
        let fetched = checkout(&partial, self.remote, sha);
        let moved = fetched.and_then(|()| std::fs::rename(&partial, &dir).map_err(Failure::io(&dir)));
        if moved.is_err() {
            let _ = std::fs::remove_dir_all(&partial);
        }
        match moved {
            Err(_) if dir.is_dir() => Ok(()),
            other => other,
        }
    }

    /// Writes the branch's commit and the one before it through a rename, so a reader never sees
    /// half of the file.
    fn record(&self, sha: &str, previous: Option<&str>) -> Result<(), Failure> {
        let file = self.head_file();
        let partial = self.heads().join(temporary(&format!("{}-", self.branch)));
        let text = previous.map_or_else(|| format!("{sha}\n"), |old| format!("{sha}\n{old}\n"));
        std::fs::create_dir_all(self.heads())
            .and_then(|()| std::fs::File::create_new(&partial))
            .and_then(|mut f| f.write_all(text.as_bytes()))
            .and_then(|()| std::fs::rename(&partial, &file))
            .map_err(Failure::io(&file))
    }

    /// Every commit a branch record names. A record still being written is left out: the run
    /// writing it fetched or touched the commit's directory, which the sweep spares while new.
    fn named(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.heads()) else {
            return vec![];
        };
        entries
            .flatten()
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .flat_map(|e| commits_in(&e.path()))
            .collect()
    }

    /// Removes what an interrupted run or a failed removal left behind: temporary directories and
    /// records, and commit directories that no branch records and that this run did not resolve.
    /// Only those untouched for a day go, which no live run, of whatever process id, reaches.
    fn sweep(&self, resolved: &str, now: SystemTime) {
        let named = self.named();
        let unrecorded = |name: &str| is_commit(name) && name != resolved && !named.iter().any(|n| n == name);
        remove_stale(&self.core, now, |name| name.starts_with('.') || unrecorded(name));
        remove_stale(&self.heads(), now, |name| name.starts_with('.'));
    }
}

fn remove_stale(dir: &Path, now: SystemTime, doomed: impl Fn(&str) -> bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if !doomed(&entry.file_name().to_string_lossy()) || !is_stale(&entry, now) {
            continue;
        }
        let _ = if entry.file_type().is_ok_and(|t| t.is_dir()) {
            std::fs::remove_dir_all(entry.path())
        } else {
            std::fs::remove_file(entry.path())
        };
    }
}

fn commits_in(file: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    text.lines()
        .map(str::trim)
        .filter(|l| is_commit(l))
        .map(str::to_owned)
        .collect()
}

fn is_stale(entry: &std::fs::DirEntry, now: SystemTime) -> bool {
    let modified = entry.metadata().and_then(|m| m.modified());
    modified.is_ok_and(|t| now.duration_since(t).is_ok_and(|age| age > STALE))
}

/// Moves the modification time of `dir` by adding and removing an entry, which works on every
/// platform: setting it directly needs a handle Windows gives a directory only on request.
fn touch(dir: &Path) {
    let mark = dir.join(temporary(""));
    let _ = std::fs::File::create_new(&mark).and_then(|_| std::fs::remove_file(&mark));
}

/// A name no other run draws: the process id alone repeats across containers sharing the cache.
/// It stays short, which keeps the paths git checks out on Windows under 260 characters.
fn temporary(prefix: &str) -> String {
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    format!(".{prefix}{}-{:08x}", std::process::id(), hasher.finish() as u32)
}

/// The first of `names` that `create_dir` makes in `core`, so that no two runs share one.
fn claim(core: &Path, names: impl IntoIterator<Item = String>) -> Result<PathBuf, Failure> {
    let mut taken = Failure::Other(format!("no temporary directory in {}", core.display()));
    for name in names {
        let dir = core.join(name);
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => taken = Failure::io(&dir)(e),
            Err(e) => return Err(Failure::io(&dir)(e)),
        }
    }
    Err(taken)
}

/// Checks out the resolved commit rather than `FETCH_HEAD`, and makes sure of it, so that the tree
/// matches the directory name it moves to. The sparse patterns are written to the file
/// `sparse-checkout set --no-cone` would write, since that flag needs Git 2.35.
fn checkout(dir: &Path, remote: &str, sha: &str) -> Result<(), Failure> {
    let info = dir.join(".git/info");
    // No template, so the repository has no hooks of its own; `command` points `core.hooksPath` there.
    git(dir, &["init", "-q", "--template="])?;
    git(
        dir,
        &["fetch", "-q", "--depth", "1", "--filter=blob:none", "--", remote, sha],
    )?;
    git(dir, &["config", "core.sparseCheckout", "true"])?;
    let patterns = info.join("sparse-checkout");
    std::fs::create_dir_all(&info)
        .and_then(|()| std::fs::write(&patterns, SPARSE.join("\n") + "\n"))
        .map_err(Failure::io(&patterns))?;
    git(dir, &["checkout", "-q", sha])?;
    let head = git(dir, &["rev-parse", "HEAD"])?;
    if head.trim() != sha {
        return Err(Failure::Other(format!(
            "{} holds {} rather than {sha}",
            dir.display(),
            head.trim()
        )));
    }
    Ok(())
}

/// The standard output of git run in `dir`, or the error its standard error gives. Only `ls-remote`
/// writes nothing, so only its failure is the remote's: a fetch or a checkout also fails on a full
/// disk or a lock, and git translates the messages that would tell them apart.
fn git(dir: &Path, args: &[&str]) -> Result<String, Failure> {
    let step = args[0];
    let fail = |why: String| match step {
        "ls-remote" => Failure::Unreachable(why),
        _ => Failure::Other(format!("git {step} failed: {why}")),
    };
    let limit = if matches!(step, "fetch" | "checkout") {
        TRANSFER
    } else {
        ANSWER
    };
    let out = output_within(command(dir).args(args), limit)
        .map_err(|e| Failure::Other(format!("git could not run ({e})")))?
        .ok_or_else(|| fail(format!("ran for over {} s", limit.as_secs())))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    Err(fail(error_of(&out)))
}

/// The first fatal line of standard error says why; the last is often advice such as "and the
/// repository exists.".
fn error_of(out: &Output) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let fatal = stderr.lines().find_map(|l| l.strip_prefix("fatal: "));
    let why = fatal.or_else(|| stderr.lines().rev().map(str::trim).find(|l| !l.is_empty()));
    why.map_or_else(|| format!("exited with {}", out.status), str::to_owned)
}

/// The output of `command`, or `None` once it has run longer than `limit` and is killed with every
/// process it started. The pipes are read on threads, so that a full one never blocks the child,
/// and for at most `LINGER` after it exits, since a helper it started may hold them open past it.
fn output_within(command: &mut Command, limit: Duration) -> std::io::Result<Option<Output>> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let pipes = [drain(child.stdout.take()), drain(child.stderr.take())];
    let Some(status) = wait_within(&mut child, limit)? else {
        return Ok(None);
    };
    let closed = Instant::now() + LINGER;
    while pipes.iter().any(|(reader, _)| !reader.is_finished()) && Instant::now() < closed {
        std::thread::sleep(POLL);
    }
    let [stdout, stderr] = pipes.map(|(_, bytes)| std::mem::take(&mut *lock(&bytes)));
    Ok(Some(Output { status, stdout, stderr }))
}

fn wait_within(child: &mut Child, limit: Duration) -> std::io::Result<Option<ExitStatus>> {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if start.elapsed() > limit {
            kill_tree(child.id());
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(POLL);
    }
}

type Sink = Arc<Mutex<Vec<u8>>>;

/// The bytes read so far stay with the caller, who may stop waiting before the pipe closes.
fn drain(pipe: Option<impl Read + Send + 'static>) -> (JoinHandle<()>, Sink) {
    let sink = Sink::default();
    let bytes = Arc::clone(&sink);
    let reader = std::thread::spawn(move || {
        let Some(mut pipe) = pipe else {
            return;
        };
        let mut chunk = [0; 8192];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => lock(&bytes).extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    });
    (reader, sink)
}

fn lock(sink: &Sink) -> std::sync::MutexGuard<'_, Vec<u8>> {
    sink.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Stops `root`, then round by round the processes the stopped ones started, so that none forks
/// past the kill. A stopped parent reaps no child, so an id found stays its process's until the
/// kill. The rounds end when none is new, or after 8 against a tree that keeps growing.
#[cfg(unix)]
fn kill_tree(root: u32) {
    let mut tree = vec![root];
    let mut new = tree.clone();
    for _ in 0..8 {
        signal("STOP", &new);
        new = processes()
            .into_iter()
            .filter(|(pid, parent)| tree.contains(parent) && !tree.contains(pid))
            .map(|(pid, _)| pid)
            .collect();
        if new.is_empty() {
            break;
        }
        tree.extend(&new);
    }
    signal("KILL", &tree);
}

/// Sends `name` through the shell's `kill`, since a slim image may have no `kill` program.
#[cfg(unix)]
fn signal(name: &str, pids: &[u32]) {
    let _ = Command::new("sh")
        .arg("-c")
        .arg(format!("kill -s {name} -- \"$@\""))
        .arg("sh")
        .args(pids.iter().map(u32::to_string))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Every process id with its parent's.
#[cfg(target_os = "linux")]
fn processes() -> Vec<(u32, u32)> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return vec![];
    };
    entries
        .flatten()
        .filter_map(|e| {
            let pid = e.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(e.path().join("stat")).ok()?;
            Some((pid, parent_in(&stat)?))
        })
        .collect()
}

/// The parent id in a `/proc/<pid>/stat`. The command name before it is in parentheses and may
/// hold any character, so the fields count from the last `)`.
#[cfg(target_os = "linux")]
fn parent_in(stat: &str) -> Option<u32> {
    let (_, fields) = stat.rsplit_once(')')?;
    fields.split_whitespace().nth(1)?.parse().ok()
}

/// Every process id with its parent's.
#[cfg(all(unix, not(target_os = "linux")))]
fn processes() -> Vec<(u32, u32)> {
    let out = Command::new("ps")
        .args(["-A", "-o", "pid=", "-o", "ppid="])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let text = out.map_or_else(|_| String::new(), |o| String::from_utf8_lossy(&o.stdout).into_owned());
    text.lines()
        .filter_map(|l| {
            let mut ids = l.split_whitespace().map(str::parse);
            Some((ids.next()?.ok()?, ids.next()?.ok()?))
        })
        .collect()
}

#[cfg(windows)]
fn kill_tree(root: u32) {
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID"])
        .arg(root.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Git in `dir` alone: no repository above it, none the caller's environment names, and none of
/// the user's hooks, the fsmonitor one included, which `core.hooksPath` does not cover. A remote that asks for credentials fails rather than waiting on a prompt
/// nobody may see.
fn command(dir: &Path) -> Command {
    let mut command = isolated(dir);
    if !ssh_chosen(dir) {
        command.env("GIT_SSH_COMMAND", SSH);
    }
    let hooks = dir.join(".git/hooks");
    command
        .args(STALL)
        .args(["-c", "core.fsmonitor=false"])
        .arg("-c")
        .arg(format!("core.hooksPath={}", hooks.display()));
    command
}

fn isolated(dir: &Path) -> Command {
    let mut command = Command::new("git");
    for var in LOCAL_ENV {
        command.env_remove(var);
    }
    if let Some(parent) = dir.parent() {
        command.env("GIT_CEILING_DIRECTORIES", parent);
    }
    command.env("GIT_TERMINAL_PROMPT", "0").arg("-C").arg(dir);
    command
}

/// Whether the user chose how git runs ssh, a choice the batch options must not replace.
fn ssh_chosen(dir: &Path) -> bool {
    static CHOSEN: OnceLock<bool> = OnceLock::new();
    *CHOSEN.get_or_init(|| {
        let config = || isolated(dir).args(["config", "--get", "core.sshCommand"]).output();
        ["GIT_SSH_COMMAND", "GIT_SSH"]
            .iter()
            .any(|v| std::env::var_os(v).is_some())
            || config().is_ok_and(|out| out.status.success())
    })
}

fn is_commit(text: &str) -> bool {
    text.len() >= 40 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stable_series_has_its_own_branch_and_a_saas_one_is_prefixed() {
        assert_eq!(branch(Series::parse("18.0").unwrap()), "18.0");
        assert_eq!(branch(Series::parse("saas~18.1").unwrap()), "saas-18.1");
        assert_eq!(name(Series::parse("saas~18.1").unwrap()), "saas~18.1");
    }

    #[test]
    fn the_cache_root_follows_the_environment() {
        let home = Some(PathBuf::from("/home/u"));
        let root = |vars: &[(&str, &str)]| {
            let vars: Vec<(String, String)> = vars.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
            cache_root_from(
                |n| vars.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone()),
                home.clone(),
            )
        };
        let own = [("ODOOSCAN_CACHE_DIR", "c"), ("XDG_CACHE_HOME", "/x")];
        assert_eq!(root(&own), Some(PathBuf::from("c")));
        assert_eq!(root(&[("XDG_CACHE_HOME", "/x")]), Some(PathBuf::from("/x/odooscan")));
        let fallback = PathBuf::from("/home/u/.cache/odooscan");
        assert_eq!(
            root(&[("XDG_CACHE_HOME", "x"), ("ODOOSCAN_CACHE_DIR", "")]),
            Some(fallback.clone())
        );
        let local = if cfg!(windows) {
            PathBuf::from("L/odooscan")
        } else {
            fallback
        };
        assert_eq!(root(&[("LOCALAPPDATA", "L")]), Some(local));
        assert_eq!(cache_root_from(|_| None, None), None);
    }

    #[test]
    fn a_remote_is_a_url_or_an_absolute_path() {
        let default = Ok(DEFAULT_REMOTE.to_owned());
        assert_eq!(remote_from(None), default);
        assert_eq!(remote_from(Some(String::new())), default);
        let url = "https://example.com/odoo";
        assert_eq!(remote_from(Some(url.to_owned())), Ok(url.to_owned()));
        assert!(remote_from(Some("--upload-pack=touch x".to_owned())).is_err());
        let here = std::path::absolute(".").unwrap().display().to_string();
        assert_eq!(remote_from(Some(".".to_owned())), Ok(here));
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn cache_in(core: &Path) -> Cache<'static> {
        let _ = std::fs::remove_dir_all(core);
        std::fs::create_dir_all(core).unwrap();
        Cache {
            core: core.to_owned(),
            branch: "18.0".to_owned(),
            series: "18.0",
            remote: DEFAULT_REMOTE,
        }
    }

    /// A record left between its write and its rename names a commit, yet keeps it no longer than
    /// any other leftover.
    #[test]
    fn a_sweep_removes_only_stale_leftovers() {
        let core = std::env::temp_dir().join(format!("odooscan-sweep-{}", std::process::id()));
        let commit = |c: char| c.to_string().repeat(40);
        let cache = cache_in(&core);
        for dir in [".1".to_owned(), commit('a'), commit('b'), commit('c'), commit('d')] {
            std::fs::create_dir_all(core.join(dir)).unwrap();
        }
        cache.record(&commit('a'), Some(&commit('b'))).unwrap();
        std::fs::write(core.join("heads/.18.0-1-0"), commit('d')).unwrap();
        cache.sweep(&commit('c'), SystemTime::now());
        assert!(core.join(".1").is_dir() && core.join(commit('d')).is_dir());
        cache.sweep(&commit('c'), SystemTime::now() + 2 * STALE);
        assert_eq!(
            names_in(&core),
            [commit('a'), commit('b'), commit('c'), "heads".to_owned()]
        );
        assert_eq!(names_in(&core.join("heads")), ["18.0"]);
        let _ = std::fs::remove_dir_all(core);
    }

    /// A repository in `root` with one commit holding `odoo/release.py`, and that commit.
    fn remote_in(root: &Path) -> (String, String) {
        let _ = std::fs::remove_dir_all(root);
        let remote = root.join("odoo.git");
        std::fs::create_dir_all(remote.join("odoo")).unwrap();
        std::fs::write(
            remote.join("odoo/release.py"),
            "version_info = (18, 0, 0, FINAL, 0, '')\n",
        )
        .unwrap();
        let user = ["-c", "user.name=odooscan", "-c", "user.email=odooscan@example.com"];
        git(&remote, &["init", "-q", "--template="]).unwrap();
        git(&remote, &["add", "-A"]).unwrap();
        git(
            &remote,
            &[&user[..], &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "core"]].concat(),
        )
        .unwrap();
        let sha = git(&remote, &["rev-parse", "HEAD"]).unwrap().trim().to_owned();
        (remote.display().to_string(), sha)
    }

    /// Another container may run a fetch under the same process id, so its directory is no leftover.
    #[test]
    fn a_fetch_neither_takes_nor_uses_a_foreign_temporary_directory() {
        let root = std::env::temp_dir().join(format!("odooscan-claim-{}", std::process::id()));
        let (remote, sha) = remote_in(&root);
        let core = root.join("core");
        let mut cache = cache_in(&core);
        cache.remote = &remote;
        let foreign = format!(".{}", std::process::id());
        std::fs::create_dir(core.join(&foreign)).unwrap();
        std::fs::write(core.join(&foreign).join("mark"), "foreign").unwrap();
        cache.fetch(&sha).unwrap();
        assert!(core.join(&sha).join("odoo/release.py").is_file());
        assert_eq!(names_in(&core), [foreign.clone(), sha]);
        assert_eq!(names_in(&core.join(&foreign)), ["mark"]);
        let taken = claim(&core, [foreign.clone()]);
        assert!(matches!(taken, Err(Failure::Other(why)) if why.contains(&foreign)));
        let claimed = claim(&core, [foreign.clone(), ".2".to_owned()]).unwrap();
        assert_eq!(claimed, core.join(".2"));
        assert_eq!(
            std::fs::read_to_string(core.join(&foreign).join("mark")).unwrap(),
            "foreign"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn a_step_past_its_deadline_is_killed() {
        let start = Instant::now();
        let out = output_within(Command::new("sleep").arg("5"), Duration::from_millis(100)).unwrap();
        assert!(out.is_none() && start.elapsed() < Duration::from_secs(4));
        let out = output_within(Command::new("git").arg("--version"), ANSWER)
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).starts_with("git version"));
    }

    #[test]
    fn a_failed_checkout_is_not_blamed_on_the_remote() {
        let root = std::env::temp_dir().join(format!("odooscan-lock-{}", std::process::id()));
        let (remote, sha) = remote_in(&root);
        let dir = root.join("checkout");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git/index.lock"), "").unwrap();
        let failure = checkout(&dir, &remote, &sha).unwrap_err();
        let cause = failure.cause(&remote);
        assert!(matches!(failure, Failure::Other(_)), "{cause}");
        assert!(
            cause.starts_with("git checkout failed: ") && !cause.contains("could not reach"),
            "{cause}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// Without the tree kill, the subshell outlives its killed parent and writes the mark.
    #[cfg(unix)]
    #[test]
    fn a_step_past_its_deadline_is_killed_with_what_it_started() {
        let mark = std::env::temp_dir().join(format!("odooscan-tree-{}", std::process::id()));
        let _ = std::fs::remove_file(&mark);
        let script = format!("(sleep 1; touch '{}') & wait", mark.display());
        let out = output_within(Command::new("sh").args(["-c", &script]), Duration::from_millis(200)).unwrap();
        assert!(out.is_none());
        std::thread::sleep(Duration::from_millis(1500));
        assert!(!mark.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_helper_holding_the_pipes_open_delays_the_output_by_at_most_the_linger() {
        let start = Instant::now();
        let out = output_within(Command::new("sh").args(["-c", "sleep 30 & echo done"]), ANSWER)
            .unwrap()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "done\n");
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_parent_id_follows_the_last_parenthesis_of_a_stat() {
        assert_eq!(parent_in("12 (a) b) S 7 7 1 0"), Some(7));
        assert_eq!(parent_in("12 a S 7"), None);
    }
}
