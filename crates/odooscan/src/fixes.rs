//! `check --fix` and `check --diff`: the rewrites that the reported findings carry, file by file.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use odooscan_core::Diagnostic;
use odooscan_core::fix::{self, Fix, Outcome};

/// A file as the fixes of its findings rewrite it.
struct Rewrite<'d> {
    path: &'d Path,
    before: Vec<u8>,
    after: Vec<u8>,
    /// The findings whose fix the rewrite applies.
    fixed: Vec<&'d Diagnostic>,
}

fn rewrites<'d>(diags: &[&'d Diagnostic], rel: &dyn Fn(&Path) -> String) -> Vec<Rewrite<'d>> {
    let mut by_file: BTreeMap<&Path, Vec<&Diagnostic>> = BTreeMap::new();
    for d in diags.iter().filter(|d| d.fix.is_some()) {
        by_file.entry(&d.path).or_default().push(d);
    }
    by_file
        .into_iter()
        .filter_map(|(path, found)| rewrite(path, found, rel))
        .collect()
}

/// The file at `path` with the fixes of `found` applied in the order of their positions. A file
/// that no longer holds what the scan read is left alone whole, since the other fixes were made
/// against the same text.
fn rewrite<'d>(path: &'d Path, found: Vec<&'d Diagnostic>, rel: &dyn Fn(&Path) -> String) -> Option<Rewrite<'d>> {
    let before = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("notice: {}: {e}; not fixed", rel(path));
            return None;
        }
    };
    let fixes: Vec<&Fix> = found.iter().filter_map(|d| d.fix.as_ref()).collect();
    let (after, outcomes) = fix::apply(&before, &fixes);
    if outcomes.contains(&Outcome::Mismatch) {
        eprintln!("notice: {} changed since it was checked; not fixed", rel(path));
        return None;
    }
    let fixed = found
        .into_iter()
        .zip(outcomes)
        .filter(|(_, o)| *o == Outcome::Applied)
        .map(|(d, _)| d)
        .collect();
    Some(Rewrite {
        path,
        before,
        after,
        fixed,
    })
}

/// Prints the rewrites of the fixes of `diags` as unified diffs, and writes nothing. Exits 1 when
/// anything is reported, as a check does, so that a diff that leaves findings fails.
pub fn diff(diags: &[&Diagnostic], rel: &dyn Fn(&Path) -> String) -> ExitCode {
    let rewrites = rewrites(diags, rel);
    let fixable: usize = rewrites.iter().map(|r| r.fixed.len()).sum();
    let files = rewrites.len();
    let (shown, failed) = foreseen(rewrites, rel);
    let mut stdout = io::stdout().lock();
    for r in &shown {
        let name = rel(r.path);
        let (before, after) = (String::from_utf8_lossy(&r.before), String::from_utf8_lossy(&r.after));
        let text = similar::TextDiff::from_lines(&before, &after)
            .unified_diff()
            .header(&format!("a/{name}"), &format!("b/{name}"))
            .to_string();
        let _ = stdout.write_all(text.as_bytes());
    }
    let left = match files - shown.len() {
        0 => String::new(),
        n => format!(", {n} left out"),
    };
    eprintln!(
        "{fixable} of {} diagnostics have a fix, in {files} files{left}",
        diags.len()
    );
    if failed {
        ExitCode::from(2)
    } else if diags.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// The rewrites that `--fix` would write, as far as the metadata of their files tells without
/// opening them for write, which could wake a file watcher, and whether one failed as `--fix`
/// would. The others get the notice or the error that `write` gives.
fn foreseen<'d>(rewrites: Vec<Rewrite<'d>>, rel: &dyn Fn(&Path) -> String) -> (Vec<Rewrite<'d>>, bool) {
    let (mut shown, mut failed) = (Vec::new(), false);
    for r in rewrites {
        match target(r.path).map(|(_, original)| kept_unopened(&original)) {
            Ok(None) => shown.push(r),
            Ok(Some(why)) => eprintln!("notice: {} {why}; not fixed", rel(r.path)),
            Err(e) => {
                eprintln!("error: {}: {e}", rel(r.path));
                failed = true;
            }
        }
    }
    (shown, failed)
}

