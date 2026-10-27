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
            let standalone = trimmed.starts_with('#') || trimmed.starts_with("<!--");
            map.insert(i as u32 + if standalone { 2 } else { 1 }, codes);
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
    }
}
