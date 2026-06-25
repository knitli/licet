//! CLI definition (clap) and command dispatch (contracts/cli.md).

pub mod apply;
pub mod check;
pub mod init;
pub mod lint;

use std::path::PathBuf;

use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

use crate::error::{ExitCode, LicetError, Result};
use crate::spdx;
use crate::walk::Selection;

/// `licet` — declarative license-header management.
#[derive(Debug, Parser)]
#[command(name = "licet", version, about, long_about = None)]
#[command(disable_version_flag = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Print the tool version and embedded SPDX license-list version (FR-028).
    #[arg(long, short = 'V', global = true)]
    pub version: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Non-writing gate: classify drift and exit pass/fail.
    Check(CheckArgs),
    /// Reconcile files to declared intent.
    Apply(ApplyArgs),
    /// Derive a config from current repository state.
    Init(InitArgs),
    /// Report REUSE-compatibility posture and license-text completeness.
    Lint(LintArgs),
    /// Generate a shell completion script (write to your shell's completion dir).
    Completions(CompletionsArgs),
}

/// Output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    Human,
    Json,
}

/// Flags shared by `check` and `apply` (selection, config, format, cache).
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Path to the declarative config.
    #[arg(long, default_value = "license.toml", global = true)]
    pub config: PathBuf,

    /// Output rendering.
    #[arg(long, value_enum, default_value_t = Format::Human, global = true)]
    pub format: Format,

    /// Restrict evaluation to an explicit file list.
    #[arg(long, num_args = 1.., global = true)]
    pub files: Vec<PathBuf>,

    /// Read the file list from a file (one path per line).
    #[arg(long, global = true)]
    pub files_from: Option<PathBuf>,

    /// Read the file list from stdin.
    #[arg(long, global = true)]
    pub stdin: bool,

    /// Restrict to git-staged files.
    #[arg(long, global = true)]
    pub staged: bool,

    /// Restrict to files changed vs `<rev>` (default HEAD).
    #[arg(long, value_name = "REV", num_args = 0..=1, default_missing_value = "HEAD", global = true)]
    pub changed: Option<String>,

    /// Disable the scan cache.
    #[arg(long, global = true)]
    pub no_cache: bool,

    /// Relocate the scan cache.
    #[arg(long, value_name = "PATH", global = true)]
    pub cache: Option<PathBuf>,
}

impl CommonArgs {
    /// Resolve the file selection, enforcing mutual exclusivity (FR-027).
    pub fn selection(&self) -> Result<Selection> {
        let mut chosen = 0;
        if !self.files.is_empty() || self.files_from.is_some() || self.stdin {
            chosen += 1;
        }
        if self.staged {
            chosen += 1;
        }
        if self.changed.is_some() {
            chosen += 1;
        }
        if chosen > 1 {
            return Err(LicetError::Config(
                "selection flags --staged / --changed / --files are mutually exclusive".to_string(),
            ));
        }

        if self.staged {
            return Ok(Selection::Staged);
        }
        if let Some(rev) = &self.changed {
            return Ok(Selection::Changed(Some(rev.clone())));
        }
        if !self.files.is_empty() || self.files_from.is_some() || self.stdin {
            let mut files = self.files.clone();
            if let Some(from) = &self.files_from {
                let text = std::fs::read_to_string(from)
                    .map_err(|e| LicetError::Config(format!("cannot read --files-from: {e}")))?;
                files.extend(
                    text.lines()
                        .filter(|l| !l.trim().is_empty())
                        .map(PathBuf::from),
                );
            }
            if self.stdin {
                use std::io::Read;
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf)?;
                files.extend(
                    buf.lines()
                        .filter(|l| !l.trim().is_empty())
                        .map(PathBuf::from),
                );
            }
            return Ok(Selection::Files(files));
        }
        Ok(Selection::FullTree)
    }
}

#[derive(Debug, Args)]
pub struct CheckArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Print which rule matched `<path>` and why, then exit.
    #[arg(long, value_name = "PATH")]
    pub explain: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ApplyArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Add the declared header without removing existing license lines.
    #[arg(long)]
    pub additive: bool,
    /// When a file has multiple headers, choose which block is replaced.
    #[arg(long, value_name = "INDEX")]
    pub target_header: Option<usize>,
    /// Modify files even when the working tree has uncommitted changes.
    #[arg(long)]
    pub allow_dirty: bool,
    /// Compute and print the reconciliation plan without modifying any file.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct InitArgs {
    #[arg(long, default_value = "license.toml")]
    pub config: PathBuf,
    /// Derive from existing REUSE state (headers + REUSE.toml/.reuse/dep5).
    #[arg(long)]
    pub from_reuse: bool,
    /// Output path for the generated config.
    #[arg(long)]
    pub output: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub format: Format,
}

#[derive(Debug, Args)]
pub struct LintArgs {
    #[arg(long, default_value = "license.toml")]
    pub config: PathBuf,
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub format: Format,
    /// Permit fetching license ids absent from the offline bundle.
    #[arg(long)]
    pub allow_network: bool,
}

#[derive(Debug, Args)]
pub struct CompletionsArgs {
    /// Target shell (bash, zsh, fish, powershell, elvish).
    #[arg(value_enum)]
    pub shell: Shell,
}

/// Print a completion script for `shell` to stdout.
pub fn print_completions(shell: Shell) {
    let mut cmd = Cli::command();
    let name = cmd.get_name().to_string();
    clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
}

/// Render the `--version` line including the embedded SPDX list version (FR-028).
pub fn version_string() -> String {
    format!(
        "licet {} (SPDX license list {})",
        env!("CARGO_PKG_VERSION"),
        spdx::spdx_list_version()
    )
}

/// Dispatch a parsed CLI to its command, returning the process exit code.
pub fn dispatch(cli: Cli) -> Result<ExitCode> {
    if cli.version {
        println!("{}", version_string());
        return Ok(ExitCode::Success);
    }
    match cli.command {
        Some(Command::Check(args)) => check::run(args),
        Some(Command::Apply(args)) => apply::run(args),
        Some(Command::Init(args)) => init::run(args),
        Some(Command::Lint(args)) => lint::run(args),
        Some(Command::Completions(args)) => {
            print_completions(args.shell);
            Ok(ExitCode::Success)
        }
        None => Err(LicetError::Config(
            "no subcommand given (try `licet check`, `apply`, `init`, `lint`, or `--version`)"
                .to_string(),
        )),
    }
}
