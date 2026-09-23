//! The hosted builder's conversion bridge: route conversion runs in the visitor's browser through
//! the same `no_std` code the device and the CLI run, so the bytes agree by construction.
//!
//! Four pure functions and an error vocabulary. No frame loop, no canvas, no state.
//!
//! A failure crosses to JS as a thrown `Error` whose `message` is written for a rider and whose
//! `code` ([`ErrorCode`]) is the stable identifier a caller branches on. Every
//! [`obc_formats::io::Error`] variant is mapped by hand in [`convert`], and the matches are
//! exhaustive so a new variant breaks this build.
//!
//! The conversion core ([`convert`]) is target-independent and tested natively; only the bindgen
//! shim below is wasm-specific.

mod convert;

pub use convert::{
    gpx_to_obcr, obcr_to_track, obcr_to_waypoints, track_to_gpx, ConvertFailure, ErrorCode, RouteWaypoint,
    MAX_STORED_POINTS,
};

#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::prelude::*;

    use crate::convert::ConvertFailure;

    /// Module start: surface Rust panics in the console instead of an opaque `unreachable` trap.
    #[wasm_bindgen(start)]
    pub fn start() {
        console_error_panic_hook::set_once();
    }

    /// Convert a GPX file's bytes into `.obcr` bytes, naming the route `name` and typing it
    /// `bike` (`0..=3`: Road, Gravel, MTB, Touring). `bike` crosses as `f64` because a narrower
    /// integer would wrap or truncate a bad JS number into a valid type. The header truncates an over-long name on a
    /// char boundary rather than refusing it.
    ///
    /// Throws an `Error` carrying `code` and `message` on failure; see [`crate::ErrorCode`].
    #[wasm_bindgen]
    pub fn obc_convert_gpx_to_obcr(bytes: &[u8], name: &str, bike: f64) -> Result<Vec<u8>, JsValue> {
        let whole = bike.fract() == 0.0 && (0.0..=3.0).contains(&bike);
        let bike = whole.then_some(bike as u8).and_then(obc_route::BikeType::from_u8).ok_or_else(|| {
            to_js(ConvertFailure {
                code: crate::ErrorCode::Internal,
                message: format!("Internal error: {bike} is not a bike type. This is a bug in the builder."),
            })
        })?;
        crate::convert::gpx_to_obcr(bytes, name, bike).map_err(to_js)
    }

    /// Convert a finished ride-v5 object into a GPX 1.1 document, naming the track `name`.
    ///
    /// Throws an `Error` carrying `code` and `message` on failure; see [`crate::ErrorCode`].
    #[wasm_bindgen]
    pub fn obc_convert_track_to_gpx(bytes: &[u8], name: &str) -> Result<String, JsValue> {
        crate::convert::track_to_gpx(bytes, name).map_err(to_js)
    }

    /// Decode a `.obcr` route's polyline for the device page's preview: flat `[lat, lon, ele]`
    /// triples in route order, crossing as one `Float64Array`.
    ///
    /// Throws an `Error` carrying `code` and `message` on failure; see [`crate::ErrorCode`].
    #[wasm_bindgen]
    pub fn obc_convert_obcr_to_track(bytes: &[u8]) -> Result<Vec<f64>, JsValue> {
        crate::convert::obcr_to_track(bytes).map_err(to_js)
    }

    /// Decode a `.obcr` route's waypoint table: an `Array` of plain
    /// `{name, lat, lon, ele, category, distAlongM}` objects in ascending `distAlongM` order.
    /// `ele` is `null` where the source carried none, `category` is the stored byte raw, and
    /// `distAlongM` is the stored placement-time distance and not a recomputation. A route without
    /// waypoints yields `[]`.
    ///
    /// Plain objects rather than a flat array because names are strings, and at most 32 waypoints
    /// cross per route.
    ///
    /// Throws an `Error` carrying `code` and `message` on failure; see [`crate::ErrorCode`].
    #[wasm_bindgen]
    pub fn obc_convert_obcr_to_waypoints(bytes: &[u8]) -> Result<js_sys::Array, JsValue> {
        let wps = crate::convert::obcr_to_waypoints(bytes).map_err(to_js)?;
        let arr = js_sys::Array::new();
        for w in wps {
            let obj = js_sys::Object::new();
            set(&obj, "name", &JsValue::from_str(&w.name));
            set(&obj, "lat", &JsValue::from_f64(w.lat));
            set(&obj, "lon", &JsValue::from_f64(w.lon));
            set(&obj, "ele", &w.ele.map_or(JsValue::NULL, JsValue::from_f64));
            set(&obj, "category", &JsValue::from_f64(f64::from(w.category)));
            set(&obj, "distAlongM", &JsValue::from_f64(f64::from(w.dist_along_m)));
            arr.push(&obj.into());
        }
        Ok(arr)
    }

    /// `Reflect::set` on a fresh plain object, which cannot fail, so the result is ignored.
    fn set(obj: &js_sys::Object, key: &str, value: &JsValue) {
        let _ = js_sys::Reflect::set(obj, &JsValue::from_str(key), value);
    }

    /// Build the JS exception as a real `Error` instance, so it carries a stack and survives
    /// `instanceof Error`, with the stable code hung off it as a plain property.
    ///
    /// A `#[wasm_bindgen]` struct would cross the boundary too, but it would not be an `Error`, so
    /// `catch (e) { e.message }` and every logger that formats errors would come up empty.
    fn to_js(f: ConvertFailure) -> JsValue {
        let err = js_sys::Error::new(&f.message);
        err.set_name("ObcConvertError");
        // `Reflect::set` only fails on a frozen target, and `err` is fresh. Ignored rather than
        // unwrapped, so a surprise here still throws a usable Error instead of trapping the module.
        let _ = js_sys::Reflect::set(&err, &JsValue::from_str("code"), &JsValue::from_str(f.code.as_str()));
        err.into()
    }
}
