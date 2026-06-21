//! microSD storage for the board: map / routes / track log over FatFs (issue #36).
//!
//! This is the board half of "data on SD": it owns the concrete SPI bus → [`SdCard`] →
//! [`VolumeManager`] stack and reconciles the FAT filesystem to the shared app's *intent*,
//! exactly as the simulator's `RouteStore` / `TrackStore` reconcile a folder of files on the
//! host. The reusable, board-agnostic adapters it hands the format code live in
//! [`obc_platform::sd`] ([`SdByteSource`] / [`SdByteSink`] / [`SdTrackSink`]); everything here
//! is STM32F429-specific (SPI4 + the GPIO chip-select) and will be re-pointed, not rewritten,
//! for the nRF board.
//!
//! Mirroring the host stores keeps one behaviour across both: routes stream from the card
//! through a [`RouteReader`], the chosen map is read **resident** into SDRAM, and the ride is
//! logged to a temp `.obct` that's converted to a `<route>.gpx` on Finish.
//!
//! ## Card layout (FAT16/FAT32)
//!   `/<name>.obcm`   — the map tile (first one found in the root is loaded)
//!   `/routes/*.obcr` — the route catalog the Route menu lists
//!   `/tracks/`       — saved `<route>.gpx` rides (created if absent); the in-progress log
//!                      lives here as `TRACK.OBT` and is deleted once converted.
//!
//! ## SPI wiring (DISC1, SPI4)
//!   SCK PE2 · MOSI PE6 · MISO PE5 (internal pull-up) · CS **PD7** (software) · GND · 3V3/5V.
//! The card is initialised at 400 kHz (SD spec) then the bus is re-clocked to
//! [`SD_FAST_HZ`] for bulk transfer — see [`init`].

use embassy_stm32::gpio::{Output, Pull};
use embassy_stm32::spi::{Config as SpiConfig, Spi};
use embassy_stm32::time::Hertz;
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::{
    LfnBuffer, Mode, RawDirectory, RawFile, SdCard, ShortFileName, TimeSource, Timestamp,
    VolumeIdx, VolumeManager,
};
use heapless::{String, Vec};
use obc_app::MAX_ROUTES;
use obc_platform::{SdByteSink, SdByteSource, SdTrackSink};
use obc_route::{track_to_gpx, RouteIndex, RouteSummary, NAME_CAP};

/// SD clock for bulk transfer once the card is initialised. Conservative for breadboard
/// jumpers (SPI4 is on the 90 MHz PCLK2; embassy picks the /8 prescaler → ~11.25 MHz, well
/// within the 25 MHz default-speed limit and forgiving of wiring). Init itself stays ≤400 kHz.
const SD_FAST_HZ: u32 = 12_000_000;

/// The in-progress ride log on the card — a header-less array of fixed track records (8.3
/// name). Truncated-and-reused per ride, converted to `<route>.gpx`, then deleted on Finish.
const TRACK_TMP: &str = "TRACK.OBT";

/// The concrete SD stack for this board: embassy's blocking SPI4 wrapped as the `SpiDevice` the
/// card driver wants, an [`SdCard`], and a 4-file/4-dir [`VolumeManager`]. The chip-select is a
/// no-op [`NoCs`] — the *real* CS (PD7) is held low for the whole session (see [`NoCs`]/[`init`]).
type SdSpi = Spi<'static, embassy_stm32::mode::Blocking, embassy_stm32::spi::mode::Master>;
type SdDev = ExclusiveDevice<SdSpi, NoCs, Delay>;
type Sd = SdCard<SdDev, Delay>;
type Vmgr = VolumeManager<Sd, NullTime>;

/// FAT timestamps need a clock; the device has none yet (see [`obc_route::TrackPoint::t_ms`]),
/// so every file gets the epoch. Real dates wait on a clock source, like the GPX `<time>`.
/// `pub(crate)` only because it surfaces in the adapter return types the loop names.
pub(crate) struct NullTime;
impl TimeSource for NullTime {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp {
            year_since_1970: 0,
            zero_indexed_month: 0,
            zero_indexed_day: 0,
            hours: 0,
            minutes: 0,
            seconds: 0,
        }
    }
}