/// Writes each file that the fixes of `diags` rewrite, once with every fix of its findings. Returns
/// whether a file was written, since the findings left in it then moved, or status 2 when a write
/// failed, once every file was tried.
pub fn write(diags: &[&Diagnostic], rel: &dyn Fn(&Path) -> String) -> Result<bool, ExitCode> {
    let mut failed = false;
    let (mut files, mut fixed) = (0, 0);
    for r in rewrites(diags, rel) {
        match replace(r.path, &r.after) {
            Ok(None) => {
                files += 1;
                fixed += r.fixed.len();
            }
            Ok(Some(why)) => eprintln!("notice: {} {why}; not fixed", rel(r.path)),
            Err(e) => {
                eprintln!("error: {}: {e}", rel(r.path));
                failed = true;
            }
        }
    }
    eprintln!("fixed {fixed} diagnostics in {files} files");
    if failed { Err(ExitCode::from(2)) } else { Ok(files > 0) }
}

/// Writes `bytes` to a new file beside the one `path` names, then renames it over that one, so that
/// a full disk or an interrupt leaves the original whole. The file keeps its mode and its group,
/// and a link its target. Returns why the file was left alone, when it was.
fn replace(path: &Path, bytes: &[u8]) -> io::Result<Option<&'static str>> {
    let (path, original) = target(path)?;
    if let Some(why) = kept(&path, &original) {
        return Ok(Some(why));
    }
    let (temp, file) = create_beside(&path)?;
    put(&path, &temp, file, &original, bytes)
}

/// The file a link at `path` ends at, which the rewrite replaces, and its metadata.
fn target(path: &Path) -> io::Result<(PathBuf, Metadata)> {
    let path = path.canonicalize()?;
    let original = std::fs::metadata(&path)?;
    Ok((path, original))
}

/// Fills `file`, the new file `temp` names, and renames it over `path`. Removes it when it is left
/// unused, by a skip or a failure.
fn put(path: &Path, temp: &Path, file: File, original: &Metadata, bytes: &[u8]) -> io::Result<Option<&'static str>> {
    let done = fill(file, original, bytes).and_then(|why| match why {
        None => std::fs::rename(temp, path).map(|()| None),
        why => Ok(why),
    });
    if !matches!(done, Ok(None)) {
        let _ = std::fs::remove_file(temp);
    }
    done
}

/// Why `--fix` leaves the file at `path` alone before writing anything: replacing a file that the
/// user running the check cannot write would bypass its mode, and replacing one with other hard links would split it from
/// them. Skipping a read-only file also spares the cleanup a read-only temporary file, which
/// Windows refuses to remove.
fn kept(path: &Path, original: &Metadata) -> Option<&'static str> {
    if OpenOptions::new().write(true).open(path).is_err() {
        return Some("is not writable");
    }
    linked(original)
}

/// What `kept` says, from the mode and the link count of `original` alone. Neither tells who runs
/// the check nor what else refuses the write, so the diff still shows, for example, a file that
/// others may write but not the user, or that an ACL or a read-only mount protects, and leaves out
/// a read-only file even for root, which `--fix` run as root writes.
fn kept_unopened(original: &Metadata) -> Option<&'static str> {
    if original.permissions().readonly() {
        return Some("is not writable");
    }
    linked(original)
}

#[cfg(unix)]
fn linked(original: &Metadata) -> Option<&'static str> {
    use std::os::unix::fs::MetadataExt;
    (original.nlink() > 1).then_some("has other hard links")
}

#[cfg(not(unix))]
fn linked(_: &Metadata) -> Option<&'static str> {
    None
}

