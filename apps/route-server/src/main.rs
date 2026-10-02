use std::path::Path;
use tower_http::cors::CorsLayer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory =
        std::env::args().nth(1).ok_or("Usage: route-server PACKAGE_DIRECTORY [--verify|--build-overlays]")?;
    if std::env::args().nth(2).as_deref() == Some("--build-overlays") {
        route_server::prepare_overlays(Path::new(&directory))?;
        eprintln!("Overlay index prepared");
        return Ok(());
    }
    if std::env::args().nth(2).as_deref() == Some("--verify") {
        let path = Path::new(&directory);
        if path.join("blocks.json").exists() {
            route_engine::blocks::Files::open(path)?.verify()?;
        } else {
            route_engine::directory::Directory::open(path)?.verify()?;
        }
        if path.join("overlays.sqlite").exists() || path.join("layers").exists() {
            route_server::OverlaySource::open(path)?;
        }
        eprintln!("Package object closure verified");
        return Ok(());
    }
    let address = std::env::var("ROUTE_LISTEN").unwrap_or_else(|_| "127.0.0.1:8788".into());
    let workers = std::env::var("ROUTE_WORKERS").unwrap_or_else(|_| "2".into()).parse()?;
    let mut app = route_server::app(Path::new(&directory), workers)?;
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
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
