mod access;
pub mod native;
mod native_overlays;
use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, Query, State},
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

mod overlay_source;
mod overlays;
pub use overlay_source::OverlaySource;
pub use overlays::Overlays;

pub fn prepare_overlays(directory: &Path) -> Result<(), Error> {
    overlays::Overlays::build(directory)
}

struct Workers {
    routers: Mutex<Vec<Router<Box<dyn route_engine::data::RoutingData + Send>>>>,
    permits: Arc<Semaphore>,
    metadata: Value,
    overlays: OverlaySource,
    overlay_permits: Arc<Semaphore>,
}

fn default_memory_budget(package: &route_engine::package::Package<impl route_engine::package::Source>) -> usize {
    (768 * 1024 * 1024usize)
        .saturating_add(package.manifest().landmarks.as_ref().map_or(0, |index| index.decoded_bytes()))
}

pub fn app(directory: &Path, workers: usize) -> Result<axum::Router, Error> {
    if !(1..=8).contains(&workers) {
        return Err(Error::InvalidRequest("Use 1 to 8 workers".into()));
    }
    let mut routers: Vec<Router<Box<dyn route_engine::data::RoutingData + Send>>> = Vec::new();
    if directory.join("blocks.json").exists() {
        let source = route_engine::blocks::Files::open(directory)?;
        let budget = (768 * 1024 * 1024usize).saturating_add(source.roads() as usize * 16);
        for _ in 0..workers {
            routers.push(Router::new(Box::new(source.fork()), budget));
        }
    } else {
        let source = Directory::open(directory)?;
        for _ in 0..workers {
            routers.push(Router::new(Box::new(source.fork()), default_memory_budget(&source)));
        }
    }
    let overlays = OverlaySource::open(directory)?;
    let metadata = metadata(&routers[0]);
    let state = Arc::new(Workers {
        routers: Mutex::new(routers),
        permits: Arc::new(Semaphore::new(workers)),
        metadata,
        overlays,
        overlay_permits: Arc::new(Semaphore::new(2)),
    });
    Ok(axum::Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/region", get(region))
        .route("/v1/route", post(route))
        .route("/v1/overlays", get(map_overlays))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(CompressionLayer::new())
        .with_state(state))
}

async fn map_overlays(
    State(workers): State<Arc<Workers>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let Ok(permit) = workers.overlay_permits.clone().try_acquire_owned() else {
        return failure(Error::Limit);
    };
    let cancel = Cancel(Arc::new(AtomicBool::new(false)));
    let flag = cancel.0.clone();
    let started = Instant::now();
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        workers.overlays.query(&params, &|| flag.load(Ordering::Relaxed) || started.elapsed() > Duration::from_secs(15))
    })
    .await
    {
        Ok(Ok(data)) => (
            [
                (axum::http::header::CACHE_CONTROL, "public, max-age=3600"),
                (axum::http::header::CONTENT_TYPE, "application/json"),
            ],
            data,
        )
            .into_response(),
        Ok(Err(error)) => failure(error),
        Err(_) => failure(Error::Limit),
    }
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

pub(crate) fn error_body(error: Error) -> (u16, Value) {
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
    (status.as_u16(), json!({ "code": code, "message": message }))
}

fn failure(error: Error) -> Response {
    let (status, body) = error_body(error);
    (StatusCode::from_u16(status).unwrap(), Json(body)).into_response()
}

pub(crate) fn metadata<P: route_engine::data::RoutingData>(router: &Router<P>) -> Value {
    let package = router.package();
    json!({ "package": package.identity(), "region": package.region(), "bounds": package.bounds(),
        "profiles": package.profiles(), "attribution": package.attribution(), "warnings": package.warnings() })
}
