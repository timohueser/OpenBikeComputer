//! **Does the nowcast actually beat what it replaces?** (WXR9 #1251)
//!
//! #1251 calls the verification harness "the deliverable that makes the rest arguable", and this is
//! it. It runs entirely on bytes the repository already carries: the 2020-08-10 Midwest derecho
//! event pack holds eight **observed** MRMS frames at +15 … +120 minutes (`truth/`) and the baked
//! service tree the real baker made of the same event (`service/`), all byte-verified by
//! `event_pack.rs`.
//!
//! ## The experiment
//!
//! The two earliest truth frames are 19:02 Z and 19:18 Z, sixteen minutes apart. They are the
//! **observation pair** — exactly the input `crate::derive::radar_nowcast` gets in production, where
//! the MRMS adapter carries one earlier observation alongside the anchor. Everything after 19:18 Z
//! is withheld and used as ground truth:
//!
//! | at        | lead from 19:18 | truth frame |
//! | --------- | --------------- | ----------- |
//! | 19:32 Z   | +14 min         | `truth/f44.obcg` |
//! | 19:48 Z   | +30 min         | `truth/f60.obcg` |
//! | 20:02 Z   | +44 min         | `truth/f74.obcg` |
//! | 20:18 Z   | +60 min         | `truth/f90.obcg` |
//! | 20:32 Z   | +74 min         | `truth/f104.obcg` |
//! | 20:48 Z   | +90 min         | `truth/f120.obcg` |
//!
//! Three forecasts are scored against each:
//!
//! * **nowcast** — 19:18 Z advected by the estimated motion field;
//! * **persistence** — 19:18 Z, frozen. The thing a nowcast must beat to be worth its CPU, and the
//!   thing #1248 refused to publish under a future validity;
//! * **model** — the pack's own published frame nearest that instant, which since #1248 is HRRR at
//!   3 km. This is what the mosaic paints at those offsets **today**, so it is the baseline the
//!   handoff decision turns on.
//!
//! The model frames sit on the cycle's quarter-hour grid and the truth frames on MRMS's two-minute
//! cadence, so each pairing is up to three minutes apart. That mismatch is the pack's own ladder
//! convention (`TruthFrame::offset_min` is floored onto the observation cadence) and it is small
//! against the 15-minute grid, but it is stated rather than hidden — it very slightly flatters the
//! model, which is the safe direction for a test whose conclusion is "the nowcast wins".
//!
//! ## What it asserts
//!
//! Printed numbers are the deliverable; the assertions are the ratchet. They are deliberately
//! coarse — this is one event, and a threshold tight enough to pin a specific CSI would fail on the
//! next pack — but they do pin the two claims that decide whether WXR9 ships at all: the nowcast
//! beats persistence, and it beats the model out to `derive::NOWCAST_MAX_LEAD_MIN`.

use obc_formats::{obcg, precip4};
use obc_wx_bake::canonical::{CycleTimes, FRAME_STEP_MIN};
use obc_wx_bake::derive::{self, NOWCAST_MAX_LEAD_MIN};
use obc_wx_bake::flow::{self, FlowParams};
use obc_wx_bake::geometry::GridGeometry;
use obc_wx_bake::pack::{self, Event};
use obc_wx_bake::skill::{Scores, LIGHT_RAIN};
use obc_wx_bake::source::{mrms, BakedFrame, BakedSource, SourceClass};
use obc_wx_bake::timefmt;

const EVENT_ID: &str = "us-derecho-2020-08-10";

fn pack_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/events").join(EVENT_ID)
}

