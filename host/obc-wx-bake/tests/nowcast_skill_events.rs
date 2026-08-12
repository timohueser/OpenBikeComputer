//! **How far out is the nowcast honestly worth publishing?** — the two-event measurement (#1248).
//!
//! `nowcast_skill.rs` scores the engine over one storm. That storm is the 2020-08-10 derecho: fast,
//! organised, coherent, translating at 19.4 m/s — the friendliest case optical-flow advection will
//! ever be handed, and the only evidence behind `derive::NOWCAST_MAX_LEAD_MIN`. A horizon set from
//! one favourable event is a horizon set from a coincidence, so this harness adds the opposite case
//! and scores **both** the same way, at every lead the packs reach and at three intensity
//! thresholds rather than two.
//!
//! ## The second event, and why it is the hard case
//!
//! `us-airmass-2023-06-24` — 24 June 2023, 20:00 Z (3 pm CDT) over Iowa. Scattered airmass
//! convection: many small cells that form, rain and die inside the window, rather than one system
//! arriving across it. ("Airmass" strictly — diurnal convection inside one air mass, not tied to a
//! front. It is *not* a claim about storm speed; this field translates at 15.6 m/s.) The pack was
//! chosen by measurement, not by memory (the screening compared 30-odd summer afternoons over the
//! same box), and the two events separate on every statistic that matters to an advection scheme
//! except the one an earlier draft leaned on:
//!
//! | | derecho 2020-08-10 | airmass 2023-06-24 |
//! | --- | --- | --- |
//! | mean flow speed under rain (printed below) | 19.4 m/s | 15.6 m/s — *not* the discriminator |
//! | wet components in the crop | 62 | **235** |
//! | mean component area | 1331 cells | **79 cells** (~10 km across) |
//! | largest component | 79,149 cells — **one system** | 8,690 cells |
//! | wet fraction over the ladder | 36.6 % falling to 18.3 % (**it leaves**) | 8.3 % rising to 14.7 % (**it grows in place**) |
//! | persistence CSI >=0.25 at +60 | 0.343 | **0.192** |
//!
//! The last two rows are the point, and the first row is a warning. **Speed is not what makes a case
//! hard** — an earlier draft of this comment claimed 29.5 against 10.4 m/s, from a mean computed by
//! feeding node indices to a sampler that takes cell positions, and told a tidy story about weak
//! flow that the corrected number does not support. What actually separates the two events is that
//! the derecho's field stays itself and leaves the window, while the airmass field is continuously
//! rebuilt out of cells that did not exist ten minutes earlier: persistence decays twice as fast on
//! it, and development is exactly what an advection engine with no decay or initiation model cannot
//! represent. If a lead cap is going to be honest anywhere it has to be honest here.
//!
//! ## The anchor rule, and the +105/+120 question #1248 asks
//!
//! The engine needs two observations to estimate motion from, and the score needs truth after the
//! anchor. `nowcast_skill.rs` spends the ladder's two oldest rungs on the observation pair, which
//! is why it tops out at +90: the anchor lands 30 minutes into a 120-minute ladder. **That is a
//! pack-content limit, not a harness limit, and it is now fixable** — see `tests/events/README.md`:
//! the derecho pack carries no observation before its anchor because MTArchive answered 404 for
//! every 2020-08-10 key on the day it was captured. It answers again. The airmass pack, captured
//! against a live mirror, carries its motion-history observation as an ordinary member, so its
//! anchor is the pack's own anchor and the full ladder scores: **+14 … +120**.
//!
//! So the rule here is stated once and applied to both packs: *use the observation pair the pack
//! carries, and score every truth rung after it.* For the airmass pack that is (19:50, 20:00) and
//! eight rungs; for the derecho it is (18:48, 19:02) — frame 0 plus the ladder's first rung, which
//! recovers +106 where the old harness stopped at +90. The derecho's +120 needs the 18:38
//! observation the pack does not carry, and re-capturing it is #1278's call, not this branch's.
//!
//! ## The model baseline is the same pack with the radar withheld
//!
//! An OBCG object carries no provenance, so "which published frame is the model" cannot be read off
//! `service/`, and on the airmass pack it is not even a fixed answer: f+15 … f+60 are the nowcast
//! layer itself. The baseline is therefore re-derived — `run_cycle` over the pack's own upstream
//! with **only** the HRRR adapter, onto the pack's own lattice. That is the same bytes, the same
//! projection and the same quantization the published model frames get, minus the radar that would
//! otherwise outrank it. It answers at every lead, which is what a crossover measurement needs.
//!
//! ## Which numbers these are
//!
//! **Every figure quoted in this file, in `tests/events/README.md` and in
//! `derive::NOWCAST_MAX_LEAD_MIN` was measured against the engine as merged**, i.e. after both of
//! the corrections that landed while this measurement was being taken:
//!
//! * motion estimated from **measurement instants**, never from a published frame's cadence stamp.
//!   Deriving `dt` from two OBCG headers stretched the derecho's true 840 s baseline to 1,020 s and
//!   advected it 18 % too slowly. This harness reads `event.window_start`; `OBCG_Spec.md` §3.2 and
//!   `radar_nowcast`'s own comment now say why a header cannot be used for it.
//! * `flow::MAX_FILL_NODES` raised from 6 to 9. Re-scoring across it moved **both events by
//!   `<= 0.001` at every lead and threshold**, and moved no crossover: the bound governs
//!   extrapolation into nodes with no trackable echo nearby, and a window holding 235 separate
//!   components almost never has one. "Many small components" is not "isolated echo".
//!
//! The numbers are therefore current, and they are not a coincidence of either fix. Re-run them with
//! `cargo test --release -p obc-wx-bake --test nowcast_skill_events -- --nocapture`.

