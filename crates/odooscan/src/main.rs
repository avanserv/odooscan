use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand, ValueEnum};
use odooscan_core::rules::{RULES, Severity};
use odooscan_core::series::Series;
use odooscan_core::{Diagnostic, ScanOptions, scan};
use serde_json::json;

#[derive(Parser)]
#[command(version, about = "Quality and security scanner for Odoo modules")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check modules and report diagnostics.
    Check {
        /// Module directories, or directories to search for modules.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,
        /// Addons directories or Odoo checkouts to index for context; repeat or separate with commas.
        #[arg(long, value_delimiter = ',', env = "ODOOSCAN_ADDONS_PATH")]
        addons_path: Vec<PathBuf>,
        /// Series for modules whose manifest version does not name one, e.g. 18.0.
        #[arg(long, value_parser = parse_series)]
        odoo_version: Option<Series>,
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
        /// Only report these codes or code prefixes, e.g. OD1,OD304.
        #[arg(long, value_delimiter = ',')]
        select: Vec<String>,
        /// Never report these codes or code prefixes.
        #[arg(long, value_delimiter = ',')]
        ignore: Vec<String>,
        /// Print a count per rule instead of each diagnostic.
        #[arg(long)]
        statistics: bool,
    },
    /// List the rules, optionally only those that apply to a series.
    Rules {
        #[arg(long, value_parser = parse_series)]
        odoo_version: Option<Series>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
    Sarif,
}

fn parse_series(s: &str) -> Result<Series, String> {
    Series::parse(s).ok_or_else(|| format!("'{s}' is not a series such as 18.0"))
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Rules { odoo_version } => {
            for r in RULES
                .iter()
                .filter(|r| odoo_version.is_none_or(|s| r.applies_to(Some(s))))
            {
                println!(
                    "{}  {:<30} {:<12} {}",
                    r.code,
                    r.name,
                    format!("{:?}", r.category).to_lowercase(),
                    r.summary
                );
            }
            ExitCode::SUCCESS
        }
        Command::Check {
            paths,
            addons_path,
            odoo_version,
            format,
            select,
            ignore,
            statistics,
        } => {
            let started = Instant::now();
            let report = scan(&ScanOptions {
                paths,
                addons_paths: addons_path,
                default_series: odoo_version,
            });
            let matches = |code: &str, list: &[String]| list.iter().any(|p| code.starts_with(p.as_str()));
            let diags: Vec<&Diagnostic> = report
                .diagnostics
                .iter()
                .filter(|d| (select.is_empty() || matches(d.code, &select)) && !matches(d.code, &ignore))
                .collect();
            for notice in &report.notices {
                eprintln!("notice: {notice}");
            }
            let cwd = std::env::current_dir().unwrap_or_default();
            let rel = |p: &Path| p.strip_prefix(&cwd).unwrap_or(p).display().to_string();
            let mut stdout = std::io::stdout().lock();
            match format {
                Format::Text if statistics => {
                    let mut counts: Vec<(&str, usize)> = vec![];
                    for d in &diags {
                        match counts.iter_mut().find(|(c, _)| *c == d.code) {
                            Some((_, n)) => *n += 1,
                            None => counts.push((d.code, 1)),
                        }
                    }
                    counts.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
                    for (code, n) in counts {
                        let _ = writeln!(stdout, "{n:>6}  {code}  {}", odooscan_core::rules::rule(code).name);
                    }
                }
                Format::Text => {
                    let color = std::io::stdout().is_terminal();
                    for d in &diags {
                        let (on, off) = match (color, d.severity) {
                            (false, _) => ("", ""),
                            (true, Severity::Error) => ("\x1b[1;31m", "\x1b[0m"),
                            (true, Severity::Warning) => ("\x1b[1;33m", "\x1b[0m"),
                            (true, Severity::Info) => ("\x1b[1;36m", "\x1b[0m"),
                        };
                        let _ = writeln!(
                            stdout,
                            "{}:{}:{}: {on}{}{off} {}",
                            rel(&d.path),
                            d.line,
                            d.column,
                            d.code,
                            d.message
                        );
                    }
                }
                Format::Json => {
                    let _ = serde_json::to_writer_pretty(&mut stdout, &diags);
                }
                Format::Sarif => {
                    let _ = serde_json::to_writer_pretty(&mut stdout, &sarif(&diags, &rel));
                }
            }
            eprintln!(
                "{} diagnostics in {} modules ({} indexed, {} files) in {:.2?}",
                diags.len(),
                report.modules_checked,
                report.modules_indexed,
                report.files,
                started.elapsed()
            );
            if diags.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

fn sarif(diags: &[&Diagnostic], rel: &dyn Fn(&Path) -> String) -> serde_json::Value {
    let rules: Vec<_> = RULES
        .iter()
        .map(|r| json!({"id": r.code, "name": r.name, "shortDescription": {"text": r.summary}, "properties": {"category": r.category}}))
        .collect();
    let results: Vec<_> = diags
        .iter()
        .map(|d| {
            let level = match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Info => "note",
            };
            json!({
                "ruleId": d.code,
                "level": level,
                "message": {"text": d.message},
                "locations": [{"physicalLocation": {
                    "artifactLocation": {"uri": rel(&d.path)},
                    "region": {"startLine": d.line, "startColumn": d.column},
                }}],
            })
        })
        .collect();
    json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{"tool": {"driver": {"name": "odooscan", "version": env!("CARGO_PKG_VERSION"), "rules": rules}}, "results": results}],
    })
}
