//! **What WXR9 costs a real cycle** (#1251: "CPU. Measure before building.")
//!
//! WXR1 (#1254) measured one full wet global cycle at **12.4 s wall on 4 threads / 398 MB peak
//! RSS**, against a five-minute budget and `MemoryMax=1G`. #1251 requires the nowcast to be
//! measured on top of that rather than assumed to fit, so this is the same shape of measurement
//! with the derivation stage in it.
//!
//! It is `#[ignore]`d: it materialises every source's window at production size and then bakes 24
//! shards x 9 frames of the global lattice, which is minutes of CI time for a number that only
//! moves when someone changes the engine. Run it deliberately, and run it under a tool that reports
//! peak RSS, because this process's high-water mark is the number that matters:
//!
//! ```text
//! cargo test -p obc-wx-bake --release --test nowcast_cost -- --ignored --nocapture
//! /usr/bin/time -l cargo test -p obc-wx-bake --release --test nowcast_cost -- --ignored --nocapture   # macOS
//! /usr/bin/time -v ...                                                                                # Linux
//! ```
//!
//! The synthetic fields are not random noise. A field of independent random cells has no spatial
//! structure, which would make the flow estimator's job impossible and the deflate codec's job
//! trivial — both in the wrong direction. These are a coarse random field smoothly upsampled and
//! translated between frames, which gives roughly the wet-tile fraction WXR1 measured (30-52 %) and
//! a motion field the estimator can actually find.

use std::time::Instant;

use obc_wx_bake::canonical::{bake_cycle, CycleTimes, Mosaic, BAKE_THREADS, CANONICAL, CYCLE_FRAMES};
use obc_wx_bake::derive;
use obc_wx_bake::geometry::GridGeometry;
use obc_wx_bake::source::{
    dwd_rv, gfs, hrrr, icon_eu, mrms, opera, opera_cirrus, opera_nimbus, Attribution, BakedFrame, BakedSource,
    SourceClass,
};

/// A deterministic value noise field, translated by `(dx, dy)` cells — one frame of a moving storm
/// pattern. Coarse cells of ~48 source cells, bilinearly smoothed, thresholded so most of the
/// domain is dry and the wet part has a rate gradient.
fn field(window: &GridGeometry, dx: f64, dy: f64, seed: u64) -> Vec<u8> {
    let coarse = 48.0;
    let hash = |x: i64, y: i64| -> f64 {
        let mut value = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
        value ^= value >> 29;
        value = value.wrapping_mul(0xBF58_476D_1CE4_E5B9) ^ seed.wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^= value >> 32;
        (value & 0xFFFF) as f64 / 65_535.0
    };
    let smooth = |x: f64, y: f64| -> f64 {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (ex, ey) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
        let (x0, y0) = (x0 as i64, y0 as i64);
        let bottom = hash(x0, y0) * (1.0 - ex) + hash(x0 + 1, y0) * ex;
        let top = hash(x0, y0 + 1) * (1.0 - ex) + hash(x0 + 1, y0 + 1) * ex;
        bottom * (1.0 - ey) + top * ey
    };
    let mut cells = Vec::with_capacity(window.cells());
    for row in 0..window.height {
        for col in 0..window.width {
            let x = (f64::from(col) - dx) / coarse;
            let y = (f64::from(row) - dy) / coarse;
            // Two octaves, so the field has both systems and cores inside them.
            let value = smooth(x, y) * 0.7 + smooth(x * 3.1, y * 3.1) * 0.3;
            cells.push(if value < 0.58 { 0 } else { (((value - 0.58) / 0.42) * 12.0).round() as u8 });
        }
    }
    cells
}

