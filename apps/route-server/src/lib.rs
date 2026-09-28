use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json,
};
use route_engine::{directory::Directory, Control, Error, Request, Router};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use tower_http::compression::CompressionLayer;

struct Workers {
    routers: Mutex<Vec<Router<Directory>>>,
    permits: Arc<Semaphore>,
    metadata: Value,
}

pub fn app(directory: &Path, workers: usize) -> Result<axum::Router, Error> {
    if !(1..=8).contains(&workers) {
        return Err(Error::InvalidRequest("Use 1 to 8 workers".into()));
    }
    let mut routers = Vec::new();
    for _ in 0..workers {
        routers.push(Router::new(Directory::open(directory)?, 64 * 1024 * 1024));
    }
    let package = routers[0].package();
    let metadata = json!({ "package": package.identity(), "region": package.manifest().region, "bounds": package.manifest().bounds,
        "profiles": package.manifest().metrics.keys().collect::<Vec<_>>(), "attribution": package.manifest().attribution, "warnings": package.manifest().warnings });
    let state =
        Arc::new(Workers { routers: Mutex::new(routers), permits: Arc::new(Semaphore::new(workers)), metadata });
    Ok(axum::Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/region", get(region))
        .route("/v1/route", post(route))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(CompressionLayer::new())
        .with_state(state))
}

async fn region(State(workers): State<Arc<Workers>>) -> Json<Value> {
    Json(workers.metadata.clone())
}

struct Cancel(Arc<AtomicBool>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

async fn route(State(workers): State<Arc<Workers>>, request: Result<Json<Request>, JsonRejection>) -> Response {
    let request = match request {
        Ok(Json(request)) => request,
        Err(_) => return failure(Error::InvalidRequest("Expected a route request with valid JSON fields".into())),
    };
    let Ok(permit) = workers.permits.clone().try_acquire_owned() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "code": "busy", "message": "Routing is busy. Try again shortly." })),
        )
            .into_response();
    };
    let cancel = Cancel(Arc::new(AtomicBool::new(false)));
    let flag = cancel.0.clone();
    let started = Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut router = workers.routers.lock().map_err(|_| Error::Limit)?.pop().ok_or(Error::Limit)?;
        let cancelled = || flag.load(Ordering::Relaxed) || started.elapsed() > Duration::from_secs(15);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            router.routes(&request, &Control { cancelled: &cancelled, ..Control::default() })
        }))
        .unwrap_or(Err(Error::Limit));
        workers.routers.lock().map_err(|_| Error::Limit)?.push(router);
        result
    })
    .await;
    match result {
        Ok(Ok(route)) => Json(route).into_response(),
        Ok(Err(error)) => failure(error),
        Err(_) => failure(Error::Limit),
    }
}

fn failure(error: Error) -> Response {
    let (status, code, message) = match error {
        Error::InvalidRequest(message) => (StatusCode::BAD_REQUEST, "invalid_request", message),
        Error::MissingRegion(_) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "missing_region",
            "The installed region does not cover these points or is incomplete.".into(),
        ),
        Error::NoSnap(index) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "no_snap",
            format!("No accessible road within 250 m of point {}.", index + 1),
        ),
        Error::NoPath => {
            (StatusCode::UNPROCESSABLE_ENTITY, "no_path", "No legal route connects these points in this region.".into())
        }
        Error::Cancelled => {
            (StatusCode::REQUEST_TIMEOUT, "cancelled", "The routing request was cancelled or timed out.".into())
        }
        Error::Limit => (StatusCode::SERVICE_UNAVAILABLE, "limit", "This route exceeds the service limits.".into()),
        Error::InvalidData(_) => {
            (StatusCode::INTERNAL_SERVER_ERROR, "invalid_data", "Routing data failed validation.".into())
        }
    };
    (status, Json(json!({ "code": code, "message": message }))).into_response()
}
