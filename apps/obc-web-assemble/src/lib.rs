//! The hosted builder's assembly bridge: the OBCA engine compiled to wasm and driven from a
//! browser tab. The cells the builder downloaded go in, and one `.obcm` comes out, byte-for-byte
//! identical to what the native CLI produces from the same inputs.
//!
//! Nothing here names a file. What crosses is a digest, a length and sometimes the bytes; only the
//! caller knows whether that becomes `MAP.OBCM` on a card or a save dialog's suggestion.
//!
//! `.run()` blocks for the whole assembly, about 20 s at country scale, so it belongs in a Web
//! Worker. `builder/app/src/lib/assemble/bridge.ts` holds that contract and what a cancel button
//! must do.
//!
//! A failure crosses to JS as a thrown `Error` whose `message` is the engine's own and whose `code`
//! ([`ErrorCode`]) is the stable identifier a caller branches on.
//!
//! The driver ([`driver`]) and the memory model ([`estimate`]) are target-independent and tested
//! natively; only the bindgen shim below is wasm-specific.

pub mod driver;
pub mod estimate;

pub use driver::{
    assemble, assemble_cells, assemble_cells_with_known_empty, assemble_everything, AssembleFailure, BridgeOptions,
    CellBytes, CellReads, ErrorCode, Hooks, KnownEmptyCell, MapWrites, NoHooks, Outcome, Phase, ScratchWrites,
    SealedMap, SourceCell, TerrainCellBytes, TerrainLattice, Wiring,
};
pub use estimate::{
    estimate_memory, estimate_memory_with_budget, MemoryEstimate, Residency, ENGINE_FLOOR, INPUT_READ_CACHE_BYTES,
    OUTPUT_PER_CELL_BYTE, PRACTICAL_BUDGET, SPILL_PER_NAV_BYTE, VERIFY_READ_CACHE_BYTES, WASM32_ADDRESS_SPACE,
    WASM_ALLOC_MARGIN,
};