/// The mounted card: the volume manager plus the open root/`routes`/`tracks` directory handles
/// and the per-frame reconcile state (which route's geometry is open, which session's log).
pub struct Storage {
    vmgr: Vmgr,
    root: RawDirectory,
    /// `/routes`, or `None` if the card has no such folder (catalog is then empty).
    routes_dir: Option<RawDirectory>,
    /// `/tracks` (created on mount if absent), or `None` if it couldn't be opened/created
    /// (rides then can't be saved, but the rest still works).
    tracks_dir: Option<RawDirectory>,
    /// 8.3 filename of each catalog entry, parallel to the app's route order — so a selected
    /// route index reopens the right `.obcr` (the menu shows the *internal* route name, not
    /// the filename, so 8.3 truncation is invisible).
    route_files: Vec<ShortFileName, MAX_ROUTES>,
    /// The active route's open geometry file: `(catalog index, handle, length)`. Reopened only
    /// when the selected route changes.
    open_route: Option<(usize, RawFile, u32)>,
    /// The map `.obcm`, opened once at startup and held open for the whole session: `(handle,
    /// length)`. The map streams through this (issue #37) instead of being read resident into
    /// SDRAM — `map_source` hands out a fresh [`SdByteSource`] over it each redraw.
    open_map: Option<(RawFile, u32)>,
    /// The open ride log for the current tracking session.
    open_track: Option<OpenTrack>,
    /// The real chip-select (PD7), held LOW for the whole session so the card stays selected.
    /// embedded-sdmmc drives a no-op [`NoCs`] instead; toggling a real CS breaks CMD0 on embassy.
    /// Kept here only to keep the pin driven low — never touched after [`init`].
    _cs: Output<'static>,
}

/// One open `.obct` ride log: the session it belongs to, its file handle, and the save name
/// (the route name, frozen at begin, so a later "swap route" can't rename a finished file).
struct OpenTrack {
    session: u32,
    file: RawFile,
    name: String<NAME_CAP>,
}

/// Bring up the SD card: wrap the SPI bus + CS as a `SpiDevice`, initialise the card at the
/// bus's current (slow) clock, re-clock to [`SD_FAST_HZ`], then mount the FAT volume. Returns
/// `None` on any failure (no card, not FAT, unreadable) so the caller degrades gracefully to
/// the baked-in tile — never panicking (acceptance criterion).
pub fn init(mut spi: SdSpi, mut cs: Output<'static>) -> Option<Storage> {
    // ≥74 wake-up clocks with CS high (SD spec), then hold CS LOW for the whole session.
    // `ExclusiveDevice` drives a no-op [`NoCs`], so the real CS never toggles high between a
    // command and its reply — which embassy's SPI can't survive (the card drops the bus and
    // CMD0's `0x01` is lost). Validated on glass: toggling = CardNotFound, held low = mounts.
    cs.set_high();
    let _ = spi.blocking_write(&[0xFFu8; 10]);
    cs.set_low();
    let dev = ExclusiveDevice::new(spi, NoCs, Delay).ok()?;
    let card = SdCard::new(dev, Delay);
    // `num_bytes` forces the SPI init sequence (must be ≤400 kHz, the bus's current setting).
    match card.num_bytes() {
        Ok(bytes) => defmt::info!("SD: card initialised, {=u64} MB", bytes >> 20),
        Err(e) => {
            defmt::warn!("SD: no card / init failed: {}", defmt::Debug2Format(&e));
            return None;
        }
    }
    // Card is up — re-clock the bus for bulk reads/writes (init speed would crawl the 1.4 MB map).
    card.spi(|dev| {
        let mut fast = SpiConfig::default();
        fast.frequency = Hertz(SD_FAST_HZ);
        fast.miso_pull = Pull::Up;
        let _ = dev.bus_mut().set_config(&fast);
    });
    Storage::mount(card, cs)
}

