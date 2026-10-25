//! `obc data` with the steps of every product, the producers of prepared sources and the page of
//! `obc data ui`.

use std::process::ExitCode;

fn main() -> ExitCode {
    obc_data::engine::child(obc_data_steps::FUNCTIONS);
    // A landmark capture can call this binary back to select with the rules of the compiler.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(result) = obc_pack::landmarks::select::run(&args) {
        return match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => obc_data::cli::failed(error),
        };
    }
    obc_data::cli::main(obc_data_steps::PRODUCTS, obc_data_steps::PREPARED, Some(obc_data_steps::ui::run))
}
