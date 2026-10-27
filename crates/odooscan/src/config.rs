use std::path::{Component, Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::Deserialize;

const CONFIG_FILE: &str = "odooscan.toml";
const PYPROJECT: &str = "pyproject.toml";

/// The `[tool.odooscan]` table, or the whole of `odooscan.toml`. Keys mirror the `check` flags.
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Config {
    pub addons_path: Vec<PathBuf>,
    pub odoo_version: Option<String>,
    pub select: Vec<String>,
    pub ignore: Vec<String>,
    pub exclude: Vec<String>,
    pub baseline: Option<PathBuf>,
}

#[derive(Deserialize)]
struct Pyproject {
    tool: Option<Tool>,
}

#[derive(Deserialize)]
struct Tool {
    odooscan: Option<Config>,
}

pub struct Loaded {
    pub config: Config,
    /// Relative paths in the file are read from here.
    pub dir: PathBuf,
}

/// The file named by `--config`, or else the nearest `odooscan.toml`, or `pyproject.toml` with a
/// `[tool.odooscan]` table, in `start` or a directory above it.
pub fn load(explicit: Option<&Path>, start: &Path) -> Result<Option<Loaded>, String> {
    if let Some(path) = explicit {
        return read(path)?
            .map(Some)
            .ok_or_else(|| format!("{}: no [tool.odooscan] table", path.display()));
    }
    for dir in start.ancestors() {
        for path in [dir.join(CONFIG_FILE), dir.join(PYPROJECT)] {
            if path.is_file()
                && let Some(loaded) = read(&path)?
            {
                return Ok(Some(loaded));
            }
        }
    }
    Ok(None)
}

fn read(path: &Path) -> Result<Option<Loaded>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let invalid = |e: toml::de::Error| format!("{}: {e}", path.display());
    let config = if path.file_name() == Some(PYPROJECT.as_ref()) {
        toml::from_str::<Pyproject>(&text)
            .map_err(invalid)?
            .tool
            .and_then(|t| t.odooscan)
    } else {
        Some(toml::from_str::<Config>(&text).map_err(invalid)?)
    };
    let dir = path.parent().unwrap_or(Path::new("")).to_path_buf();
    Ok(config.map(|mut config| {
        config.addons_path = config.addons_path.iter().map(|p| dir.join(p)).collect();
        config.baseline = config.baseline.map(|p| dir.join(p));
        Loaded { config, dir }
    }))
}

/// Gitignore-style patterns. An anchored one, such as `/migrations` or `a/b`, is relative to the
/// matcher's root; an unanchored one, such as `tests/`, matches at any depth, even outside the root.
pub struct Exclude(Option<Gitignore>);

impl Exclude {
    pub fn new(root: &Path, patterns: &[String]) -> Result<Self, String> {
        if patterns.is_empty() {
            return Ok(Self(None));
        }
        // Canonical on both sides, so a file under the root is recognised as one.
        let root = root.canonicalize().map_err(|e| format!("{}: {e}", root.display()))?;
        let mut builder = GitignoreBuilder::new(&root);
        for p in patterns {
            builder.add_line(None, p).map_err(|e| format!("exclude '{p}': {e}"))?;
        }
        builder.build().map(|g| Self(Some(g))).map_err(|e| e.to_string())
    }

    pub fn matches(&self, path: &Path) -> bool {
        let (Some(matcher), Ok(path)) = (&self.0, path.canonicalize()) else {
            return false;
        };
        // The matcher takes a path relative to its root. One outside the root loses its absolute
        // prefix instead, so only unanchored patterns can match it.
        let rel: PathBuf = path.strip_prefix(matcher.path()).map_or_else(
            |_| {
                path.components()
                    .skip_while(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
                    .collect()
            },
            Path::to_path_buf,
        );
        matcher.matched_path_or_any_parents(&rel, false).is_ignore()
    }
}
