//! A checked producer worker, including the Wikimedia selection callbacks.

use std::process::ExitCode;

fn main() -> ExitCode {
    if let Err(error) = obc_data::worker::enter() {
        eprintln!("obc data: {error}");
        return ExitCode::FAILURE;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(result) = obc_pack::landmarks::select::run(&args) {
        return match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("obc data: {error}");
                ExitCode::FAILURE
            }
        };
    }
    match std::env::current_exe() {
        Ok(binary) => obc_data::fetch::capture::select_with(binary),
        Err(error) => {
            eprintln!("obc data: {error}");
            return ExitCode::FAILURE;
        }
    }
    obc_data::cli::main(obc_data_steps::PRODUCTS)
}
