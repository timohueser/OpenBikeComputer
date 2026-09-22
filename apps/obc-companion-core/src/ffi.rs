//! The C ABI the OBCKit routing module links. `include/obc_companion_core.h` is written by hand
//! beside it and declares exactly these functions.
//!
//! A map is immutable once assembled, and each route builds its own reader, cache and A* table,
//! so any thread may call. A NULL map or route is a no-op, NULL or a failure code. Failures carry
//! a message through [`obc_core_last_error`], which is thread-local.
//!
//! A panic in assembly or routing is caught and returned as a failure with its message, so a bug
//! in the core fails one request instead of ending the app.

use crate::{assemble, CellMap, Leg, RouteError};
use std::cell::RefCell;
use std::ffi::{c_char, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

fn fail<T>(message: String, failure: T) -> T {
    let text = CString::new(message).unwrap_or_else(|_| c"the message contained a NUL".to_owned());
    LAST_ERROR.with(|slot| *slot.borrow_mut() = text);
    failure
}

/// Run `f`, turning a panic into `Err` with the panic's message.
fn guarded<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|payload| {
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".into());
        Err(format!("the core panicked: {message}"))
    })
}

/// The C layout of one routed point.
#[repr(C)]
pub struct ObcCorePoint {
    pub lon_udeg: i32,
    pub lat_udeg: i32,
    /// `OBC_CORE_NO_ELEVATION` where the map has no terrain.
    pub ele_m: i16,
    pub surface: u8,
    pub elevation_incomplete: bool,
}

pub struct ObcCoreRoute {
    points: Vec<ObcCorePoint>,
    distance_m: u32,
    ascent_m: u32,
}

const ROUTED: i32 = 0;
const NO_ROAD: i32 = 1;
const NO_PATH: i32 = 2;
const EXHAUSTED: i32 = 3;
const FAILED: i32 = 4;

unsafe fn text<'a>(what: &str, s: *const c_char) -> Result<&'a str, String> {
    if s.is_null() {
        return Err(format!("the {what} is NULL"));
    }
    CStr::from_ptr(s).to_str().map_err(|_| format!("the {what} is not UTF-8"))
}

/// Assemble the map a job describes (see [`crate::Job`]) under the catalog root. NULL and a
/// message on failure.
///
/// # Safety
/// Both arguments are NULL or NUL-terminated strings that outlive the call.
#[no_mangle]
pub unsafe extern "C" fn obc_core_assemble(catalog_json: *const c_char, job_json: *const c_char) -> *mut CellMap {
    let (catalog, job) = match (text("catalog", catalog_json), text("job", job_json)) {
        (Ok(catalog), Ok(job)) => (catalog, job),
        (Err(e), _) | (_, Err(e)) => return fail(e, ptr::null_mut()),
    };
    let job = match serde_json::from_str(job) {
        Ok(job) => job,
        Err(e) => return fail(format!("the job: {e}"), ptr::null_mut()),
    };
    match guarded(|| assemble(catalog, job)) {
        Ok(map) => Box::into_raw(Box::new(map)),
        Err(e) => fail(e, ptr::null_mut()),
    }
}

/// # Safety
/// `map` is NULL or a live handle from [`obc_core_assemble`]; it is dead afterwards.
#[no_mangle]
pub unsafe extern "C" fn obc_core_map_free(map: *mut CellMap) {
    if !map.is_null() {
        drop(Box::from_raw(map));
    }
}

/// Route over `map` for bike type `bike` (0..=3). On success `*out` holds a route the caller frees.
///
/// # Safety
/// `map` is NULL or a live handle from [`obc_core_assemble`]; `out` is a valid pointer.
#[no_mangle]
pub unsafe extern "C" fn obc_core_route(
    map: *const CellMap,
    from_lon: i32,
    from_lat: i32,
    to_lon: i32,
    to_lat: i32,
    bike: u8,
    out: *mut *mut ObcCoreRoute,
) -> i32 {
    let Some(map) = map.as_ref() else { return NO_PATH };
    let Some(bike) = obc_route::BikeType::from_u8(bike) else {
        return fail(format!("bike type {bike} is not 0..=3"), FAILED);
    };
    match guarded(|| Ok(map.route((from_lon, from_lat), (to_lon, to_lat), bike))) {
        Err(message) => fail(message, FAILED),
        Ok(Ok(Leg { points, distance_m, ascent_m })) => {
            let points = points
                .into_iter()
                .map(|p| ObcCorePoint {
                    lon_udeg: p.lon,
                    lat_udeg: p.lat,
                    ele_m: p.ele.unwrap_or(obc_formats::obcr::ELEVATION_NONE),
                    surface: p.surface,
                    elevation_incomplete: p.elevation_incomplete,
                })
                .collect();
            *out = Box::into_raw(Box::new(ObcCoreRoute { points, distance_m, ascent_m }));
            ROUTED
        }
        Ok(Err(RouteError::NoRoad)) => NO_ROAD,
        Ok(Err(RouteError::NoPath)) => NO_PATH,
        Ok(Err(RouteError::Exhausted)) => EXHAUSTED,
    }
}

/// # Safety
/// `route` is NULL or a live route; `count` is a valid pointer. The points live as long as the route.
#[no_mangle]
pub unsafe extern "C" fn obc_core_route_points(route: *const ObcCoreRoute, count: *mut usize) -> *const ObcCorePoint {
    let points = route.as_ref().map_or(&[][..], |r| &r.points[..]);
    *count = points.len();
    points.as_ptr()
}

/// # Safety
/// `route` is NULL or a live route.
#[no_mangle]
pub unsafe extern "C" fn obc_core_route_distance_m(route: *const ObcCoreRoute) -> u32 {
    route.as_ref().map_or(0, |r| r.distance_m)
}

/// # Safety
/// `route` is NULL or a live route.
#[no_mangle]
pub unsafe extern "C" fn obc_core_route_ascent_m(route: *const ObcCoreRoute) -> u32 {
    route.as_ref().map_or(0, |r| r.ascent_m)
}

/// # Safety
/// `route` is NULL or a live route; it is dead afterwards.
#[no_mangle]
pub unsafe extern "C" fn obc_core_route_free(route: *mut ObcCoreRoute) {
    if !route.is_null() {
        drop(Box::from_raw(route));
    }
}

/// The calling thread's last failure, empty until one. Valid until the next failure on this thread.
#[no_mangle]
pub extern "C" fn obc_core_last_error() -> *const c_char {
    LAST_ERROR.with(|slot| slot.borrow().as_ptr())
}
