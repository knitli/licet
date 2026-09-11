//! CLI definition (clap) and command dispatch (contracts/cli.md).

pub mod add_license;
pub mod apply;
pub mod check;
pub mod init;
pub mod lint;

use std::path::{Path, PathBuf};

use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

use crate::config::CONFIG_FILENAME;
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
    /// Materialize referenced license texts into LICENSES/ from the offline bundle.
    #[command(visible_alias = "add")]
    AddLicense(AddLicenseArgs),
    /// Generate a shell completion script (write to your shell's completion dir).
    Completions(CompletionsArgs),
}

/// Output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    Human,
    Json,
}

/// How `apply` covers non-annotatable files (overrides `[output] non_annotatable`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum NonAnnotatable {
    /// Write a `<file>.license` sidecar.
    Sidecar,
    /// Append an entry to the central `REUSE.toml`.
    ReuseToml,
}

impl From<NonAnnotatable> for crate::domain::NonAnnotatableStrategy {
    fn from(n: NonAnnotatable) -> Self {
        match n {
            NonAnnotatable::Sidecar => Self::Sidecar,
            NonAnnotatable::ReuseToml => Self::ReuseToml,
        }
    }
}

/// Flags shared by `check` and `apply` (selection, config, format, cache).
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Path to the declarative config (default: `<root>/licet.toml` from
    /// the discovered root, so subdirectories work; an explicit relative
    /// path resolves from the invocation cwd).
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

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

    /// Deprecated no-op (retained for compatibility): scans are stateless
    /// and never cache. May print a stderr notice.
    #[arg(long, global = true)]
    pub no_cache: bool,

    /// Deprecated no-op (retained for compatibility): scans are stateless
    /// and never cache. May print a stderr notice.
    #[arg(long, value_name = "PATH", global = true)]
    pub cache: Option<PathBuf>,
}

impl CommonArgs {
    /// Resolve the config argument against the discovered root: an explicit
    /// path stays invocation-cwd-relative (absolute passes through); an
    /// omitted config defaults to `<root>/licet.toml` so every command
    /// works from a subdirectory (FR-001).
    pub fn config_arg(&self, cwd: &Path, root: &Path) -> PathBuf {
        match &self.config {
            Some(p) if p.is_absolute() => p.clone(),
            Some(p) => cwd.join(p),
            None => root.join(CONFIG_FILENAME),
        }
    }

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
    /// How to cover non-annotatable files, overriding `[output] non_annotatable`.
    #[arg(long, value_enum)]
    pub non_annotatable: Option<NonAnnotatable>,
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Read as the destination when `--output` is absent (init writes a
    /// config, so `--config` names where it goes, not where policy comes
    /// from). Defaults to `<root>/licet.toml`.
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Compatibility alias: init always inspects existing REUSE state
    /// (headers plus `REUSE.toml`/`.reuse/dep5`), so this changes nothing.
    #[arg(long)]
    pub from_reuse: bool,
    /// Output path for the generated config (overrides `--config`).
    /// Relative paths resolve from the invocation cwd.
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// Replace the destination when it already exists. Without it, init
    /// creates new and refuses to overwrite (existing files and symlinks
    /// are never followed or truncated).
    #[arg(long)]
    pub force: bool,
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub format: Format,
}

#[derive(Debug, Args)]
pub struct LintArgs {
    /// Explicit policy config path (compatibility only): it must exist and
    /// parse, is accepted with a deprecation notice, and never changes REUSE
    /// evaluation. Omitted by default — lint needs no configuration.
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub format: Format,
    /// Permit fetching license ids absent from the offline bundle.
    #[arg(long)]
    pub allow_network: bool,
}

#[derive(Debug, Args)]
pub struct AddLicenseArgs {
    /// SPDX identifiers to materialize into LICENSES/ (omit when using --all).
    #[arg(value_name = "SPDX-ID")]
    pub ids: Vec<String>,
    /// Materialize every referenced-but-missing license text.
    #[arg(long)]
    pub all: bool,
    /// Permit fetching ids absent from the offline bundle (the hermetic binary never
    /// reaches the network; accepted for parity with `lint`/FR-017).
    #[arg(long)]
    pub allow_network: bool,
    /// Path to the declarative config (only read by --all to discover referenced ids).
    /// Defaults to `<root>/licet.toml`; an explicit relative path resolves
    /// from the invocation cwd.
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub format: Format,
}

#[derive(Debug, Args)]
pub struct CompletionsArgs {
    /// Target shell (bash, zsh, fish, powershell, elvish).
    #[arg(value_enum)]
    pub shell: Shell,
}

/// Print a completion script for `shell` to stdout.
pub fn print_completions(shell: Shell) -> Result<()> {
    let mut cmd = Cli::command();
    let name = cmd.get_name().to_string();
    let mut buf = Vec::new();
    clap_complete::generate(shell, &mut cmd, name, &mut buf);
    emit_stdout(&String::from_utf8_lossy(&buf))
}

/// Write a complete output document to stdout.
///
/// A closed pipe (the reader went away first, e.g. `| head`) terminates
/// quietly with exit 0 instead of panicking on `EPIPE`; any other output
/// error is returned normally for the caller to report.
pub fn emit_stdout(text: &str) -> Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let mut quiet_pipe = |e: std::io::Error| {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
        e
    };
    out.write_all(text.as_bytes()).map_err(&mut quiet_pipe)?;
    out.flush().map_err(quiet_pipe)?;
    Ok(())
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
        emit_stdout(&format!("{}\n", version_string()))?;
        return Ok(ExitCode::Success);
    }
    match cli.command {
        Some(Command::Check(args)) => check::run(args),
        Some(Command::Apply(args)) => apply::run(args),
        Some(Command::Init(args)) => init::run(args),
        Some(Command::Lint(args)) => lint::run(args),
        Some(Command::AddLicense(args)) => add_license::run(args),
        Some(Command::Completions(args)) => {
            print_completions(args.shell)?;
            Ok(ExitCode::Success)
        }
        None => Err(LicetError::Config(
            "no subcommand given (try `licet check`, `apply`, `init`, `lint`, `add-license`, or \
             `--version`)"
                .to_string(),
        )),
    }
}
