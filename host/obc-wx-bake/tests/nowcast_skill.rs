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

use obc_formats::obcg;
use obc_wx_bake::derive::NOWCAST_MAX_LEAD_MIN;
use obc_wx_bake::flow::{self, FlowParams};
use obc_wx_bake::pack::{self, Event};
use obc_wx_bake::skill::Scores;
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
    let params = FlowParams::for_cells(f64::from(header.cell_size_m));
    let motion = flow::estimate_motion(&earlier.cells, &anchor.cells, width, height, dt, params)
        .expect("a derecho is not a dry field");

    eprintln!(
        "\n{EVENT_ID}: {width} x {height} cells of {} m, motion from {} -> {} ({dt:.0} s apart)",
        header.cell_size_m,
        timefmt::rfc3339(earlier.valid_at),
        timefmt::rfc3339(anchor.valid_at),
    );
    eprintln!(
        "peak motion {:.1} m/s; flow grid {} x {} nodes at stride {}",
        f64::from(motion.max_speed_cells_s()) * f64::from(header.cell_size_m),
        motion.cols,
        motion.rows,
        motion.stride
    );
    eprintln!("\n lead  method       CSI>=.25  CSI>=2.0  FSS>=.25  FSS>=2.0  cover");

    let mut wins_over_persistence = 0usize;
    let mut wins_over_model_inside_cap = 0usize;
    let mut leads_inside_cap = 0usize;
    for target in &ladder[2..] {
        let lead_s = target.valid_at - anchor.valid_at;
        let lead_min = lead_s / 60;
        let nowcast = flow::advect(&anchor.cells, width, height, &motion, lead_s as f64);
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
            if csi(&scored) > csi(&modelled) {
                wins_over_model_inside_cap += 1;
            }
        }
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
}
