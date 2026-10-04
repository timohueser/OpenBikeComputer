pub mod native;
use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json,
};
use route_engine::{
    data::{RoutingData, Selection},
    directory::Files,
    shape::LineRequest,
    Control, Error, Request, Router,
};
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
use tower_http::{compression::CompressionLayer, CompressionLevel};

type Engine = Router<Selection<Files>>;

/// The request body limit of the service and of the native provider.
const BODY_LIMIT: usize = 64 * 1024;
/// Cooperative deadlines. A shape request routes parts of its line many times.
const ROUTE_DEADLINE: Duration = Duration::from_secs(15);
const SHAPE_DEADLINE: Duration = Duration::from_secs(30);

fn invalid(kind: &str) -> Error {
    Error::InvalidRequest(format!("Expected a {kind} request with valid JSON fields"))
}

struct Workers {
    routers: Mutex<Vec<Engine>>,
    permits: Arc<Semaphore>,
    metadata: Value,
}

pub fn app(directory: &Path, workers: usize) -> Result<axum::Router, Error> {
    if !(1..=8).contains(&workers) {
        return Err(Error::InvalidRequest("Use 1 to 8 workers".into()));
    }
    let source = route_engine::open(directory)?;
    let budget = source.default_budget();
    let routers: Vec<Engine> = (0..workers).map(|_| Router::new(source.fork(), budget)).collect();
    let metadata = metadata(&routers[0]);
    let state =
        Arc::new(Workers { routers: Mutex::new(routers), permits: Arc::new(Semaphore::new(workers)), metadata });
    Ok(axum::Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/region", get(region))
        .route("/v1/route", post(route))
        .route("/v1/shape", post(shape))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        // At brotli's default quality 4, a route answer is larger than with gzip. Quality 6 costs about as much as gzip 6.
        .layer(CompressionLayer::new().quality(CompressionLevel::Precise(6)))
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
    let Ok(Json(request)) = request else { return failure(invalid("route")) };
    run(workers, ROUTE_DEADLINE, move |router, control| {
        router.routes(&request, &control).map(|response| route_engine::answer::answer(&response))
    })
    .await
}

async fn shape(State(workers): State<Arc<Workers>>, request: Result<Json<LineRequest>, JsonRejection>) -> Response {
    let Ok(Json(request)) = request else { return failure(invalid("shape")) };
    run(workers, SHAPE_DEADLINE, move |router, control| route_engine::shape::answer(router, &request, control)).await
}

/// Runs `work` on a free router. A disconnect or the deadline cancels it.
async fn run(
    workers: Arc<Workers>,
    deadline: Duration,
    work: impl FnOnce(&mut Engine, Control) -> Result<Value, Error> + Send + 'static,
) -> Response {
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
        let cancelled = || flag.load(Ordering::Relaxed) || started.elapsed() > deadline;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            work(&mut router, Control { cancelled: &cancelled, ..Control::default() })
        }))
        .unwrap_or(Err(Error::Limit));
        workers.routers.lock().map_err(|_| Error::Limit)?.push(router);
        result
    })
    .await;
    match result {
        Ok(Ok(answer)) => Json(answer).into_response(),
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
            format!("No accessible road within {} km of point {}.", route_engine::snap::REACH_M / 1000.0, index + 1),
        ),
        Error::NoPath => {
            (StatusCode::UNPROCESSABLE_ENTITY, "no_path", "No legal route connects these points in this region.".into())
        }
        Error::Cancelled => {
            (StatusCode::REQUEST_TIMEOUT, "cancelled", "The routing request was cancelled or timed out.".into())
        }
        Error::Limit => (StatusCode::SERVICE_UNAVAILABLE, "limit", "This route exceeds the service limits.".into()),
        Error::LineTooLong => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "line_too_long",
            format!(
                "Use a line of at most {} km and {} points.",
                route_engine::shape::MAX_LINE_M / 1000.0,
                route_engine::shape::MAX_LINE_POINTS
            ),
        ),
        Error::NotReproducible => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "line_not_reproducible",
            "No plan within the shaping limits follows this line on roads.".into(),
        ),
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

pub(crate) fn metadata<P: RoutingData>(router: &Router<P>) -> Value {
    let package = router.package();
    json!({ "package": package.identity(), "region": package.region(), "bounds": package.bounds(),
        "profiles": package.profiles(), "attribution": package.attribution(), "warnings": package.warnings() })
}
