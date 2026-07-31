//! `obc-web-assemble` — the hosted builder's **assembly bridge** (epic #1016, P4b).
//!
//! The cell catalog's whole promise is that any selection is an *assembly*, not a bake. This is what
//! makes that true without a backend: [`obcm-assemble`](../obcm_assemble/index.html), the OBCA
//! engine, compiled to wasm and driven from a browser tab. The cells the builder downloaded go in;
//! one `.obcm` — or a volume set's shards plus its OBCS manifest — comes out, byte-for-byte
//! identical to what the native CLI produces from the same inputs.
//!
//! Deliberately not a framework host like [`obc-web-demo`](../obc_web_demo/index.html): no frame
//! loop, no canvas, no state beyond the buffers an assembly is holding.
//!
//! | export | contract |
//! | :-- | :-- |
//! | `new Assembler(schemaJson, skinJson, optionsJson?)` | an empty assembly, waiting for cells |
//! | `.addCell(id, band, partial, bytes)` | hand over one downloaded cell; `bytes` crosses **once** |
//! | `.run(onProgress?)` | assemble; returns the summary JSON, throws a typed error |
//! | `.fileCount` / `.fileName(i)` / `.fileRole(i)` / `.fileSha256(i)` / `.fileByteLength(i)` | the finished set, shards first and the manifest **last** (OBCA §5.4) |
//! | `.takeFile(i)` | move one file's bytes out to JS and free the wasm-side copy |
//! | `.warnings()` | what OBCA says a producer SHOULD report rather than refuse |
//! | `.releaseCells()` | drop the input buffers once the output is taken |
//! | `obc_assemble_estimate(networkBandBytes, totalCellBytes)` | can this selection be assembled in a tab at all — **before** the download |
//!
//! A failure crosses to JS as a thrown `Error` whose `message` is the engine's own and whose `code`
//! ([`ErrorCode`]) is the stable identifier a caller branches on. The three that matter are kept
//! apart on purpose: `input` is a selection to fix, `capacity` is coverage to reduce, and `verify`
//! is a defect in the assembler — nothing was handed on, and nothing should be.
//!
//! The driver ([`driver`]) and the memory model ([`estimate`]) are target-independent and unit-tested
//! natively by the workspace `cargo test`; only the bindgen shim below is wasm-specific.

pub mod driver;
pub mod estimate;

pub use driver::{
    assemble_cells, AssembleFailure, BridgeOptions, CellBytes, ErrorCode, Hooks, NoHooks, Outcome, OutputFile, Phase,
};
pub use estimate::{
    estimate_memory, MemoryEstimate, OUTPUT_PER_CELL_BYTE, PEAK_PER_NAV_BYTE, PRACTICAL_BUDGET, WASM32_ADDRESS_SPACE,
};