#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::prelude::*;

    use crate::driver::{
        assemble, AssembleFailure, BridgeOptions, CellBytes, CellReads, ErrorCode, Hooks, KnownEmptyCell, MapWrites,
        Outcome, Phase, ScratchWrites, SealedMap, SourceCell, TerrainCellBytes, TerrainLattice, Wiring,
    };

    /// Module start: surface Rust panics in the console instead of an opaque `unreachable` trap.
    #[wasm_bindgen(start)]
    pub fn start() {
        console_error_panic_hook::set_once();
    }

    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_namespace = console, js_name = warn)]
        fn console_warn(msg: &str);
    }

    /// The browser's [`Hooks`]: `Date.now()` for the phase split, and the caller's callbacks for
    /// progress, abort, and the sunk map's identity.
    struct JsHooks {
        on_progress: Option<js_sys::Function>,
        /// `sealed(sha256, byteLength)`: the sink's report, which carries an identity because the
        /// host wrote the bytes itself and never saw them.
        on_sealed: Option<js_sys::Function>,
        /// `Date.now()` is wall clock and can step backwards, for example after an NTP correction.
        /// The engine subtracts consecutive readings, so a step back would underflow a `u64`.
        /// Clamping keeps the seam monotonic at the cost of a phase timing that reads as zero.
        last_us: u64,
        /// Whether the throwing-callback warning was printed. A broken progress callback throws a
        /// hundred times a run, and one line is enough.
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
            // A callback that throws does not abort the run: losing a long assembly to a typo in a
            // progress bar is the worse failure. A caller aborts by returning true.
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

        /// Tell JS what the map the host's own sink wrote turned out to be.
        ///
        /// A throw fails the run as `io`: the file exists, and a map reported as finished whose
        /// identity nobody recorded is worse than a run that says it failed.
        fn map_sealed(&mut self, map: SealedMap) -> Result<(), String> {
            let Some(f) = &self.on_sealed else { return Ok(()) };
            let SealedMap { sha256, byte_length } = map;
            match f.call2(&JsValue::NULL, &JsValue::from_str(&sha256), &JsValue::from_f64(byte_length as f64)) {
                Ok(_) => Ok(()),
                Err(e) => Err(format!(
                    "the map sink threw while reporting the finished file ({e:?}). The bytes are written but their \
                     identity was not recorded; discard the file and re-run."
                )),
            }
        }
    }

    /// The browser's [`MapWrites`]: one OPFS `FileSystemSyncAccessHandle` behind four JS calls.
    ///
    /// `write` hands the host a view that is the engine's buffer, and `readAt` hands it one that is
    /// the destination, so bytes move between linear memory and the file with no copy on the JS
    /// side.
    struct JsSink {
        create: js_sys::Function,
        write: js_sys::Function,
        read_at: js_sys::Function,
        seal: js_sys::Function,
    }

    impl JsSink {
        /// Read the four methods off the object a caller passed. A missing or non-callable one is
        /// refused before a byte is written.
        fn from_object(obj: &js_sys::Object) -> Result<JsSink, AssembleFailure> {
            let method = |name: &str| -> Result<js_sys::Function, AssembleFailure> {
                let v = js_sys::Reflect::get(obj, &JsValue::from_str(name)).map_err(|_| AssembleFailure {
                    code: ErrorCode::Internal,
                    message: format!("the map sink has no {name:?}"),
                })?;
                v.dyn_into::<js_sys::Function>().map_err(|_| AssembleFailure {
                    code: ErrorCode::Internal,
                    message: format!(
                        "the map sink's {name:?} is not a function — a sink must provide create, write, readAt, seal \
                         and sealed."
                    ),
                })
            };
            Ok(JsSink {
                create: method("create")?,
                write: method("write")?,
                read_at: method("readAt")?,
                seal: method("seal")?,
            })
        }

        /// Every call answers the same way: truthy is success, anything else is the host refusing.
        fn taken(call: Result<JsValue, JsValue>, what: &str) -> Result<(), String> {
            match call {
                Ok(v) if v.is_truthy() => Ok(()),
                Ok(_) => Err(format!("the sink's {what} returned a falsy value")),
                Err(e) => Err(format!("the sink's {what} threw ({e:?})")),
            }
        }
    }

    impl MapWrites for JsSink {
        fn create(&self) -> Result<(), String> {
            JsSink::taken(self.create.call0(&JsValue::NULL), "create")
        }

        fn write(&self, bytes: &[u8]) -> Result<(), String> {
            // SAFETY: the same contract as `JsReads::read`. The view aliases linear memory and is
            // made, passed and dropped inside one synchronous JS call that only reads from it. No
            // Rust allocation can run in between, and the callback must not re-enter the assembler
            // or keep the view.
            let src = unsafe { js_sys::Uint8Array::view(bytes) };
            JsSink::taken(self.write.call1(&JsValue::NULL, &src), "write")
        }

        fn read_at(&self, offset: u64, into: &mut [u8]) -> Result<(), String> {
            // SAFETY: as in `JsReads::read`, a per-call view, filled and dropped inside the call.
            // A sunk map's offset can pass 4 GiB, and `f64` carries it exactly to 2^53.
            let dest = unsafe { js_sys::Uint8Array::view_mut_raw(into.as_mut_ptr(), into.len()) };
            JsSink::taken(self.read_at.call2(&JsValue::NULL, &JsValue::from_f64(offset as f64), &dest), "readAt")
        }

        fn seal(&self) -> Result<(), String> {
            JsSink::taken(self.seal.call0(&JsValue::NULL), "seal")
        }
    }

    /// The browser's [`ScratchWrites`]: five JS methods over a pool of OPFS sync access handles,
    /// crossed the way [`JsSink`] crosses. `create` and `len` answer with a number, where `-1` is
    /// the refusal; the other three answer truthy or failed.
    struct JsScratch {
        create: js_sys::Function,
        append: js_sys::Function,
        read_at: js_sys::Function,
        len: js_sys::Function,
        remove: js_sys::Function,
    }

    impl JsScratch {
        fn from_object(obj: &js_sys::Object) -> Result<JsScratch, AssembleFailure> {
            let method = |name: &str| -> Result<js_sys::Function, AssembleFailure> {
                let v = js_sys::Reflect::get(obj, &JsValue::from_str(name)).map_err(|_| AssembleFailure {
                    code: ErrorCode::Internal,
                    message: format!("the scratch store has no {name:?}"),
                })?;
                v.dyn_into::<js_sys::Function>().map_err(|_| AssembleFailure {
                    code: ErrorCode::Internal,
                    message: format!(
                        "the scratch store's {name:?} is not a function — a scratch store must provide create, \
                         append, readAt, len and remove."
                    ),
                })
            };
            Ok(JsScratch {
                create: method("create")?,
                append: method("append")?,
                read_at: method("readAt")?,
                len: method("len")?,
                remove: method("remove")?,
            })
        }

        /// A truthy-or-failed call, like the sink's.
        fn taken(call: Result<JsValue, JsValue>, what: &str) -> Result<(), String> {
            match call {
                Ok(v) if v.is_truthy() => Ok(()),
                Ok(_) => Err(format!("the scratch store's {what} returned a falsy value")),
                Err(e) => Err(format!("the scratch store's {what} threw ({e:?})")),
            }
        }

        /// A call that answers a non-negative number, where `-1` (or anything else) is the refusal.
        fn counted(call: Result<JsValue, JsValue>, what: &str) -> Result<f64, String> {
            match call {
                Ok(v) => match v.as_f64() {
                    Some(n) if n >= 0.0 => Ok(n),
                    _ => Err(format!("the scratch store's {what} refused (out of pool slots, or the id is gone)")),
                },
                Err(e) => Err(format!("the scratch store's {what} threw ({e:?})")),
            }
        }
    }

    impl ScratchWrites for JsScratch {
        fn create(&self) -> Result<u32, String> {
            JsScratch::counted(self.create.call0(&JsValue::NULL), "create").map(|n| n as u32)
        }

        fn append(&self, id: u32, bytes: &[u8]) -> Result<(), String> {
            // SAFETY: the same contract as `JsSink::write`: a per-call view over linear memory,
            // made, read and dropped inside one synchronous JS call.
            let src = unsafe { js_sys::Uint8Array::view(bytes) };
            JsScratch::taken(self.append.call2(&JsValue::NULL, &JsValue::from_f64(id as f64), &src), "append")
        }

        fn read_at(&self, id: u32, offset: u64, into: &mut [u8]) -> Result<(), String> {
            // SAFETY: as in `JsSink::read_at`, a per-call view, filled and dropped inside the call.
            let dest = unsafe { js_sys::Uint8Array::view_mut_raw(into.as_mut_ptr(), into.len()) };
            // A spill offset can pass 4 GiB, and `f64` carries it exactly to 2^53.
            JsScratch::taken(
                self.read_at.call3(
                    &JsValue::NULL,
                    &JsValue::from_f64(id as f64),
                    &JsValue::from_f64(offset as f64),
                    &dest,
                ),
                "readAt",
            )
        }

        fn len(&self, id: u32) -> Result<u64, String> {
            JsScratch::counted(self.len.call1(&JsValue::NULL, &JsValue::from_f64(id as f64)), "len").map(|n| n as u64)
        }

        fn remove(&self, id: u32) -> Result<(), String> {
            JsScratch::taken(self.remove.call1(&JsValue::NULL, &JsValue::from_f64(id as f64)), "remove")
        }
    }

    /// The browser's [`CellReads`]: one JS call per cache miss, filling a view over wasm's own
    /// linear memory.
    ///
    /// `FileSystemSyncAccessHandle.read(buffer, {at})` takes any `ArrayBufferView`, so a view that
    /// is the destination block moves the bytes from the file into the wasm heap in one step.
    struct JsReads {
        /// `read(slot, offset, dest) -> boolean`. Falsy means the read failed.
        on_read: js_sys::Function,
    }

    impl CellReads for JsReads {
        fn read(&self, slot: usize, offset: u64, buf: &mut [u8]) -> Result<(), String> {
            // SAFETY: `view_mut_raw` aliases linear memory and anything that grows the heap
            // invalidates it. This one is made, passed and dropped inside a single synchronous JS
            // call that only fills it, so no Rust allocation can run in between. It is built per
            // call: a stored view would be detached by the next heap growth, and the reads it
            // served would silently return nothing.
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

    /// One assembly: cells in, one map out.
    ///
    /// The lifecycle is fixed: construct, `addCell` for every downloaded cell, `run`, then take the
    /// file. Cells may be handed over as they finish downloading; nothing is parsed until `run`.
    #[wasm_bindgen]
    pub struct Assembler {
        schema_json: String,
        skin_json: String,
        options: BridgeOptions,
        cells: Vec<CellBytes>,
        /// Cells the host keeps outside wasm memory and serves on demand. A cell's slot, which
        /// `run`'s read callback is given, is its index here.
        source_cells: Vec<SourceCell>,
        known_empty: Vec<KnownEmptyCell>,
        /// The catalog's terrain lattice, once the caller declares one. `None` leaves the map's
        /// terrain region empty, which is a complete map with flat profiles.
        terrain: Option<TerrainLattice>,
        terrain_cells: Vec<TerrainCellBytes>,
        outcome: Option<Outcome>,
        /// Whether the bytes were already moved out to JS. An emptied buffer looks the same as a
        /// legitimately empty one, and [`Assembler::take_file`] must tell them apart.
        taken: bool,
    }

    #[wasm_bindgen]
    impl Assembler {
        /// Start an assembly at a schema and a skin, as JSON text.
        ///
        /// `options_json` is an optional object: `{acceptHoles, acceptPartial, readBlockBytes,
        /// mergeBudgetBytes}`, every field optional. Unknown keys are ignored, so a newer builder
        /// can talk to an older module. There is no `skipVerify`: the read-back is a precondition
        /// of writing a map.
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
                outcome: None,
                taken: false,
            })
        }

        /// Hand over one downloaded cell: its catalog identity, its `partial` flag, and its
        /// verified bytes.
        ///
        /// `bytes` crosses the boundary once. wasm-bindgen copies the `Uint8Array` into linear
        /// memory and this takes ownership of that copy, so the JS side may drop its reference.
        #[wasm_bindgen(js_name = addCell)]
        pub fn add_cell(&mut self, id: String, band: String, partial: bool, bytes: Vec<u8>) {
            self.cells.push(CellBytes { id, band, partial, bytes });
        }

        /// Hand over one downloaded cell by reference: the identity and `partial` flag
        /// [`Assembler::add_cell`] takes, the length the catalog published, and an opaque key the
        /// host's read callback resolves. The bytes never enter wasm memory.
        ///
        /// Returns the cell's slot, which is the first argument `run`'s `on_read` is called with.
        ///
        /// The catalog's byte count is what the engine reads as the cell's length, so a read past it
        /// is refused here. A wrong one surfaces as a format error at open.
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

        /// Add one selected, canonical zero-byte cell. It affects the output bbox and the coverage
        /// checks but has no buffer to transfer or graft.
        #[wasm_bindgen(js_name = addKnownEmpty)]
        pub fn add_known_empty(&mut self, id: String, band: String) {
            self.known_empty.push(KnownEmptyCell { id, band });
        }

        /// Declare the catalog's terrain lattice. Calling it is what gives the map a terrain region
        /// at all; a catalog with no terrain block never calls it, and the map assembles with the
        /// pair at `(0, 0)`.
        ///
        /// Declaring the lattice with no cells is legal: it writes a region that is all directory,
        /// which says the ground is canonically void and not that the raster failed to arrive.
        #[wasm_bindgen(js_name = setTerrain)]
        pub fn set_terrain(&mut self, posting_log2: u8, cell_log2: u8) {
            self.terrain = Some(TerrainLattice { posting_log2, cell_log2 });
        }

        /// Hand over one downloaded terrain cell: its id on the terrain grid, the `sha256` the
        /// pinned terrain index published, and the whole `.obcd` object.
        ///
        /// A known-empty square is not handed over at all. It has no object, and an absent cell
        /// reads the same as an all-`NODATA` one.
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

        /// Assemble, and return the summary as JSON: the same document `obcm-assemble --json`
        /// prints. The bytes are then taken with [`Assembler::take_file`], unless a `sink` wrote
        /// them, in which case the host already has them.
        ///
        /// This blocks. A country-scale assembly is about 20 s of straight-line compute, so run it
        /// in a Web Worker and post progress out.
        ///
        /// `on_progress(phase, fraction)` is called at every phase boundary and about a hundred
        /// times over the write and the read-back. `fraction` is overall completion, weighted by the
        /// measured phase split. A truthy return asks for an abort, honoured at the next write or
        /// verify read. A callback that throws is warned about once and never cancels the run.
        ///
        /// `on_read(slot, offset, dest) -> boolean` fetches the bytes of every cell added with
        /// [`Assembler::add_cell_by_key`], and must be present if any were. It is called
        /// synchronously from inside the run, which is what makes a `FileSystemSyncAccessHandle`
        /// usable, and it must fill `dest` completely and return `true`. A falsy return or a throw
        /// fails the run as `io` naming the cell.
        ///
        /// `dest` is a view straight onto wasm's linear memory and is valid only for the duration of
        /// the call. Fill it and return. Do not keep it, do not hand it to anything asynchronous,
        /// and do not call back into the assembler from inside it.
        ///
        /// `sink` is the output's version of `on_read`: an object with `create()`, `write(bytes)`,
        /// `readAt(offset, into)`, `seal()` and `sealed(sha256, byteLength)`. In the browser those
        /// are one OPFS `FileSystemSyncAccessHandle` opened in the worker before the run. With one,
        /// the map is never in wasm memory, which is the only shape a country-scale map has.
        ///
        /// The four byte-moving methods return `true` for success. `bytes` and `into` obey the same
        /// view rule as `dest`. A `sealed` that throws also fails the run as `io`, because the file
        /// exists and the caller does not know which bytes are in it.
        ///
        /// `scratch` is where the engine's spill goes instead of into wasm memory: an object with
        /// `create()` (a non-negative id, or `-1` to refuse), `append(id, bytes)`,
        /// `readAt(id, offset, into)`, `len(id)` (the byte count, or `-1`) and `remove(id)`. In the
        /// browser those are a pool of OPFS sync access handles opened before the run. A browser
        /// that can wire this should, because the spill is the merge's edge stream.
        ///
        /// Throws an `Error` carrying `code` and `message` on failure; see [`crate::ErrorCode`].
        pub fn run(
            &mut self,
            on_progress: Option<js_sys::Function>,
            on_read: Option<js_sys::Function>,
            sink: Option<js_sys::Object>,
            scratch: Option<js_sys::Object>,
        ) -> Result<String, JsValue> {
            let (map_sink, on_sealed) = match &sink {
                Some(obj) => {
                    let sealed = js_sys::Reflect::get(obj, &JsValue::from_str("sealed"))
                        .ok()
                        .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
                        .ok_or_else(|| {
                            to_js(AssembleFailure {
                                code: ErrorCode::Internal,
                                message: "the map sink has no callable \"sealed\" — a sink that cannot report the \
                                          finished file would write bytes nobody can identify."
                                    .into(),
                            })
                        })?;
                    (Some(JsSink::from_object(obj).map_err(to_js)?), Some(sealed))
                }
                None => (None, None),
            };
            let mut hooks = JsHooks { on_progress, on_sealed, last_us: 0, warned: false };
            let reads = on_read.map(|on_read| JsReads { on_read });
            let js_scratch = match &scratch {
                Some(obj) => Some(JsScratch::from_object(obj).map_err(to_js)?),
                None => None,
            };
            let wiring = Wiring {
                cells: core::mem::take(&mut self.cells),
                source_cells: core::mem::take(&mut self.source_cells),
                reads: reads.as_ref().map(|r| r as &dyn CellReads),
                known_empty: core::mem::take(&mut self.known_empty),
                terrain: self.terrain,
                terrain_cells: core::mem::take(&mut self.terrain_cells),
                sink: map_sink.as_ref().map(|s| s as &dyn MapWrites),
                scratch: js_scratch.as_ref().map(|s| s as &dyn ScratchWrites),
            };
            let out = assemble(wiring, &self.schema_json, &self.skin_json, &self.options, &mut hooks).map_err(to_js)?;
            let summary = out.summary_json.clone();
            self.taken = false;
            self.outcome = Some(out);
            Ok(summary)
        }

        /// The finished map's lowercase-hex SHA-256. Empty before a successful `run`.
        #[wasm_bindgen(getter, js_name = fileSha256)]
        pub fn file_sha256(&self) -> String {
            self.outcome.as_ref().map(|o| o.sha256.clone()).unwrap_or_default()
        }

        /// The finished map's size, readable without moving the bytes, and still true after
        /// [`Assembler::take_file`] empties the buffer. `0` before a successful `run`.
        ///
        /// `f64` rather than `usize`: a sunk map can be larger than this address space.
        #[wasm_bindgen(getter, js_name = fileByteLength)]
        pub fn file_byte_length(&self) -> f64 {
            self.outcome.as_ref().map_or(0.0, |o| o.byte_length as f64)
        }

        /// Whether the bytes are here to take. `false` after a run with a `sink`, which wrote them
        /// to the host's own storage and never held them.
        #[wasm_bindgen(getter, js_name = hasFile)]
        pub fn has_file(&self) -> bool {
            self.outcome.as_ref().is_some_and(|o| o.bytes.is_some()) && !self.taken
        }

        /// Move the map's bytes out to JS, freeing the wasm-side copy.
        ///
        /// A second call throws `internal`, and so does a run that used a `sink`. An empty array
        /// would let the natural retry shape write a 0-byte map to a card and report success.
        #[wasm_bindgen(js_name = takeFile)]
        pub fn take_file(&mut self) -> Result<Vec<u8>, JsValue> {
            if self.taken {
                return Err(to_js(AssembleFailure {
                    code: ErrorCode::Internal,
                    message: "the map was already taken — its bytes now belong to JS, and this call would have \
                              returned an empty file. Keep the array `takeFile()` returned rather than calling it \
                              twice."
                        .into(),
                }));
            }
            let bytes = self.outcome.as_mut().and_then(|o| o.bytes.take()).ok_or_else(|| {
                to_js(AssembleFailure {
                    code: ErrorCode::Internal,
                    message: "there is no map to take — either `run` has not finished, or it was given a sink \
                                  and the bytes went straight to the host's own storage."
                        .into(),
                })
            })?;
            self.taken = true;
            Ok(bytes)
        }

        /// What a producer reports rather than refuses: the size warning, dropped duplicate POIs,
        /// degree-cap truncations. An assembly with warnings is still a legal map.
        pub fn warnings(&self) -> js_sys::Array {
            match &self.outcome {
                Some(o) => o.warnings.iter().map(|w| JsValue::from_str(w)).collect(),
                None => js_sys::Array::new(),
            }
        }

        /// Drop the input cell buffers. Automatic on `run`, and exposed for the caller that
        /// abandons an assembly it was still feeding.
        #[wasm_bindgen(js_name = releaseCells)]
        pub fn release_cells(&mut self) {
            self.cells = Vec::new();
            self.source_cells = Vec::new();
            self.known_empty = Vec::new();
            self.terrain_cells = Vec::new();
        }
    }

    /// Project the peak memory of assembling a selection, before downloading it. Pass the
    /// catalog's byte totals for the selected cells plus the run's residency mode, and get
    /// `{engineBytes, inputBytes, outputBytes, peakBytes, budgetBytes, ceilingBytes, fits,
    /// headroomBytes}`.
    ///
    /// The caller must state what this run will actually have: `input_on_disk` only when the cells
    /// will stream from OPFS, `output_sunk` only when a `sink` will be wired into
    /// [`Assembler::run`].
    ///
    /// This prices the run against wasm32's 4 GiB address space, where the file-size ledger prices
    /// the output against the per-file wall. A selection can pass one and fail the other.
    ///
    /// `budget_bytes` overrides the number `fits` is judged against. The default is a desktop
    /// judgement ([`crate::PRACTICAL_BUDGET`]); a caller on a phone should pass what that device
    /// will grant. A non-finite or non-positive value falls back to the default.
    #[wasm_bindgen]
    pub fn obc_assemble_estimate(
        network_band_bytes: f64,
        total_cell_bytes: f64,
        terrain_bytes: f64,
        merge_budget_bytes: f64,
        input_on_disk: bool,
        output_sunk: bool,
        budget_bytes: Option<f64>,
    ) -> js_sys::Object {
        let e = crate::estimate::estimate_memory_with_budget(
            network_band_bytes,
            total_cell_bytes,
            terrain_bytes,
            merge_budget_bytes,
            crate::estimate::Residency { input_on_disk, output_sunk },
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

    /// `Reflect::set` on a fresh plain object, which cannot fail, so the result is ignored.
    fn set(obj: &js_sys::Object, key: &str, value: &JsValue) {
        let _ = js_sys::Reflect::set(obj, &JsValue::from_str(key), value);
    }

    /// Build the JS exception as a real `Error` instance, so it carries a stack and survives
    /// `instanceof Error`, with the stable code hung off it as a plain property.
    ///
    /// A `#[wasm_bindgen]` struct would cross the boundary too, but it would not be an `Error`, so
    /// `catch (e) { e.message }` and every logger that formats errors would come up empty.
    fn to_js(f: AssembleFailure) -> JsValue {
        let err = js_sys::Error::new(&f.message);
        err.set_name("ObcAssembleError");
        // `Reflect::set` only fails on a frozen target, and `err` is fresh. Ignored rather than
        // unwrapped, so a surprise here still throws a usable Error instead of trapping the module.
        let _ = js_sys::Reflect::set(&err, &JsValue::from_str("code"), &JsValue::from_str(f.code.as_str()));
        err.into()
    }
}