/// A no-op chip-select for [`ExclusiveDevice`]. embedded-sdmmc issues each byte as its own
/// `SpiDevice` op, so a real CS would toggle high between a command and its reply — which
/// embassy's SPI doesn't survive (the card drops the bus in the gap; CMD0's `0x01` is lost).
/// Holding the *real* CS low for the whole session and feeding `ExclusiveDevice` this no-op
/// keeps the card selected across commands. Validated: raw CMD0 with CS held low returns `0x01`.
/// `pub(crate)` only because it surfaces in the adapter return types (like [`NullTime`]).
pub(crate) struct NoCs;
impl embedded_hal::digital::ErrorType for NoCs {
    type Error = core::convert::Infallible;
}
impl embedded_hal::digital::OutputPin for NoCs {
    fn set_low(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn set_high(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl Storage {
    /// Mount the first FAT volume and open the root / `routes` / `tracks` directories.
    fn mount(card: Sd, cs: Output<'static>) -> Option<Storage> {
        let vmgr = VolumeManager::new(card, NullTime);
        let volume = match vmgr.open_raw_volume(VolumeIdx(0)) {
            Ok(v) => v,
            Err(e) => {
                defmt::warn!("SD: no FAT volume: {}", defmt::Debug2Format(&e));
                return None;
            }
        };
        let root = vmgr.open_root_dir(volume).ok()?;
        let routes_dir = vmgr.open_dir(root, "routes").ok();
        // `/tracks` must exist to save rides — create it if the card doesn't have one yet.
        let tracks_dir = match vmgr.open_dir(root, "tracks") {
            Ok(d) => Some(d),
            Err(_) => {
                let _ = vmgr.make_dir_in_dir(root, "tracks");
                vmgr.open_dir(root, "tracks").ok()
            }
        };
        defmt::info!(
            "SD: mounted; /routes {=bool}, /tracks {=bool}",
            routes_dir.is_some(),
            tracks_dir.is_some()
        );
        Some(Storage {
            vmgr,
            root,
            routes_dir,
            tracks_dir,
            route_files: Vec::new(),
            open_route: None,
            open_map: None,
            open_track: None,
            _cs: cs,
        })
    }

    /// Scan `/routes` for `*.obcr`, read each header into a [`RouteSummary`], and return the
    /// catalog for [`App::set_routes`](obc_app::App::set_routes). Also records the 8.3 filenames
    /// (parallel to the catalog) so a later selection reopens the right file. Filenames are
    /// collected first, then opened — opening a file inside the iteration callback would
    /// re-enter the volume manager's lock.
    ///
    /// Matching is on the **long** name: the 8.3 short name truncates `.obcr`/`.obcm` to the
    /// 3-char `OBC`, so the short extension can't tell routes from maps. The long name also lets
    /// us skip macOS `._*`/`.DS_Store` clutter (any dot-prefixed name).
    pub fn scan_routes(&mut self) -> Vec<RouteSummary, MAX_ROUTES> {
        let mut catalog: Vec<RouteSummary, MAX_ROUTES> = Vec::new();
        self.route_files.clear();
        let Some(dir) = self.routes_dir else { return catalog };

        let mut names: Vec<ShortFileName, MAX_ROUTES> = Vec::new();
        let mut lfn_storage = [0u8; 256];
        let mut lfn = LfnBuffer::new(&mut lfn_storage);
        let _ = self.vmgr.iterate_dir_lfn(dir, &mut lfn, |e, long| {
            if !e.attributes.is_directory() && long_has_ext(long, b".obcr") && !names.is_full() {
                let _ = names.push(e.name.clone());
            }
        });

        for n in &names {
            let Ok(file) = self.vmgr.open_file_in_dir(dir, n, Mode::ReadOnly) else { continue };
            let len = self.vmgr.file_length(file).unwrap_or(0);
            let src = SdByteSource::new(&self.vmgr, file, len);
            if let Ok(sum) = RouteSummary::read(&src) {
                if catalog.push(sum).is_ok() {
                    let _ = self.route_files.push(n.clone());
                }
            }
            let _ = self.vmgr.close_file(file);
        }
        defmt::info!("SD: {=usize} route(s) in /routes", catalog.len());
        catalog
    }

    /// Open the first `*.obcm` in the card root and hold it open for the session, so the map can
    /// **stream** from it (issue #37) rather than be read resident into SDRAM. Returns the file
    /// length on success, or `None` if there's no map file / it can't be opened (the caller then
    /// falls back to the baked-in tile). Call once at startup; [`map_source`](Self::map_source)
    /// then hands out a reader over the open handle.
    pub fn open_map(&mut self) -> Option<u32> {
        if let Some((_, len)) = self.open_map {
            return Some(len);
        }
        let mut found: Option<ShortFileName> = None;
        let mut lfn_storage = [0u8; 256];
        let mut lfn = LfnBuffer::new(&mut lfn_storage);
        let _ = self.vmgr.iterate_dir_lfn(self.root, &mut lfn, |e, long| {
            if found.is_none() && !e.attributes.is_directory() && long_has_ext(long, b".obcm") {
                found = Some(e.name.clone());
            }
        });
        let name = found?;
        let file = self.vmgr.open_file_in_dir(self.root, &name, Mode::ReadOnly).ok()?;
        let len = self.vmgr.file_length(file).unwrap_or(0);
        if len == 0 {
            let _ = self.vmgr.close_file(file);
            return None;
        }
        self.open_map = Some((file, len));
        Some(len)
    }

    /// A [`ByteSource`](obc_route::ByteSource) over the open map file, for building a per-frame
    /// [`Reader`](obc_reader::Reader) that streams the index + geometry chunks. `None` if no map
    /// was opened ([`open_map`](Self::open_map) returned `None` → baked-tile fallback). Cheap —
    /// the source just wraps the already-open handle, so it's rebuilt every redraw (like the
    /// route source), keeping no borrow across the `&mut self` route/track operations.
    pub fn map_source(&self) -> Option<SdByteSource<'_, Sd, NullTime>> {
        self.open_map.map(|(f, len)| SdByteSource::new(&self.vmgr, f, len))
    }

    /// Make the open route geometry match the app's selected route (a catalog index), reopening
    /// the `.obcr` only when the selection changes — cheap to call every frame, like the sim's
    /// `RouteStore::sync_active`.
    pub fn reconcile_route(&mut self, want: Option<usize>) {
        if self.open_route.map(|(i, _, _)| i) == want {
            return;
        }
        if let Some((_, f, _)) = self.open_route.take() {
            let _ = self.vmgr.close_file(f);
        }
        if let (Some(i), Some(dir)) = (want, self.routes_dir) {
            if let Some(n) = self.route_files.get(i) {
                if let Ok(file) = self.vmgr.open_file_in_dir(dir, n, Mode::ReadOnly) {
                    let len = self.vmgr.file_length(file).unwrap_or(0);
                    self.open_route = Some((i, file, len));
                }
            }
        }
    }

    /// A [`ByteSource`](obc_route::ByteSource) over the active route's open file, for opening a
    /// [`RouteReader`](obc_route::RouteReader) to stream geometry from. `None` when no route is
    /// loaded.
    pub fn route_source(&self) -> Option<SdByteSource<'_, Sd, NullTime>> {
        self.open_route.map(|(_, f, len)| SdByteSource::new(&self.vmgr, f, len))
    }

    /// Parse the active route's [`RouteIndex`] — the header plus the **full chunk-meta walk**,
    /// the one up-front per-route cost. The render loop builds this once when the active route
    /// changes and reuses it across frames (issue #44): a redraw then streams only the visible
    /// geometry chunks, instead of re-walking the whole index off the card at panel rate (what
    /// calling [`RouteIndex::read`] every redraw would do). `None` when no route is open or the
    /// index read fails (a flaky link) — the loop retries the build on a later redraw, so a
    /// transient glitch doesn't hide the route.
    pub fn build_route_index(&self) -> Option<RouteIndex> {
        let src = self.route_source()?;
        RouteIndex::read(&src).ok()
    }

    /// Reconcile the open ride log to the app's tracking intent — call once per frame *before*
    /// ticking, mirroring the sim's `TrackStore::reconcile`. Drains the one-shot disposition
    /// first (finalising / abandoning the current log), then opens a fresh log when the session
    /// id changes. `name` is the active route's name (the save filename).
    pub fn reconcile_track(
        &mut self,
        action: Option<obc_app::TrackAction>,
        session: Option<u32>,
        name: &str,
    ) {
        use obc_app::TrackAction;
        match action {
            Some(TrackAction::Save) => self.finalize_track(),
            Some(TrackAction::Discard) => self.abandon_track(),
            None => {}
        }
        match session {
            Some(id) if self.open_track.as_ref().map(|o| o.session) != Some(id) => {
                self.begin_track(id, name)
            }
            None => self.abandon_track(), // no session → ensure nothing is left open
            _ => {}                       // same session → keep appending
        }
    }

    /// The [`TrackSink`](obc_app::TrackSink) for the open log, or `None` when not recording.
    pub fn track_sink(&self) -> Option<SdTrackSink<'_, Sd, NullTime>> {
        self.open_track.as_ref().map(|o| SdTrackSink::new(&self.vmgr, o.file))
    }