fn frames(
    window: &GridGeometry,
    count: u32,
    step_s: i64,
    first: i64,
    class: SourceClass,
    seed: u64,
) -> Vec<BakedFrame> {
    (0..count)
        .map(|index| {
            let valid_at = first + i64::from(index) * step_s;
            // 15 m/s eastward and 4 m/s northward, in this window's own cells.
            let seconds = (valid_at - first) as f64;
            let per_cell = f64::from(window.cell_size_m).max(1.0);
            BakedFrame {
                offset_min: (i64::from(index) * step_s / 60) as u32,
                valid_at,
                class,
                cells: field(window, 15.0 * seconds / per_cell, 4.0 * seconds / per_cell, seed),
            }
        })
        .collect()
}

fn source(
    id: &'static str,
    window: GridGeometry,
    reference_time: i64,
    frames: Vec<BakedFrame>,
    motion_history: Vec<BakedFrame>,
) -> BakedSource {
    BakedSource {
        id,
        geometry: window,
        reference_time,
        attribution: Attribution { text: "synthetic", url: "https://example.invalid" },
        frames,
        motion_history,
    }
}

/// Every source the production cycle bakes, at its real window and its real frame cadence.
fn production_sources(now: i64) -> Vec<BakedSource> {
    let run = now - 3_600;
    vec![
        // DWD RV: a run five minutes off the quarter hour, which is where RV's runs really are, and
        // members **selected onto the canonical instants** exactly as `dwd_rv::selected_leads` does
        // since #1278's M4. So there is nothing here for `derive::uniform_frames` to morph — which
        // is the point, and the harness would report it if the selection regressed.
        source(
            dwd_rv::ID,
            dwd_rv::GEOMETRY,
            now - 300,
            frames(&dwd_rv::GEOMETRY, 9, 900, now, SourceClass::Forecast, 1),
            Vec::new(),
        ),
        // MRMS: one observation, plus the motion-history frame WXR9 fetches.
        source(
            mrms::ID,
            mrms::GEOMETRY,
            now - 120,
            frames(&mrms::GEOMETRY, 1, 120, now - 120, SourceClass::Observation, 2),
            frames(&mrms::GEOMETRY, 1, 120, now - 120 - mrms::MOTION_LAG_SECONDS, SourceClass::Observation, 2),
        ),
        source(
            opera_cirrus::ID,
            opera_cirrus::CONTRACT.geometry(),
            now - 300,
            frames(&opera::WINDOW, 1, 300, now - 300, SourceClass::Observation, 3),
            frames(&opera::WINDOW, 1, 300, now - 300 - opera_cirrus::MOTION_LAG_SECONDS, SourceClass::Observation, 3),
        ),
        source(
            opera_nimbus::ID,
            opera_nimbus::CONTRACT.geometry(),
            now - 900,
            frames(&opera::WINDOW, 1, 900, now - 900, SourceClass::Observation, 4),
            Vec::new(),
        ),
        source(
            hrrr::ID,
            hrrr::GEOMETRY,
            run,
            frames(&hrrr::GEOMETRY, hrrr::LEADS_MIN.len() as u32, 900, run + 900, SourceClass::Forecast, 5),
            Vec::new(),
        ),
        source(
            icon_eu::ID,
            icon_eu::GEOMETRY,
            run,
            frames(&icon_eu::GEOMETRY, icon_eu::LEADS_H.len() as u32, 3_600, run + 3_600, SourceClass::Forecast, 6),
            Vec::new(),
        ),
        source(
            gfs::ID,
            gfs::GEOMETRY,
            run,
            frames(&gfs::GEOMETRY, gfs::LEADS_H.len() as u32, 3_600, run + 3_600, SourceClass::Forecast, 7),
            Vec::new(),
        ),
    ]
}

fn megabytes(cells: usize) -> f64 {
    cells as f64 / 1_048_576.0
}

