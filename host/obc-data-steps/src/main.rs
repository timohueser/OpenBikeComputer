//! Build and launch the producer worker from the current checkout.

mod launcher;

fn main() -> std::process::ExitCode {
    match launcher::run() {
        Ok(code) => std::process::ExitCode::from(code),
        Err(error) => {
            eprintln!("obc data: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
