//! Build and launch the producer worker from the current checkout.

mod launcher;

fn main() -> std::process::ExitCode {
    match launcher::run() {
        Ok(code) => std::process::ExitCode::from(code),
        Err(error) => obc_data::cli::failed(error),
    }
}
