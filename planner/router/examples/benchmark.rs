//! Complete native route requests with fresh and retained application caches.
#[path = "support/benchmark.rs"]
mod benchmark;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let package = args.next().ok_or("Usage: benchmark PACKAGE REQUESTS.json [ITERATIONS] [MEMORY_MIB]")?;
    let cases: Vec<benchmark::Case> =
        serde_json::from_slice(&std::fs::read(args.next().ok_or("Missing request corpus")?)?)?;
    let iterations = args.next().map(|s| s.parse()).transpose()?.unwrap_or(3);
    let memory_mib: Option<usize> = args.next().map(|s| s.parse()).transpose()?;
    let memory =
        memory_mib.map(|mib| mib.checked_mul(1024 * 1024).ok_or("Invalid routing memory budget")).transpose()?;
    let mode = args.next();
    let options = benchmark::Options { retained: mode.as_deref() == Some("retained") };
    let result = benchmark::run(std::path::Path::new(&package), &cases, iterations, memory, options)?;
    serde_json::to_writer(std::io::stdout(), &result)?;
    Ok(())
}
