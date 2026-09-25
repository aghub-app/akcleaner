use std::{
    env,
    io::{self, IsTerminal},
    path::{Path, PathBuf},
    process::ExitCode,
};

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};

use crate::{discovery, interactive, rules};

#[derive(Debug, Parser)]
#[command(
    name = "akcleaner",
    version,
    color = clap::ColorChoice::Never,
    about = "交互式清理 agent 集成；也可使用 scan 只读检查"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Clean {
        #[arg(long)]
        rules_dir: Option<PathBuf>,
        #[arg(long)]
        home: Option<PathBuf>,
        #[arg(long)]
        product: Option<String>,
    },
    Rules {
        #[command(subcommand)]
        command: RulesCommand,
    },
    Scan {
        #[arg(long)]
        rules_dir: Option<PathBuf>,
        #[arg(long)]
        home: Option<PathBuf>,
        #[arg(long)]
        product: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum RulesCommand {
    Check {
        #[arg(long)]
        rules_dir: Option<PathBuf>,
    },
}

pub fn run() -> anyhow::Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Clean {
        rules_dir: None,
        home: None,
        product: None,
    }) {
        Command::Rules {
            command: RulesCommand::Check { rules_dir },
        } => {
            let path = rules_dir.unwrap_or(resolve_cwd_rules_dir()?);
            let loaded = rules::load_rules(&path)?;
            println!(
                "Rules valid: {} products, {} rules.",
                loaded.len(),
                loaded
                    .iter()
                    .map(|ruleset| ruleset.rules.len())
                    .sum::<usize>()
            );
            for ruleset in loaded {
                println!("{} ({})", ruleset.product.name, ruleset.product.id);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Scan {
            rules_dir,
            home,
            product,
            json,
        } => run_scan(rules_dir, home, product.as_deref(), json),
        Command::Clean {
            rules_dir,
            home,
            product,
        } => Ok(run_clean(rules_dir, home, product.as_deref()).unwrap_or(ExitCode::FAILURE)),
    }
}

fn run_scan(
    rules_dir: Option<PathBuf>,
    home: Option<PathBuf>,
    product: Option<&str>,
    json: bool,
) -> anyhow::Result<ExitCode> {
    let loaded = load_for_scan_or_clean(rules_dir)?;
    let selected_rules = select_product(loaded, product)?;
    let home = resolve_home(home)?;
    let report = discovery::scan(&home, &selected_rules);
    if json {
        serde_json::to_writer_pretty(std::io::stdout(), &report)
            .context("cannot serialize scan report")?;
        println!();
    } else {
        print_text_report(&home, &report);
    }
    Ok(if report.is_complete() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn run_clean(
    rules_dir: Option<PathBuf>,
    home: Option<PathBuf>,
    product: Option<&str>,
) -> anyhow::Result<ExitCode> {
    let loaded = load_for_scan_or_clean(rules_dir)?;
    let selected_rules = select_product(loaded, product)?;
    if !interactive_terminal_available() {
        return Ok(ExitCode::FAILURE);
    }

    let home = resolve_home(home)?;
    let report = discovery::scan(&home, &selected_rules);
    interactive::run_clean(&home, &selected_rules, &report)
}

fn load_for_scan_or_clean(path: Option<PathBuf>) -> anyhow::Result<Vec<rules::RuleSet>> {
    match path {
        Some(path) => rules::load_rules(&path)
            .with_context(|| format!("cannot load rules from {}", display_path(&path))),
        None => rules::load_bundled_rules().context("cannot load bundled product rules"),
    }
}

fn select_product(
    mut loaded: Vec<rules::RuleSet>,
    product_id: Option<&str>,
) -> anyhow::Result<Vec<rules::RuleSet>> {
    if let Some(product_id) = product_id {
        loaded.retain(|ruleset| ruleset.product.id == product_id);
        if loaded.is_empty() {
            bail!("unknown product id {product_id:?} in the loaded rules");
        }
    }
    Ok(loaded)
}

fn resolve_home(path: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    let home = match path {
        Some(path) => path,
        None => current_home()?.to_owned(),
    };
    if home.is_absolute() {
        Ok(home)
    } else {
        Ok(env::current_dir()
            .context("cannot determine current directory")?
            .join(home))
    }
}

fn current_home() -> anyhow::Result<PathBuf> {
    let value = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .context("cannot determine current user's home; pass --home explicitly")?;
    Ok(PathBuf::from(value))
}

fn resolve_cwd_rules_dir() -> anyhow::Result<PathBuf> {
    Ok(env::current_dir()
        .context("cannot determine current directory")?
        .join("rules"))
}

fn interactive_terminal_available() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal() && io::stderr().is_terminal()
}

fn print_text_report(home: &Path, report: &discovery::ScanReport) {
    if report.findings.is_empty() {
        println!("No findings.");
    } else {
        for finding in &report.findings {
            println!(
                "{} {} {} {} [{} / {}]: {}",
                safe_text(&finding.product_id),
                safe_text(&finding.rule_id),
                safe_text(&finding.surface),
                display_home_path(home, &finding.path),
                match &finding.attribution {
                    discovery::Attribution::Confirmed => "confirmed",
                    discovery::Attribution::Candidate => "candidate",
                    discovery::Attribution::Conflicting => "conflicting",
                },
                match &finding.integrity {
                    discovery::Integrity::Unchanged => "unchanged",
                    discovery::Integrity::Modified => "modified",
                    discovery::Integrity::Unknown => "unknown",
                },
                safe_text(&finding.evidence)
            );
            if !finding.locator.is_empty() {
                println!("  locator: {}", safe_text(&finding.locator));
            }
        }
    }
    for diagnostic in &report.diagnostics {
        println!(
            "Diagnostic {} {} {}: {}",
            safe_text(&diagnostic.surface),
            display_home_path(home, &diagnostic.path),
            safe_text(&diagnostic.code),
            safe_text(&diagnostic.message)
        );
    }
    println!("Coverage: {}", safe_text(&report.coverage.scope));
}

fn display_home_path(home: &Path, path: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(relative) if relative.as_os_str().is_empty() => "~".to_owned(),
        Ok(relative) => format!("~/{}", safe_text(&relative.to_string_lossy())),
        Err(_) => safe_text(&path.to_string_lossy()),
    }
}

fn display_path(path: &Path) -> String {
    safe_text(&path.to_string_lossy())
}

fn safe_text(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                format!("\\u{{{:x}}}", u32::from(character))
            } else {
                character.to_string()
            }
        })
        .collect()
}
