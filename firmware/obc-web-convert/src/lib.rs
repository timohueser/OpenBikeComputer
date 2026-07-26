//! `obc-web-convert` — the hosted builder's conversion bridge (epic #894, A2).
//!
//! The hosted tier has no backend, and this is what removes the last reason it would need one:
//! `gpx_to_obcr` and `track_to_gpx` are `no_std` Rust in crates that already compile to wasm, so
//! route conversion runs in the visitor's browser through **the exact code the device and the CLI
//! run**. Not a re-implementation in TypeScript — the same bytes, by construction.
//!
//! Deliberately not a framework host like [`obc-web-demo`](../obc_web_demo/index.html): no frame
//! loop, no canvas, no state. Two pure functions and an error vocabulary.
//!
//! | export | contract |
//! | :-- | :-- |
//! | `obc_convert_gpx_to_obcr(bytes, name) -> Uint8Array` | a GPX file's bytes → a `.obcr` route |
//! | `obc_convert_track_to_gpx(bytes, name) -> string` | a recorded `.obct` log → a GPX 1.1 document |
//!
//! A failure crosses to JS as a thrown `Error` whose `message` is written for a rider and whose
//! `code` ([`ErrorCode`]) is the stable identifier a caller branches on. Every
//! [`obc_formats::io::Error`] variant is mapped by hand in [`convert`] — the matches are
//! exhaustive so a new variant breaks this build instead of quietly inheriting someone else's
//! wording.
//!
//! The conversion core ([`convert`]) is target-independent and unit-tested natively by the
//! workspace `cargo test`; only the bindgen shim below is wasm-specific.

mod convert;

pub use convert::{gpx_to_obcr, track_to_gpx, ConvertFailure, ErrorCode, MAX_STORED_POINTS};

#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::prelude::*;

    use crate::convert::ConvertFailure;

    /// Module start (wasm-bindgen runs this during instantiation): surface Rust panics in the
    /// console instead of an opaque `unreachable` trap. Nothing else — there is no state to build,
    /// so instantiation stays as cheap as the download.
    #[wasm_bindgen(start)]
    pub fn start() {
        console_error_panic_hook::set_once();
    }

    /// Convert a GPX file's bytes into `.obcr` bytes, naming the route `name` (the OBCR header
    /// truncates it to the format's cap on a char boundary — an over-long name is not an error).
    ///
    /// Throws an `Error` carrying `code` + `message` on failure; see [`crate::ErrorCode`].
    #[wasm_bindgen]
    pub fn obc_convert_gpx_to_obcr(bytes: &[u8], name: &str) -> Result<Vec<u8>, JsValue> {
        crate::convert::gpx_to_obcr(bytes, name).map_err(to_js)
    }

    /// Convert a recorded `.obct` ride log into a GPX 1.1 document, naming the track `name`.
    ///
    /// Throws an `Error` carrying `code` + `message` on failure; see [`crate::ErrorCode`].
    #[wasm_bindgen]
    pub fn obc_convert_track_to_gpx(bytes: &[u8], name: &str) -> Result<String, JsValue> {
        crate::convert::track_to_gpx(bytes, name).map_err(to_js)
    }

    /// Build the JS exception: a real `Error` instance (so it carries a stack and survives
    /// `instanceof Error`), renamed, with the stable code hung off it as a plain property.
    ///
    /// A `#[wasm_bindgen]` struct would also cross the boundary, but it would not *be* an `Error`
    /// — `catch (e) { e.message }` and every logger that formats errors would come up empty.
    fn to_js(f: ConvertFailure) -> JsValue {
        let err = js_sys::Error::new(&f.message);
        err.set_name("ObcConvertError");
        // `Reflect::set` only fails on a frozen/exotic target; `err` is a fresh object, so this
        // cannot. Ignored rather than unwrapped so a surprise here still throws a usable Error
        // (with a message) instead of trapping the module.
        let _ = js_sys::Reflect::set(&err, &JsValue::from_str("code"), &JsValue::from_str(f.code.as_str()));
        err.into()
    }
}
