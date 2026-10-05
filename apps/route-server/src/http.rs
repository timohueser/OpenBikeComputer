//! The HTTP service: a pool of routers that answer `POST /v1/{call}` through `respond`.
use crate::{encode, error_body, internal, known, not_found, respond, too_large, Engine, BODY_LIMIT};
use axum::{
    body::Bytes,
    extract::{rejection::BytesRejection, DefaultBodyLimit, Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json,
};
use route_engine::{data::RoutingData, Error, Router};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, PoisonError,
    },
    time::Duration,
};
use tokio::sync::Semaphore;
use tower_http::{compression::CompressionLayer, CompressionLevel};

/// How long a request waits for a free router before it answers `busy`.
const QUEUE_WAIT: Duration = Duration::from_secs(1);
/// The web planner's default profile. It loads before the service accepts requests.
const WARM_PROFILE: &str = "touring";

struct Workers {
    routers: Mutex<Vec<Engine>>,
    permits: Arc<Semaphore>,
    metadata: Value,
}

pub fn app(directory: &std::path::Path, workers: usize) -> Result<axum::Router, Error> {
    if !(1..=8).contains(&workers) {
        return Err(Error::InvalidRequest("Use 1 to 8 workers".into()));
    }
    // The opened handle has no budget, so it must not outlive the forks' setup.
    let routers: Vec<Engine> = {
        let source = route_engine::open(directory)?;
        let budget = source.default_budget();
        (0..workers).map(|_| Router::new(source.fork(), budget)).collect()
    };
    let package = routers[0].package();
    // Forks share prepared profiles, so one load serves every router.
    if package.profiles().contains(&WARM_PROFILE) {
        package.prepared(WARM_PROFILE)?;
    }
    let metadata = json!({ "package": package.identity(), "region": package.region(), "bounds": package.bounds(),
        "profiles": package.profiles(), "attribution": package.attribution(), "warnings": package.warnings() });
    let state =
        Arc::new(Workers { routers: Mutex::new(routers), permits: Arc::new(Semaphore::new(workers)), metadata });
    Ok(axum::Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/region", get(region))
        .route("/v1/{call}", post(call))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        // At brotli's default quality 4, a route answer is larger than with gzip. Quality 6 costs about as much as gzip 6.
        .layer(CompressionLayer::new().quality(CompressionLevel::Precise(6)))
        .with_state(state))
}

async fn region(State(workers): State<Arc<Workers>>) -> Json<Value> {
    Json(workers.metadata.clone())
}

/// Sets the flag when the handler is dropped, so a disconnect cancels the call.
struct Cancel(Arc<AtomicBool>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

async fn call(
    State(workers): State<Arc<Workers>>,
    Path(call): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    // Requests that fail without routing never wait for a worker.
    if !known(&call) {
        return reply(encode(not_found(&call)));
    }
    let Ok(body) = body else { return reply(encode(error_body(too_large()))) };
    let Ok(Ok(permit)) = tokio::time::timeout(QUEUE_WAIT, Arc::clone(&workers.permits).acquire_owned()).await else {
        return reply(encode((503, json!({ "code": "busy", "message": "Routing is busy. Try again shortly." }))));
    };
    let cancel = Cancel(Arc::new(AtomicBool::new(false)));
    let flag = Arc::clone(&cancel.0);
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let routers = &workers.routers;
        let mut router = routers.lock().unwrap_or_else(PoisonError::into_inner).pop().expect("a permit holds a router");
        let reply = respond(&mut router, &call, &body, &flag);
        routers.lock().unwrap_or_else(PoisonError::into_inner).push(router);
        reply
    })
    .await;
    reply(result.unwrap_or_else(|error| encode(internal(&format!("A routing task failed: {error}")))))
}

fn reply((status, body): (u16, Vec<u8>)) -> Response {
    let status = StatusCode::from_u16(status).expect("a valid status");
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}
