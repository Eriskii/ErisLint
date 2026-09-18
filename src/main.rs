use std::{
    env,
    io::{self, IsTerminal, Read, Write},
    num::NonZeroUsize,
    path::PathBuf,
    process::ExitCode,
};

use anyhow::{Result, anyhow};
use clap::{Parser, ValueEnum};
use erislint::{
    config::{Config, ConfigFile, RuleFile},
    jev::JevClient,
    output::{TextOptions, write_text},
    runner::Plan,
};
use serde::Serialize;

#[derive(Parser)]
#[command(version, about = "Evaluate configurable Rust lint rules with Jev")]
struct Cli {
    /// Rust files or directories to lint. Defaults to the configuration directory.
    paths: Vec<PathBuf>,
    /// Read an unsaved snapshot from stdin, using this existing Rust file's identity.
    #[arg(long, conflicts_with_all = ["paths", "check_config", "schema"])]
    stdin_file: Option<PathBuf>,
    /// Evaluate only the function whose name starts at this UTF-8 byte offset.
    #[arg(long, requires = "stdin_file")]
    target_start: Option<usize>,
    /// Use an explicit configuration instead of discovering erislint.json.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Validate configuration and referenced rule files without calling Jev.
    #[arg(long, conflicts_with_all = ["dry_run", "schema"])]
    check_config: bool,
    /// Print AST inputs and Jev requests as JSON without reading jev_key or calling the API.
    #[arg(long, conflicts_with = "schema")]
    dry_run: bool,
    /// Print the configuration or rule-file JSON Schema.
    #[arg(long, value_enum)]
    schema: Option<SchemaKind>,
    /// Diagnostic output format.
    #[arg(long, value_enum, default_value = "text")]
    format: Format,
    /// Include every option's probability in text output.
    #[arg(long)]
    all_answers: bool,
    /// Show only error diagnostics; exit-status rules still apply to the full run.
    #[arg(long, conflicts_with = "all_answers")]
    errors_only: bool,
    /// Colorize text diagnostics. Auto uses color only in a terminal.
    #[arg(long, value_enum, default_value = "auto")]
    color: Color,
    /// Maximum number of concurrent Jev requests.
    #[arg(long, default_value = "64")]
    jobs: NonZeroUsize,
    /// Exit unsuccessfully when warnings are emitted, too.
    #[arg(long)]
    deny_warnings: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
}

#[derive(Clone, Copy, ValueEnum)]
enum Color {
    Auto,
    Always,
    Never,
}

#[derive(Clone, Copy, ValueEnum)]
enum SchemaKind {
    Config,
    Rule,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("erislint: {error:#}");
            ExitCode::from(2)
        }
    }
}

async fn run(cli: Cli) -> Result<u8> {
    if let Some(kind) = cli.schema {
        let schema = match kind {
            SchemaKind::Config => schemars::schema_for!(ConfigFile),
            SchemaKind::Rule => schemars::schema_for!(RuleFile),
        };
        write_json(&schema)?;
        return Ok(0);
    }
    let path = match cli.config {
        Some(path) => path,
        None => Config::discover(&env::current_dir()?)?,
    };
    let config = Config::load(&path)?;
    if cli.check_config {
        match cli.format {
            Format::Text => println!(
                "Configuration valid: {} rules ({})",
                config.rules.len(),
                config.path.display()
            ),
            Format::Json => write_json(
                &serde_json::json!({ "config": config.path, "rules": config.rules.len(), "valid": true }),
            )?,
        }
        return Ok(0);
    }
    let mut plan = if let Some(path) = cli.stdin_file {
        let mut source = String::new();
        io::stdin().read_to_string(&mut source)?;
        Plan::from_source(&config, &path, &source)?
    } else {
        Plan::build(&config, &cli.paths)?
    };
    if let Some(start) = cli.target_start {
        plan.select_function(start)?;
    }
    if cli.dry_run {
        write_json(&plan)?;
        return Ok(0);
    }
    let mut report =
        if plan.evaluations.is_empty() {
            plan.empty_report()
        } else {
            let key = env::var("jev_key").map_err(|_| anyhow!(
            "set the jev_key environment variable to your TypeSafe API key (or use --dry-run)"
        ))?;
            let client = JevClient::new(&key)?;
            if io::stderr().is_terminal() {
                eprintln!("Checking {} Rust files...", plan.files);
            }
            plan.run(&config, &client, cli.jobs).await?
        };
    let exit_code = u8::from(report.errors > 0 || (cli.deny_warnings && report.warnings > 0));
    if cli.errors_only {
        report.retain_errors();
    }
    match cli.format {
        Format::Text => write_text(
            io::stdout().lock(),
            &report,
            &plan,
            TextOptions {
                errors_only: cli.errors_only,
                all_answers: cli.all_answers,
                color: match cli.color {
                    Color::Auto => io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none(),
                    Color::Always => true,
                    Color::Never => false,
                },
            },
        )?,
        Format::Json => write_json(&report)?,
    }
    Ok(exit_code)
}

fn write_json(value: &impl Serialize) -> Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, value)?;
    writeln!(stdout)?;
    Ok(())
}
