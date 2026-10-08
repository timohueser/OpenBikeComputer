//! The phone's routing provider. The host serializes the calls on a handle; only
//! `planner_router_cancel` may run during a call.
use crate::{error_body, respond, Engine};
use planner_router::{data::RoutingData, Error, Router};
use serde_json::Value;
use std::{
    ffi::{c_char, CStr, CString},
    panic::catch_unwind,
    path::Path,
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, PoisonError,
    },
};

pub struct Handle {
    router: Mutex<Engine>,
    cancel: AtomicBool,
}

fn reply(status: u16, body: Value) -> *mut c_char {
    let mut bytes = format!("{{\"status\":{status},\"body\":").into_bytes();
    bytes.extend_from_slice(&serde_json::to_vec(&body).expect("JSON value serializes"));
    bytes.push(b'}');
    self::body(bytes)
}

fn body(bytes: Vec<u8>) -> *mut c_char {
    CString::new(bytes).expect("Serialized JSON has no NUL bytes").into_raw()
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
/// A zero budget uses the engine's default budget for the package.
/// The returned handle must be closed once, after all of its calls finish.
#[no_mangle]
pub unsafe extern "C" fn planner_router_open(
    root: *const c_char,
    memory_budget_bytes: usize,
    error: *mut *mut c_char,
) -> *mut Handle {
    if error.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: The caller supplies writable error pointer storage.
    unsafe { *error = ptr::null_mut() };
    let open = || {
        if root.is_null() {
            return Err(Error::InvalidRequest("Missing routing directory".into()));
        }
        // SAFETY: The caller retains the NUL-terminated path for this call.
        let root = unsafe { CStr::from_ptr(root) }
            .to_str()
            .map_err(|_| Error::InvalidRequest("Routing directory must be UTF-8".into()))?;
        let package = planner_router::open(Path::new(root))?;
        let budget = if memory_budget_bytes == 0 { package.default_budget() } else { memory_budget_bytes };
        Ok(Router::new(package, budget))
    };
    let (status, body) = match catch_unwind(open) {
        Ok(Ok(router)) => {
            return Box::into_raw(Box::new(Handle { router: Mutex::new(router), cancel: AtomicBool::new(false) }))
        }
        Ok(Err(failure)) => error_body(failure),
        Err(_) => crate::internal("Opening the routing package panicked"),
    };
    // SAFETY: The caller supplies writable error pointer storage.
    unsafe { *error = reply(status, body) };
    ptr::null_mut()
}

/// Answers `call` (`route` or `shape`) with a JSON body, as `POST /v1/{call}` does.
///
/// # Safety
/// `handle` is live with no other call in progress. `call` is a NUL-terminated string.
/// `body` points to `length` readable bytes. `status` points to writable storage.
/// The returned response must be freed with `planner_response_free`.
#[no_mangle]
pub unsafe extern "C" fn planner_router_call(
    handle: *const Handle,
    call: *const c_char,
    body: *const u8,
    length: usize,
    status: *mut u16,
) -> *mut c_char {
    if handle.is_null() || call.is_null() || body.is_null() || status.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: The host retains the handle, the call name and the body for this call.
    let (handle, call, body) = unsafe { (&*handle, CStr::from_ptr(call), std::slice::from_raw_parts(body, length)) };
    let mut router = handle.router.lock().unwrap_or_else(PoisonError::into_inner);
    // A cancel that arrives before the call starts belongs to an earlier call.
    handle.cancel.store(false, Ordering::Relaxed);
    let (code, bytes) = respond(&mut router, call.to_str().unwrap_or_default(), body, &handle.cancel);
    // SAFETY: The caller provides writable status storage.
    unsafe { *status = code };
    self::body(bytes)
}

/// Stops the call in progress on `handle`; it answers `cancelled`. Any thread may call this.
///
/// # Safety
/// `handle` is null or live.
#[no_mangle]
pub unsafe extern "C" fn planner_router_cancel(handle: *const Handle) {
    // SAFETY: The caller keeps the handle live for this call.
    if let Some(handle) = unsafe { handle.as_ref() } {
        handle.cancel.store(true, Ordering::Relaxed);
    }
}

/// # Safety
/// `handle` is null or a live handle with no calls in progress.
#[no_mangle]
pub unsafe extern "C" fn planner_router_close(handle: *mut Handle) {
    if !handle.is_null() {
        // SAFETY: The caller transfers ownership after the final call.
        drop(unsafe { Box::from_raw(handle) });
    }
}