    /// Open (truncating) a fresh `TRACK.OBT` for session `id`, to be saved as `name`.
    fn begin_track(&mut self, id: u32, name: &str) {
        self.abandon_track(); // close any previous handle first
        let Some(dir) = self.tracks_dir else { return };
        match self.vmgr.open_file_in_dir(dir, TRACK_TMP, Mode::ReadWriteCreateOrTruncate) {
            Ok(file) => {
                let mut nm = String::new();
                let _ = nm.push_str(name);
                self.open_track = Some(OpenTrack { session: id, file, name: nm });
                defmt::info!("SD: recording ride → {=str}", name);
            }
            Err(e) => defmt::warn!("SD: cannot open ride log: {}", defmt::Debug2Format(&e)),
        }
    }

    /// Convert the open `TRACK.OBT` to a non-clobbering `<name>.gpx`, deleting the temp **only
    /// once the ride is safely in a GPX**. Any path that can't guarantee a clean save — no
    /// confirmed-free name ([`unique_gpx`](Self::unique_gpx) returns `None`), the GPX won't
    /// open, or the conversion errors — keeps `TRACK.OBT` so the ride isn't lost to a transient
    /// SD glitch (it converts on a later save; a fresh ride truncates it, as before).
    fn finalize_track(&mut self) {
        let Some(ot) = self.open_track.take() else { return };
        let _ = self.vmgr.flush_file(ot.file);
        let _ = self.vmgr.close_file(ot.file);
        let Some(dir) = self.tracks_dir else { return };

        // Pick a name we can *prove* is free before touching the filesystem. If we can't (the
        // collision bound is hit, or a glitch means no candidate can be confirmed absent), bail
        // without writing — keeping the temp beats truncating an existing ride's GPX.
        let Some(gpx) = self.unique_gpx(dir, &ot.name) else {
            defmt::warn!(
                "SD: no free GPX slot for {=str} — kept TRACK.OBT (no overwrite)",
                ot.name.as_str()
            );
            return;
        };

        let Ok(src_file) = self.vmgr.open_file_in_dir(dir, TRACK_TMP, Mode::ReadOnly) else {
            return;
        };
        let len = self.vmgr.file_length(src_file).unwrap_or(0);
        let saved =
            match self.vmgr.open_file_in_dir(dir, gpx.as_str(), Mode::ReadWriteCreateOrTruncate) {
                Ok(dst_file) => {
                    let source = SdByteSource::new(&self.vmgr, src_file, len);
                    let mut sink = SdByteSink::new(&self.vmgr, dst_file);
                    let ok = match track_to_gpx(&source, &ot.name, &mut sink) {
                        Ok(()) => {
                            defmt::info!("SD: saved ride → tracks/{=str}", gpx.as_str());
                            true
                        }
                        Err(e) => {
                            defmt::warn!("SD: GPX write failed: {}", defmt::Debug2Format(&e));
                            false
                        }
                    };
                    let _ = self.vmgr.flush_file(dst_file);
                    let _ = self.vmgr.close_file(dst_file);
                    ok
                }
                Err(e) => {
                    defmt::warn!("SD: cannot open GPX: {}", defmt::Debug2Format(&e));
                    false
                }
            };
        let _ = self.vmgr.close_file(src_file);
        // Drop the temp only after the ride is confirmed written; otherwise keep it.
        if saved {
            let _ = self.vmgr.delete_file_in_dir(dir, TRACK_TMP);
        }
    }