/// A new file beside `path`, under a name that no file there has: opening an existing one would
/// write through a link planted at that name, or clobber a file of the user's, and two runs at once
/// would share it.
fn create_beside(path: &Path) -> io::Result<(PathBuf, File)> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let salt = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    for _ in 0..100 {
        let mut name = OsString::from(".");
        name.push(path.file_name().unwrap_or_default());
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        name.push(format!(".oscn-fix-{}-{salt:x}-{n}", std::process::id()));
        let temp = path.with_file_name(name);
        if let Some(file) = create_new(&temp)? {
            return Ok((temp, file));
        }
    }
    Err(io::Error::from(io::ErrorKind::AlreadyExists))
}

/// The file `temp` names, created empty, or `None` when a file or a link is there already. On Unix
/// only its user may open it until `fill` gives it the group and the mode of the original: a file
/// made under the umask could be opened by that group, or anyone, who would keep the descriptor.
fn create_new(temp: &Path) -> io::Result<Option<File>> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    match options.open(temp) {
        Ok(file) => Ok(Some(file)),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(None),
        Err(e) => Err(e),
    }
}

/// Writes `bytes` to the new file, which only its user may open, with the group and the mode of
/// `original` set first so that the text is never readable beyond what the original allows. Returns why the rewrite must not replace
/// the original, when it must not. The file is closed on return, which Windows needs before the
/// rename. The mode comes after the group, since a change of group clears the set-id bits.
fn fill(mut file: File, original: &Metadata, bytes: &[u8]) -> io::Result<Option<&'static str>> {
    let made = file.metadata()?;
    if let Some(why) = owned_apart(original, &made) {
        return Ok(Some(why));
    }
    if let Some(why) = regroup(&file, &made, original)? {
        return Ok(Some(why));
    }
    file.set_permissions(original.permissions())?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(None)
}

/// The rename would hand a file of another user to the one running the fix.
#[cfg(unix)]
fn owned_apart(original: &Metadata, temp: &Metadata) -> Option<&'static str> {
    use std::os::unix::fs::MetadataExt;
    (original.uid() != temp.uid()).then_some("belongs to another user")
}

#[cfg(not(unix))]
fn owned_apart(_: &Metadata, _: &Metadata) -> Option<&'static str> {
    None
}

/// Gives `file` the group of `original`, since a new file takes the user's primary group, or the
/// directory's under setgid or on BSD and macOS. The user may give it only a group they are in, so
/// a file of another group is left alone rather than handed over.
#[cfg(unix)]
fn regroup(file: &File, made: &Metadata, original: &Metadata) -> io::Result<Option<&'static str>> {
    use std::os::unix::fs::MetadataExt;
    if made.gid() == original.gid() {
        return Ok(None);
    }
    match std::os::unix::fs::fchown(file, None, Some(original.gid())) {
        Ok(()) => Ok(None),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Ok(Some("belongs to a group its owner is not in")),
        Err(e) => Err(e),
    }
}

