use std::collections::HashMap;
use std::path::{Path, PathBuf};

use odooscan_core::Diagnostic;
use serde::{Deserialize, Serialize};

pub const DEFAULT_FILE: &str = "odooscan-baseline.json";

/// A finding is known by its file, code and message, never its line, so editing the lines above
/// it does not bring it back.
#[derive(Serialize, Deserialize)]
struct Entry {
    path: String,
    code: String,
    message: String,
    count: usize,
}

#[derive(Serialize, Deserialize)]
struct File {
    findings: Vec<Entry>,
}

type Key = (String, String, String);

pub struct Baseline {
    /// Paths in the file are relative to it.
    dir: PathBuf,
    counts: HashMap<Key, usize>,
}

fn dir_of(path: &Path) -> PathBuf {
    let dir = path.parent().unwrap_or(Path::new(""));
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf())
}

fn key(dir: &Path, d: &Diagnostic) -> Key {
    let path = d.path.canonicalize().unwrap_or_else(|_| d.path.clone());
    let rel = path.strip_prefix(dir).unwrap_or(&path);
    let rel = rel.to_string_lossy().replace('\\', "/");
    (rel, d.code.to_owned(), d.message.clone())
}

impl Baseline {
    pub fn read(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let file: File = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut counts = HashMap::new();
        for e in file.findings {
            *counts.entry((e.path, e.code, e.message)).or_default() += e.count;
        }
        Ok(Self {
            dir: dir_of(path),
            counts,
        })
    }

    /// Writes `diags` sorted, so a rewrite diffs only by what changed.
    pub fn write(path: &Path, diags: &[&Diagnostic]) -> Result<(), String> {
        let dir = dir_of(path);
        let mut counts: HashMap<Key, usize> = HashMap::new();
        for d in diags {
            *counts.entry(key(&dir, d)).or_default() += 1;
        }
        let mut findings: Vec<Entry> = counts
            .into_iter()
            .map(|((path, code, message), count)| Entry {
                path,
                code,
                message,
                count,
            })
            .collect();
        findings.sort_by(|a, b| (&a.path, &a.code, &a.message).cmp(&(&b.path, &b.code, &b.message)));
        let mut text = serde_json::to_string_pretty(&File { findings }).map_err(|e| e.to_string())?;
        text.push('\n');
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Drops each finding the baseline still has a count for, and returns how many it dropped.
    /// A third identical finding in a file with two recorded is reported.
    pub fn hide(&mut self, diags: &mut Vec<&Diagnostic>) -> usize {
        let before = diags.len();
        diags.retain(|d| match self.counts.get_mut(&key(&self.dir, d)) {
            Some(n) if *n > 0 => {
                *n -= 1;
                false
            }
            _ => true,
        });
        before - diags.len()
    }

    /// Recorded findings that no longer occur, after `hide`.
    pub fn stale(&self) -> usize {
        self.counts.values().sum()
    }
}