    /// Drop the open log without saving (Discard, or a no-session reconcile), deleting the temp.
    fn abandon_track(&mut self) {
        if let Some(ot) = self.open_track.take() {
            let _ = self.vmgr.close_file(ot.file);
            if let Some(dir) = self.tracks_dir {
                let _ = self.vmgr.delete_file_in_dir(dir, TRACK_TMP);
            }
        }
    }

    /// A **confirmed-free** `<name>.gpx` 8.3 filename in `dir`, or `None` when none of the
    /// `BASE` / `BASE01`…`BASE{GPX_COLLISION_MAX}` candidates can be proven absent.
    ///
    /// "Free" means [`find_directory_entry`](VolumeManager::find_directory_entry) answered
    /// `NotFound` — the *only* answer that proves a name is unused. A present entry, or **any
    /// other error** (a transient `DeviceError` on the flaky breadboard link — the same
    /// condition the render loop guards a route read against), is treated as "taken", so a
    /// glitch can never green-light reusing a name and make [`finalize_track`](Self::finalize_track)
    /// truncate an existing ride's GPX. `None` therefore means "couldn't confirm any slot is
    /// free": the caller keeps the temp log rather than clobbering.
    fn unique_gpx(&self, dir: RawDirectory, name: &str) -> Option<String<12>> {
        let base = sanitize_base(name);
        let first = make_83(&base, None);
        if self.name_is_free(dir, first.as_str()) {
            return Some(first);
        }
        for d in 1..=GPX_COLLISION_MAX {
            let cand = make_83(&base, Some(d));
            if self.name_is_free(dir, cand.as_str()) {
                return Some(cand);
            }
        }
        None
    }