#[cfg(not(unix))]
fn regroup(_: &File, _: &Metadata, _: &Metadata) -> io::Result<Option<&'static str>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use odooscan_core::fix::Edit;
    use odooscan_core::rules::Severity;

    use super::*;

    fn found(path: &Path, start: usize, expected: &str) -> Diagnostic {
        Diagnostic {
            code: "OD108",
            severity: Severity::Warning,
            path: path.to_path_buf(),
            line: 1,
            column: 1,
            message: String::new(),
            flow: vec![],
            fix: Some(Fix::single(Edit {
                start,
                end: start + expected.len(),
                expected: expected.to_owned(),
                replacement: "x".to_owned(),
            })),
        }
    }

    /// An empty directory of the test `name`, apart from other runs and the other tests.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oscn-fixes-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_file_that_changed_since_the_scan_is_left_whole() {
        let dir = scratch("changed");
        let path = dir.join("x.py");
        std::fs::write(&path, "a = 1\nb = 2\n").unwrap();
        let (fresh, stale) = (found(&path, 0, "a"), found(&path, 6, "c"));
        let done = write(&[&fresh, &stale], &|p| p.display().to_string());
        let left = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!((done, left.as_str()), (Ok(false), "a = 1\nb = 2\n"));
    }

    /// The scan never reads a file through a link, but a path given to `replace` may be one.
    #[cfg(unix)]
    #[test]
    fn a_linked_file_keeps_its_link_and_its_target_is_rewritten() {
        let dir = scratch("linked");
        let (target, link) = (dir.join("target.py"), dir.join("link.py"));
        std::fs::write(&target, "a = 1\n").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let replaced = replace(&link, b"b = 2\n").unwrap();
        let read = (
            std::fs::read_link(&link).unwrap(),
            std::fs::read_to_string(&target).unwrap(),
        );
        let names = names(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!((replaced, read), (None, (target, "b = 2\n".to_owned())));
        assert_eq!(names, ["link.py", "target.py"]);
    }

    /// The names in `dir`, sorted.
    #[cfg(unix)]
    fn names(dir: &Path) -> Vec<OsString> {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        names
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_rename_leaves_no_temporary_file() {
        let dir = scratch("rename");
        let path = dir.join("x.py");
        std::fs::create_dir(&path).unwrap();
        let original = std::fs::metadata(&path).unwrap();
        let (temp, file) = create_beside(&path).unwrap();
        let put = put(&path, &temp, file, &original, b"a = 1\n");
        let left = names(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(put.is_err(), "{put:?}");
        assert_eq!(left, ["x.py"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_of_another_user_is_left_alone() {
        use std::os::unix::fs::MetadataExt;
        let root = std::fs::metadata("/").unwrap();
        let dir = scratch("owner");
        let path = dir.join("x.py");
        std::fs::write(&path, "a = 1\n").unwrap();
        let (temp, file) = create_beside(&path).unwrap();
        if file.metadata().unwrap().uid() == root.uid() {
            let _ = std::fs::remove_dir_all(&dir);
            eprintln!("skipped: the test runs as the owner of /, so no other user owns it");
            return;
        }
        let put = put(&path, &temp, file, &root, b"b = 2\n");
        let (text, left) = (std::fs::read_to_string(&path).unwrap(), names(&dir));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(put.unwrap(), Some("belongs to another user"));
        assert_eq!((text.as_str(), left), ("a = 1\n", vec![OsString::from("x.py")]));
    }

    #[cfg(unix)]
    #[test]
    fn a_temporary_file_is_created_for_its_user_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("private");
        let made = create_new(&dir.join("temp")).unwrap().unwrap();
        let mode = made.metadata().unwrap().permissions().mode();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(mode & 0o077, 0, "{mode:o}");
    }

    #[test]
    fn a_diff_reports_a_file_it_cannot_resolve_as_a_failure() {
        let dir = scratch("unresolved");
        let gone = dir.join("gone.py");
        let rewrite = Rewrite {
            path: &gone,
            before: vec![],
            after: vec![],
            fixed: vec![],
        };
        let (shown, failed) = foreseen(vec![rewrite], &|p| p.display().to_string());
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!((shown.len(), failed), (0, true));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_or_a_file_at_the_temporary_name_is_not_opened() {
        let dir = scratch("taken");
        let (target, planted, kept) = (dir.join("bashrc"), dir.join("planted"), dir.join("kept"));
        std::fs::write(&target, "export A=1\n").unwrap();
        std::os::unix::fs::symlink(&target, &planted).unwrap();
        std::fs::write(&kept, "mine\n").unwrap();
        let opened = (
            create_new(&planted).unwrap().is_some(),
            create_new(&kept).unwrap().is_some(),
        );
        let read = (
            std::fs::read_to_string(&target).unwrap(),
            std::fs::read_to_string(&kept).unwrap(),
        );
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            (opened, read.0.as_str(), read.1.as_str()),
            ((false, false), "export A=1\n", "mine\n")
        );
    }
}
