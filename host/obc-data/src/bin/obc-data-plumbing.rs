//! `obc data` without products, for scripts that fetch or use R2. It builds without the GPL step
//! crates that the `obc data` binary links.

use std::process::ExitCode;

fn main() -> ExitCode {
    obc_data::cli::main(&[])
}