#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::prelude::*;

    use crate::driver::{
        assemble_cells, AssembleFailure, BridgeOptions, CellBytes, ErrorCode, Hooks, OutputFile, Phase,
    };

    /// Module start (wasm-bindgen runs this during instantiation): surface Rust panics in the console
    /// instead of an opaque `unreachable` trap. Nothing else — there is no state to build, so
    /// instantiation stays as cheap as the download.
    #[wasm_bindgen(start)]
    pub fn start() {
        console_error_panic_hook::set_once();
    }

    /// The browser's [`Hooks`]: `Date.now()` for the phase split, and the caller's callback for
    /// progress and abort.
    struct JsHooks {
        on_progress: Option<js_sys::Function>,
        /// `Date.now()` is wall clock and can step backwards (NTP, a suspended tab). The engine
        /// subtracts consecutive readings, so a step back would underflow a `u64`; clamping here
        /// keeps the seam monotonic at the cost of a phase timing that reads as zero.
        last_us: u64,
    }

    impl Hooks for JsHooks {
        fn now_us(&mut self) -> u64 {
            let now = (js_sys::Date::now() * 1000.0) as u64;
            self.last_us = self.last_us.max(now);
            self.last_us
        }

        fn progress(&mut self, phase: Phase, fraction: f64) -> bool {
            let Some(f) = &self.on_progress else { return false };
            // A callback that throws is ignored rather than turned into an abort: it is a defect in
            // the caller's own reporting code, and losing a half-hour assembly to a typo in a
            // progress bar would be the worse failure. Abort by *returning* true.
            f.call2(&JsValue::NULL, &JsValue::from_str(phase.as_str()), &JsValue::from_f64(fraction))
                .map(|v| v.is_truthy())
                .unwrap_or(false)
        }
    }

    /// One assembly: cells in, an OBCA set out.
    ///
    /// The lifecycle is fixed — construct, `addCell` for every downloaded cell, `run`, then take the
    /// files. Cells may be handed over as they finish downloading; nothing is parsed until `run`.
    #[wasm_bindgen]
    pub struct Assembler {
        schema_json: String,
        skin_json: String,
        options: BridgeOptions,
        cells: Vec<CellBytes>,
        files: Vec<OutputFile>,
        warnings: Vec<String>,
    }

    #[wasm_bindgen]
    impl Assembler {
        /// Start an assembly at a schema and a skin (OBCC §11.3 / §11.4 documents, as JSON text).
        ///
        /// `options_json` is an optional object: `{name, cardId, targetShardBytes, acceptHoles,
        /// acceptPartial, forceSplit}`, every field optional. Unknown keys are ignored, so a newer
        /// builder can talk to an older module. There is deliberately **no** `skipVerify`: OBCA §4.8
        /// makes the read-back a precondition of writing a set, and this bridge exists to hand bytes
        /// to a device.
        #[wasm_bindgen(constructor)]
        pub fn new(schema_json: String, skin_json: String, options_json: Option<String>) -> Result<Assembler, JsValue> {
            let options = BridgeOptions::parse(options_json.as_deref().unwrap_or(""))
                .map_err(|e| to_js(AssembleFailure { code: ErrorCode::Internal, message: e }))?;
            Ok(Assembler {
                schema_json,
                skin_json,
                options,
                cells: Vec::new(),
                files: Vec::new(),
                warnings: Vec::new(),
            })
        }

        /// Hand over one downloaded cell: its catalog identity, its OBCA §3.7 `partial` flag, and its
        /// verified bytes.
        ///
        /// `bytes` crosses the boundary exactly once — wasm-bindgen copies the `Uint8Array` into
        /// linear memory and this takes ownership of that copy, so nothing is buffered twice. The JS
        /// side may drop its own reference immediately.
        #[wasm_bindgen(js_name = addCell)]
        pub fn add_cell(&mut self, id: String, band: String, partial: bool, bytes: Vec<u8>) {
            self.cells.push(CellBytes { id, band, partial, bytes });
        }

        /// How many cells are waiting.
        #[wasm_bindgen(getter, js_name = cellCount)]
        pub fn cell_count(&self) -> usize {
            self.cells.len()
        }

        /// Assemble, and return the summary as JSON — the same document `obcm-assemble --json`
        /// prints. The files themselves are then taken one at a time with [`Assembler::take_file`].
        ///
        /// `on_progress(phase, fraction)` is called at every phase boundary and about a hundred times
        /// over the write; `phase` is one of `open`/`poi`/`nav`/`plan`/`write`/`verify`/`manifest`/
        /// `done` and `fraction` is **overall** completion, weighted by the measured phase split.
        /// Returning a truthy value asks for an abort, honoured at the next write — see
        /// [`crate::driver`] for the granularity.
        ///
        /// Throws an `Error` carrying `code` + `message` on failure; see [`crate::ErrorCode`].
        pub fn run(&mut self, on_progress: Option<js_sys::Function>) -> Result<String, JsValue> {
            let mut hooks = JsHooks { on_progress, last_us: 0 };
            let cells = core::mem::take(&mut self.cells);
            let out =
                assemble_cells(cells, &self.schema_json, &self.skin_json, &self.options, &mut hooks).map_err(to_js)?;
            self.files = out.files;
            self.warnings = out.warnings;
            Ok(out.summary_json)
        }

        /// How many files the finished set has: every shard, then the OBCS manifest **last**.
        #[wasm_bindgen(getter, js_name = fileCount)]
        pub fn file_count(&self) -> usize {
            self.files.len()
        }

        /// File `index`'s derived 8.3 filename (`MS<id>S<kk>.OBM`, `MS<id>.OBS`).
        #[wasm_bindgen(js_name = fileName)]
        pub fn file_name(&self, index: usize) -> Result<String, JsValue> {
            self.file(index).map(|f| f.name.clone())
        }

        /// `"core"`, `"coarse"`, `"geometry"`, or `"manifest"`.
        #[wasm_bindgen(js_name = fileRole)]
        pub fn file_role(&self, index: usize) -> Result<String, JsValue> {
            self.file(index).map(|f| f.role.to_string())
        }

        /// File `index`'s lowercase-hex SHA-256, as the manifest records it (empty for the manifest).
        #[wasm_bindgen(js_name = fileSha256)]
        pub fn file_sha256(&self, index: usize) -> Result<String, JsValue> {
            self.file(index).map(|f| f.sha256.clone())
        }

        /// File `index`'s size, readable without moving the bytes — so a caller can plan a transfer
        /// before it pays for one.
        #[wasm_bindgen(js_name = fileByteLength)]
        pub fn file_byte_length(&self, index: usize) -> Result<usize, JsValue> {
            self.file(index).map(|f| f.bytes.len())
        }

        /// Move file `index`'s bytes out to JS, **freeing the wasm-side copy**.
        ///
        /// One file at a time is the whole point: an assembled set can be gigabytes, and taking them
        /// one by one means the transient double-residency is one file rather than the set. A second
        /// call for the same index returns an empty array — the bytes are gone, on purpose.
        #[wasm_bindgen(js_name = takeFile)]
        pub fn take_file(&mut self, index: usize) -> Result<Vec<u8>, JsValue> {
            let f = self.files.get_mut(index).ok_or_else(|| {
                to_js(AssembleFailure {
                    code: ErrorCode::Internal,
                    message: format!("file index {index} does not exist"),
                })
            })?;
            Ok(core::mem::take(&mut f.bytes))
        }

        /// Everything OBCA says a producer SHOULD *report* rather than refuse: §5.7's core-headroom
        /// warning, §4.5.2's dropped duplicate POIs, `OBCM_Spec.md` §8.3's degree-cap truncations.
        /// An assembly with warnings is still a legal set; ignoring them ships the same bytes.
        pub fn warnings(&self) -> js_sys::Array {
            self.warnings.iter().map(|w| JsValue::from_str(w)).collect()
        }

        /// Drop the input cell buffers. Automatic on `run`; exposed for the caller that abandons an
        /// assembly it was still feeding.
        #[wasm_bindgen(js_name = releaseCells)]
        pub fn release_cells(&mut self) {
            self.cells = Vec::new();
        }

        fn file(&self, index: usize) -> Result<&OutputFile, JsValue> {
            self.files.get(index).ok_or_else(|| {
                to_js(AssembleFailure {
                    code: ErrorCode::Internal,
                    message: format!("file index {index} does not exist"),
                })
            })
        }
    }

    /// Project the peak memory of assembling a selection, **before** downloading it: pass the
    /// catalog's own byte totals for the selected cells (`network` band alone, and every band) and
    /// get `{engineBytes, inputBytes, outputBytes, peakBytes, budgetBytes, ceilingBytes, fits,
    /// headroomBytes}`.
    ///
    /// This complements OBCA §5.7's file-size ledger rather than repeating it: §5.7 prices the
    /// *output* against the format's 4 GiB per-file ceiling, this prices the *run* against wasm32's
    /// 4 GiB address space. A selection can pass one and fail the other. See [`crate::estimate`] for
    /// the model and where its constants were measured.
    #[wasm_bindgen]
    pub fn obc_assemble_estimate(network_band_bytes: f64, total_cell_bytes: f64) -> js_sys::Object {
        let e = crate::estimate::estimate_memory(network_band_bytes, total_cell_bytes);
        let obj = js_sys::Object::new();
        set(&obj, "engineBytes", &JsValue::from_f64(e.engine_bytes));
        set(&obj, "inputBytes", &JsValue::from_f64(e.input_bytes));
        set(&obj, "outputBytes", &JsValue::from_f64(e.output_bytes));
        set(&obj, "peakBytes", &JsValue::from_f64(e.peak_bytes));
        set(&obj, "budgetBytes", &JsValue::from_f64(e.budget_bytes));
        set(&obj, "ceilingBytes", &JsValue::from_f64(e.ceiling_bytes));
        set(&obj, "headroomBytes", &JsValue::from_f64(e.headroom_bytes));
        set(&obj, "fits", &JsValue::from_bool(e.fits));
        obj
    }

    /// `Reflect::set` on a fresh plain object — which cannot fail (only frozen/exotic targets can),
    /// so the result is ignored for the same reason it is in [`to_js`].
    fn set(obj: &js_sys::Object, key: &str, value: &JsValue) {
        let _ = js_sys::Reflect::set(obj, &JsValue::from_str(key), value);
    }

    /// Build the JS exception: a real `Error` instance (so it carries a stack and survives
    /// `instanceof Error`), renamed, with the stable code hung off it as a plain property.
    ///
    /// A `#[wasm_bindgen]` struct would also cross the boundary, but it would not *be* an `Error` —
    /// `catch (e) { e.message }` and every logger that formats errors would come up empty.
    fn to_js(f: AssembleFailure) -> JsValue {
        let err = js_sys::Error::new(&f.message);
        err.set_name("ObcAssembleError");
        // `Reflect::set` only fails on a frozen/exotic target; `err` is a fresh object, so this
        // cannot. Ignored rather than unwrapped so a surprise here still throws a usable Error (with
        // a message) instead of trapping the module.
        let _ = js_sys::Reflect::set(&err, &JsValue::from_str("code"), &JsValue::from_str(f.code.as_str()));
        err.into()
    }
}
