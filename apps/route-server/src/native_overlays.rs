use crate::{error_body, native, OverlaySource, Overlays};
use route_engine::Error;
use std::{
    collections::HashMap,
    ffi::{c_char, c_void, CStr},
    path::Path,
    time::{Duration, Instant},
};

/// # Safety
/// `root` is a live UTF-8 C string. `error` points to writable response storage.
#[no_mangle]
pub unsafe extern "C" fn planner_overlays_open(root: *const c_char, error: *mut *mut c_char) -> *mut c_void {
    if root.is_null() || error.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: The host provides writable error storage for this call.
    unsafe { *error = std::ptr::null_mut() };
    let result = std::panic::catch_unwind(|| {
        // SAFETY: The host retains the path until this call returns.
        let root = unsafe { CStr::from_ptr(root) }.to_str().map_err(|e| Error::InvalidData(e.to_string()))?;
        let root = Path::new(root);
        OverlaySource::open(root)
    })
    .unwrap_or(Err(Error::Limit));
    match result {
        Ok(overlays) => Box::into_raw(Box::new(overlays)).cast(),
        Err(failure) => {
            let (status, body) = error_body(failure);
            // SAFETY: The host provides writable error storage for this call.
            unsafe { *error = native::reply(status, body) };
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// Both strings remain valid for this call; `error` points to writable pointer storage.
#[no_mangle]
pub unsafe extern "C" fn planner_overlays_open_package(
    file: *const c_char,
    package: *const c_char,
    error: *mut *mut c_char,
) -> *mut c_void {
    if file.is_null() || package.is_null() || error.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: The host provides writable error storage.
    unsafe { *error = std::ptr::null_mut() };
    let result = std::panic::catch_unwind(|| {
        // SAFETY: The host retains both C strings for this call.
        let file = unsafe { CStr::from_ptr(file) }.to_str().map_err(|e| Error::InvalidData(e.to_string()))?;
        // SAFETY: The host retains both C strings for this call.
        let package = unsafe { CStr::from_ptr(package) }.to_str().map_err(|e| Error::InvalidData(e.to_string()))?;
        if !route_engine::table::valid_digest(package) {
            return Err(Error::InvalidData("Invalid overlay package".into()));
        }
        Overlays::open_file(Path::new(file), package).map(OverlaySource::Region)
    })
    .unwrap_or(Err(Error::Limit));
    match result {
        Ok(overlays) => Box::into_raw(Box::new(overlays)).cast(),
        Err(failure) => {
            let (status, body) = error_body(failure);
            // SAFETY: The host provides writable error storage.
            unsafe { *error = native::reply(status, body) };
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// The handle is live, `params` contains `length` readable JSON bytes, and `status`
/// is writable. Free the returned body with `planner_response_free`.
#[no_mangle]
pub unsafe extern "C" fn planner_overlays_query(
    handle: *const c_void,
    params: *const u8,
    length: usize,
    status: *mut u16,
) -> *mut c_char {
    if handle.is_null() || params.is_null() || status.is_null() {
        return std::ptr::null_mut();
    }
    let result = std::panic::catch_unwind(|| {
        if length > 64 * 1024 {
            return Err(Error::InvalidRequest("Overlay query is too large".into()));
        }
        // SAFETY: The host retains the live handle and request bytes for this call.
        let (overlays, bytes) =
            unsafe { (&*handle.cast::<OverlaySource>(), std::slice::from_raw_parts(params, length)) };
        let params: HashMap<String, String> = serde_json::from_slice(bytes)
            .map_err(|_| Error::InvalidRequest("Expected overlay query parameters".into()))?;
        let started = Instant::now();
        overlays.query(&params, &|| started.elapsed() > Duration::from_secs(15))
    })
    .unwrap_or(Err(Error::Limit));
    let (code, body) = match result {
        Ok(body) => (200, body),
        Err(error) => {
            let (code, body) = error_body(error);
            (code, serde_json::to_vec(&body).expect("JSON error response"))
        }
    };
    // SAFETY: The host provides writable status storage for this call.
    unsafe { *status = code };
    native::body(body)
}

/// # Safety
/// The handle came from `planner_overlays_open` and has no in-flight calls.
#[no_mangle]
pub unsafe extern "C" fn planner_overlays_close(handle: *mut c_void) {
    if !handle.is_null() {
        // SAFETY: The host returns ownership once, after all queries finish.
        drop(unsafe { Box::from_raw(handle.cast::<OverlaySource>()) });
    }
}
