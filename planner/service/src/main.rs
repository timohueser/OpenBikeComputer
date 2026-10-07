use std::path::Path;
use tokio::signal::unix::{signal, SignalKind};
use tower_http::cors::CorsLayer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--obc-build-identity") {
        println!(
            "{}",
            serde_json::json!({
                "root": option_env!("OBC_ROUTE_BUILD_ROOT"), "code": option_env!("OBC_ROUTE_BUILD_CODE")
            })
        );
        return Ok(());
    }
    let directory = std::env::args().nth(1).ok_or("Usage: planner-service PACKAGE_DIRECTORY [--verify]")?;
    if std::env::args().nth(2).as_deref() == Some("--verify") {
        let path = Path::new(&directory);
        planner_router::open(path)?.verify()?;
        eprintln!("Package object closure verified");
        return Ok(());
    }
    let address = std::env::var("ROUTE_LISTEN").unwrap_or_else(|_| "127.0.0.1:8788".into());
    let workers = std::env::var("ROUTE_WORKERS").unwrap_or_else(|_| "2".into()).parse()?;
    let mut app = planner_service::app(Path::new(&directory), workers)?;
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
    eprintln!("Routing service listening on {address}");
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
