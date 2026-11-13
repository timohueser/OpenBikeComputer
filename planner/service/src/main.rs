use std::path::Path;
use tokio::signal::unix::{signal, SignalKind};
use tower_http::cors::CorsLayer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let usage = "Usage: planner-service PACKAGE_DIRECTORY [--verify] | planner-service INSTALL_DIRECTORY --scrub | \
                 planner-service INSTALL_DIRECTORY --missing DIRECTORY PACKAGE | \
                 planner-service INSTALL_DIRECTORY --install DIRECTORY PACKAGE OBJECTS THREADS | \
                 planner-service --formats | planner-service --workers";
    let directory = std::env::args().nth(1).ok_or(usage)?;
    if directory == "--formats" {
        let formats = serde_json::json!({
            "planner/routing": planner_router::package::FORMAT,
            "planner/routing-cells": planner_router::cells::FORMAT,
        });
        println!("{formats}");
        return Ok(());
    }
    if directory == "--workers" {
        println!("{}", workers()?);
        return Ok(());
    }
    let args: Vec<String> = std::env::args().skip(2).collect();
    if matches!(args.first().map(String::as_str), Some("--missing" | "--install")) {
        if let Err(e) = install(Path::new(&directory), &args) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if std::env::args().nth(2).as_deref() == Some("--scrub") {
        let differ = planner_router::install::scrub(Path::new(&directory))?;
        if !differ.is_empty() {
            return Err(
                format!("{} installed files differ from the directory: {}", differ.len(), differ.join(", ")).into()
            );
        }
        eprintln!("Each installed file has its SHA-256");
        return Ok(());
    }
    if std::env::args().nth(2).as_deref() == Some("--verify") {
        let path = Path::new(&directory);
        planner_router::open(path)?.verify()?;
        eprintln!("Package object closure verified");
        return Ok(());
    }
    let address = std::env::var("ROUTE_LISTEN").unwrap_or_else(|_| "127.0.0.1:8788".into());
    let workers = workers()?;
    // The install of format 10 that the follower names goes before the package of the argument.
    let cells = std::env::var_os("ROUTE_CELLS");
    let mut app = planner_service::app(cells.as_deref().map_or(Path::new(&directory), Path::new), workers)?;
    if let Ok(origin) = std::env::var("ROUTE_ORIGIN") {
        app = app.layer(
            CorsLayer::new()
                .allow_origin(origin.parse::<axum::http::HeaderValue>()?)
                .allow_methods([axum::http::Method::GET, axum::http::Method::POST])
                .allow_headers([axum::http::header::CONTENT_TYPE])
                .max_age(std::time::Duration::from_secs(600)),
        );
    }
    let listener = tokio::net::TcpListener::bind(&address).await?;
    eprintln!("Routing service listening on {address} with {workers} workers");
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = terminate.recv() => {}
                _ = interrupt.recv() => {}
            }
        })
        .await?;
    Ok(())
}

/// The routers of the service: `ROUTE_WORKERS`, else one for each CPU up to `MAX_WORKERS`.
/// `available_parallelism` obeys the CPU affinity and, on cgroup v2, the systemd `CPUQuota`.
fn workers() -> Result<usize, std::num::ParseIntError> {
    match std::env::var("ROUTE_WORKERS") {
        Ok(count) => count.parse(),
        Err(_) => Ok(std::thread::available_parallelism().map_or(1, |n| n.get()).min(planner_service::MAX_WORKERS)),
    }
}

/// The exit status of an install that a published object failed: the follower deletes the object
/// that the last line of standard error names, `object <SHA-256>: <why>`, and fetches it again.
const OBJECT: i32 = 3;

/// All cells and all profile groups of the package of `DIRECTORY` and `PACKAGE`, its
/// `directory.bin` and `package.json`, in `target`. `--missing` prints the installed bytes of the
/// files that `target` lacks. `--install` installs them on `THREADS` workers from `OBJECTS`, a store
/// of `obc data` that holds each object as `OBJECTS/<first two hex digits>/<SHA-256>`; it exits
/// with [`OBJECT`] when an object is missing or damaged.
fn install(target: &Path, args: &[String]) -> Result<(), String> {
    use planner_router::install;
    let read = |path: &String| std::fs::read(path).map_err(|e| format!("{path}: {e}"));
    match args {
        [flag, directory, package] if flag == "--missing" => {
            let plan = install::plan(&read(directory)?, &read(package)?, None, None).map_err(|e| e.to_string())?;
            println!("{}", install::installed_bytes(&plan.missing(target)));
        }
        [flag, directory, package, objects, threads] if flag == "--install" => {
            let threads = threads.parse().map_err(|_| format!("THREADS is not a count: {threads}"))?;
            let objects = Path::new(objects);
            let fetch = |sha256: &str| std::fs::File::open(objects.join(&sha256[..2]).join(sha256));
            let (directory, package) = (read(directory)?, read(package)?);
            let report = match install::install(target, (&directory, &package), (None, None), threads, fetch) {
                Ok(report) => report,
                Err(e @ install::Error::Object { .. }) => {
                    eprintln!("{e}");
                    std::process::exit(OBJECT);
                }
                Err(e) => return Err(e.to_string()),
            };
            eprintln!("Installed {} files of {} bytes in {}", report.files, report.bytes, target.display());
        }
        _ => return Err(format!("unknown arguments: {}", args.join(" "))),
    }
    Ok(())
}
