//! `obc-web-demo` — the landing page's thin wasm host (epic #624, S6).
//!
//! The page ships the **real firmware render path** — the same `obc-app`/`obc-render` stack the
//! nRF54L runs — behind a deliberately tiny JS surface. No egui/winit/wgpu: JS owns the rAF loop,
//! the `<canvas>`, and the long-press hold timers; this crate owns everything true (app state,
//! replay, planner, framebuffer). One input path for everything — the page's DOM buttons, the
//! keyboard, and the guided-tour engine all speak the same `obc_demo_cmd` vocabulary.
//!
//! Exports (all under `window.wasmBindings` via Trunk):
//!
//! | export | contract |
//! | :-- | :-- |
//! | `obc_demo_tick(now_ms) -> bool` | advance + render one frame; `true` if the frame changed |
//! | `obc_demo_frame() -> Uint8ClampedArray` | RGBA view of the 240×320 frame, for `putImageData` |
//! | `obc_demo_cmd(cmd)` | queue a command (drained per tick) — see [`demo::parse_cmd`] |
//! | `obc_demo_state() -> String` | the current screen's `Screen::name()` |
//! | `obc_demo_ready() -> bool` | first frame rendered |
//! | `obc_demo_screens() -> Vec<String>` | every `Screen::name()` — the tour drift-guard (S3) |
//!
//! The demo core ([`demo`], [`frame`]) is target-independent and unit-tested natively; only this
//! bindgen shim is wasm-specific.

// On the native build only the tests reference the demo core (the bindgen surface is wasm-only),
// so quiet the resulting dead-code noise there; the wasm build sees every use.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

mod demo;
mod frame;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;

    use wasm_bindgen::prelude::*;

    use crate::demo::Demo;

    thread_local! {
        /// The one demo instance (wasm is single-threaded; a thread-local `RefCell` is the
        /// standard interior-mutability shape for bindgen exports). Heap-allocated — `Demo`
        /// embeds the app + map cache. Built in [`init`] at module start; the lazy fallback
        /// below covers a call racing ahead of it.
        static DEMO: RefCell<Option<Box<Demo>>> = const { RefCell::new(None) };
    }

    /// Run `f` on the demo, building it first if needed.
    fn with_demo<R>(f: impl FnOnce(&mut Demo) -> R) -> R {
        DEMO.with(|d| f(d.borrow_mut().get_or_insert_with(Demo::new)))
    }

    /// One-time startup: panic messages to the console, then build the demo (map-table parse +
    /// GPX parse — cheap next to the download) so the first `obc_demo_tick` only renders.
    pub fn init() {
        console_error_panic_hook::set_once();
        with_demo(|_| ());
    }

    /// Advance one frame on the JS rAF clock; `true` if the frame changed (only then does the
    /// page need `obc_demo_frame` + `putImageData`).
    #[wasm_bindgen]
    pub fn obc_demo_tick(now_ms: f64) -> bool {
        with_demo(|d| d.tick(now_ms))
    }

    /// A **view** of the current RGBA frame (240×320×4, opaque alpha) over wasm memory — wrap it
    /// in an `ImageData` and `putImageData` it immediately. Do not retain it: any later wasm call
    /// may grow the memory and detach the view. Zero-copy on the Rust side.
    ///
    /// Built through the explicit `new Uint8ClampedArray(memory.buffer, ptr, len)` constructor:
    /// `js_sys::Uint8ClampedArray::view` hands back a plain `Uint8Array` at runtime (u8-element
    /// views share one generated shim), and the `ImageData(data, w, h)` constructor type-checks
    /// for the *clamped* array — verified in-browser, the wrong type throws.
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

    /// Queue one command (drained on the next tick). Vocabulary: `press`, `back`, `hold`,
    /// `backhold`, `turn:<n>`, `play`, `pause`, `seek:<secs>`, `enter`, `exit`, `ambient`.
    /// Unknown or malformed input is ignored — the page can't crash the demo with a typo.
    #[wasm_bindgen]
    pub fn obc_demo_cmd(cmd: &str) {
        with_demo(|d| d.cmd(cmd));
    }

    /// The current screen's variant name (`"Map"`, `"NavPlanning"`, …) — the closed-loop signal
    /// the page advances a guided demo on.
    #[wasm_bindgen]
    pub fn obc_demo_state() -> String {
        with_demo(|d| d.state().to_string())
    }

    /// True once the first frame has rendered (swap the poster for the live canvas).
    #[wasm_bindgen]
    pub fn obc_demo_ready() -> bool {
        with_demo(|d| d.ready())
    }

    /// Every screen's `Screen::name()`, straight from the one `screens!` table — the drift-guard
    /// hook (S3): a tour scripted against a name not in this list fails CI instead of stalling.
    #[wasm_bindgen]
    pub fn obc_demo_screens() -> Vec<String> {
        obc_app::Screen::NAMES.iter().map(|s| s.to_string()).collect()
    }
}

/// Wasm entry (Trunk runs `main` during module init, before `TrunkApplicationStarted` fires):
/// install the panic hook and build the demo.
#[cfg(target_arch = "wasm32")]
fn main() {
    web::init();
}

/// This crate only ships on wasm; the native build exists so the workspace can type-check and
/// unit-test the demo core (`cargo test` from `firmware/`).
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("obc-web-demo is the landing page's wasm host — build it via `trunk build --config docs/Trunk.toml`.");
    eprintln!("(The native target exists only so `cargo test` covers the demo core.)");
    std::process::exit(2);
}