/// This process's resident set right now, in MB, via `ps`.
///
/// Shelling out rather than calling `getrusage`, because the alternative is a `libc` dependency in
/// a crate that has deliberately never had one, for a number this test prints and nothing reads.
/// The **peak** still comes from `/usr/bin/time`; these are the stage markers that say where it was
/// reached.
fn rss_mb() -> String {
    let pid = std::process::id().to_string();
    match std::process::Command::new("ps").args(["-o", "rss=", "-p", &pid]).output() {
        Ok(output) => match String::from_utf8_lossy(&output.stdout).trim().parse::<f64>() {
            Ok(kilobytes) => format!("{:.0} MB rss", kilobytes / 1024.0),
            Err(_) => "rss unavailable".into(),
        },
        Err(_) => "rss unavailable".into(),
    }
}

#[test]
#[ignore = "materialises every source at production size and bakes the global lattice; run deliberately"]
fn a_full_cycle_at_production_scale() {
    // **On a quarter hour** (#1278 r1, n9). `CycleTimes::anchored_at` floors to the cadence, so an
    // arbitrary `now` puts the synthetic HRRR steps a few hundred seconds off every canonical
    // instant and the harness charges job B nine interpolations HRRR would never need: in
    // production its run is on the hour and its 15-minute leads land exactly on the quarter hours.
    // That inflated the very measurement the horizon argument rests on, in the safe direction but
    // confusingly. DWD RV is deliberately *not* aligned — its run really is on a five-minute
    // boundary, and since #1278's M4 the adapter selects its members by canonical instant anyway.
    let now = 1_760_000_000i64 / 900 * 900;
    let times = CycleTimes::anchored_at(now);
    assert_eq!(times.reference_time, now, "the harness must not be measuring an off-cadence anchor");

    let built = Instant::now();
    let sources = production_sources(now);
    let resident: usize = sources
        .iter()
        .map(|source| source.frames.iter().chain(&source.motion_history).map(|frame| frame.cells.len()).sum::<usize>())
        .sum();
    eprintln!(
        "\nsources built in {:.1} s: {:.0} MB of cells across {} layers",
        built.elapsed().as_secs_f64(),
        megabytes(resident),
        sources.len()
    );
    eprintln!("  after building the sources: {}", rss_mb());

    // The same pinned pool `run_cycle` derives on, so this box's core count does not flatter a
    // number the 4-vCPU production VPS has to live with.
    let pool = rayon::ThreadPoolBuilder::new().num_threads(BAKE_THREADS).build().expect("a derive pool");
    let derivation = Instant::now();
    let (sources, report) = pool.install(|| derive::derive_sources(sources, times));
    let derive_s = derivation.elapsed().as_secs_f64();
    let after: usize =
        sources.iter().map(|source| source.frames.iter().map(|frame| frame.cells.len()).sum::<usize>()).sum();
    eprintln!("derive: {derive_s:.2} s, cells resident {:.0} -> {:.0} MB", megabytes(resident), megabytes(after));
    for line in report.lines() {
        eprintln!("{line}");
    }
    eprintln!("  after deriving: {}", rss_mb());
    eprintln!(
        "  nowcast horizon {} min, so {} forward frames per radar layer",
        derive::NOWCAST_MAX_LEAD_MIN,
        derive::NOWCAST_MAX_LEAD_MIN / 15
    );

    let mosaic = Mosaic::from_sources(sources).expect("every layer is ranked");
    eprintln!("  entering the bake: {}", rss_mb());
    let baking = Instant::now();
    let (mut objects, mut bytes) = (0usize, 0u64);
    bake_cycle(&CANONICAL, &mosaic, times, BAKE_THREADS, &mut |object| {
        objects += 1;
        bytes += object.bytes.len() as u64;
        Ok(())
    })
    .expect("the cycle bakes");
    let bake_s = baking.elapsed().as_secs_f64();
    eprintln!(
        "bake: {bake_s:.1} s on {BAKE_THREADS} threads, {objects} objects / {:.1} MB ({CYCLE_FRAMES} frames x {} shards)",
        bytes as f64 / 1e6,
        CANONICAL.shard_count()
    );
    eprintln!("  after the bake: {}", rss_mb());
    eprintln!("TOTAL derive + bake: {:.1} s wall on {BAKE_THREADS} threads\n", derive_s + bake_s);
}