    /// Whether `name` is **confirmed absent** in `dir` — i.e. safe to create without
    /// overwriting. Only `embedded_sdmmc::Error::NotFound` counts as free; a found entry or any
    /// other error is "not free" (see [`unique_gpx`](Self::unique_gpx)).
    fn name_is_free(&self, dir: RawDirectory, name: &str) -> bool {
        matches!(self.vmgr.find_directory_entry(dir, name), Err(embedded_sdmmc::Error::NotFound))
    }
}

/// Largest collision counter [`Storage::unique_gpx`] tries for a repeat save of the same route
/// name: `BASE01`…`BASE99`, so up to **100 distinct GPX files** per route name (the un-suffixed
/// `BASE.GPX` plus 99 numbered). Past that the ride is kept as the temp `.obct` rather than
/// risking an overwrite — generous enough that the bound is never hit in practice (a route
/// re-ridden 100 times with none of the saves ever cleared off the card).
const GPX_COLLISION_MAX: u8 = 99;

/// Whether a directory entry's **long** name ends with `ext` (e.g. `b".obcr"`, case-
/// insensitive) and isn't a dot-prefixed file (macOS `._*` AppleDouble / `.DS_Store`). The long
/// name is required because the 8.3 short name can't represent a 4-char extension — both
/// `.obcr` and `.obcm` truncate to `OBC`. A `None` long name (a plain 8.3 file) never matches,
/// which is fine: every `.obcr`/`.obcm` forces a long-filename entry.
fn long_has_ext(long: Option<&str>, ext: &[u8]) -> bool {
    let Some(name) = long else { return false };
    let b = name.as_bytes();
    !b.starts_with(b".")
        && b.len() >= ext.len()
        && b[b.len() - ext.len()..].eq_ignore_ascii_case(ext)
}

/// Reduce a route name to an 8.3 base: up to 8 upper-cased ASCII alphanumerics, never empty.
fn sanitize_base(name: &str) -> String<8> {
    let mut s = String::new();
    for c in name.chars() {
        if s.len() >= 8 {
            break;
        }
        let u = c.to_ascii_uppercase();
        if u.is_ascii_alphanumeric() {
            let _ = s.push(u);
        }
    }
    if s.is_empty() {
        let _ = s.push_str("RIDE");
    }
    s
}

/// Build a `BASE.GPX` 8.3 name, or `BASE<dd>.GPX` (base trimmed to 6 chars, `dd` the
/// zero-padded collision counter `01`…`99`) when disambiguating a repeat save. Two digits
/// (vs. the old single digit, which silently fell back to clobbering on the 11th save) widen
/// the per-name space to 100 before [`Storage::unique_gpx`] gives up — see [`GPX_COLLISION_MAX`].
fn make_83(base: &str, suffix: Option<u8>) -> String<12> {
    let mut s: String<12> = String::new();
    match suffix {
        None => {
            let _ = s.push_str(base);
        }
        Some(d) => {
            for c in base.chars().take(6) {
                let _ = s.push(c);
            }
            let _ = s.push((b'0' + (d / 10) % 10) as char);
            let _ = s.push((b'0' + d % 10) as char);
        }
    }
    let _ = s.push_str(".GPX");
    s
}
