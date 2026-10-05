//! Routing calls for the HTTP service and the phone's native provider. Both answer through `respond`.
#[cfg(feature = "http")]
mod http;
pub mod native;
#[cfg(feature = "http")]
pub use http::app;

use route_engine::{
    data::{RoutingData, Selection},
    directory::Files,
    shape::LineRequest,
    Control, Error, Request, Router,
};
use serde_json::{json, Value};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub type Engine = Router<Selection<Files>>;

/// The request body limit of every call.
const BODY_LIMIT: usize = 64 * 1024;
/// Cooperative deadlines. A shape request routes parts of its line many times.
const ROUTE_DEADLINE: Duration = Duration::from_secs(15);
const SHAPE_DEADLINE: Duration = Duration::from_secs(30);

/// Answers one call, `route` or `shape`, with an HTTP status and a JSON body. The call stops
/// when `cancelled` is set or at its deadline. A panic answers `internal` and rebuilds `router`.
pub fn respond(router: &mut Engine, call: &str, body: &[u8], cancelled: &AtomicBool) -> (u16, Vec<u8>) {
    if !known(call) {
        return encode(not_found(call));
    }
    let started = Instant::now();
    let deadline = if call == "shape" { SHAPE_DEADLINE } else { ROUTE_DEADLINE };
    let stop = || cancelled.load(Ordering::Relaxed) || started.elapsed() > deadline;
    let control = Control { cancelled: &stop, ..Control::default() };
    let invalid = || Error::InvalidRequest(format!("Expected a {call} request with valid JSON fields"));
    let result = catch_unwind(AssertUnwindSafe(|| {
        if body.len() > BODY_LIMIT {
            return Err(too_large());
        }
        if call == "route" {
            let request: Request = serde_json::from_slice(body).map_err(|_| invalid())?;
            router.routes(&request, &control).map(|response| route_engine::answer::answer(&response))
        } else {
            let request: LineRequest = serde_json::from_slice(body).map_err(|_| invalid())?;
            route_engine::shape::answer(router, &request, control)
        }
    }));
    encode(match result {
        Ok(Ok(answer)) => (200, answer),
        Ok(Err(error)) => error_body(error),
        Err(_) => {
            // The panic can leave the router's caches half updated. A fork shares only complete data.
            let budget = router.package().memory_budget();
            *router = Router::new(router.package().fork(), budget);
            internal(&format!("A {call} call panicked; its router was rebuilt"))
        }
    })
}

/// The calls that `respond` answers.
pub(crate) fn known(call: &str) -> bool {
    matches!(call, "route" | "shape")
}

pub(crate) fn encode((status, value): (u16, Value)) -> (u16, Vec<u8>) {
    (status, serde_json::to_vec(&value).expect("JSON value serializes"))
}

pub(crate) fn not_found(call: &str) -> (u16, Value) {
    (404, json!({ "code": "not_found", "message": format!("Unknown call {call:?}. Use route or shape.") }))
}

pub(crate) fn too_large() -> Error {
    Error::InvalidRequest(format!("Use a request body of at most {} KiB", BODY_LIMIT / 1024))
}

/// A failure of the service itself, not of the request or the data. The cause goes to the log.
pub(crate) fn internal(cause: &str) -> (u16, Value) {
    eprintln!("{cause}");
    (500, json!({ "code": "internal", "message": "The routing service failed. Try again." }))
}

pub(crate) fn error_body(error: Error) -> (u16, Value) {
    let (status, code, message) = match error {
        Error::InvalidRequest(message) => (400, "invalid_request", message),
        Error::MissingRegion(_) => {
            (422, "missing_region", "The installed region does not cover these points or is incomplete.".into())
        }
        Error::NoSnap(index) => (
            422,
            "no_snap",
            format!("No accessible road within {} km of point {}.", route_engine::snap::REACH_M / 1000.0, index + 1),
        ),
        Error::NoPath => (422, "no_path", "No legal route connects these points in this region.".into()),
        Error::Cancelled => (408, "cancelled", "The routing request was cancelled or timed out.".into()),
        Error::Limit => (503, "limit", "This route exceeds the service limits.".into()),
        Error::LineTooLong => (
            422,
            "line_too_long",
            format!(
                "Use a line of at most {} km and {} points.",
                route_engine::shape::MAX_LINE_M / 1000.0,
                route_engine::shape::MAX_LINE_POINTS
            ),
        ),
        Error::NotReproducible => {
            (422, "line_not_reproducible", "No plan within the shaping limits follows this line on roads.".into())
        }
        Error::InvalidData(cause) => {
            eprintln!("Routing data failed validation: {cause}");
            (500, "invalid_data", "Routing data failed validation.".into())
        }
    };
    (status, json!({ "code": code, "message": message }))
}
