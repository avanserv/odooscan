//! The rewrites a finding carries, as byte edits of the file it was found in. Offsets are the
//! parser's, into the file's bytes as read, so a BOM and CRLF line ends stay where they are.

/// Replaces `start..end` with `replacement`, once those bytes still read `expected`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub expected: String,
    pub replacement: String,
}

/// The edits that fix one finding, applied all or none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    pub edits: Vec<Edit>,
}

impl Fix {
    pub fn single(edit: Edit) -> Self {
        Self { edits: vec![edit] }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Applied,
    /// The file no longer holds the text the scan read.
    Mismatch,
    /// An accepted fix touches the same bytes or starts where it does, so the order of two
    /// insertions there would be a guess.
    Overlap,
}

/// `bytes` with the fixes applied in their order, each dropped on a mismatch or an overlap with
/// one accepted before it, and what became of each.
pub fn apply(bytes: &[u8], fixes: &[&Fix]) -> (Vec<u8>, Vec<Outcome>) {
    let mut accepted: Vec<&Edit> = vec![];
    let outcomes = fixes
        .iter()
        .map(|candidate| {
            let outcome = judge(bytes, candidate, &accepted);
            if outcome == Outcome::Applied {
                accepted.extend(&candidate.edits);
            }
            outcome
        })
        .collect();
    accepted.sort_by_key(|e| e.start);
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    for edit in accepted {
        out.extend_from_slice(&bytes[at..edit.start]);
        out.extend_from_slice(edit.replacement.as_bytes());
        at = edit.end;
    }
    out.extend_from_slice(&bytes[at..]);
    (out, outcomes)
}

fn judge(bytes: &[u8], candidate: &Fix, accepted: &[&Edit]) -> Outcome {
    let matches = |e: &Edit| e.start <= e.end && bytes.get(e.start..e.end) == Some(e.expected.as_bytes());
    if !candidate.edits.iter().all(matches) {
        return Outcome::Mismatch;
    }
    let clashes = |e: &Edit, others: &[&Edit]| {
        others
            .iter()
            .any(|o| o.start == e.start || (e.start < o.end && o.start < e.end))
    };
    let own: Vec<&Edit> = candidate.edits.iter().collect();
    let crossed = candidate
        .edits
        .iter()
        .enumerate()
        .any(|(i, e)| clashes(e, accepted) || clashes(e, &own[..i]));
    if crossed { Outcome::Overlap } else { Outcome::Applied }
}

/// The edit that removes `start..end` with its whole line, terminator included, when nothing but
/// blanks shares the line with it.
pub fn remove_line(src: &str, start: usize, end: usize) -> Option<Edit> {
    let text = src.get(start..end)?;
    if text.contains(['\n', '\r']) {
        return None;
    }
    let line_start = src[..start].rfind(['\n', '\r']).map_or(0, |i| i + 1);
    let blank = |s: &str| s.bytes().all(|b| matches!(b, b' ' | b'\t'));
    if !blank(&src[line_start..start]) {
        return None;
    }
    let rest = &src[end..];
    let trail = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    let after = &rest[trail..];
    let terminator = match after.as_bytes() {
        [b'\r', b'\n', ..] => 2,
        [b'\n' | b'\r', ..] => 1,
        [] => 0,
        _ => return None,
    };
    let line_end = end + trail + terminator;
    Some(Edit {
        start: line_start,
        end: line_end,
        expected: src[line_start..line_end].to_owned(),
        replacement: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_edit(start: usize, end: usize, expected: &str, replacement: &str) -> Edit {
        Edit {
            start,
            end,
            expected: expected.to_owned(),
            replacement: replacement.to_owned(),
        }
    }

    fn new_fix(start: usize, end: usize, expected: &str, replacement: &str) -> Fix {
        Fix::single(new_edit(start, end, expected, replacement))
    }

    fn applied(src: &str, fixes: &[&Fix]) -> (String, Vec<Outcome>) {
        let (out, outcomes) = apply(src.as_bytes(), fixes);
        (String::from_utf8(out).unwrap(), outcomes)
    }

    #[test]
    fn fixes_apply_in_any_order_of_offsets() {
        let late = new_fix(8, 11, "baz", "qux");
        let early = new_fix(0, 3, "foo", "x");
        let (out, outcomes) = applied("foo bar baz", &[&late, &early]);
        assert_eq!(out, "x bar qux");
        assert_eq!(outcomes, [Outcome::Applied, Outcome::Applied]);
    }

    #[test]
    fn a_fix_overlapping_an_accepted_one_is_dropped_whole() {
        let first = new_fix(0, 7, "foo bar", "x");
        let second = Fix {
            edits: vec![new_edit(8, 11, "baz", "y"), new_edit(4, 7, "bar", "z")],
        };
        let (out, outcomes) = applied("foo bar baz", &[&first, &second]);
        assert_eq!(out, "x baz");
        assert_eq!(outcomes, [Outcome::Applied, Outcome::Overlap]);
    }

    #[test]
    fn two_insertions_at_one_offset_keep_the_first() {
        let a = new_fix(4, 4, "", "a");
        let b = new_fix(4, 4, "", "b");
        let (out, outcomes) = applied("foo bar", &[&a, &b]);
        assert_eq!(out, "foo abar");
        assert_eq!(outcomes, [Outcome::Applied, Outcome::Overlap]);
    }

    #[test]
    fn edits_that_only_touch_both_apply() {
        let a = new_fix(0, 3, "foo", "x");
        let b = new_fix(3, 4, " ", "_");
        assert_eq!(applied("foo bar", &[&a, &b]).0, "x_bar");
    }

    #[test]
    fn a_fix_whose_text_changed_is_dropped() {
        let stale = new_fix(4, 7, "baz", "x");
        let past_end = new_fix(4, 20, "bar", "x");
        let (out, outcomes) = applied("foo bar", &[&stale, &past_end]);
        assert_eq!(out, "foo bar");
        assert_eq!(outcomes, [Outcome::Mismatch, Outcome::Mismatch]);
    }

    #[test]
    fn a_duplicate_fix_applies_once() {
        let a = new_fix(4, 7, "bar", "baz");
        let (out, outcomes) = applied("foo bar", &[&a, &a.clone()]);
        assert_eq!(out, "foo baz");
        assert_eq!(outcomes, [Outcome::Applied, Outcome::Overlap]);
    }

    #[test]
    fn bytes_outside_the_edits_are_kept() {
        let src = "\u{feff}a = 1\r\nb = 2\r\n";
        let start = src.find('b').unwrap();
        let (out, _) = applied(src, &[&new_fix(start, start + 1, "b", "c")]);
        assert_eq!(out, "\u{feff}a = 1\r\nc = 2\r\n");
    }

    fn removed(src: &str, node: &str) -> Option<String> {
        let start = src.find(node).unwrap();
        let removal = Fix::single(remove_line(src, start, start + node.len())?);
        Some(applied(src, &[&removal]).0)
    }

    #[test]
    fn a_node_alone_on_its_line_goes_with_the_line() {
        let src = "class A:\n    @api.multi\n    def f(self):\n";
        assert_eq!(removed(src, "@api.multi").unwrap(), "class A:\n    def f(self):\n");
        let crlf = "class A:\r\n    @api.multi  \r\n    def f(self):\r\n";
        assert_eq!(removed(crlf, "@api.multi").unwrap(), "class A:\r\n    def f(self):\r\n");
        assert_eq!(removed("x\n@api.multi", "@api.multi").unwrap(), "x\n");
    }

    #[test]
    fn a_node_sharing_its_line_stays() {
        assert_eq!(removed("    @api.multi  # keep\n", "@api.multi"), None);
        assert_eq!(removed("    x; @api.multi\n", "@api.multi"), None);
    }
}
