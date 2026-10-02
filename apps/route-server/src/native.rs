//! Retained native provider. Calls on each handle must be serialized by the host.
pub use crate::native_overlays::{
    planner_overlays_close, planner_overlays_open, planner_overlays_open_package, planner_overlays_query,
};
use crate::{error_body, metadata};
use route_engine::{directory::Directory, Control, Error, Request, Router};
use serde_json::Value;

type NativeRouter = Router<Box<dyn route_engine::data::RoutingData + Send>>;
use std::{
    ffi::{c_char, CStr, CString},
    panic::{catch_unwind, AssertUnwindSafe},
    path::Path,
    ptr,
    time::{Duration, Instant},
};

pub(crate) fn reply(status: u16, body: Value) -> *mut c_char {
    reply_bytes(status, &serde_json::to_vec(&body).expect("JSON value serializes"))
}

pub(crate) fn reply_bytes(status: u16, body: &[u8]) -> *mut c_char {
    let mut bytes = format!("{{\"status\":{status},\"body\":").into_bytes();
    bytes.extend_from_slice(body);
    bytes.push(b'}');
    self::body(bytes)
}

pub(crate) fn body(bytes: Vec<u8>) -> *mut c_char {
    CString::new(bytes).expect("Serialized JSON has no NUL bytes").into_raw()
}

fn failure(error: Error) -> *mut c_char {
    let (status, body) = error_body(error);
    reply(status, body)
}

/// # Safety
/// `response` is null or an unfreed pointer returned by a planner native response function.
#[no_mangle]
pub unsafe extern "C" fn planner_response_free(response: *mut c_char) {
    if !response.is_null() {
        // SAFETY: The caller transfers ownership of this response back to Rust.
        drop(unsafe { CString::from_raw(response) });
    }
}

/// # Safety
/// `root` is a NUL-terminated UTF-8 string. `error` points to writable pointer storage.
/// A zero budget uses the host default, including the optional index cache.
/// The returned handle must be closed once, after all of its calls finish.
#[no_mangle]
pub unsafe extern "C" fn planner_router_open(
    root: *const c_char,
    memory_budget_bytes: usize,
    error: *mut *mut c_char,
) -> *mut NativeRouter {
    if error.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: The caller supplies writable error pointer storage.
    unsafe { *error = ptr::null_mut() };
    let run = || {
        if root.is_null() {
            return Err(Error::InvalidRequest("Missing routing directory".into()));
        }
        // SAFETY: The caller retains the NUL-terminated path for this call.
        let root = unsafe { CStr::from_ptr(root) }
            .to_str()
            .map_err(|_| Error::InvalidRequest("Routing directory must be UTF-8".into()))?;
        let root = Path::new(root);
        let (package, default): (Box<dyn route_engine::data::RoutingData + Send>, usize) =
            if root.join("blocks.json").exists() {
                {
                    let package = route_engine::blocks::Files::open(root)?;
                    let roads: usize = package.roads() as usize;
                    (Box::new(package), (768 * 1024 * 1024usize).saturating_add(roads.saturating_mul(16)))
                }
            } else {
                let package = Directory::open(root)?;
                let budget = crate::default_memory_budget(&package);
                (Box::new(package), budget)
            };
        let budget = if memory_budget_bytes == 0 { default } else { memory_budget_bytes };
        Ok(Router::new(package, budget))
    };
    match catch_unwind(run).unwrap_or(Err(Error::Limit)) {
        Ok(router) => Box::into_raw(Box::new(router)),
        Err(reason) => {
            // SAFETY: The caller supplies writable error pointer storage.
            unsafe { *error = failure(reason) };
            ptr::null_mut()
        }
    }
}

/// # Safety
/// `router` is a live, exclusively borrowed native router handle. `body` points to `length` bytes.
/// `status` points to writable HTTP status storage.
/// The returned response must be freed with `planner_response_free`.
#[no_mangle]
pub unsafe extern "C" fn planner_router_request(
    router: *mut NativeRouter,
    body: *const u8,
    length: usize,
    status: *mut u16,
) -> *mut c_char {
    if status.is_null() {
        return ptr::null_mut();
    }
    let run = || {
        if router.is_null() || body.is_null() || length > 64 * 1024 {
            return Err(Error::InvalidRequest("Expected a route request with valid JSON fields".into()));
        }
        // SAFETY: The host retains the input buffer and grants exclusive access to the handle.
        let (router, body) = unsafe { (&mut *router, std::slice::from_raw_parts(body, length)) };
        let request: Request = serde_json::from_slice(body)
            .map_err(|_| Error::InvalidRequest("Expected a route request with valid JSON fields".into()))?;
        let started = Instant::now();
        let cancelled = || started.elapsed() > Duration::from_secs(15);
        let response = router.routes(&request, &Control { cancelled: &cancelled, ..Control::default() })?;
        serde_json::to_vec(&crate::answer::answer(&response)).map_err(|error| Error::InvalidData(error.to_string()))
    };
    match catch_unwind(AssertUnwindSafe(run)).unwrap_or(Err(Error::Limit)) {
        Ok(bytes) => {
            // SAFETY: The caller provides writable status storage.
            unsafe { *status = 200 };
            self::body(bytes)
        }
        Err(error) => {
            let (code, value) = error_body(error);
            // SAFETY: The caller provides writable status storage.
            unsafe { *status = code };
            self::body(serde_json::to_vec(&value).expect("JSON value serializes"))
        }
    }
}

/// # Safety
/// `router` is a live, exclusively borrowed native router handle. `status` points to writable storage.
/// The returned response must be freed with `planner_response_free`.
#[no_mangle]
pub unsafe extern "C" fn planner_router_region(router: *const NativeRouter, status: *mut u16) -> *mut c_char {
    if status.is_null() {
        return ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        if router.is_null() {
            return Err(Error::InvalidRequest("Missing routing handle".into()));
        }
        // SAFETY: The host retains the router and serializes access for this call.
        Ok(metadata(unsafe { &*router }))
    }))
    .unwrap_or(Err(Error::Limit));
    let (code, value) = match result {
        Ok(value) => (200, value),
        Err(error) => error_body(error),
    };
    // SAFETY: The caller provides writable status storage.
    unsafe { *status = code };
    body(serde_json::to_vec(&value).expect("JSON value serializes"))
}

/// # Safety
/// `router` is null or a live native router handle with no calls in progress.
#[no_mangle]
pub unsafe extern "C" fn planner_router_close(router: *mut NativeRouter) {
    if !router.is_null() {
        // SAFETY: The caller transfers ownership after the final call.
        drop(unsafe { Box::from_raw(router) });
    }
}
