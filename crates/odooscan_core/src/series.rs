use std::fmt;

/// An Odoo release series such as 17.0, or 17.2 for saas~17.2 and 9.10 for 9.saas~10.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Series {
    pub major: u16,
    pub minor: u16,
}

impl Series {
    /// Parses `17.0`, `saas~17.2` or `9.saas~10`. Of the `N.saas~M` releases, only 9.saas~7 to
    /// 9.saas~11 are dated, so every other one reads as unknown. A SaaS release before 11.0 has
    /// no other spelling, so `9.10`, `10.1` and `saas~9.3` read as unknown too.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if let Some((major, minor)) = text.split_once(".saas~") {
            let minor = minor.parse().ok()?;
            return (major == "9" && NINE_SAAS.contains(&minor)).then_some(Self { major: 9, minor });
        }
        let text = text.trim_start_matches("saas~");
        let (major, minor) = text.split_once('.')?;
        let series = Self {
            major: major.parse().ok()?,
            minor: minor.parse().ok()?,
        };
        (!series.dotted_saas()).then_some(series)
    }

    /// Odoo prefixes a module version with the series only when it has five parts.
    pub fn from_module_version(version: &str) -> Option<Self> {
        let parts: Vec<&str> = version.trim().split('.').collect();
        if parts.len() != 5 {
            return None;
        }
        Self::parse(&format!("{}.{}", parts[0], parts[1]))
    }

    /// Whether Odoo spells the series `N.saas~M`, as it does a SaaS release before 11.0. 6.1 is a
    /// stable release of its own.
    fn dotted_saas(self) -> bool {
        self.minor != 0 && (7..=10).contains(&self.major)
    }

    /// The series as Odoo names it in `major_version` (18.0 `odoo/release.py:15-17`), which also
    /// heads a module version: `18.0`, `saas~18.1` or `9.saas~10`.
    pub fn name(self) -> String {
        if self.minor == 0 || self.major < 11 {
            self.to_string()
        } else {
            format!("saas~{self}")
        }
    }

    /// The odoo/odoo branch of the series: `18.0`, `saas-18.1`, or `saas-10` for 9.saas~10.
    pub fn branch(self) -> String {
        if self.dotted_saas() {
            format!("saas-{}", self.minor)
        } else if self.minor == 0 || self.major < 11 {
            self.to_string()
        } else {
            format!("saas-{self}")
        }
    }
}

/// The minors of the 9.0 SaaS releases whose gates are dated. 9.saas~12 is midway through the 10.0
/// ORM rewrite, so it stays unknown.
const NINE_SAAS: std::ops::RangeInclusive<u16> = 7..=11;

impl fmt::Display for Series {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.dotted_saas() {
            write!(f, "{}.saas~{}", self.major, self.minor)
        } else {
            write!(f, "{}.{}", self.major, self.minor)
        }
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
        assert!(matches(">=9.saas~10,<10.0", Series::parse("9.saas~11").unwrap()));
        assert!(!matches("<=9.saas~9", Series::parse("9.saas~10").unwrap()));
    }

    #[test]
    fn nine_saas_releases_sit_between_9_0_and_10_0() {
        let order = ["9.0", "9.saas~7", "9.saas~11", "10.0"].map(|s| Series::parse(s).unwrap());
        assert!(order.windows(2).all(|w| w[0] < w[1]));
        let unknown = [
            "9.saas~6",
            "9.saas~12",
            "9.saas~13",
            "8.saas~6",
            "10.saas~14",
            "9.saas~x",
            "10.1",
            "9.12",
            "9.10",
            "saas~9.3",
        ];
        for text in unknown {
            assert_eq!(Series::parse(text), None, "{text}");
        }
        assert_eq!(
            Series::from_module_version("9.saas~10.1.0.0"),
            Some(Series { major: 9, minor: 10 })
        );
    }

    #[test]
    fn spells_a_series_as_odoo_names_it_and_its_branch() {
        // Input, then `Display`, `name()` and `branch()`.
        let cases = [
            ("18.0", "18.0", "18.0", "18.0"),
            ("saas~18.1", "18.1", "saas~18.1", "saas-18.1"),
            ("saas~11.3", "11.3", "saas~11.3", "saas-11.3"),
            ("11.0", "11.0", "11.0", "11.0"),
            ("9.0", "9.0", "9.0", "9.0"),
            ("9.saas~10", "9.saas~10", "9.saas~10", "saas-10"),
            ("6.1", "6.1", "6.1", "6.1"),
        ];
        for (input, shown, name, branch) in cases {
            let series = Series::parse(input).unwrap();
            assert_eq!(
                (series.to_string().as_str(), series.name().as_str()),
                (shown, name),
                "{input}"
            );
            assert_eq!(series.branch(), branch, "{input}");
        }
    }
}
