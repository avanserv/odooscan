//! The hook method names a file's text mentions. Odoo runs a compute, inverse, search or onchange
//! method only by the name something gives it, and a file can give it from Python, a data file, a
//! script or a template alike, so the scan reads every word rather than the parsed facts.

use std::sync::LazyLock;

use regex::Regex;

/// The prefixes of the methods Odoo runs only by a name it is given.
pub const HOOK_PREFIXES: &[&str] = &["_compute_", "_inverse_", "_search_", "_onchange_", "onchange_"];

/// A word starting with a hook prefix. An ASCII boundary keeps the search on the fast engines,
/// which a Unicode one leaves on any non-ASCII text; hook names are ASCII.
static HOOK_WORD: LazyLock<Regex> = LazyLock::new(|| {
    let prefixes = HOOK_PREFIXES.join("|");
    Regex::new(&format!(r"(?-u:\b)(?:{prefixes})[A-Za-z0-9_]*")).unwrap()
});

/// The hook names of a file, sorted and without repeats.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Mentions {
    /// The words that name a method whole.
    pub names: Vec<String>,
    /// The words a name is built from: one ending in `_`, or followed by `%`, `{` or `$`, as in
    /// `"_compute_%s" % name`, `f"_compute_{name}"` or `` `_compute_${name}` ``.
    pub prefixes: Vec<String>,
}

impl Mentions {
    /// Every hook word of `text` but the one each `def` defines, which names nothing to call.
    pub fn scan(text: &str) -> Self {
        let mut mentions = Self::default();
        for word in HOOK_WORD.find_iter(text) {
            if after_def(&text[..word.start()]) {
                continue;
            }
            let next = text[word.end()..].bytes().next();
            let built = word.as_str().ends_with('_') || matches!(next, Some(b'%' | b'{' | b'$'));
            let list = if built {
                &mut mentions.prefixes
            } else {
                &mut mentions.names
            };
            list.push(word.as_str().to_owned());
        }
        for list in [&mut mentions.names, &mut mentions.prefixes] {
            list.sort_unstable();
            list.dedup();
        }
        mentions
    }
}

/// Whether `before` ends with the keyword `def` and the blanks after it.
fn after_def(before: &str) -> bool {
    let keyword = before.trim_end_matches([' ', '\t']);
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    keyword.len() < before.len()
        && keyword
            .strip_suffix("def")
            .is_some_and(|head| !head.bytes().next_back().is_some_and(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str) -> Vec<String> {
        Mentions::scan(text).names
    }

    #[test]
    fn a_def_names_nothing() {
        let text = "    def _compute_total(self):\n        async def  _onchange_partner(self):\n";
        assert!(names(text).is_empty());
        assert_eq!(names("undef _compute_total"), ["_compute_total"]);
        assert_eq!(names("def_compute_total"), Vec::<String>::new());
    }

    #[test]
    fn reads_names_from_any_text() {
        let text = r#"compute="_compute_total" self._inverse_total() "onchange_customer" # _search_total"#;
        assert_eq!(
            names(text),
            ["_compute_total", "_inverse_total", "_search_total", "onchange_customer"]
        );
        assert_eq!(names("orm.call('a.b', '_compute_total')"), ["_compute_total"]);
    }

    #[test]
    fn a_hook_word_starts_a_word() {
        assert!(names("x_compute_total __compute_total re_search_total").is_empty());
        assert_eq!(names("_onchange_partner"), ["_onchange_partner"]);
    }

    #[test]
    fn a_built_name_is_a_prefix() {
        let text = r#""_compute_%s" % f f"_inverse_{f}" `_search_${f}` '_compute_line_' + f "onchange_x%s""#;
        let mentions = Mentions::scan(text);
        assert!(mentions.names.is_empty());
        assert_eq!(
            mentions.prefixes,
            ["_compute_", "_compute_line_", "_inverse_", "_search_", "onchange_x"]
        );
    }
}
