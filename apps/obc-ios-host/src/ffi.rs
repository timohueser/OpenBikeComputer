//! The C ABI the Swift shell links: one opaque `ObcHost`. `include/obc_ios_host.h` is written by
//! hand beside it and declares exactly these functions.
//!
//! Every call on a host is main-thread only. The display link, the touch areas and the CoreLocation
//! and CoreMotion delegates all run there, and nothing here synchronises. The two calls that take
//! no host, [`obc_ios_card_state`] and [`obc_ios_import_map`], may run on any thread while no host
//! is open, because a large map copy must not freeze the UI. A NULL host is a no-op, so a shell
//! that outlives its host cannot take the app down. A path that is NULL or not UTF-8 fails through
//! [`obc_ios_last_error`], which is thread-local: read it on the thread that made the call.
//!
//! A panic prints one tagged line to stderr through the hook the first entry point installs, and
//! then aborts. `extern "C"` turns an unwind into an abort by itself, so there is no `catch_unwind`
//! and no half-alive host to call into afterwards.

use crate::{card_state, import_map, CardState, Host, FRAME_H, FRAME_W};
use obc_app::Screen;
use obc_ports::{Button, Fix};
use std::cell::RefCell;
use std::ffi::{c_char, CStr, CString};
use std::path::Path;
use std::ptr;
use std::sync::{Once, OnceLock};

thread_local! {
    /// The last failure's message, alive until the next failing call replaces it.
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// Record why a call failed and hand back the value C reads as the failure.
fn fail<T>(message: String, failure: T) -> T {
    // The messages are formatted from paths that arrived as C strings, so an interior NUL is not
    // reachable. A message that carries one anyway still has to say something.
    let text = CString::new(message).unwrap_or_else(|_| c"the message contained a NUL".to_owned());
    LAST_ERROR.with(|slot| *slot.borrow_mut() = Some(text));
    failure
}

/// One C string as a path, or `None` once the reason is recorded.
///
/// # Safety
/// `raw` is NULL or a NUL-terminated string that outlives the call.
unsafe fn as_path<'a>(raw: *const c_char, what: &str) -> Option<&'a Path> {
    if raw.is_null() {
        return fail(format!("{what} is NULL"), None);
    }
    match CStr::from_ptr(raw).to_str() {
        Ok(text) => Some(Path::new(text)),
        Err(_) => fail(format!("{what} is not UTF-8"), None),
    }
}

/// Run `body` on the open host, or answer `absent` when C passed NULL.
///
/// # Safety
/// `host` is NULL or an open handle from [`obc_ios_open`].
unsafe fn with<T>(host: *mut Host, absent: T, body: impl FnOnce(&mut Host) -> T) -> T {
    host.as_mut().map_or(absent, body)
}

/// [`with`] for the read-only side of the surface.
///
/// # Safety
/// As [`with`].
unsafe fn read<T>(host: *const Host, absent: T, body: impl FnOnce(&Host) -> T) -> T {
    host.as_ref().map_or(absent, body)
}

/// Report a panic on the one stream Xcode shows. Installed by the entry points a shell can reach
/// first, so the message is there whichever of them runs.
fn install_panic_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| std::panic::set_hook(Box::new(|info| eprintln!("obc-ios-host panic: {info}"))));
}

/// What is at `card_path`: 0 no card file, 1 a card with no map, 2 a card with a map. -1 and
/// [`obc_ios_last_error`] when the card cannot be read.
///
/// # Safety
/// `card_path` is NULL or a NUL-terminated string that outlives the call.
#[no_mangle]
pub unsafe extern "C" fn obc_ios_card_state(card_path: *const c_char) -> i32 {
    install_panic_hook();
    let Some(card) = as_path(card_path, "card path") else {
        return -1;
    };
    match card_state(card) {
        Ok(CardState::Missing) => 0,
        Ok(CardState::NoMap) => 1,
        Ok(CardState::Ready) => 2,
        Err(error) => fail(error, -1),
    }
}

