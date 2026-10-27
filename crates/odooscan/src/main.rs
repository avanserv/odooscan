mod config;
mod output;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::{Args, Parser, Subcommand, ValueEnum};
use odooscan_core::rules::RULES;
use odooscan_core::series::Series;
use odooscan_core::{Diagnostic, ScanOptions, scan};

use crate::config::{Config, Exclude};

#[derive(Parser)]
#[command(name = "oscn", version, about = "Quality and security scanner for Odoo modules")]
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

/// A flag or environment variable replaces the setting of the same name in the configuration file.
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
    /// Never report findings in files matching these gitignore-style patterns.
    #[arg(long, value_delimiter = ',')]
    exclude: Vec<String>,
    /// Settings file; by default the nearest odooscan.toml, or pyproject.toml with [tool.odooscan].
    #[arg(long)]
    config: Option<PathBuf>,
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

/// What `check` runs with once flags and the configuration file are merged.
struct Settings {
    scan: ScanOptions,
    select: Vec<String>,
    ignore: Vec<String>,
    exclude: Exclude,
}

impl Settings {
    fn resolve(args: &CheckArgs, cwd: &Path) -> Result<Self, String> {
        let loaded = config::load(args.config.as_deref(), cwd)?;
        let (file, dir) = loaded.map_or_else(|| (Config::default(), cwd.to_path_buf()), |l| (l.config, l.dir));
        let odoo_version = match (args.odoo_version, &file.odoo_version) {
            (Some(s), _) => Some(s),
            (None, Some(v)) => Some(parse_series(v)?),
            (None, None) => None,
        };
        // A pattern applies where it is written: flags from here, the file from its directory.
        let exclude = if args.exclude.is_empty() {
            Exclude::new(&dir, &file.exclude)?
        } else {
            Exclude::new(cwd, &args.exclude)?
        };
        Ok(Self {
            scan: ScanOptions {
                paths: args.paths.clone(),
                addons_paths: or(&args.addons_path, file.addons_path),
                default_series: odoo_version,
            },
            select: or(&args.select, file.select),
            ignore: or(&args.ignore, file.ignore),
            exclude,
        })
    }

    fn reports(&self, d: &Diagnostic) -> bool {
        let matches = |list: &[String]| list.iter().any(|p| d.code.starts_with(p.as_str()));
        (self.select.is_empty() || matches(&self.select)) && !matches(&self.ignore) && !self.exclude.matches(&d.path)
    }
}

fn or<T: Clone>(flag: &[T], file: Vec<T>) -> Vec<T> {
    if flag.is_empty() { file } else { flag.to_vec() }
}

fn parse_series(s: &str) -> Result<Series, String> {
    Series::parse(s).ok_or_else(|| format!("'{s}' is not a series such as 18.0"))
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Rules { odoo_version } => rules(odoo_version),
        Command::Check(args) => check(&args),
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

fn check(args: &CheckArgs) -> ExitCode {
    let started = Instant::now();
    let cwd = std::env::current_dir().unwrap_or_default();
    let settings = match Settings::resolve(args, &cwd) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    let report = scan(&settings.scan);
    let diags: Vec<&Diagnostic> = report.diagnostics.iter().filter(|d| settings.reports(d)).collect();
    for notice in &report.notices {
        eprintln!("notice: {notice}");
    }
    let rel = |p: &Path| p.strip_prefix(&cwd).unwrap_or(p).display().to_string();
    let mut stdout = std::io::stdout().lock();
    match args.format {
        Format::Text if args.statistics => output::print_statistics(&mut stdout, &diags),
        Format::Text => output::print_text(&mut stdout, &diags, &rel),
        Format::Json => {
            let _ = serde_json::to_writer_pretty(&mut stdout, &diags);
        }
        Format::Sarif => {
            let _ = serde_json::to_writer_pretty(&mut stdout, &output::sarif(&diags, &rel));
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
