use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::{Args, Parser, Subcommand, ValueEnum};
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
    Check(CheckArgs),
    /// List the rules, optionally only those that apply to a series.
    Rules {
        #[arg(long, value_parser = parse_series)]
        odoo_version: Option<Series>,
    },
}

#[derive(Args)]
struct CheckArgs {
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
        Command::Rules { odoo_version } => rules(odoo_version),
        Command::Check(args) => check(args),
    }
}

fn rules(odoo_version: Option<Series>) -> ExitCode {
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

fn check(args: CheckArgs) -> ExitCode {
    let started = Instant::now();
    let report = scan(&ScanOptions {
        paths: args.paths,
        addons_paths: args.addons_path,
        default_series: args.odoo_version,
    });
    let matches = |code: &str, list: &[String]| list.iter().any(|p| code.starts_with(p.as_str()));
    let diags: Vec<&Diagnostic> = report
        .diagnostics
        .iter()
        .filter(|d| (args.select.is_empty() || matches(d.code, &args.select)) && !matches(d.code, &args.ignore))
        .collect();
    for notice in &report.notices {
        eprintln!("notice: {notice}");
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    let rel = |p: &Path| p.strip_prefix(&cwd).unwrap_or(p).display().to_string();
    let mut stdout = std::io::stdout().lock();
    match args.format {
        Format::Text if args.statistics => print_statistics(&mut stdout, &diags),
        Format::Text => print_text(&mut stdout, &diags, &rel),
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

fn print_statistics(out: &mut impl Write, diags: &[&Diagnostic]) {
    let mut counts: Vec<(&str, usize)> = vec![];
    for d in diags {
        match counts.iter_mut().find(|(c, _)| *c == d.code) {
            Some((_, n)) => *n += 1,
            None => counts.push((d.code, 1)),
        }
    }
    counts.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    for (code, n) in counts {
        let _ = writeln!(out, "{n:>6}  {code}  {}", odooscan_core::rules::rule(code).name);
    }
}

fn print_text(out: &mut impl Write, diags: &[&Diagnostic], rel: &dyn Fn(&Path) -> String) {
    let color = std::io::stdout().is_terminal();
    for d in diags {
        let (on, off) = if color { ansi(d.severity) } else { ("", "") };
        let _ = writeln!(
            out,
            "{}:{}:{}: {on}{}{off} {}",
            rel(&d.path),
            d.line,
            d.column,
            d.code,
            d.message
        );
    }
}

fn ansi(severity: Severity) -> (&'static str, &'static str) {
    match severity {
        Severity::Error => ("\x1b[1;31m", "\x1b[0m"),
        Severity::Warning => ("\x1b[1;33m", "\x1b[0m"),
        Severity::Info => ("\x1b[1;36m", "\x1b[0m"),
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
