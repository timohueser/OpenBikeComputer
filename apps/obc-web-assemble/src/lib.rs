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
//! | `.addCellByKey(id, band, partial, byteLength, key)` | …or leave the bytes outside wasm and read them on demand (#1116 B2) |
//! | `.setTerrain(postingLog2, cellLog2)` / `.addTerrainCell(id, sha256, bytes)` | the raster (EL4) |
//! | `.run(onProgress?, onFile?, onRead?)` | assemble; returns the summary JSON, throws a typed error |
//! | `.fileCount` / `.fileName(i)` / `.fileRole(i)` / `.fileSha256(i)` / `.fileByteLength(i)` | whatever `onFile` did **not** take, shards first and the manifest **last** (OBCA §5.4) |
//! | `.takeFile(i)` | move one file's bytes out to JS and free the wasm-side copy; twice throws |
//! | `.warnings()` | what OBCA says a producer SHOULD report rather than refuse |
//! | `.releaseCells()` | drop the input buffers once the output is taken |
//! | `obc_assemble_estimate(networkBandBytes, totalCellBytes, budgetBytes?)` | can this selection be assembled in a tab at all — **before** the download |
//!
//! `.run()` **blocks** for the whole assembly — ~20 s at country scale — so it belongs in a **Web
//! Worker**, not on the main thread. That contract, and what a cancel button has to do given it, is
//! written down in `builder/app/src/lib/assemble/bridge.ts`.
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
    assemble, assemble_cells, assemble_cells_with_known_empty, assemble_everything, AssembleFailure, BridgeOptions,
    CellBytes, CellReads, ErrorCode, Hooks, Inputs, KnownEmptyCell, NoHooks, Outcome, OutputFile, Phase, SourceCell,
    TerrainCellBytes, TerrainLattice,
};
pub use estimate::{
    estimate_memory, estimate_memory_with_budget, MemoryEstimate, OUTPUT_PER_CELL_BYTE, PEAK_PER_NAV_BYTE,
    PRACTICAL_BUDGET, WASM32_ADDRESS_SPACE,
};

