//! `licet` binary entry: parse CLI, dispatch, map result → exit code (contracts/cli.md).

use std::process::ExitCode as ProcExit;

use clap::Parser;
use licet::cli::{self, Cli};
use licet::error::ExitCode;

fn main() -> ProcExit {
    // `--version` is handled inside dispatch so it can include the SPDX list version.
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            // clap's help/version requests print to stdout and exit 0; real usage errors
            // map to exit code 2 (contracts/cli.md).
            let _ = e.print();
            return match e.kind() {
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => {
                    ProcExit::from(0)
                }
                _ => ProcExit::from(ExitCode::Usage.code() as u8),
            };
        }
    };

    match cli::dispatch(cli) {
        Ok(code) => ProcExit::from(code.code() as u8),
        Err(e) => {
            eprintln!("error: {e}");
            ProcExit::from(e.exit_code().code() as u8)
        }
    }
}
