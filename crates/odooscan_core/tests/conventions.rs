//! Conventions of the repository's own text that no linter knows: a cited Odoo path names a
//! series the `Makefile` pins, so whoever checks the claim has that checkout in `.oracle/`, and a
//! series is spelled as Odoo's `release.py` spells it, since `saas-X.Y` names the git branch.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use ignore::WalkBuilder;
use regex::Regex;

/// Odoo's majors run from 8.0 and are always `N.0`, so a tool version like `Python 3.12` or
/// `PostgreSQL 16.4` is none; other minors appear only in the saas forms.
const SERIES: &str = r"(?:\d+\.saas~\d+|saas~\d+\.\d+|saas-\d+(?:\.\d+)?|(?:[89]|[1-9]\d)\.0)";
/// Spaces, or one line break: a wrapped doc comment puts a comment leader after it, a wrapped
/// Markdown line in `docs/` none.
const GAP: &str = r"(?:[ \t]+|[ \t]*\n[ \t]*(?:(?:///|//!|//|#)[ \t]*)?)";
/// A `:line` cites a line of the path before it, so it carries a series too.
const PATH: &str = r"`(?P<path>(?:odoo/|addons/|:\d)[^`\n]*)`";

static SERIES_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(SERIES).unwrap());

/// Each member of a list counts, as in `16.0 and 18.0`, also when the list wraps.
static SERIES_LIST: LazyLock<String> =
    LazyLock::new(|| format!(r"(?P<list>{SERIES}(?:(?:,{GAP}?|,?{GAP}(?:and|or){GAP}){SERIES})*)"));

/// A series must not follow a word character, `.`, `~` or `-`, so the `10.3` of a version `2.10.3`
/// is none.
static BEFORE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"(?m)(?:^|[^\w.~-]){}{GAP}{PATH}", *SERIES_LIST)).unwrap());

/// The same bound on the right, as the regex crate has no lookahead: `on 21.0.3` is no series,
/// while the period that ends a sentence, with no digit after it, still bounds one.
static AFTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?m){PATH}{GAP}on{GAP}{}(?:$|[^\w.~-]|\.(?:$|\D))",
        *SERIES_LIST
    ))
    .unwrap()
});

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the workspace root")
}

/// The series of the `ODOO_<series> :=` lines of the `Makefile`, the ones `.oracle/` can hold.
static PINNED: LazyLock<Vec<String>> = LazyLock::new(pinned);

fn pinned() -> Vec<String> {
    let makefile = std::fs::read_to_string(root().join("Makefile")).expect("Makefile");
    let pin = Regex::new(r"(?m)^ODOO_(\S+)[ \t]*:=").unwrap();
    let series: Vec<String> = pin.captures_iter(&makefile).map(|c| c[1].to_owned()).collect();
    assert!(!series.is_empty(), "the Makefile pins no ODOO_<series> commit");
    series
}

/// Every UTF-8 file under `crates/` and `docs/`, as its path from the root and its text. Fixtures
/// are skipped: they are code for the scanner to read, not the repository's own text.
fn tree() -> Vec<(PathBuf, String)> {
    let mut walk = WalkBuilder::new(root().join("crates"));
    walk.add(root().join("docs"));
    walk.filter_entry(|e| !e.path().ends_with("tests/fixtures"))
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|e| {
            let text = std::fs::read_to_string(e.path()).ok()?;
            Some((e.path().strip_prefix(root()).ok()?.to_path_buf(), text))
        })
        .collect()
}

