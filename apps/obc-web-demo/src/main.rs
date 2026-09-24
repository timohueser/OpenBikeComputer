//! The landing page's thin wasm host. The page ships the real firmware render path behind a small
//! JS surface: JS owns the rAF loop, the `<canvas>` and the long-press hold timers, and this crate
//! owns the app state, the replay, the planner and the framebuffer.
//!
//! The page's DOM buttons, the keyboard and the guided-tour engine all speak the same
//! `obc_demo_cmd` vocabulary. Every export lands under `window.wasmBindings`.
//!
//! The demo core ([`demo`]) is target-independent and unit-tested natively; only this bindgen shim
//! is wasm-specific.

// On the native build only the tests reference the demo core, because the bindgen surface is
// wasm-only. The wasm build sees every use.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

mod demo;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;

    use wasm_bindgen::prelude::*;

    use crate::demo::Demo;

    thread_local! {
        /// The one demo instance. wasm is single-threaded, so a thread-local `RefCell` is the
        /// interior-mutability shape bindgen exports use. Built in [`init`] at module start; the
        /// lazy fallback below covers a call that races ahead of it.
        static DEMO: RefCell<Option<Box<Demo>>> = const { RefCell::new(None) };
    }

    /// Run `f` on the demo, building it first if needed.
    fn with_demo<R>(f: impl FnOnce(&mut Demo) -> R) -> R {
        DEMO.with(|d| f(d.borrow_mut().get_or_insert_with(Demo::new)))
    }

    /// One-time startup: panic messages to the console, then build the demo, so the first
    /// `obc_demo_tick` only renders.
    pub fn init() {
        console_error_panic_hook::set_once();
        with_demo(|_| ());
    }

    /// Advance one frame on the JS rAF clock, and answer whether the frame changed. The page needs
    /// `obc_demo_frame` and `putImageData` only then.
    #[wasm_bindgen]
    pub fn obc_demo_tick(now_ms: f64) -> bool {
        with_demo(|d| d.tick(now_ms))
    }

    /// A view of the current RGBA frame over wasm memory. Wrap it in an `ImageData` and
    /// `putImageData` it immediately. Do not retain it: any later wasm call can grow the memory and
    /// detach the view.
    ///
    /// Built through the explicit `new Uint8ClampedArray(memory.buffer, ptr, len)` constructor.
    /// `js_sys::Uint8ClampedArray::view` hands back a plain `Uint8Array` at runtime, and the
    /// `ImageData(data, w, h)` constructor throws for anything but the clamped array.
    #[wasm_bindgen]
    pub fn obc_demo_frame() -> js_sys::Uint8ClampedArray {
        use wasm_bindgen::JsCast as _;
        with_demo(|d| {
            let buf = d.frame();
            let mem = wasm_bindgen::memory().unchecked_into::<js_sys::WebAssembly::Memory>();
            js_sys::Uint8ClampedArray::new_with_byte_offset_and_length(
                &mem.buffer(),
                buf.as_ptr() as u32,
                buf.len() as u32,
            )
        })
    }

    /// Queue one command, drained on the next tick. See [`demo::parse_cmd`] for the vocabulary.
    /// Unknown input is ignored, so the page cannot crash the demo with a typo.
    #[wasm_bindgen]
    pub fn obc_demo_cmd(cmd: &str) {
        with_demo(|d| d.cmd(cmd));
    }

    /// Match the page theme before the first frame or after a visitor changes it.
    #[wasm_bindgen]
    pub fn obc_demo_set_dark(dark: bool) {
        with_demo(|d| d.set_dark(dark));
    }

    /// The current screen's variant name: the closed-loop signal the page advances a guided demo
    /// on.
    #[wasm_bindgen]
    pub fn obc_demo_state() -> String {
        with_demo(|d| d.state().to_string())
    }

    /// True once the first frame has rendered, so the page can swap the poster for the canvas.
    #[wasm_bindgen]
    pub fn obc_demo_ready() -> bool {
        with_demo(|d| d.ready())
    }

    /// Peak View remains the base while a drawer is open.
    #[wasm_bindgen]
    pub fn obc_demo_peak_active() -> bool {
        with_demo(|d| d.peak_active())
    }

    /// Current heading for the browser's simulated compass control.
    #[wasm_bindgen]
    pub fn obc_demo_heading() -> u16 {
        with_demo(|d| d.heading())
    }

    /// True once the current Peak View heading has terrain and summit visibility.
    #[wasm_bindgen]
    pub fn obc_demo_peak_ready() -> bool {
        with_demo(|d| d.peak_ready())
    }

    /// True only after Find has measured choices and released the trial planner.
    #[wasm_bindgen]
    pub fn obc_demo_find_ready() -> bool {
        with_demo(|d| d.find_ready())
    }

    /// Shared immutable Visit review state; screen names alone cannot distinguish planning.
    #[wasm_bindgen]
    pub fn obc_demo_visit_status() -> String {
        with_demo(|d| format!("{:?}", d.visit_status()))
    }

    /// A queued reset is Pending until cleanup and baseline installation finish.
    #[wasm_bindgen]
    pub fn obc_demo_reset_status() -> String {
        with_demo(|d| format!("{:?}", d.reset_status()))
    }

    /// Every screen's `Screen::name()`, straight from the `screens!` table. A tour scripted against
    /// a name not in this list fails CI instead of stalling.
    #[wasm_bindgen]
    pub fn obc_demo_screens() -> Vec<String> {
        obc_app::Screen::NAMES.iter().map(|s| s.to_string()).collect()
    }
}

/// Wasm entry: Trunk runs `main` during module init, before `TrunkApplicationStarted` fires.
#[cfg(target_arch = "wasm32")]
fn main() {
    web::init();
}

/// This crate only ships on wasm. The native build exists so the workspace can type-check and
/// unit-test the demo core.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("obc-web-demo is the landing page's wasm host — build it via `trunk build --config docs/Trunk.toml`.");
    eprintln!("(The native target exists only so `cargo test` covers the demo core.)");
    std::process::exit(2);
}
