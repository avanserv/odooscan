use std::collections::hash_map::Entry;

use rustc_hash::FxHashMap;

/// Inline suppressions: `odooscan: ignore[OD101,OD203]`, bare `odooscan: ignore`, or `noqa: OD101`.
/// A comment alone on its line covers the next line; a trailing one covers its own line.
#[derive(Default)]
pub struct Suppressions(FxHashMap<u32, Vec<String>>);

impl Suppressions {
    pub fn parse(text: &str) -> Self {
        let mut map = FxHashMap::default();
        for (i, line) in text.lines().enumerate() {
            let codes = if let Some(rest) = line.split_once("odooscan: ignore").map(|(_, r)| r) {
                rest.strip_prefix('[')
                    .and_then(|r| r.split_once(']'))
                    .map_or_else(Vec::new, |(codes, _)| split(codes))
            } else if let Some((_, rest)) = line.split_once("noqa:") {
                let codes: Vec<String> = split(rest).into_iter().filter(|c| c.starts_with("OD")).collect();
                if codes.is_empty() {
                    continue;
                }
                codes
            } else {
                continue;
            };
            let trimmed = line.trim_start();
            let standalone = ["#", "<!--", "//"].iter().any(|c| trimmed.starts_with(c));
            map.insert(i as u32 + if standalone { 2 } else { 1 }, codes);
        }
        Self(map)
    }

    /// The suppressions with the line each covers moved by `to`. Lines moved together merge their
    /// codes, a bare `odooscan: ignore` still covering every rule.
    pub(crate) fn moved(self, to: impl Fn(u32) -> u32) -> Self {
        let mut map: FxHashMap<u32, Vec<String>> = FxHashMap::default();
        for (line, codes) in self.0 {
            match map.entry(to(line)) {
                Entry::Vacant(slot) => {
                    slot.insert(codes);
                }
                Entry::Occupied(mut slot) if !slot.get().is_empty() => {
                    if codes.is_empty() {
                        slot.get_mut().clear();
                    } else {
                        slot.get_mut().extend(codes);
                    }
                }
                Entry::Occupied(_) => {}
            }
        }
        Self(map)
    }

    pub fn allows(&self, line: u32, code: &str) -> bool {
        self.0
            .get(&line)
            .is_none_or(|codes| !codes.is_empty() && !codes.iter().any(|c| c == code))
    }
}

fn split(codes: &str) -> Vec<String> {
    codes
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(|c| c.trim_end_matches("-->").trim())
        .filter(|c| !c.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_and_trailing() {
        let s = Suppressions::parse("# odooscan: ignore[OD101]\ncr.execute(q)\nx = 1  # noqa: OD103, E501\n");
        assert!(!s.allows(2, "OD101"));
        assert!(s.allows(2, "OD102"));
        assert!(!s.allows(3, "OD103"));
        assert!(s.allows(1, "OD101"));
        let js = Suppressions::parse("// odooscan: ignore[OD601]\nimport { a } from \"@web/x\";\n");
        assert!(!js.allows(2, "OD601"));
    }

    #[test]
    fn moved_lines_merge_their_codes() {
        let s = Suppressions::parse("# noqa: OD801\n# noqa: OD802\n# odooscan: ignore\n# noqa: OD803\nx\n");
        let moved = s.moved(|line| if line < 5 { 9 } else { line });
        assert!(!moved.allows(9, "OD801"));
        assert!(!moved.allows(9, "OD805"));
        assert!(!moved.allows(5, "OD803"));
        assert!(moved.allows(5, "OD801"));
    }
}
