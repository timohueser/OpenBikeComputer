//! `obc data` without products, for scripts that use R2. It builds without the GPL step crates
//! that the `obc data` binary links.

fn main() -> std::process::ExitCode {
    obc_data::cli::main(&[])
}
