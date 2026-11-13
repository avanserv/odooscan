//! The hook method names a file's text mentions, and the words of a module's text. Odoo runs a
//! compute, inverse, search or onchange method only by the name something gives it, and a module
//! uses another from Python, a data file, a script or a template alike, so the scan reads every
//! word rather than the parsed facts.

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

/// An identifier, or identifiers joined by `.` or `/`, as a model, an XML id, a Python module path,
/// a JS specifier or an asset path spells one. A `/` that starts the word, as in a URL path, stays
/// on it, and `./` or `../` steps join as `/` does, since `@x/../lib/y` loads from `x`. ASCII, as
/// `HOOK_WORD`.
static WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:(?-u:\B)/)?(?-u:\b)[A-Za-z_][A-Za-z0-9_]*(?:(?:/(?:\.\.?/)*|\.)[A-Za-z0-9_]+)*").unwrap()
});

/// The words of some text, sorted and without repeats. They sit in one buffer, one per line, since
/// a module holds tens of thousands and the index keeps those of every checked module.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Words {
    lines: String,
}

impl Words {
    /// Every word of `text`, comments included.
    pub fn scan(text: &str) -> Self {
        Self::of(WORD.find_iter(text).map(|m| m.as_str()))
    }

    /// The words of all of `parts`.
    pub fn merge<'w>(parts: impl IntoIterator<Item = &'w Words>) -> Self {
        Self::of(parts.into_iter().flat_map(Words::iter))
    }

    fn of<'w>(words: impl Iterator<Item = &'w str>) -> Self {
        let mut words: Vec<&str> = words.collect();
        words.sort_unstable();
        words.dedup();
        Self {
            lines: words.join("\n"),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.lines.split('\n').filter(|w| !w.is_empty())
    }

    /// The parts a word joins with `.` or `/`, none for a word that joins nothing. A leading `/`
    /// joins nothing, so a one-step URL path names no module.
    pub fn segments(word: &str) -> impl Iterator<Item = &str> {
        let word = word.strip_prefix('/').unwrap_or(word);
        let joined = word.contains(['.', '/']);
        word.split(['.', '/']).filter(move |s| joined && !s.is_empty())
    }
}

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

    fn words(text: &str) -> Vec<String> {
        Words::scan(text).iter().map(str::to_owned).collect()
    }

    #[test]
    fn a_word_joins_identifiers_with_dots_and_slashes() {
        let text = r#"self.env['sale.order'] # odoo.addons.sale.x
import { a } from "@sale/js/a"; <t t-call="web.layout"/> 'v': '18.0.1.0' x9 9x"#;
        assert_eq!(
            words(text),
            [
                "a",
                "call",
                "from",
                "import",
                "odoo.addons.sale.x",
                "sale.order",
                "sale/js/a",
                "self.env",
                "t",
                "v",
                "web.layout",
                "x9"
            ]
        );
        assert_eq!(words("sale.order. Done"), ["Done", "sale.order"]);
        assert_eq!(words("wait...sale"), ["sale", "wait"]);
    }

    #[test]
    fn a_word_keeps_a_leading_slash_and_steps_through_dot_dirs() {
        let text = r#"<iframe src="/map?z=2"/> from "@auth/../lib/x"; a/./b x/y"#;
        assert_eq!(
            words(text),
            ["/map", "a/./b", "auth/../lib/x", "from", "iframe", "src", "x/y", "z"]
        );
    }

    #[test]
    fn words_merge_without_repeats() {
        let merged = Words::merge([&Words::scan("b a"), &Words::scan("c a")]);
        assert_eq!(merged.iter().collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(Words::default().iter().count(), 0);
    }

    #[test]
    fn only_a_joined_word_has_segments() {
        assert_eq!(Words::segments("sale/js/a").collect::<Vec<_>>(), ["sale", "js", "a"]);
        assert_eq!(Words::segments("sale").count(), 0);
        assert_eq!(Words::segments("/map").count(), 0);
        assert_eq!(Words::segments("/web/image").collect::<Vec<_>>(), ["web", "image"]);
        assert_eq!(Words::segments("auth/../lib").collect::<Vec<_>>(), ["auth", "lib"]);
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