#![cfg(feature = "external-fixtures")]

use std::path::{Path, PathBuf};

use obc_formats::{obcg, precip4};
use obc_wx_bake::canonical::run_cycle;
use obc_wx_bake::flow::{self, FlowParams, MotionField};
use obc_wx_bake::pack::window::sub_lattice;
use obc_wx_bake::pack::{self, capture, rebake, Event};
use obc_wx_bake::publish::DirStore;
use obc_wx_bake::skill::contingency;
use obc_wx_bake::source::{hrrr, mrms, Adapter};
use obc_wx_bake::timefmt;

/// The three thresholds #1248 asks for, as intensity codes: `>= 0.25`, `>= 1.0` and `>= 6.0` mm/h.
/// Every one is an exact band edge in [`precip4::quantize_rate_mm_per_hour`], so none of them
/// splits a band.
const THRESHOLDS: [(u8, &str); 3] = [(3, ">=0.25"), (5, ">=1.0"), (8, ">=6.0")];

/// Both packs are a regional crop of CONUS, so the advection never wraps in longitude. (`advect`
/// grew the flag for the global model domains, where the east and west edges are neighbours.)
const WRAP_X: bool = false;

const EVENTS: [&str; 2] = ["us-derecho-2020-08-10", "us-airmass-2023-06-24"];

fn pack_root(id: &str) -> PathBuf {
    match id {
        "us-derecho-2020-08-10" => obc_fixtures::root().join("weather-event-derecho"),
        "us-airmass-2023-06-24" => obc_fixtures::root().join("weather-event-airmass"),
        _ => panic!("unregistered event pack {id}"),
    }
}

