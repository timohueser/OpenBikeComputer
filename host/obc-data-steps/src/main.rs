//! `obc data` with the products whose steps make the releases. It also answers the selection
//! commands that the Wikimedia captures call back (`obc_pack::landmarks::select`).

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(result) = obc_pack::landmarks::select::run(&args) {
        return match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("obc data: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if let Ok(binary) = std::env::current_exe() {
        obc_data::fetch::capture::select_with(binary);
    }
    obc_data::cli::main(obc_data_steps::PRODUCTS)
}
