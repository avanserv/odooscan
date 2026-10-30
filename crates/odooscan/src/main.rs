mod baseline;
mod config;
mod output;

use std::io::IsTerminal;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::{Args, Parser, Subcommand, ValueEnum};
use odooscan_core::rules::RULES;
use odooscan_core::series::Series;
use odooscan_core::{Diagnostic, ScanOptions, scan};

use crate::baseline::Baseline;
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
    /// Print a rule's documentation, given its code or its name.
    Explain { rule: String },
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
    /// Hide the findings recorded in this file.
    #[arg(long)]
    baseline: Option<PathBuf>,
    /// Record every current finding in the baseline file, by default odooscan-baseline.json.
    #[arg(long)]
    write_baseline: bool,
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
    /// Annotations on a GitHub pull request.
    Github,
    /// A GitLab Code Quality report.
    Gitlab,
}

/// What `check` runs with once flags and the configuration file are merged.
struct Settings {
    scan: ScanOptions,
    select: Vec<String>,
    ignore: Vec<String>,
    exclude: Exclude,
    baseline: Option<PathBuf>,
}

impl Settings {
    fn resolve(args: &CheckArgs, cwd: &Path) -> Result<Self, String> {
        let loaded = config::load(args.config.as_deref(), cwd)?;
        let (file, dir) = loaded.map_or_else(|| (Config::default(), cwd.to_path_buf()), |l| (l.config, l.dir));
        let odoo_version = default_series(args.odoo_version, file.odoo_version.as_deref())?;
        // A pattern applies where it is written: flags from here, the file from its directory.
        let exclude = if args.exclude.is_empty() {
            Exclude::new(&dir, &file.exclude)?
        } else {
            Exclude::new(cwd, &args.exclude)?
        };
        let baseline = args.baseline.clone().or(file.baseline);
        let baseline = match baseline {
            None if args.write_baseline => Some(dir.join(baseline::DEFAULT_FILE)),
            b => b,
        };
        Ok(Self {
            scan: ScanOptions {
                paths: args.paths.clone(),
                addons_paths: addons_paths(&or(&args.addons_path, file.addons_path))?,
                default_series: odoo_version,
            },
            select: or(&args.select, file.select),
            ignore: or(&args.ignore, file.ignore),
            exclude,
            baseline,
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

fn default_series(flag: Option<Series>, file: Option<&str>) -> Result<Option<Series>, String> {
    match (flag, file) {
        (Some(s), _) => Ok(Some(s)),
        (None, Some(v)) => parse_series(v).map(Some),
        (None, None) => Ok(None),
    }
}

/// Indexing around a missing entry would report every module behind it as absent.
fn addons_paths(entries: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let paths: Vec<PathBuf> = entries.iter().map(|p| config::expand_home(p)).collect();
    match paths.iter().find(|p| !p.is_dir()) {
        Some(p) => Err(format!("addons path {}: no such directory", p.display())),
        None => Ok(paths),
    }
}

fn parse_series(s: &str) -> Result<Series, String> {
    Series::parse(s).ok_or_else(|| format!("'{s}' is not a series such as 18.0"))
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Rules { odoo_version } => rules(odoo_version),
        Command::Check(args) => check(&args),
        Command::Explain { rule } => explain(&rule),
    }
}

fn explain(query: &str) -> ExitCode {
    if let Some(rule) = odooscan_core::rules::find(query) {
        print!("{}", rule.doc);
        return ExitCode::SUCCESS;
    }
    eprintln!("error: no rule is called '{query}'");
    let nearest = odooscan_core::rules::nearest(query);
    if nearest.is_empty() {
        eprintln!("`oscn rules` lists them all");
    } else {
        eprintln!("did you mean:");
        for r in nearest {
            eprintln!("  {}  {}", r.code, r.name);
        }
    }
    ExitCode::from(2)
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
    let mut diags: Vec<&Diagnostic> = report.diagnostics.iter().filter(|d| settings.reports(d)).collect();
    for notice in &report.notices {
        eprintln!("notice: {notice}");
    }
    if let (Some(path), true) = (&settings.baseline, args.write_baseline) {
        return write_baseline(path, &diags);
    }
    if let Some(path) = &settings.baseline
        && let Err(e) = apply_baseline(path, &mut diags)
    {
        eprintln!("error: {e}");
        return ExitCode::from(2);
    }
    print(args, &diags, &cwd);
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

fn print(args: &CheckArgs, diags: &[&Diagnostic], cwd: &Path) {
    // Scanning `.` yields `./a/b.py`, which GitLab and GitHub do not match to the file `a/b.py`.
    let rel = |p: &Path| {
        let p = p.strip_prefix(cwd).unwrap_or(p);
        p.components()
            .filter(|c| *c != Component::CurDir)
            .collect::<PathBuf>()
            .display()
            .to_string()
    };
    let mut stdout = std::io::stdout().lock();
    match args.format {
        Format::Text if args.statistics => output::print_statistics(&mut stdout, diags),
        Format::Text => output::print_text(&mut stdout, diags, &rel, std::io::stdout().is_terminal()),
        Format::Json => {
            let _ = serde_json::to_writer_pretty(&mut stdout, diags);
        }
        Format::Sarif => {
            let _ = serde_json::to_writer_pretty(&mut stdout, &output::sarif(diags, &rel));
        }
        Format::Github => output::print_github(&mut stdout, diags, &rel),
        Format::Gitlab => {
            let _ = serde_json::to_writer_pretty(&mut stdout, &output::gitlab(diags, &rel));
        }
    }
}

fn write_baseline(path: &Path, diags: &[&Diagnostic]) -> ExitCode {
    match Baseline::write(path, diags) {
        Ok(()) => {
            eprintln!("recorded {} findings in {}", diags.len(), path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

fn apply_baseline(path: &Path, diags: &mut Vec<&Diagnostic>) -> Result<(), String> {
    let mut baseline = Baseline::read(path)?;
    let hidden = baseline.hide(diags);
    if hidden > 0 {
        eprintln!("notice: the baseline hid {hidden} findings");
    }
    let stale = baseline.stale();
    if stale > 0 {
        eprintln!("notice: {stale} findings in the baseline are fixed; run with --write-baseline to drop them");
    }
    Ok(())
}