/// Put the map at `obcm_path` on the card at `card_path`, creating the card when it is absent.
/// Call it with no host open. 0 on success; [`obc_ios_last_error`] says why otherwise.
///
/// # Safety
/// Both are NULL or NUL-terminated strings that outlive the call.
#[no_mangle]
pub unsafe extern "C" fn obc_ios_import_map(card_path: *const c_char, obcm_path: *const c_char) -> i32 {
    install_panic_hook();
    let Some(card) = as_path(card_path, "card path") else {
        return -1;
    };
    let Some(obcm) = as_path(obcm_path, "map path") else {
        return -1;
    };
    match import_map(card, obcm) {
        Ok(()) => 0,
        Err(error) => fail(error, -1),
    }
}

/// Open the device over an existing card, its settings file and the directory committed rides are
/// exported into. NULL on failure, with the reason in [`obc_ios_last_error`].
///
/// # Safety
/// Each is NULL or a NUL-terminated string that outlives the call.
#[no_mangle]
pub unsafe extern "C" fn obc_ios_open(
    card_path: *const c_char,
    settings_path: *const c_char,
    exports_dir: *const c_char,
) -> *mut Host {
    install_panic_hook();
    let Some(card) = as_path(card_path, "card path") else {
        return ptr::null_mut();
    };
    let Some(settings) = as_path(settings_path, "settings path") else {
        return ptr::null_mut();
    };
    let Some(exports) = as_path(exports_dir, "exports directory") else {
        return ptr::null_mut();
    };
    match Host::open(card, settings, exports) {
        Ok(host) => Box::into_raw(host),
        Err(error) => fail(error, ptr::null_mut()),
    }
}

/// Close the host and free it. The handle is dead afterwards.
///
/// # Safety
/// `host` is NULL or an open handle from [`obc_ios_open`], closed exactly once.
#[no_mangle]
pub unsafe extern "C" fn obc_ios_close(host: *mut Host) {
    if !host.is_null() {
        drop(Box::from_raw(host));
    }
}

/// Advance one display-link frame on the shell's monotonic clock; `true` when the frame changed
/// and the shell has to blit it.
///
/// # Safety
/// `host` is NULL or an open handle from [`obc_ios_open`].
#[no_mangle]
pub unsafe extern "C" fn obc_ios_tick(host: *mut Host, now_ms: f64) -> bool {
    with(host, false, |host| host.tick(now_ms))
}

/// The rendered frame: `obc_ios_frame_width() * obc_ios_frame_height() * 4` RGBA bytes with opaque
/// alpha, valid until the next call on this host.
///
/// # Safety
/// `host` is NULL or an open handle from [`obc_ios_open`].
#[no_mangle]
pub unsafe extern "C" fn obc_ios_frame(host: *const Host) -> *const u8 {
    read(host, ptr::null(), |host| host.frame().as_ptr())
}

/// The panel width in pixels.
#[no_mangle]
pub extern "C" fn obc_ios_frame_width() -> u32 {
    FRAME_W
}

/// The panel height in pixels.
#[no_mangle]
pub extern "C" fn obc_ios_frame_height() -> u32 {
    FRAME_H
}

/// One `CLLocation`. A NaN course or speed is an unknown one, and `unix_secs == 0` is a fix with
/// no UTC stamp.
///
/// # Safety
/// `host` is NULL or an open handle from [`obc_ios_open`].
#[no_mangle]
pub unsafe extern "C" fn obc_ios_push_fix(
    host: *mut Host,
    lat_udeg: i32,
    lon_udeg: i32,
    course_deg: f32,
    speed_mps: f32,
    unix_secs: u32,
) {
    let fix = Fix {
        lat: lat_udeg,
        lon: lon_udeg,
        course: (!course_deg.is_nan()).then_some(course_deg),
        speed_mps: (!speed_mps.is_nan()).then_some(speed_mps),
    };
    with(host, (), |host| host.sensors().push_fix(fix, (unix_secs != 0).then_some(unix_secs)))
}