#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::prelude::*;

    use crate::driver::{
        assemble, AssembleFailure, BridgeOptions, CellBytes, CellReads, ErrorCode, Hooks, Inputs, KnownEmptyCell,
        OutputFile, Phase, SourceCell, TerrainCellBytes, TerrainLattice,
    };

    /// Module start (wasm-bindgen runs this during instantiation): surface Rust panics in the console
    /// instead of an opaque `unreachable` trap. Nothing else — there is no state to build, so
    /// instantiation stays as cheap as the download.
    #[wasm_bindgen(start)]
    pub fn start() {
        console_error_panic_hook::set_once();
    }

    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_namespace = console, js_name = warn)]
        fn console_warn(msg: &str);
    }

    /// The browser's [`Hooks`]: `Date.now()` for the phase split, and the caller's callback for
    /// progress and abort.
    struct JsHooks {
        on_progress: Option<js_sys::Function>,
        /// `on_file(name, role, sha256, bytes)`, called from inside `run` as each shard passes its
        /// §4.8 verify. Its presence is what turns the hand-off on at all.
        on_file: Option<js_sys::Function>,
        /// `Date.now()` is wall clock and can step backwards (NTP, a suspended tab). The engine
        /// subtracts consecutive readings, so a step back would underflow a `u64`; clamping here
        /// keeps the seam monotonic at the cost of a phase timing that reads as zero.
        last_us: u64,
        /// Whether the throwing-callback warning has already been printed. A progress callback fires
        /// a hundred times a run and a broken one throws every time; one line is a bug report, a
        /// hundred is a reason to stop reading the console.
        warned: bool,
    }

    impl Hooks for JsHooks {
        fn now_us(&mut self) -> u64 {
            let now = (js_sys::Date::now() * 1000.0) as u64;
            self.last_us = self.last_us.max(now);
            self.last_us
        }

        fn progress(&mut self, phase: Phase, fraction: f64) -> bool {
            let Some(f) = &self.on_progress else { return false };
            // A callback that throws does not abort the run: it is a defect in the caller's own
            // reporting code, and losing a half-hour assembly to a typo in a progress bar would be
            // the worse failure. Abort by *returning* true. It is not swallowed either — silence
            // would leave a dead progress bar looking like a hung assembler.
            match f.call2(&JsValue::NULL, &JsValue::from_str(phase.as_str()), &JsValue::from_f64(fraction)) {
                Ok(v) => v.is_truthy(),
                Err(e) => {
                    if !self.warned {
                        self.warned = true;
                        console_warn(&format!(
                            "obc-web-assemble: the progress callback threw ({e:?}). The assembly continues and the \
                             callback keeps being called; this is reported once. To cancel, *return* a truthy value \
                             rather than throwing."
                        ));
                    }
                    false
                }
            }
        }

        fn wants_shards(&self) -> bool {
            self.on_file.is_some()
        }

        /// Hand one verified shard to JS and free the wasm-side buffer.
        ///
        /// The order is the whole point. The bytes are copied into a JS `Uint8Array`, the Rust
        /// `Vec` is dropped, and only *then* is the callback invoked — so the wasm heap is already
        /// back down to the rest of the run before the worker starts transferring the buffer on.
        /// The copy itself is unavoidable (linear memory cannot be donated to an `ArrayBuffer`), and
        /// it is exactly what `takeFile` has always done; what changes is when.
        ///
        /// A callback that **throws** is not survivable the way a thrown progress callback is: the
        /// shard's bytes are already gone, so continuing would produce a set that is missing a file
        /// and report it as finished. It fails the run instead, as `io` — the sink did fail.
        fn take_shard(&mut self, shard: OutputFile) -> Result<Option<OutputFile>, String> {
            let Some(f) = &self.on_file else { return Ok(Some(shard)) };
            let OutputFile { name, role, sha256, bytes } = shard;
            let array = js_sys::Uint8Array::from(bytes.as_slice());
            drop(bytes);
            let args = js_sys::Array::of4(
                &JsValue::from_str(&name),
                &JsValue::from_str(role),
                &JsValue::from_str(&sha256),
                &array,
            );
            match f.apply(&JsValue::NULL, &args) {
                Ok(_) => Ok(None),
                Err(e) => Err(format!(
                    "the file sink threw while taking {name} ({e:?}), and the shard's bytes had already been handed \
                     to it. The set is incomplete and was not finished; nothing that was written is a map until the \
                     OBCS manifest exists (OBCA §5.4), so discard whatever was saved and re-run."
                )),
            }
        }
    }

    /// The browser's [`CellReads`]: one JS call per cache miss, filling a view over wasm's own
    /// linear memory (#1116 B2).
    ///
    /// The view is the reason this is cheap. `FileSystemSyncAccessHandle.read(buffer, {at})` takes
    /// any `ArrayBufferView`, so handing it one that *is* the destination block means the bytes go
    /// from the file into the wasm heap in one step — no intermediate `ArrayBuffer`, nothing copied
    /// on the JS side, nothing copied back. What crosses per call is a slot number, an offset and
    /// one freshly-made view object.
    struct JsReads {
        /// `read(slot, offset, dest) -> boolean`. Falsy means the read failed; see
        /// `builder/app/src/lib/assemble/bridge.ts` for the contract as callers see it.
        on_read: js_sys::Function,
    }

    impl CellReads for JsReads {
        fn read(&self, slot: usize, offset: u32, buf: &mut [u8]) -> Result<(), String> {
            // SAFETY: `view_mut_raw` aliases linear memory and is invalidated by anything that grows
            // it. This one is made, passed, and dropped inside a single synchronous JS call that
            // does nothing but fill it — no Rust allocation can run in between, and the callback is
            // documented not to re-enter the assembler. It is deliberately built per call rather
            // than cached for exactly that reason: a stored view would be detached by the next heap
            // growth, and the reads it served would silently return nothing.
            let dest = unsafe { js_sys::Uint8Array::view_mut_raw(buf.as_mut_ptr(), buf.len()) };
            let taken = self.on_read.call3(
                &JsValue::NULL,
                &JsValue::from_f64(slot as f64),
                &JsValue::from_f64(offset as f64),
                &dest,
            );
            match taken {
                Ok(v) if v.is_truthy() => Ok(()),
                Ok(_) => Err("the read callback returned a falsy value".into()),
                Err(e) => Err(format!("the read callback threw ({e:?})")),
            }
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
        /// Cells the host keeps outside wasm memory and serves on demand (#1116 B2). A cell's
        /// **slot** — what `run`'s read callback is given — is its index here, which is what
        /// [`Assembler::add_cell_by_key`] returns.
        source_cells: Vec<SourceCell>,
        known_empty: Vec<KnownEmptyCell>,
        /// The catalog's terrain lattice, once the caller declares one. `None` leaves the set
        /// without a `terrain` role — a complete map with flat profiles (`OBCC_Spec.md` §13).
        terrain: Option<TerrainLattice>,
        terrain_cells: Vec<TerrainCellBytes>,
        files: Vec<OutputFile>,
        /// Which files have already been moved out to JS. An emptied buffer is indistinguishable
        /// from a legitimately empty one, and the difference decides between handing back an
        /// unusable file and saying why — see [`Assembler::take_file`].
        taken: Vec<bool>,
        warnings: Vec<String>,
    }

    #[wasm_bindgen]
    impl Assembler {
        /// Start an assembly at a schema and a skin (OBCC §4 / §5 documents, as JSON text).
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
                source_cells: Vec::new(),
                known_empty: Vec::new(),
                terrain: None,
                terrain_cells: Vec::new(),
                files: Vec::new(),
                taken: Vec::new(),
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

        /// Hand over one downloaded cell **by reference**: the same identity and `partial` flag as
        /// [`Assembler::add_cell`], the length the catalog published, and an opaque key the host's
        /// own read callback resolves. The bytes never enter wasm memory (#1116 B2).
        ///
        /// Returns the cell's **slot** — the first argument `run`'s `on_read` is called with. Slots
        /// are handed out in call order from `0`; the return value exists so a host never has to
        /// assume that.
        ///
        /// The catalog's byte count is not a hint: it is what the engine reads as the cell's length,
        /// so a read past it is refused here rather than left to whatever the host would return. A
        /// wrong one surfaces as a format error at open, exactly as a truncated download does.
        ///
        /// Passing any of these without an `on_read` in [`Assembler::run`] fails the run as
        /// `internal` before a byte is read.
        #[wasm_bindgen(js_name = addCellByKey)]
        pub fn add_cell_by_key(
            &mut self,
            id: String,
            band: String,
            partial: bool,
            byte_length: u32,
            key: String,
        ) -> u32 {
            self.source_cells.push(SourceCell { id, band, partial, byte_length, key });
            (self.source_cells.len() - 1) as u32
        }

        /// Add one selected, canonical zero-byte cell. It affects the output
        /// bbox and coverage checks but has no buffer to transfer or graft.
        #[wasm_bindgen(js_name = addKnownEmpty)]
        pub fn add_known_empty(&mut self, id: String, band: String) {
            self.known_empty.push(KnownEmptyCell { id, band });
        }

        /// Declare the catalog's terrain lattice (`OBCC_Spec.md` §13.1's `posting_log2` /
        /// `cell_log2`). Calling it is what makes the set carry a `terrain` role at all; a catalog
        /// with no terrain block simply never calls it, and the map assembles exactly as before.
        ///
        /// Declaring the lattice with **no** cells is legal and meaningful: it writes a shard that
        /// is all directory, which says "this ground is canonically void" (open ocean, outside the
        /// dataset's coverage) rather than "the raster failed to arrive".
        #[wasm_bindgen(js_name = setTerrain)]
        pub fn set_terrain(&mut self, posting_log2: u8, cell_log2: u8) {
            self.terrain = Some(TerrainLattice { posting_log2, cell_log2 });
        }

        /// Hand over one downloaded terrain cell: its id on the terrain grid, the `sha256` the
        /// pinned terrain index published, and the whole `.obcd` object.
        ///
        /// A **known-empty** square is not handed over at all — it has no object, and an absent
        /// cell reads identically to an all-`NODATA` one (`OBCT_Spec.md` §4.3), which is exactly
        /// why the catalog publishes ocean as a row run rather than as megabytes of sentinel.
        #[wasm_bindgen(js_name = addTerrainCell)]
        pub fn add_terrain_cell(&mut self, id: String, sha256: String, bytes: Vec<u8>) {
            self.terrain_cells.push(TerrainCellBytes { id, sha256, bytes });
        }

        /// How many selected cells are waiting, in either form, including zero-byte coverage.
        #[wasm_bindgen(getter, js_name = cellCount)]
        pub fn cell_count(&self) -> usize {
            self.cells.len() + self.source_cells.len() + self.known_empty.len()
        }

        /// How many terrain cells are waiting.
        #[wasm_bindgen(getter, js_name = terrainCellCount)]
        pub fn terrain_cell_count(&self) -> usize {
            self.terrain_cells.len()
        }

        /// Assemble, and return the summary as JSON — the same document `obcm-assemble --json`
        /// prints. The files themselves are then taken one at a time with [`Assembler::take_file`].
        ///
        /// **This blocks.** A country-scale assembly is ~20 s of straight-line compute, so calling it
        /// on the main thread freezes the tab for the duration; run it in a Web Worker and post
        /// progress out. See `bridge.ts` for the full contract, cancellation included.
        ///
        /// `on_progress(phase, fraction)` is called at every phase boundary and about a hundred times
        /// over the write and the §4.8 read-back; `phase` is one of `open`/`poi`/`nav`/`plan`/
        /// `write`/`verify`/`manifest`/`done` and `fraction` is **overall** completion, weighted by
        /// the measured phase split. Returning a truthy value asks for an abort, honoured at the next
        /// write or verify read — see [`crate::driver`] for the granularity. A callback that
        /// *throws* is warned about once and otherwise ignored; it never cancels the run.
        ///
        /// `on_file(name, role, sha256, bytes)` is the **eviction** seam (#1116 B1), and passing it
        /// is what turns eviction on. It is called synchronously from inside `run`, once per shard,
        /// as soon as that shard's §4.8 read-back has passed — and the wasm-side buffer is freed
        /// before it runs, so the output's residency over a whole assembly is one shard rather than
        /// the whole set. A file taken this way is **not** in `fileCount` afterwards; `takeFile`
        /// still hands on everything that was not (the terrain shard, the manifest).
        ///
        /// Take the `Uint8Array` and return promptly: the assembly is blocked behind the callback,
        /// and nothing can be awaited from inside it. Post it on and let the consumer do the slow
        /// part. If it **throws**, the run fails as `io` — by then the bytes are gone, and a set with
        /// a hole in it must not be reported as finished. A run that fails or is cancelled may
        /// already have handed shards out; cleaning them up is the caller's job (§5.4 makes them
        /// invisible as a map until the manifest exists, so nothing half-usable reaches a device).
        ///
        /// `on_read(slot, offset, dest) -> boolean` is how the bytes of every cell added with
        /// [`Assembler::add_cell_by_key`] are fetched (#1116 B2), and it must be present if any
        /// were. It is called synchronously from inside the run — which is exactly what makes a
        /// `FileSystemSyncAccessHandle` usable, since those exist only in a dedicated worker and
        /// only synchronously — and it must **fill `dest` completely** and return `true`. Anything
        /// falsy, or a throw, fails the run as `io` naming the cell; a short read is a failure, not
        /// a partial success.
        ///
        /// `dest` is a view straight onto wasm's linear memory and is valid **only for the duration
        /// of the call**. Fill it and return; do not keep it, do not hand it to anything
        /// asynchronous, and do not call back into the assembler from inside it.
        ///
        /// Reads are served from a small block cache on the wasm side, so this is called on the
        /// order of once per 64 KiB of a cell rather than once per engine read — which is what makes
        /// a per-call JS crossing affordable at all (see [`crate::driver`]'s module header).
        ///
        /// Throws an `Error` carrying `code` + `message` on failure; see [`crate::ErrorCode`].
        pub fn run(
            &mut self,
            on_progress: Option<js_sys::Function>,
            on_file: Option<js_sys::Function>,
            on_read: Option<js_sys::Function>,
        ) -> Result<String, JsValue> {
            let mut hooks = JsHooks { on_progress, on_file, last_us: 0, warned: false };
            let reads = on_read.map(|on_read| JsReads { on_read });
            let inputs = Inputs {
                cells: core::mem::take(&mut self.cells),
                source_cells: core::mem::take(&mut self.source_cells),
                reads: reads.as_ref().map(|r| r as &dyn CellReads),
                known_empty: core::mem::take(&mut self.known_empty),
                terrain: self.terrain,
                terrain_cells: core::mem::take(&mut self.terrain_cells),
            };
            let out = assemble(inputs, &self.schema_json, &self.skin_json, &self.options, &mut hooks).map_err(to_js)?;
            self.taken = vec![false; out.files.len()];
            self.files = out.files;
            self.warnings = out.warnings;
            Ok(out.summary_json)
        }

        /// How many files of the finished set are still here: every shard `on_file` did not take,
        /// then the OBCS manifest **last**. With no `on_file`, that is the whole set.
        #[wasm_bindgen(getter, js_name = fileCount)]
        pub fn file_count(&self) -> usize {
            self.files.len()
        }

        /// File `index`'s derived 8.3 filename (`MS<id>S<kk>.OBM`, `MS<id>.OBS`).
        #[wasm_bindgen(js_name = fileName)]
        pub fn file_name(&self, index: usize) -> Result<String, JsValue> {
            self.file(index).map(|f| f.name.clone())
        }

        /// `"core"`, `"coarse"`, `"geometry"`, `"terrain"`, or `"manifest"`.
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
        /// before it pays for one. It reads `0` once the file has been taken, because the bytes are
        /// genuinely gone; the JS wrapper snapshots it at that moment instead, and a second
        /// [`Assembler::take_file`] throws rather than let the two disagree.
        #[wasm_bindgen(js_name = fileByteLength)]
        pub fn file_byte_length(&self, index: usize) -> Result<usize, JsValue> {
            self.file(index).map(|f| f.bytes.len())
        }

        /// Move file `index`'s bytes out to JS, **freeing the wasm-side copy**.
        ///
        /// One file at a time is the whole point: an assembled set can be gigabytes, and taking them
        /// one by one means the transient double-residency is one file rather than the set.
        ///
        /// A second call for the same index **throws** `internal`. It used to return an empty array,
        /// which is the worse answer: the natural retry shape — take, upload, catch, take again —
        /// would then write a 0-byte `.OBM` to a card and report success, and the file's own
        /// `byteLength` (read before the take, as a caller planning a transfer does) would still
        /// claim the original size. A shard that silently becomes empty is a corrupt map; a thrown
        /// error is a bug the caller can see.
        #[wasm_bindgen(js_name = takeFile)]
        pub fn take_file(&mut self, index: usize) -> Result<Vec<u8>, JsValue> {
            if self.taken.get(index).copied().unwrap_or(false) {
                let name = self.files.get(index).map(|f| f.name.as_str()).unwrap_or("?");
                return Err(to_js(AssembleFailure {
                    code: ErrorCode::Internal,
                    message: format!(
                        "file {index} ({name}) was already taken — its bytes now belong to JS, and this call would \
                         have returned an empty file. Keep the array `take()` returned rather than calling it twice."
                    ),
                }));
            }
            let f = self.files.get_mut(index).ok_or_else(|| {
                to_js(AssembleFailure {
                    code: ErrorCode::Internal,
                    message: format!("file index {index} does not exist"),
                })
            })?;
            let bytes = core::mem::take(&mut f.bytes);
            self.taken[index] = true;
            Ok(bytes)
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
            self.source_cells = Vec::new();
            self.known_empty = Vec::new();
            self.terrain_cells = Vec::new();
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
    ///
    /// `budget_bytes` overrides the number `fits` is judged against. The default is a **desktop**
    /// judgement ([`crate::PRACTICAL_BUDGET`], 3 GiB); a caller that knows it is on a phone should
    /// pass what that device will actually grant. Anything non-finite or non-positive falls back to
    /// the default rather than refusing everything.
    #[wasm_bindgen]
    pub fn obc_assemble_estimate(
        network_band_bytes: f64,
        total_cell_bytes: f64,
        budget_bytes: Option<f64>,
    ) -> js_sys::Object {
        let e = crate::estimate::estimate_memory_with_budget(
            network_band_bytes,
            total_cell_bytes,
            budget_bytes.unwrap_or(crate::estimate::PRACTICAL_BUDGET),
        );
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