/// A private scratch directory. The counter matters: the two tests in this file both load both
/// packs, so a name derived from the event alone collides when they run in parallel.
fn scratch(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("obc-wx-skill-{}-{serial}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Every cell of an OBCG frame, row 0 = south — a corridor client's tile-by-tile decode.
fn all_cells(bytes: &[u8]) -> (obcg::Header, Vec<u8>) {
    let header = obcg::decode_header(bytes[..obcg::HEADER_LEN].try_into().unwrap()).unwrap();
    let edge = u32::from(header.tile_edge);
    let mut grid = vec![0u8; header.width as usize * header.height as usize];
    let mut tile = vec![0u8; header.tile_cells()];
    for tile_row in 0..header.tile_rows() {
        for tile_col in 0..header.tile_cols() {
            let tile_index = header.tile_index(tile_col, tile_row).unwrap();
            let page = header.page_of_entry(tile_index);
            let page_offset = header.page_offset(page).unwrap() as usize;
            let page_slice = &bytes[page_offset..page_offset + header.page_bytes() as usize];
            obcg::validate_page(&header, page_slice).unwrap();
            let within = (tile_index - page * u32::from(header.entries_per_page)) as usize;
            let entry = obcg::decode_entry(page_slice, within).unwrap();
            let payload = if entry.is_dry() {
                &[][..]
            } else {
                &bytes[entry.data_offset as usize..entry.data_offset as usize + usize::from(entry.encoded_len)]
            };
            obcg::decode_tile_cells(&header, &entry, payload, &mut tile).unwrap();
            for row in 0..edge.min(header.height - tile_row * edge) {
                for col in 0..edge.min(header.width - tile_col * edge) {
                    grid[((tile_row * edge + row) * header.width + tile_col * edge + col) as usize] =
                        tile[(row * edge + col) as usize];
                }
            }
        }
    }
    (header, grid)
}

struct Frame {
    valid_at: i64,
    cells: Vec<u8>,
}

/// One event, decoded and ready to score: the observation pair, the truth rungs after it, and the
/// radar-withheld model tree.
struct Case {
    id: &'static str,
    width: u32,
    height: u32,
    /// The lattice's **label**, not a physical size. See [`Case::cell_metres`].
    south_lat_udeg: i32,
    cell_lat_udeg: u32,
    cell_lon_udeg: u32,
    /// The two observations motion is estimated from, oldest first.
    earlier: Frame,
    anchor: Frame,
    /// Whether the earlier observation came from the pack's own motion-history member (the
    /// production shape) or from spending the ladder's first rung (the fallback).
    motion_from_member: bool,
    ladder: Vec<Frame>,
    /// The model baseline at every published offset, oldest first.
    model: Vec<Frame>,
}

fn read(root: &Path, relative: &str) -> Vec<u8> {
    let path = pack::resolve(root, relative).expect("pack-relative path");
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The pack's `service/` frame 0 — the anchor observation, on the pack's lattice.
///
/// Its `valid_at` is taken from `event.window_start`, the instant the **observation** is for,
/// rather than from the object's own header: since #1248 a published frame is stamped with the
/// canonical cadence instant it answers for, which on the derecho pack is 18:45 for an 18:48
/// observation. Three minutes is 5 % of an hour's lead and 18 % of the derecho's motion baseline,
/// so taking the stamp would advect the storm measurably too slowly and then score it against a
/// mis-stated lead.
fn frame_zero(root: &Path, event: &Event) -> Frame {
    let observed_at = timefmt::parse_rfc3339(&event.window_start).expect("window_start");
    let anchor = timefmt::parse_rfc3339(&event.bake.now).expect("bake.now");
    let mut best: Option<(i64, Frame)> = None;
    for object in &event.service {
        if object.key == event.manifest_key {
            continue;
        }
        let bytes = read(root, &format!("{}/{}", pack::SERVICE_DIR, object.key));
        let (header, cells) = all_cells(&bytes);
        // Frame 0 is the newest published frame at or before the cycle's own anchor instant.
        if header.valid_at > anchor {
            continue;
        }
        let distance = anchor - header.valid_at;
        if best.as_ref().is_none_or(|(closest, _)| distance < *closest) {
            best = Some((distance, Frame { valid_at: observed_at, cells }));
        }
    }
    best.expect("the pack publishes an observation frame").1
}

/// Bake the observation valid at `valid_at` onto the pack's lattice from the pack's own bytes, or
/// `None` when the pack does not carry it. This is `capture::bake_truth_frame` — the same
/// projection the truth ladder was made with, so the result is comparable cell for cell.
fn observation_at(root: &Path, event: &Event, valid_at: i64) -> Option<Frame> {
    let lattice = sub_lattice(&event.bake.bbox_udeg).expect("pack lattice");
    let mut upstream = rebake::replay_upstream(root, event).expect("the pack's service members");
    let bytes = capture::bake_truth_frame(&mut upstream, &lattice, valid_at).ok()?;
    let (_, cells) = all_cells(&bytes);
    Some(Frame { valid_at, cells })
}

/// The model baseline: the pack re-baked with HRRR alone, onto the pack's own lattice.
fn model_tree(root: &Path, event: &Event, id: &str) -> Vec<Frame> {
    let destination = scratch(&format!("model-{id}"));
    let lattice = sub_lattice(&event.bake.bbox_udeg).expect("pack lattice");
    let now = timefmt::parse_rfc3339(&event.bake.now).expect("bake.now");
    let mut upstream = rebake::replay_upstream(root, event).expect("the pack's service members");
    let mut store = DirStore::new(&destination);
    let hrrr_adapter = hrrr::Hrrr;
    let adapters: Vec<&dyn Adapter> = vec![&hrrr_adapter];
    run_cycle(&lattice, &adapters, &mut upstream, &mut store, now, 1, false).expect("a model-only cycle");
    let tree = pack::read_tree(&destination).expect("the model tree");
    let mut frames: Vec<Frame> = tree
        .iter()
        .filter(|(key, _)| key.ends_with(".obcg"))
        .map(|(_, bytes)| {
            let (header, cells) = all_cells(bytes);
            Frame { valid_at: header.valid_at, cells }
        })
        .collect();
    frames.sort_by_key(|frame| frame.valid_at);
    let _ = std::fs::remove_dir_all(&destination);
    frames
}

impl Case {
    fn load(id: &'static str) -> Self {
        let root = pack_root(id);
        let event = Event::read(&root).expect("the pack parses");
        let anchor = frame_zero(&root, &event);
        let mut ladder: Vec<Frame> = event
            .truth_frames
            .iter()
            .map(|frame| {
                let (header, cells) = all_cells(&read(&root, &frame.path));
                Frame { valid_at: header.valid_at, cells }
            })
            .collect();
        ladder.sort_by_key(|frame| frame.valid_at);

        // The production shape first: the observation `mrms::MOTION_LAG_SECONDS` before the anchor,
        // if the pack carries it. Otherwise spend the ladder's first rung and move the anchor.
        let (earlier, anchor, motion_from_member) =
            match observation_at(&root, &event, anchor.valid_at - mrms::MOTION_LAG_SECONDS) {
                Some(history) => (history, anchor, true),
                None => {
                    let first = ladder.remove(0);
                    (anchor, first, false)
                }
            };
        let ladder = ladder.into_iter().filter(|frame| frame.valid_at > anchor.valid_at).collect();

        let width = 0;
        let _ = width;
        let header =
            obcg::decode_header(read(&root, &event.truth_frames[0].path)[..obcg::HEADER_LEN].try_into().unwrap())
                .unwrap();
        Self {
            id,
            width: header.width,
            height: header.height,
            south_lat_udeg: header.south_lat_udeg,
            cell_lat_udeg: header.cell_lat_udeg,
            cell_lon_udeg: header.cell_lon_udeg,
            earlier,
            anchor,
            motion_from_member,
            ladder,
            model: model_tree(&root, &event, id),
        }
    }

    fn motion(&self) -> MotionField {
        let dt = (self.anchor.valid_at - self.earlier.valid_at) as f64;
        assert!(dt > 0.0, "{}: the observation pair must be ordered in time", self.id);
        let midpoint_lat =
            (f64::from(self.south_lat_udeg) + (f64::from(self.height) / 2.0) * f64::from(self.cell_lat_udeg)) / 1e6;
        let params = FlowParams::for_geographic_cells(self.cell_lat_udeg, self.cell_lon_udeg, midpoint_lat);
        flow::estimate_motion(&self.earlier.cells, &self.anchor.cells, self.width, self.height, dt, params)
            .unwrap_or_else(|| panic!("{}: neither observation is a dry field", self.id))
    }

    /// The published model frame nearest `valid_at`.
    fn model_at(&self, valid_at: i64) -> &Frame {
        self.model
            .iter()
            .min_by_key(|frame| (frame.valid_at - valid_at).abs())
            .unwrap_or_else(|| panic!("{}: the model tree is empty", self.id))
    }

    /// The metres one cell spans at `row`, **east-west and north-south separately**.
    ///
    /// **This is the trap that has now caught this measurement three times, so it lives in one
    /// function with its name on it.** The lattice is 0.01 degrees square in *angle*, and
    /// `header.cell_size_m` (1113) is a **label** — the metres 0.01 degrees of *latitude* spans,
    /// which the format carries so a client can scale a bar. A cell is not square on the ground:
    /// at the 42 N these packs sit at, 0.01 degrees of longitude is ~827 m, a quarter narrower.
    ///
    /// A flow field is in **cells per second**, so converting it to m/s with one scalar silently
    /// asserts square cells. Both these storms move nearly due east, so the error lands almost
    /// entirely on the component that matters and inflates the answer by ~35 %.
    fn cell_metres(&self, row: u32) -> (f64, f64) {
        const METRES_PER_DEGREE: f64 = 111_320.0;
        let latitude = (f64::from(self.south_lat_udeg) + f64::from(row) * f64::from(self.cell_lat_udeg)) / 1e6;
        let east_west = f64::from(self.cell_lon_udeg) / 1e6 * METRES_PER_DEGREE * latitude.to_radians().cos();
        let north_south = f64::from(self.cell_lat_udeg) / 1e6 * METRES_PER_DEGREE;
        (east_west, north_south)
    }

    /// Mean flow speed over the nodes that sit under **rain**, in m/s.
    ///
    /// Three traps, all of which have bitten:
    ///
    /// * [`MotionField::sample`] takes a **continuous cell position**, not a node index. Feeding it
    ///   `0..cols` samples the leftmost `cols / stride` node columns — a sliver of the west edge —
    ///   and reports whatever the weather is doing there as the storm's speed. Node `(i, j)` sits at
    ///   cell `(i * stride + stride / 2, j * stride + stride / 2)`.
    /// * averaging over *every* node averages the storm together with the dry ground around it,
    ///   where the flow is zero by construction ([`flow::MAX_FILL_NODES`] leaves distant nodes
    ///   still). That is a measure of how much of the window is raining, not of how fast the rain is
    ///   moving. Only nodes with rain under them count.
    /// * a cell is **not square on the ground** — see [`Case::cell_metres`].
    fn mean_speed_m_s(&self, motion: &MotionField) -> f64 {
        let (stride_x, stride_y) = (motion.stride_x, motion.stride_y);
        let (mut sum, mut counted) = (0.0f64, 0u64);
        for row in 0..motion.rows {
            for col in 0..motion.cols {
                let (x, y) = (col * stride_x + stride_x / 2, row * stride_y + stride_y / 2);
                if x >= self.width || y >= self.height {
                    continue;
                }
                let code = self.anchor.cells[(y * self.width + x) as usize];
                if code == precip4::INTENSITY_NODATA || code < 3 {
                    continue;
                }
                let (dx, dy) = motion.sample(x as f32, y as f32);
                let (east_west, north_south) = self.cell_metres(y);
                sum += (f64::from(dx) * east_west).hypot(f64::from(dy) * north_south);
                counted += 1;
            }
        }
        if counted == 0 {
            return 0.0;
        }
        sum / counted as f64
    }
}

fn csi(forecast: &[u8], truth: &[u8], threshold: u8) -> f64 {
    contingency(forecast, truth, threshold).csi().unwrap_or(0.0)
}

fn wet_fraction(cells: &[u8], threshold: u8) -> f64 {
    let wet = cells.iter().filter(|&&code| code != precip4::INTENSITY_NODATA && code >= threshold).count();
    wet as f64 / cells.len() as f64
}

// ---------------------------------------------------------------------------------------------
// The scale cascade: does damping each spatial scale by its own measured persistence help?
// ---------------------------------------------------------------------------------------------

/// Mean of a `2 * radius + 1` box at every cell, edge-truncated, over a summed-area table.
fn box_mean(values: &[f32], width: u32, height: u32, radius: u32) -> Vec<f32> {
    let (w, h) = (width as usize, height as usize);
    let radius = radius as isize;
    let mut area = vec![0.0f64; (w + 1) * (h + 1)];
    for row in 0..h {
        let mut running = 0.0f64;
        for col in 0..w {
            running += f64::from(values[row * w + col]);
            area[(row + 1) * (w + 1) + col + 1] = area[row * (w + 1) + col + 1] + running;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for row in 0..h as isize {
        for col in 0..w as isize {
            let (x0, y0) = ((col - radius).max(0) as usize, (row - radius).max(0) as usize);
            let (x1, y1) = ((col + radius + 1).min(w as isize) as usize, (row + radius + 1).min(h as isize) as usize);
            let sum =
                area[y1 * (w + 1) + x1] - area[y0 * (w + 1) + x1] - area[y1 * (w + 1) + x0] + area[y0 * (w + 1) + x0];
            let cells = ((x1 - x0) * (y1 - y0)) as f64;
            out[row as usize * w + col as usize] = if cells > 0.0 { (sum / cells) as f32 } else { 0.0 };
        }
    }
    out
}

/// The band radii of the cascade, in cells (~km). Three bands is the coarsest decomposition that
/// can say anything: synoptic/frontal structure, mesoscale clusters, and individual convective
/// cells — which are the three things with visibly different lifetimes.
const BAND_RADII: [u32; 2] = [32, 8];

/// A field split into `[large, meso, small]` components that sum back to the original.
fn cascade(cells: &[u8], width: u32, height: u32) -> [Vec<f32>; 3] {
    // Work in code space. The codes are already a near-logarithmic ladder over rain rate, which is
    // the space a cascade decomposition wants to be in; no-data is treated as dry for the purpose
    // of the decomposition and restored afterwards.
    let field: Vec<f32> =
        cells.iter().map(|&code| if code == precip4::INTENSITY_NODATA { 0.0 } else { f32::from(code) }).collect();
    let large = box_mean(&field, width, height, BAND_RADII[0]);
    let medium = box_mean(&field, width, height, BAND_RADII[1]);
    let meso: Vec<f32> = medium.iter().zip(&large).map(|(m, l)| m - l).collect();
    let small: Vec<f32> = field.iter().zip(&medium).map(|(f, m)| f - m).collect();
    [large, meso, small]
}

/// Pearson correlation between two bands, over the cells where either has amplitude.
fn correlation(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len() as f64;
    let (mean_a, mean_b) =
        (a.iter().map(|&v| f64::from(v)).sum::<f64>() / n, b.iter().map(|&v| f64::from(v)).sum::<f64>() / n);
    let (mut cov, mut var_a, mut var_b) = (0.0, 0.0, 0.0);
    for (&x, &y) in a.iter().zip(b) {
        let (x, y) = (f64::from(x) - mean_a, f64::from(y) - mean_b);
        cov += x * y;
        var_a += x * x;
        var_b += y * y;
    }
    if var_a <= 0.0 || var_b <= 0.0 {
        return 0.0;
    }
    cov / (var_a.sqrt() * var_b.sqrt())
}

/// The per-band AR(1) coefficients, measured **Lagrangian**: the earlier frame is advected forward
/// onto the anchor before the bands are correlated, so what is measured is the loss of amplitude,
/// not the translation the advection step already handles.
fn band_persistence(case: &Case, motion: &MotionField) -> [f64; 3] {
    let dt = (case.anchor.valid_at - case.earlier.valid_at) as f64;
    let moved = flow::advect(&case.earlier.cells, case.width, case.height, motion, dt, WRAP_X);
    let before = cascade(&moved, case.width, case.height);
    let after = cascade(&case.anchor.cells, case.width, case.height);
    let mut rho = [0.0f64; 3];
    for band in 0..3 {
        rho[band] = correlation(&before[band], &after[band]).clamp(0.0, 1.0);
    }
    rho
}

/// Re-quantize a [`damped_field`] back to intensity codes, carrying no-data through.
///
/// Split from [`damped_field`] rather than folded into it, and neither of them advects: when the
/// decay path advected for itself, the cost block below timed a second full `advect` and a second
/// full `cascade` inside the post-process's window, and reported it as 1.5x the advection it rides
/// on when the work a cycle would actually add is nearer 1.1x.
fn decayed_nowcast(damped: &[f32], advected: &[u8]) -> Vec<u8> {
    advected
        .iter()
        .enumerate()
        .map(|(index, &code)| {
            if code == precip4::INTENSITY_NODATA {
                return code;
            }
            let rounded = damped[index].round();
            if rounded <= 0.0 {
                precip4::INTENSITY_DRY
            } else {
                (rounded as u8).min(precip4::INTENSITY_MAX)
            }
        })
        .collect()
}

/// **Probability matching**: keep the decayed field's *ordering* and restore the advected field's
/// *distribution*.
///
/// This is the answer to the objection the decay measurement raises. Damping amplitude flattens the
/// convective cores — it is a mean-square-optimal filter, and the mean-square-optimal answer to
/// "how hard is it raining" is always "less hard than that". Probability matching fixes it without
/// giving the smoothing back: rank the cells by the damped value, then hand out the *sorted codes
/// the advected field already contained*, highest to highest. The output is a permutation of an
/// existing field's own values, so **every published code is a code that source stated** — the
/// no-fabrication rule is satisfied by construction rather than by argument.
fn probability_matched(damped: &[f32], donor: &[u8]) -> Vec<u8> {
    let mut order: Vec<u32> =
        (0..damped.len() as u32).filter(|&i| donor[i as usize] != precip4::INTENSITY_NODATA).collect();
    let mut values: Vec<u8> = order.iter().map(|&i| donor[i as usize]).collect();
    order.sort_unstable_by(|&a, &b| {
        damped[b as usize].partial_cmp(&damped[a as usize]).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b))
    });
    values.sort_unstable_by(|a, b| b.cmp(a));
    let mut out = donor.to_vec();
    for (rank, &index) in order.iter().enumerate() {
        out[index as usize] = values[rank];
    }
    out
}

/// The damped continuous field the two decay variants share.
fn damped_field(case: &Case, advected: &[u8], lead_s: f64, rho: [f64; 3]) -> Vec<f32> {
    let dt = (case.anchor.valid_at - case.earlier.valid_at) as f64;
    let steps = lead_s / dt;
    let bands = cascade(advected, case.width, case.height);
    let weights = [rho[0].powf(steps) as f32, rho[1].powf(steps) as f32, rho[2].powf(steps) as f32];
    (0..advected.len())
        .map(|index| bands[0][index] * weights[0] + bands[1][index] * weights[1] + bands[2][index] * weights[2])
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Is a CSI difference bigger than the noise? — the moving-block bootstrap behind the cap.
// ---------------------------------------------------------------------------------------------

/// Block edge in cells. The domain is 1024 x 768, so this tiles it **exactly** into 16 x 12 = 192
/// blocks with no ragged edge to special-case.
const BOOTSTRAP_BLOCK: u32 = 64;
const BOOTSTRAP_DRAWS: usize = 2000;

/// `(event, threshold, lead, (gap, low, high, P(nowcast >= model)))` — one bootstrapped comparison.
type Significance = (&'static str, &'static str, i64, (f64, f64, f64, f64));

/// Per-block `(hits, misses, false alarms)` for one forecast at one threshold.
fn block_counts(forecast: &[u8], truth: &[u8], width: u32, height: u32, threshold: u8) -> Vec<[u64; 3]> {
    let (cols, rows) = (width / BOOTSTRAP_BLOCK, height / BOOTSTRAP_BLOCK);
    let mut blocks = vec![[0u64; 3]; (cols * rows) as usize];
    for row in 0..rows * BOOTSTRAP_BLOCK {
        for col in 0..cols * BOOTSTRAP_BLOCK {
            let index = (row * width + col) as usize;
            let observed = truth[index];
            if observed == precip4::INTENSITY_NODATA {
                continue;
            }
            let predicted = forecast[index];
            let predicted_wet = predicted != precip4::INTENSITY_NODATA && predicted >= threshold;
            let observed_wet = observed >= threshold;
            let block = &mut blocks[((row / BOOTSTRAP_BLOCK) * cols + col / BOOTSTRAP_BLOCK) as usize];
            match (predicted_wet, observed_wet) {
                (true, true) => block[0] += 1,
                (false, true) => block[1] += 1,
                (true, false) => block[2] += 1,
                (false, false) => {}
            }
        }
    }
    blocks
}

fn csi_of(counts: [u64; 3]) -> f64 {
    let denominator = counts[0] + counts[1] + counts[2];
    if denominator == 0 {
        0.0
    } else {
        counts[0] as f64 / denominator as f64
    }
}

/// `(model - nowcast, low, high, P(nowcast >= model))` from a block bootstrap over the two fields.
///
/// **Why blocks and not cells.** Rain is spatially correlated over tens of kilometres, so treating
/// 786,432 cells as independent samples would give a confidence interval far too narrow to be worth
/// printing — every difference would look decisive. Resampling 64 x 64 km blocks with replacement
/// keeps the correlation inside a block intact and only assumes independence between blocks, which
/// is the standard construction for a verification score on a raster.
///
/// Deterministic by construction: a fixed-seed xorshift, so the interval is a property of the branch
/// and not of the day it was run.
fn bootstrap_gap(
    nowcast: &[u8],
    model: &[u8],
    truth: &[u8],
    width: u32,
    height: u32,
    threshold: u8,
) -> (f64, f64, f64, f64) {
    let now_blocks = block_counts(nowcast, truth, width, height, threshold);
    let model_blocks = block_counts(model, truth, width, height, threshold);
    let count = now_blocks.len();
    let mut state: u64 = 0x9E3779B97F4A7C15 ^ u64::from(threshold);
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut gaps = Vec::with_capacity(BOOTSTRAP_DRAWS);
    let mut wins = 0usize;
    for _ in 0..BOOTSTRAP_DRAWS {
        let (mut now_sum, mut model_sum) = ([0u64; 3], [0u64; 3]);
        for _ in 0..count {
            let pick = (next() % count as u64) as usize;
            for i in 0..3 {
                now_sum[i] += now_blocks[pick][i];
                model_sum[i] += model_blocks[pick][i];
            }
        }
        let gap = csi_of(model_sum) - csi_of(now_sum);
        if gap <= 0.0 {
            wins += 1;
        }
        gaps.push(gap);
    }
    gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let point = csi_of(model_blocks.iter().fold([0u64; 3], |mut acc, b| {
        for i in 0..3 {
            acc[i] += b[i];
        }
        acc
    })) - csi_of(now_blocks.iter().fold([0u64; 3], |mut acc, b| {
        for i in 0..3 {
            acc[i] += b[i];
        }
        acc
    }));
    (
        point,
        gaps[BOOTSTRAP_DRAWS / 40],
        gaps[BOOTSTRAP_DRAWS - BOOTSTRAP_DRAWS / 40 - 1],
        wins as f64 / BOOTSTRAP_DRAWS as f64,
    )
}

// ---------------------------------------------------------------------------------------------

#[test]
fn the_skill_table_for_both_events_at_every_lead_and_three_thresholds() {
    let mut crossovers = Vec::new();
    let mut costs = Vec::new();
    let mut significance: Vec<Significance> = Vec::new();
    for id in EVENTS {
        let case = Case::load(id);
        let motion = case.motion();
        eprintln!(
            "\n=== {id} — {} x {} cells, {:.0} m east-west by {:.0} m north-south at mid-window ===\n\
             observation pair {} -> {} ({}), mean flow under rain {:.1} m/s",
            case.width,
            case.height,
            case.cell_metres(case.height / 2).0,
            case.cell_metres(case.height / 2).1,
            timefmt::rfc3339(case.earlier.valid_at),
            timefmt::rfc3339(case.anchor.valid_at),
            if case.motion_from_member {
                "the pack's own motion-history member"
            } else {
                "frame 0 plus the ladder's first rung"
            },
            case.mean_speed_m_s(&motion),
        );
        let rho = band_persistence(&case, &motion);
        eprintln!(
            "band persistence per {} s step: large(>{} km) {:.3}  meso({}-{} km) {:.3}  cell(<{} km) {:.3}",
            case.anchor.valid_at - case.earlier.valid_at,
            BAND_RADII[0],
            rho[0],
            BAND_RADII[1],
            BAND_RADII[0],
            rho[1],
            BAND_RADII[1],
            rho[2],
        );
        eprintln!("\n lead  wet%   method       CSI>=0.25  CSI>=1.0  CSI>=6.0   (nowcast+decay in brackets)");

        for target in &case.ladder {
            let lead_s = target.valid_at - case.anchor.valid_at;
            let lead_min = lead_s / 60;
            let advect_started = std::time::Instant::now();
            let nowcast = flow::advect(&case.anchor.cells, case.width, case.height, &motion, lead_s as f64, WRAP_X);
            let advect_cost = advect_started.elapsed();
            // Everything from here to `cascade_cost` is the **post-process only**, on a field that
            // has already been advected — which is what would actually be added to a cycle.
            let cascade_started = std::time::Instant::now();
            let damped = damped_field(&case, &nowcast, lead_s as f64, rho);
            let decayed = decayed_nowcast(&damped, &nowcast);
            let matched = probability_matched(&damped, &nowcast);
            let cascade_cost = cascade_started.elapsed();
            costs.push((advect_cost, cascade_cost));
            let model = case.model_at(target.valid_at);
            let skew = (model.valid_at - target.valid_at).abs();

            let line = |label: &str, forecast: &[u8]| {
                let mut row = format!("       {label:<12}");
                for (threshold, _) in THRESHOLDS {
                    row.push_str(&format!(" {:>9.3}", csi(forecast, &target.cells, threshold)));
                }
                row
            };
            let observed = |threshold: u8| {
                target.cells.iter().filter(|&&code| code != precip4::INTENSITY_NODATA && code >= threshold).count()
            };
            eprintln!(
                "+{lead_min:>3}m  wet {:.1}% / {} / {} cells at the three thresholds",
                100.0 * wet_fraction(&target.cells, 3),
                observed(5),
                observed(8),
            );
            eprintln!("{}", line("nowcast", &nowcast));
            eprintln!("{}", line("+decay", &decayed));
            eprintln!("{}", line("+decay+PMM", &matched));
            eprintln!("{}", line("persistence", &case.anchor.cells));
            eprintln!("{}  (model valid {skew} s away)", line("model", &model.cells));

            for (threshold, name) in THRESHOLDS {
                crossovers.push((
                    id,
                    name,
                    lead_min,
                    csi(&nowcast, &target.cells, threshold),
                    csi(&decayed, &target.cells, threshold),
                    csi(&matched, &target.cells, threshold),
                    csi(&model.cells, &target.cells, threshold),
                ));
                let gap = bootstrap_gap(&nowcast, &model.cells, &target.cells, case.width, case.height, threshold);
                significance.push((id, name, lead_min, gap));
            }
        }
    }

    // What the post-process would cost if it shipped. Single-threaded, on the 786,432-cell pack
    // lattice, per megacell so the production domain can be reasoned about rather than guessed at.
    //
    // **Minimum, not mean.** These are timings taken inside a test suite that may be sharing the
    // machine with a compile or another test binary, and load can only ever make a sample slower.
    // The minimum over the 15 forward frames is the closest available estimate of the work itself;
    // a mean reports how busy the machine was.
    //
    // **Quote the post-process's own cost, not the ratio.** `flow::advect` parallelises internally
    // through rayon and this post-process does not, so the ratio compares a parallel operation
    // against a serial one — it is not like for like, and it duly swings between 0.97x and 1.51x
    // across runs of this very test while the per-megacell serial cost stays put. The budgetable
    // number is `post-process ms/Mcell`; the ratio is printed only to show it is the same order of
    // magnitude as the advection, not to be carried into a capacity plan.
    let cells = 1024.0 * 768.0 / 1e6;
    let least = |pick: fn(&(std::time::Duration, std::time::Duration)) -> std::time::Duration| {
        costs.iter().map(|cost| pick(cost).as_secs_f64()).fold(f64::INFINITY, f64::min)
    };
    let advect = least(|cost| cost.0);
    let cascade = least(|cost| cost.1);
    eprintln!(
        "\ncost per forward frame, fastest of {} frames: post-process (cascade+decay+PMM, serial) \
         {3:.1} ms = {4:.1} ms/Mcell — the budgetable figure. advect (rayon-parallel) {1:.1} ms \
         = {2:.1} ms/Mcell, ratio {5:.2}x, not like-for-like (see the comment above)",
        costs.len(),
        advect * 1e3,
        advect * 1e3 / cells,
        cascade * 1e3,
        cascade * 1e3 / cells,
        cascade / advect,
    );

    // The crossover is a point estimate on one realisation of the weather, so on its own it says
    // less than it looks like it says. This is the interval around it.
    eprintln!(
        "\n=== is the gap bigger than the noise? ({BOOTSTRAP_DRAWS} draws over {} blocks of {BOOTSTRAP_BLOCK}^2 cells) ===",
        (1024 / BOOTSTRAP_BLOCK) * (768 / BOOTSTRAP_BLOCK)
    );
    eprintln!("  model - nowcast, 95 % interval, and P(nowcast >= model). Negative gap = nowcast ahead.");
    for id in EVENTS {
        for (_, name) in THRESHOLDS {
            for (_, _, lead, (point, low, high, wins)) in significance.iter().filter(|row| row.0 == id && row.1 == name)
            {
                let verdict = if *low > 0.0 {
                    "model ahead, significant"
                } else if *high < 0.0 {
                    "nowcast ahead, significant"
                } else {
                    "not significant"
                };
                eprintln!(
                    "{id:<24} {name:<7} +{lead:>3}m  {point:+.4} [{low:+.4}, {high:+.4}]  P={wins:.2}  {verdict}",
                );
            }
        }
    }

    eprintln!("\n=== where advected radar stops beating the model ===");
    for id in EVENTS {
        for (_, name) in THRESHOLDS {
            let rows: Vec<_> = crossovers.iter().filter(|row| row.0 == id && row.1 == name).collect();
            let plain = rows.iter().find(|row| row.3 <= row.6).map(|row| row.2);
            let decayed = rows.iter().find(|row| row.4 <= row.6).map(|row| row.2);
            let matched = rows.iter().find(|row| row.5 <= row.6).map(|row| row.2);
            let last = rows.last().map(|row| row.2).unwrap_or(0);
            let show = |crossover: Option<i64>| match crossover {
                Some(lead) => format!("+{lead} min"),
                None => format!("none by +{last}"),
            };
            eprintln!(
                "{id:<24} {name:<7} plain {:<14} decay {:<14} decay+PMM {}",
                show(plain),
                show(decayed),
                show(matched)
            );
        }
    }

    // The ratchet, deliberately coarse: on both events, at the rider-relevant threshold, advecting
    // the observed field must beat the model at +15. If that ever stops being true the nowcast has
    // no window at all and the layer should be withdrawn rather than capped.
    for id in EVENTS {
        let first = crossovers
            .iter()
            .find(|row| row.0 == id && row.1 == ">=0.25")
            .unwrap_or_else(|| panic!("{id}: no scored leads"));
        assert!(
            first.3 > first.6,
            "{id}: at +{} min the nowcast scored {:.3} against the model's {:.3}",
            first.2,
            first.3,
            first.6
        );
    }
}

/// **The fabrication measurement behind the blend recommendation.**
///
/// #1242 refused to average two quantized intensity fields because the average is a code neither
/// source stated. This puts a number on both halves of that trade at every lead: how much of the
/// field a code-averaging blend would invent, and how visible the alternative — a hard switch — is
/// as a seam.
#[test]
fn what_a_blend_would_invent_and_what_a_hard_switch_shows() {
    for id in EVENTS {
        let case = Case::load(id);
        let motion = case.motion();
        eprintln!("\n=== {id}: blend cost versus seam cost ===");
        eprintln!(" lead   invented-wet%   seam wet/dry disagreement");
        for target in &case.ladder {
            let lead_s = target.valid_at - case.anchor.valid_at;
            let nowcast = flow::advect(&case.anchor.cells, case.width, case.height, &motion, lead_s as f64, WRAP_X);
            let model = &case.model_at(target.valid_at).cells;
            // A textbook lead-weighted ramp: all radar at the anchor, all model at +2 h.
            let weight = (lead_s as f64 / 7200.0).clamp(0.0, 1.0) as f32;

            let (mut wet, mut invented, mut disagree, mut covered) = (0u64, 0u64, 0u64, 0u64);
            for (&a, &b) in nowcast.iter().zip(model) {
                if a == precip4::INTENSITY_NODATA || b == precip4::INTENSITY_NODATA {
                    continue;
                }
                covered += 1;
                let blended = ((1.0 - weight) * f32::from(a) + weight * f32::from(b)).round() as u8;
                if blended >= 3 {
                    wet += 1;
                    if blended != a && blended != b {
                        invented += 1;
                    }
                }
                if (a >= 3) != (b >= 3) {
                    disagree += 1;
                }
            }
            eprintln!(
                "+{:>3}m   {:>12.1}   {:>12.3}",
                lead_s / 60,
                100.0 * invented as f64 / wet.max(1) as f64,
                disagree as f64 / covered.max(1) as f64,
            );
        }
    }
}
