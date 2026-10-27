use std::fmt;

/// An Odoo release series such as 17.0, or 17.2 for a SaaS release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Series {
    pub major: u16,
    pub minor: u16,
}

impl Series {
    /// Parses `17.0` or `saas~17.2`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_start_matches("saas~");
        let (major, minor) = text.split_once('.')?;
        Some(Self {
            major: major.parse().ok()?,
            minor: minor.parse().ok()?,
        })
    }

    /// Odoo prefixes a module version with the series only when it has five parts.
    pub fn from_module_version(version: &str) -> Option<Self> {
        let parts: Vec<&str> = version.trim().split('.').collect();
        if parts.len() != 5 {
            return None;
        }
        Self::parse(&format!("{}.{}", parts[0], parts[1]))
    }
}

impl fmt::Display for Series {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// Whether `series` satisfies a comma-separated list of comparisons such as `>=17.0,<19.0`.
/// The grammar is the subset of PEP 440 specifiers that series numbers need.
pub fn matches(spec: &str, series: Series) -> bool {
    spec.split(',').all(|clause| {
        let clause = clause.trim();
        let (op, rest) = ["==", "!=", ">=", "<=", ">", "<"]
            .iter()
            .find_map(|op| clause.strip_prefix(op).map(|rest| (*op, rest)))
            .unwrap_or(("==", clause));
        let Some(bound) = Series::parse(rest) else { return false };
        match op {
            "==" => series == bound,
            "!=" => series != bound,
            ">=" => series >= bound,
            "<=" => series <= bound,
            ">" => series > bound,
            _ => series < bound,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_version_needs_five_parts() {
        assert_eq!(Series::from_module_version("17.0.1.2.0"), Series::parse("17.0"));
        assert_eq!(Series::from_module_version("1.2.0"), None);
    }

    #[test]
    fn specifiers() {
        let s18 = Series::parse("18.0").unwrap();
        assert!(matches(">=17.0", s18));
        assert!(!matches(">=17.0,<18.0", s18));
        assert!(matches(">=17.0,<18.0", Series::parse("saas~17.4").unwrap()));
    }
}
