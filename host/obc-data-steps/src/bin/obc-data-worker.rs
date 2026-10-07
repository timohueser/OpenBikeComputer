//! A checked producer worker, including the Wikimedia selection callbacks.

use std::process::ExitCode;

fn main() -> ExitCode {
    if let Err(error) =
        obc_data::worker::enter(option_env!("OBC_DATA_COMPILED_ROOT"), option_env!("OBC_DATA_COMPILED_CODE"))
    {
        return obc_data::cli::failed(error);
    }
    obc_pack::step::geos_startup();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(result) = obc_pack::landmarks::select::run(&args) {
        return match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => obc_data::cli::failed(error),
        };
    }
    match std::env::current_exe() {
        Ok(binary) => obc_data::fetch::capture::select_with(binary),
        Err(error) => return obc_data::cli::failed(error.to_string()),
    }
    obc_data::cli::main(obc_data_steps::PRODUCTS)
}
