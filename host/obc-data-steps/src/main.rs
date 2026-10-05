//! `obc data` with the products whose steps make the releases.

use std::process::ExitCode;

fn main() -> ExitCode {
    obc_data::cli::main(obc_data_steps::PRODUCTS)
}
