//! Operator verify-replay tool. Links the library; does not reimplement compare.

use prairie_roots_farm_os_lib::{farm_dir_verify, verify_replay_cli_error, VerifyOutcome};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args();
    let _argv0 = args.next();
    let farm = match args.next() {
        Some(p) => p,
        None => {
            eprintln!("{}", verify_replay_cli_error(None));
            return ExitCode::from(1);
        }
    };
    if let Some(extra) = args.next() {
        eprintln!("{}", verify_replay_cli_error(Some(&extra)));
        return ExitCode::from(1);
    }

    let farm_dir = PathBuf::from(&farm);
    match farm_dir_verify(&farm_dir) {
        Ok(outcome) => exit_for_outcome(&outcome),
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

fn exit_for_outcome(outcome: &VerifyOutcome) -> ExitCode {
    if outcome.exit_nonzero() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