/// Each series a backticked Odoo path is cited with, as its line, the series and the path: a
/// series or list of them before the path, or `on` and the series after it.
fn citations(text: &str) -> Vec<(usize, String, String)> {
    let line = |offset: usize| text[..offset].matches('\n').count() + 1;
    let mut found: Vec<_> = [&*BEFORE, &*AFTER]
        .into_iter()
        .flat_map(|re| re.captures_iter(text))
        .flat_map(|c| {
            let list = c.name("list").unwrap();
            let path = c["path"].to_owned();
            SERIES_RE
                .find_iter(list.as_str())
                .map(|s| (line(list.start() + s.start()), s.as_str().to_owned(), path.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

#[test]
fn cited_series_are_pinned() {
    let pins = PINNED.join(", ");
    let errors: Vec<String> = tree()
        .iter()
        .flat_map(|(file, text)| reported(text).into_iter().map(move |cited| (file, cited)))
        .map(|(file, (line, series, path))| {
            format!(
                "{}:{line}: {series} `{path}`, cite a series the Makefile pins ({pins})",
                file.display()
            )
        })
        .collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

/// The citations of `text` whose series the `Makefile` does not pin.
fn reported(text: &str) -> Vec<(usize, String, String)> {
    citations(text)
        .into_iter()
        .filter(|(_, series, _)| !PINNED.contains(series))
        .collect()
}

fn cite(line: usize, series: &str, path: &str) -> (usize, String, String) {
    (line, series.to_owned(), path.to_owned())
}

// A reported input is joined at run time, so `cited_series_are_pinned` does not find it here.
#[test]
fn reports_a_series_before_a_path() {
    let text = ["(16.0", " `", "odoo/tools/js_transpiler.py:22", "`)."].concat();
    assert_eq!(reported(&text), [cite(1, "16.0", "odoo/tools/js_transpiler.py:22")]);
}

#[test]
fn reports_a_series_before_a_line_of_the_path_before() {
    let text = ["18.0 `odoo/models.py:4`, saas~17.2", " `", ":12", "`"].concat();
    assert_eq!(reported(&text), [cite(1, "saas~17.2", ":12")]);
}

#[test]
fn reports_each_member_of_a_list() {
    let text = ["16.0 and 18.0", " `", "addons/web/x.js", "`"].concat();
    assert_eq!(reported(&text), [cite(1, "16.0", "addons/web/x.js")]);
    let text = ["17.0, 16.0", " `", "odoo/http.py", "`"].concat();
    assert_eq!(
        reported(&text),
        [cite(1, "16.0", "odoo/http.py"), cite(1, "17.0", "odoo/http.py")]
    );
}

#[test]
fn reports_a_series_that_ends_the_comment_line_before() {
    let text = ["/// It reads it (16.0\n", "/// `", "odoo/fields.py:10", "`)."].concat();
    assert_eq!(reported(&text), [cite(1, "16.0", "odoo/fields.py:10")]);
}

#[test]
fn reports_a_series_that_ends_the_markdown_line_before() {
    let text = ["16.0\n", "`", "odoo/models.py", "`"].concat();
    assert_eq!(reported(&text), [cite(1, "16.0", "odoo/models.py")]);
}

#[test]
fn reports_each_member_of_a_wrapped_list() {
    let text = ["/// (17.0 and\n", "/// 18.0 `", "odoo/x.py", "`)"].concat();
    assert_eq!(reported(&text), [cite(1, "17.0", "odoo/x.py")]);
}

#[test]
fn reports_a_series_after_a_path() {
    let text = ["x\n(`", "odoo/models.py:5021", "` on", " 17.0)."].concat();
    assert_eq!(reported(&text), [cite(2, "17.0", "odoo/models.py:5021")]);
}

#[test]
fn reports_a_series_after_a_path_that_ends_a_sentence() {
    let text = ["Fixed in `", "odoo/x.py", "` on", " 17.0."].concat();
    assert_eq!(reported(&text), [cite(1, "17.0", "odoo/x.py")]);
    let text = ["/// `", "odoo/x.py", "` on 17.0", " and\n/// 16.0."].concat();
    assert_eq!(
        reported(&text),
        [cite(1, "17.0", "odoo/x.py"), cite(2, "16.0", "odoo/x.py")]
    );
}

#[test]
fn reports_every_form_of_a_series() {
    for series in ["saas~17.2", "saas-17", "saas-17.2", "8.saas~6"] {
        let text = [series, " `", "odoo/models.py", "`"].concat();
        assert_eq!(reported(&text), [cite(1, series, "odoo/models.py")]);
    }
}

#[test]
fn passes_a_pinned_series_prose_a_hash_and_other_paths() {
    for text in [
        "18.0 `odoo/models.py:5021`, 20.0 `:5489`",
        "(`odoo/models.py:5021` on 18.0)",
        "(`odoo/models.py:5021` on 18.0.",
        "From saas~14.3 an empty `depends` reads as `['base']`.",
        "saas~17.2 1c47e0c1d694 `odoo/models.py`",
        "(1c47e0c1d694, 18.0 `odoo/tools/babel/python_extractor.py`)",
        "14.0 `static/src`",
    ] {
        assert_eq!(reported(text), [], "{text}");
    }
}

#[test]
fn passes_a_version_that_is_no_series() {
    for text in [
        "2.10.3 `messages/extract.py`",
        "2.10.3 `odoo/tools/misc.py`",
        "Python 3.12 `odoo/release.py`",
        "Python 3.12\n`odoo/release.py`",
        "`odoo/x.py` on 2.10.3",
        "`odoo/x.py` on 17.0.3",
        "PostgreSQL 16.4 `odoo/sql_db.py`",
        "Node 20.11 `addons/web/x.js`",
        "`odoo/sql_db.py` on 16.4.",
    ] {
        assert_eq!(reported(text), [], "{text}");
    }
}

/// A hyphen names the branch, not the series.
static HYPHENATED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bsaas-\d+(?:\.\d+)?").unwrap());

/// Each series of `text` spelled as its git branch, as its line and the spelling. A ref right after
/// `origin/`, a backtick or a quote names the branch on purpose, so it passes; one quoted further
/// back in the span does not.
fn hyphenated(text: &str) -> Vec<(usize, String)> {
    HYPHENATED
        .find_iter(text)
        .filter(|m| {
            let before = &text[..m.start()];
            !(before.ends_with("origin/") || before.ends_with(['"', '`']))
        })
        .map(|m| (text[..m.start()].matches('\n').count() + 1, m.as_str().to_owned()))
        .collect()
}

#[test]
fn series_are_spelled_with_the_tilde() {
    let errors: Vec<String> = tree()
        .iter()
        .flat_map(|(file, text)| hyphenated(text).into_iter().map(move |hit| (file, hit)))
        .map(|(file, (line, series))| {
            format!(
                "{}:{line}: {series} names a git branch; spell the series saas~X.Y (N.saas~M before 11.0), or put origin/, a backtick or a quote right before a ref",
                file.display()
            )
        })
        .collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

#[test]
fn reports_a_series_spelled_as_its_branch() {
    let text = ["x\n/// not in saas", "-15.2)"].concat();
    assert_eq!(hyphenated(&text), [(2, "saas-15.2".to_owned())]);
    let text = ["saas", "-12 adds it"].concat();
    assert_eq!(hyphenated(&text), [(1, "saas-12".to_owned())]);
    let text = ["\"before saas", "-15.3\""].concat();
    assert_eq!(hyphenated(&text), [(1, "saas-15.3".to_owned())]);
}

#[test]
fn passes_a_quoted_ref_and_the_series_spellings() {
    for text in [
        "origin/saas-15.2",
        "`saas-18.1`",
        "\"saas-18.1\"",
        "saas~15.2",
        "10.saas~15",
        "custom/saas153",
        "saas_15_2",
        "format!(\"saas-{series}\")",
    ] {
        assert_eq!(hyphenated(text), [], "{text}");
    }
}
