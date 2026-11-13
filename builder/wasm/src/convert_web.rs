#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::prelude::*;

    use crate::convert::ConvertFailure;

    /// Convert a GPX file's bytes into `.obcr` bytes, naming the route `name` and typing it
    /// `bike` (`0..=3`: Road, Gravel, MTB, Touring). `bike` crosses as `f64` because a narrower
    /// integer would wrap or truncate a bad JS number into a valid type. The header truncates an over-long name on a
    /// char boundary rather than refusing it.
    ///
    /// Throws an `Error` carrying `code` and `message` on failure; see [`crate::convert::ErrorCode`].
    #[wasm_bindgen]
    pub fn obc_convert_gpx_to_obcr(bytes: &[u8], name: &str, bike: f64) -> Result<Vec<u8>, JsValue> {
        let whole = bike.fract() == 0.0 && (0.0..=3.0).contains(&bike);
        let bike = whole.then_some(bike as u8).and_then(obc_route::BikeType::from_u8).ok_or_else(|| {
            to_js(ConvertFailure {
                code: crate::convert::ErrorCode::Internal,
                // A static message: formatting the `f64` would link float printing into the module.
                message: "Internal error: the bike type is not 0 to 3. This is a bug in the builder.".into(),
            })
        })?;
        crate::convert::gpx_to_obcr(bytes, name, bike).map_err(to_js)
    }

    /// Convert a finished ride-v6 object into a GPX 1.1 document, naming the track `name`.
    ///
    /// Throws an `Error` carrying `code` and `message` on failure; see [`crate::convert::ErrorCode`].
    #[wasm_bindgen]
    pub fn obc_convert_track_to_gpx(bytes: &[u8], name: &str) -> Result<String, JsValue> {
        crate::convert::track_to_gpx(bytes, name).map_err(to_js)
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