/// `CLHeading.trueHeading` in degrees clockwise from north.
///
/// # Safety
/// `host` is NULL or an open handle from [`obc_ios_open`].
#[no_mangle]
pub unsafe extern "C" fn obc_ios_push_heading(host: *mut Host, degrees: f32) {
    with(host, (), |host| host.sensors().push_heading(degrees))
}

/// `CMAltimeter` absolute altitude in metres.
///
/// # Safety
/// `host` is NULL or an open handle from [`obc_ios_open`].
#[no_mangle]
pub unsafe extern "C" fn obc_ios_push_altitude(host: *mut Host, metres: f32) {
    with(host, (), |host| host.sensors().push_altitude(metres))
}

/// `UIDevice.batteryLevel` as whole percent.
///
/// # Safety
/// `host` is NULL or an open handle from [`obc_ios_open`].
#[no_mangle]
pub unsafe extern "C" fn obc_ios_push_battery(host: *mut Host, percent: u8) {
    with(host, (), |host| host.sensors().push_battery(percent))
}

/// One button edge from a touch area, held state in and edges out. `button` carries the header's
/// `ObcButton` as a raw value, so a value outside the enum is ignored and never trusted as a
/// discriminant.
///
/// # Safety
/// `host` is NULL or an open handle from [`obc_ios_open`].
#[no_mangle]
pub unsafe extern "C" fn obc_ios_button(host: *mut Host, button: u32, down: bool) {
    let button = match button {
        0 => Button::Up,
        1 => Button::Down,
        2 => Button::Select,
        3 => Button::Back,
        _ => return,
    };
    with(host, (), |host| host.push_button(button, down))
}

/// Import one `.obcr` or `.gpx` route into the card. 0 on success; [`obc_ios_last_error`] says why
/// otherwise.
///
/// # Safety
/// `host` is NULL or an open handle from [`obc_ios_open`]; `path` is NULL or a NUL-terminated
/// string that outlives the call.
#[no_mangle]
pub unsafe extern "C" fn obc_ios_import_route(host: *mut Host, path: *const c_char) -> i32 {
    let Some(host) = host.as_mut() else {
        return fail("no host is open".to_string(), -1);
    };
    let Some(route) = as_path(path, "route path") else {
        return -1;
    };
    match host.import_route(route) {
        Ok(_) => 0,
        Err(error) => fail(error, -1),
    }
}

/// The current screen's variant name, as a static NUL-terminated string.
///
/// # Safety
/// `host` is NULL or an open handle from [`obc_ios_open`].
#[no_mangle]
pub unsafe extern "C" fn obc_ios_screen(host: *const Host) -> *const c_char {
    read(host, ptr::null(), |host| screen_name(host.screen()))
}

/// `Screen::name()`, NUL-terminated. The table is built once from `Screen::NAMES`, so a new screen
/// needs no second spelling of its name here.
fn screen_name(name: &str) -> *const c_char {
    static NAMES: OnceLock<Vec<CString>> = OnceLock::new();
    let names = NAMES.get_or_init(|| Screen::NAMES.iter().filter_map(|name| CString::new(*name).ok()).collect());
    names.iter().find(|known| known.to_bytes() == name.as_bytes()).map_or(ptr::null(), |name| name.as_ptr())
}

/// Whether a ride is open. The shell keeps the screen awake while it is.
///
/// # Safety
/// `host` is NULL or an open handle from [`obc_ios_open`].
#[no_mangle]
pub unsafe extern "C" fn obc_ios_recording(host: *const Host) -> bool {
    read(host, false, Host::recording)
}

/// Why the last call on this thread failed; empty when none has. Valid until the next failing call.
#[no_mangle]
pub extern "C" fn obc_ios_last_error() -> *const c_char {
    LAST_ERROR.with(|slot| slot.borrow().as_ref().map_or(c"".as_ptr(), |text| text.as_ptr()))
}
