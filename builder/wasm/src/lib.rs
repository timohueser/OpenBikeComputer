//! The builder's conversion, assembly and preview core with one browser boundary.

pub mod convert;
pub mod driver;
pub mod estimate;
pub mod gpx;
pub mod preview;

#[cfg(target_arch = "wasm32")]
mod browser;

pub use driver::*;
pub use estimate::*;
pub use preview::*;

mod assemble_web;
mod convert_web;
mod preview_web;

#[cfg(all(target_arch = "wasm32", feature = "test-device"))]
mod test_device;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}
