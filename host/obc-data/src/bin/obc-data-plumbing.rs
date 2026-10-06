//! `obc data` without products, for scripts that fetch or use R2. It builds without the GPL step
//! crates that the `obc data` binary links.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|command| matches!(command.as_str(), "commit" | "commit-start" | "commit-status")) {
        return match obc_data::cli::commit_cli::main(&args) {
            Ok(code) => ExitCode::from(code),
            Err(message) => {
                eprintln!("obc data commit: {message}");
                ExitCode::FAILURE
            }
        };
    }
    obc_data::cli::main(&[])
}