fn read(relative: &str) -> Vec<u8> {
    let path = pack::resolve(&pack_root(), relative).expect("pack-relative path");
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// Every cell of an OBCG frame, in row order (row 0 = south) — the same tile-by-tile decode
/// `event_pack.rs` uses, which is how a corridor client reads a frame.
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

/// The truth ladder, oldest first, decoded.
fn truth_ladder(event: &Event) -> (obcg::Header, Vec<Frame>) {
    let mut header = None;
    let mut frames: Vec<Frame> = event
        .truth_frames
        .iter()
        .map(|frame| {
            let (decoded, cells) = all_cells(&read(&frame.path));
            let valid_at = timefmt::parse_rfc3339(&frame.valid_at).expect("truth valid_at");
            assert_eq!(decoded.valid_at, valid_at, "{}: the header and event.json disagree", frame.path);
            header = Some(decoded);
            Frame { valid_at, cells }
        })
        .collect();
    frames.sort_by_key(|frame| frame.valid_at);
    (header.expect("the pack has truth frames"), frames)
}

/// The published frame nearest `valid_at` — the model baseline, read out of the pack's own
/// `service/` tree exactly as a client would fetch it.
fn model_frame(event: &Event, valid_at: i64) -> (i64, Vec<u8>) {
    let mut best: Option<(i64, String)> = None;
    for object in &event.service {
        if object.key == event.manifest_key {
            continue;
        }
        let path = format!("{}/{}", pack::SERVICE_DIR, object.key);
        let bytes = read(&path);
        let header = obcg::decode_header(bytes[..obcg::HEADER_LEN].try_into().unwrap()).unwrap();
        let distance = (header.valid_at - valid_at).abs();
        if best.as_ref().is_none_or(|(closest, _)| distance < *closest) {
            best = Some((distance, path));
        }
    }
    let (distance, path) = best.expect("the pack publishes frames");
    (distance, all_cells(&read(&path)).1)
}

#[test]
fn the_nowcast_beats_persistence_and_the_model_over_the_derecho() {
    let event = Event::read(&pack_root()).expect("the derecho pack");
    let (header, ladder) = truth_ladder(&event);
    assert!(ladder.len() >= 4, "the skill harness needs a truth ladder, found {}", ladder.len());
    let (width, height) = (header.width, header.height);

    // The observation pair: the two oldest truth frames, which is the shape the MRMS adapter hands
    // `derive::radar_nowcast` in production (one anchor plus one motion-history observation).
    let earlier = &ladder[0];
    let anchor = &ladder[1];
    let dt = (anchor.valid_at - earlier.valid_at) as f64;
    assert!(dt > 0.0, "the observation pair must be ordered in time");
    // **These are measurement instants, and the test says so out loud** (#1283). A truth frame is
    // emitted on its own instant, so its header is the observation time — but a *published* f0
    // states the cadence slot instead, and PR #1283 recovered a baseline from two of those and
    // advected the derecho 18 % too slowly. Both rungs here land off the quarter hour, which is
    // exactly what a cadence instant cannot do, so a future refactor that quietly starts reading
    // slot times fails here rather than shipping a uniformly slow field.
    let cadence = i64::from(FRAME_STEP_MIN) * 60;
    for rung in [earlier, anchor] {
        assert_ne!(
            rung.valid_at % cadence,
            0,
            "{} is on the cadence — this harness must difference measurement instants, not slots",
            timefmt::rfc3339(rung.valid_at)
        );
    }
    let midpoint_lat =
        (f64::from(header.south_lat_udeg) + (f64::from(header.height) / 2.0) * f64::from(header.cell_lat_udeg)) / 1e6;
    let params = FlowParams::for_geographic_cells(header.cell_lat_udeg, header.cell_lon_udeg, midpoint_lat);
    let motion = flow::estimate_motion(&earlier.cells, &anchor.cells, width, height, dt, params)
        .expect("a derecho is not a dry field");

    eprintln!(
        "\n{EVENT_ID}: {width} x {height} cells of {} m, motion from {} -> {} ({dt:.0} s apart)",
        header.cell_size_m,
        timefmt::rfc3339(earlier.valid_at),
        timefmt::rfc3339(anchor.valid_at),
    );
    eprintln!(
        "peak raster motion {:.4} cells/s; flow grid {} x {} nodes at strides {} x {}",
        motion.max_speed_cells_s(),
        motion.cols,
        motion.rows,
        motion.stride_x,
        motion.stride_y
    );
    eprintln!("\n lead  method       CSI>=.25  CSI>=2.0  FSS>=.25  FSS>=2.0  cover");

    let mut wins_over_persistence = 0usize;
    let mut wins_over_model_inside_cap = 0usize;
    let mut leads_inside_cap = 0usize;
    let mut verifiable_lead_min = 0i64;
    // For the seam below: the advected frame at each lead, kept.
    let mut nowcast_frames: Vec<(i64, Vec<u8>)> = Vec::new();
    for target in &ladder[2..] {
        let lead_s = target.valid_at - anchor.valid_at;
        let lead_min = lead_s / 60;
        let nowcast = flow::advect(&anchor.cells, width, height, &motion, lead_s as f64, false);
        let (model_skew, model) = model_frame(&event, target.valid_at);

        let scored = Scores::of(&nowcast, &target.cells, width, height);
        let persisted = Scores::of(&anchor.cells, &target.cells, width, height);
        let modelled = Scores::of(&model, &target.cells, width, height);
        eprintln!("+{lead_min:>3}m  nowcast      {}", scored.row());
        eprintln!("       persistence  {}", persisted.row());
        eprintln!("       model        {}   (valid {model_skew} s from the truth frame)", modelled.row());

        let csi = |scores: &Scores| scores.light.csi().unwrap_or(0.0);
        if csi(&scored) > csi(&persisted) {
            wins_over_persistence += 1;
        }
        if lead_min <= i64::from(NOWCAST_MAX_LEAD_MIN) {
            leads_inside_cap += 1;
            verifiable_lead_min = verifiable_lead_min.max(lead_min);
            if csi(&scored) > csi(&modelled) {
                wins_over_model_inside_cap += 1;
            }
        }
        nowcast_frames.push((target.valid_at, nowcast));
    }
    eprintln!();

    let leads = ladder.len() - 2;
    // Advecting the observed field must beat freezing it. If this ever fails the motion estimate is
    // worthless and the whole source should be withdrawn from the mosaic, not tuned.
    assert!(
        wins_over_persistence >= leads - 1,
        "the nowcast beat persistence at only {wins_over_persistence} of {leads} lead times"
    );
    // And inside the published cap it must beat the model, or the cap is set wrong — which is the
    // one thing #1251 says must be proved rather than assumed.
    assert!(
        leads_inside_cap > 0 && wins_over_model_inside_cap == leads_inside_cap,
        "the nowcast beat the model at {wins_over_model_inside_cap} of the {leads_inside_cap} lead times inside \
         NOWCAST_MAX_LEAD_MIN = {NOWCAST_MAX_LEAD_MIN}; the cap must not promise skill the measurement does not show"
    );
    // **And the ladder must actually reach the cap** (#1278 r1, M3). Without this the assertion above
    // is vacuous for every lead past the last truth frame: the reviewer set the constant to 120 and
    // the test passed unchanged, which is precisely the property the comment at the constant claims
    // it does not have. The largest lead this pack can verify is +90; a horizon past it would be a
    // trend extrapolation wearing a measurement's clothes, and unlocking one means a **second event
    // pack** whose ladder goes further, not a bigger number here.
    assert!(
        verifiable_lead_min >= i64::from(NOWCAST_MAX_LEAD_MIN),
        "NOWCAST_MAX_LEAD_MIN is {NOWCAST_MAX_LEAD_MIN} min but this pack's truth ladder only verifies +{verifiable_lead_min}; \
         the horizon must never promise skill beyond the leads that were measured — extend the pack, not the constant"
    );

    // ── The seam (#1278 r1, m7) ──────────────────────────────────────────────────────────────────
    //
    // At the horizon the published timeline stops being advected radar and becomes the model, in one
    // 15-minute step. The reviewer measured that discontinuity at a +60 cap as **0.146** wet/dry
    // disagreement, against 0.063 between two consecutive nowcast frames and 0.046 between two
    // consecutive model frames — a rider scrubbing the timeline sees the storm lose most of its area
    // and come back a different shape. Raising the cap moves the seam to a weaker nowcast, so it
    // should shrink; this measures whether it did.
    eprintln!("seam (wet/dry disagreement at >= 0.25 mm/h between consecutive published frames):");
    let disagreement = |left: &[u8], right: &[u8]| -> f64 {
        let (mut differ, mut counted) = (0u64, 0u64);
        for (a, b) in left.iter().zip(right) {
            let wet = |code: &u8| *code != precip4::INTENSITY_NODATA && *code >= LIGHT_RAIN;
            counted += 1;
            differ += u64::from(wet(a) != wet(b));
        }
        differ as f64 / counted.max(1) as f64
    };
    for pair in nowcast_frames.windows(2) {
        eprintln!(
            "  nowcast {} -> nowcast {}: {:.3}",
            timefmt::rfc3339(pair[0].0),
            timefmt::rfc3339(pair[1].0),
            disagreement(&pair[0].1, &pair[1].1)
        );
    }
    // The seam itself, at whatever the cap is: the last nowcast frame against the model frame the
    // mosaic publishes one cadence step later.
    let step = i64::from(FRAME_STEP_MIN) * 60;
    for (valid_at, cells) in &nowcast_frames {
        let lead_min = (*valid_at - anchor.valid_at) / 60;
        let (skew, model) = model_frame(&event, valid_at + step);
        let marker = if lead_min == i64::from(NOWCAST_MAX_LEAD_MIN) { "  <- the shipped seam" } else { "" };
        eprintln!(
            "  nowcast +{lead_min}m -> model +{}m: {:.3}   (model frame {skew} s from the target instant){marker}",
            lead_min + i64::from(FRAME_STEP_MIN),
            disagreement(cells, &model)
        );
    }
    eprintln!();
}

/// **What the morph publishes, on real bytes** (#1278 r1, M1 + M5).
///
/// Job B's interpolation is the half of WXR9 with no truth ladder of its own — no pack carries GFS
/// or ICON-EU — so it is measured here on the radar frames instead, which is a *harder* test than
/// the hourly model steps it actually runs on: consecutive radar composites change shape far faster
/// than consecutive model fields. The bracket is 19:02 -> 19:32 and the target is 19:18, an instant
/// the pack has a real observation for.
///
/// Round 1 of #1278's review measured the blend this replaces, over exactly this bracket: 22.6 % of
/// wet cells carried an intensity code neither parent held, the wet fraction was 0.186 against a
/// truth of 0.166, the mean wet code was 5.53 against 6.26 — a bigger, fainter storm than either
/// parent — and **43,130 cells published dry where one advected parent had no data at all**, which
/// `OBCG_Spec.md` §3.2 forbids without exception.
///
/// The three assertions here are those three findings turned into properties. They are exact rather
/// than tolerances, because none of them is a matter of degree.
#[test]
fn a_morphed_frame_publishes_only_values_its_parent_actually_held() {
    let event = Event::read(&pack_root()).expect("the derecho pack");
    let (header, ladder) = truth_ladder(&event);
    let (width, height) = (header.width, header.height);
    assert!(ladder.len() >= 3);
    let (earlier, target, later) = (&ladder[0], &ladder[1], &ladder[2]);
    let dt = (later.valid_at - earlier.valid_at) as f64;
    let offset = (target.valid_at - earlier.valid_at) as f64;
    let midpoint_lat =
        (f64::from(header.south_lat_udeg) + (f64::from(header.height) / 2.0) * f64::from(header.cell_lat_udeg)) / 1e6;
    let params = FlowParams::for_geographic_cells(header.cell_lat_udeg, header.cell_lon_udeg, midpoint_lat);
    let motion = flow::estimate_motion(&earlier.cells, &later.cells, width, height, dt, params)
        .expect("a derecho is not a dry field");
    let span = flow::Span { dt_seconds: dt, offset_seconds: offset, wrap_x: false };
    let morphed = flow::morph(&earlier.cells, &later.cells, width, height, &motion, span);
    // Which parent the selection took, and how far it was carried — re-derived here rather than
    // read off the implementation.
    let weight = span.weight();
    let takes_later = flow::nearer_is_later(weight);
    let parent = if takes_later { &later.cells } else { &earlier.cells };
    let carried = if takes_later { offset - dt } else { offset };
    let advected_parent = flow::advect(parent, width, height, &motion, carried, false);

    eprintln!(
        "\n{EVENT_ID} morph: bracket {} -> {}, target {} (weight {weight:.2}, taking the {} parent)",
        timefmt::rfc3339(earlier.valid_at),
        timefmt::rfc3339(later.valid_at),
        timefmt::rfc3339(target.valid_at),
        if takes_later { "later" } else { "earlier" },
    );
    for (name, cells) in [
        ("truth            ", target.cells.as_slice()),
        ("morphed          ", morphed.as_slice()),
        ("parent, raw      ", parent.as_slice()),
        ("parent, advected ", advected_parent.as_slice()),
    ] {
        let (wet, missing, mean) = field_stats(cells);
        eprintln!("  {name}: wet {wet:.4}  no-data {missing:.4}  mean wet code {mean:.2}");
    }
    eprintln!(
        "  morph vs the real observation at that instant: {}",
        Scores::of(&morphed, &target.cells, width, height).row()
    );
    eprintln!(
        "  the nearest native frame instead:              {}\n",
        Scores::of(parent, &target.cells, width, height).row()
    );

    // 1. **No invented values — in the strong form** (#1278 r2, n19). Set membership over a
    //    12-code ladder is nearly free and would pass for a great many wrong implementations, so the
    //    assertion is the identity instead: a morph *is* one advection of one parent, cell for cell.
    //    Anything that combined, averaged, filled or reordered fails this outright.
    assert_eq!(morphed, advected_parent, "a morph must be exactly its nearer parent, advected — nothing else");
    // The weaker property is still worth naming, because it is the one the spec states.
    let held: std::collections::BTreeSet<u8> = parent.iter().copied().collect();
    let invented = morphed.iter().filter(|code| **code != precip4::INTENSITY_NODATA && !held.contains(code)).count();
    assert_eq!(invented, 0, "{invented} morphed cells carry an intensity the parent never held");

    // 2. **Missing stays missing.** A morphed cell is no-data exactly where its advected parent is,
    //    so a trajectory that left the domain publishes 15 and the mosaic falls through — never dry.
    let (_, morph_missing, morph_mean) = field_stats(&morphed);
    let (_, advected_missing, _) = field_stats(&advected_parent);
    assert!(morph_missing > 0.0, "a real morph must inherit the upwind blind spot, not paper over it");
    assert!(
        (morph_missing - advected_missing).abs() < 1e-9,
        "morph no-data {morph_missing} must equal its advected parent's {advected_missing}"
    );
    let dry_over_missing = morphed
        .iter()
        .zip(&advected_parent)
        .filter(|(out, source)| **source == precip4::INTENSITY_NODATA && **out == precip4::INTENSITY_DRY)
        .count();
    assert_eq!(dry_over_missing, 0, "{dry_over_missing} cells published dry where the parent had no data (§3.2)");

    // 3. **No area or intensity drift.** The blend grew the wet area and damped the mean code by
    //    about a band; a selection cannot, because what it publishes *is* a parent, moved.
    let (morph_wet, _, _) = field_stats(&morphed);
    let (parent_wet, _, parent_mean) = field_stats(parent);
    assert!(
        (morph_mean - parent_mean).abs() < 0.5,
        "mean wet code drifted from {parent_mean} to {morph_mean} — a selection must not damp intensity"
    );
    assert!(morph_wet <= parent_wet * 1.05, "the wet area grew from {parent_wet} to {morph_wet}");

    // ── Job B's own seam (#1278 r2, R2-4) ────────────────────────────────────────────────────────
    //
    // `morph` takes the earlier parent below the halfway point and the later one above it, so the
    // published timeline changes source at the middle of every bracket. `nearer_is_later` used to
    // claim that was invisible. It is not, and this is the number that replaces the claim: the
    // wet/dry step across the switch against the step within one parent, over a real hourly bracket
    // — the `dt` GFS and ICON-EU actually present. Radar overstates it against model fields, so it
    // is an upper bound on what a rider over a floor-only region sees.
    // 20:02 Z — exactly an hour after the 19:02 Z bracket start, so the `dt` is the one GFS and
    // ICON-EU actually present rather than a convenient rung.
    let hourly = ladder.iter().find(|frame| frame.valid_at - earlier.valid_at == 3_600).expect("an hourly rung");
    let hourly_dt = (hourly.valid_at - earlier.valid_at) as f64;
    let hourly_motion = flow::estimate_motion(&earlier.cells, &hourly.cells, width, height, hourly_dt, params)
        .expect("an hourly bracket over a derecho has motion");
    let frame_at = |offset: f64| {
        flow::morph(
            &earlier.cells,
            &hourly.cells,
            width,
            height,
            &hourly_motion,
            flow::Span { dt_seconds: hourly_dt, offset_seconds: offset, wrap_x: false },
        )
    };
    let step = (i64::from(FRAME_STEP_MIN) * 60) as f64;
    let disagreement = |left: &[u8], right: &[u8]| -> f64 {
        let wet = |code: &u8| *code != precip4::INTENSITY_NODATA && *code >= LIGHT_RAIN;
        left.iter().zip(right).filter(|(a, b)| wet(a) != wet(b)).count() as f64 / left.len() as f64
    };
    // The two frames either side of the halfway point, and two consecutive frames inside one parent.
    let (before, after) = (frame_at(hourly_dt / 2.0 - step / 2.0), frame_at(hourly_dt / 2.0 + step / 2.0));
    let (later_a, later_b) = (frame_at(hourly_dt / 2.0 + step / 2.0), frame_at(hourly_dt / 2.0 + 3.0 * step / 2.0));
    let across = disagreement(&before, &after);
    let within = disagreement(&later_a, &later_b);
    eprintln!(
        "job B seam over a {:.0} s bracket: {across:.4} across the parent switch, {within:.4} within one parent \
         ({:+.0} % excess)\n",
        hourly_dt,
        (across / within - 1.0) * 100.0
    );
    // Not a threshold on the excess — this is one radar event and the number belongs in the doc, not
    // in a gate. What is asserted is that the seam is the same order as the ordinary frame-to-frame
    // change rather than a different one: a switch that doubled the step would be a mechanism
    // problem, not a documentation problem.
    assert!(across < within * 2.0, "the parent switch costs {across} against {within} within a parent");
}

/// `(wet fraction, no-data fraction, mean wet code)` — the three numbers the review's M5 turns on.
fn field_stats(cells: &[u8]) -> (f64, f64, f64) {
    let (mut wet, mut missing, mut sum) = (0u64, 0u64, 0u64);
    for code in cells {
        match *code {
            precip4::INTENSITY_NODATA => missing += 1,
            code if code >= LIGHT_RAIN => {
                wet += 1;
                sum += u64::from(code);
            }
            _ => {}
        }
    }
    let total = cells.len() as f64;
    (wet as f64 / total, missing as f64 / total, if wet > 0 { sum as f64 / wet as f64 } else { 0.0 })
}

/// **`derive::radar_nowcast` on real bytes** (#1278 r1, n13).
///
/// The skill harness calls `flow::advect` directly, and the derecho pack's motion-history key
/// records a 404, so until now the derivation's own arithmetic — which observation becomes the
/// anchor, that leads run from the *observation's* instant rather than from the cycle anchor, and
/// where the horizon clips — was proved on synthetic blobs only. This drives it through real MRMS
/// composites, by handing it the pack's own truth frames as the observation pair it would otherwise
/// have fetched.
#[test]
fn the_derived_nowcast_leads_from_the_observation_on_real_composites() {
    let event = Event::read(&pack_root()).expect("the derecho pack");
    let (header, ladder) = truth_ladder(&event);
    let observed_at = ladder[1].valid_at;
    // A cycle anchored a few minutes before the observation — the ordinary case, and the one where
    // leading from the anchor instead of from the observation is wrong by most of a frame step.
    let times = CycleTimes::anchored_at(observed_at - 200);
    let as_observation = |source: &Frame| BakedFrame {
        offset_min: 0,
        valid_at: source.valid_at,
        class: SourceClass::Observation,
        cells: source.cells.clone(),
    };
    let source = BakedSource {
        id: mrms::ID,
        geometry: GridGeometry {
            south_lat_udeg: header.south_lat_udeg,
            west_lon_udeg: header.west_lon_udeg,
            cell_lat_udeg: header.cell_lat_udeg,
            cell_lon_udeg: header.cell_lon_udeg,
            width: header.width,
            height: header.height,
            cell_size_m: header.cell_size_m,
            tile_edge: header.tile_edge,
            entries_per_page: header.entries_per_page,
        },
        reference_time: observed_at,
        attribution: mrms::ATTRIBUTION,
        frames: vec![as_observation(&ladder[1])],
        motion_history: vec![as_observation(&ladder[0])],
    };
    let nowcast =
        derive::radar_nowcast(&source, times).expect("real radar has motion").expect("mrms has a nowcast row");

    assert_eq!(nowcast.id, mrms::NOWCAST.id);
    assert!(nowcast.motion_history.is_empty(), "nothing nowcasts a nowcast");
    assert_eq!(nowcast.reference_time, observed_at, "the derived source is anchored on the observation");
    let instants: Vec<i64> = nowcast.frames.iter().map(|frame| frame.valid_at).collect();
    let expected: Vec<i64> = times
        .offsets_min()
        .filter(|offset| *offset > 0 && *offset <= NOWCAST_MAX_LEAD_MIN)
        .map(|offset| times.valid_at(offset))
        .filter(|instant| *instant > observed_at)
        .collect();
    assert_eq!(instants, expected, "the nowcast must fill exactly the canonical slots inside the horizon");

    let east_of_mass = |cells: &[u8]| -> f64 {
        let (mut sum, mut mass) = (0.0f64, 0.0f64);
        for (index, code) in cells.iter().enumerate() {
            if *code == precip4::INTENSITY_NODATA || *code < LIGHT_RAIN {
                continue;
            }
            sum += f64::from(*code) * (index as u32 % header.width) as f64;
            mass += f64::from(*code);
        }
        sum / mass
    };
    for derived in &nowcast.frames {
        assert!(matches!(derived.class, SourceClass::Forecast), "a derived frame is never an observation");
        // The lead is measured from the observation, not from the cycle anchor: the two differ by
        // the observation's age, which is up to most of a frame step.
        assert_eq!(i64::from(derived.offset_min), (derived.valid_at - observed_at) / 60);
        assert_ne!(derived.cells, source.frames[0].cells, "an advected frame is not the frozen anchor");
        // …and the field really moved rather than being re-labelled: the derecho went east.
        assert!(
            east_of_mass(&derived.cells) > east_of_mass(&source.frames[0].cells),
            "the derecho moved east; f+{} did not",
            derived.offset_min
        );
    }
    assert!(
        nowcast.frames.last().expect("frames").valid_at - observed_at <= i64::from(NOWCAST_MAX_LEAD_MIN) * 60,
        "the nowcast published a frame past its horizon"
    );
}
