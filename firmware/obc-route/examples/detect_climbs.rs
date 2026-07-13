//! Eyeball harness for climb detection (#507): run [`RouteReader::detect_climbs`] over a real
//! route file and print the detected climbs as a table, so the five tuning knobs
//! ([`MIN_GAIN`], [`MIN_AVG_GRADE`], [`MAX_DROP`], [`MAX_FLAT`], [`MIN_LEN`] in
//! `obc_route::climb`) can be tuned against real komoot exports before they're locked in.
//!
//! ```text
//! cargo run -p obc-route --example detect_climbs -- <path-to-route.gpx | route.obcr>
//! ```
//!
//! Accepts either a **GPX** (komoot exports these — converted to `.obcr` bytes in memory via the
//! same converter the device uses, so detection sees the exact decimated geometry that would be
//! stored) or an already-packed **`.obcr`**. Prints one row per climb — index, start/end km,
//! length, gain, average grade, and top elevation — plus the count and the header's total ascent
//! for comparison.
//!
//! Host-only (std): the lib is `no_std`, but an example compiles for the host, so a plain
//! `Vec<u8>` [`ByteSink`] and a slice [`ByteSource`] are all the plumbing it needs.
//!
//! NOTE: this reads an arbitrary path — point it at your own routes (the repo's `test_routes/`
//! is personal ride data and gitignored). Don't commit route files through this harness.

use std::path::Path;

use obc_route::climb::{MAX_DROP, MAX_FLAT, MIN_AVG_GRADE, MIN_GAIN, MIN_LEN};
use obc_route::{ByteSink, Climbs, Error, RouteIndex, RouteReader, SliceSource};

/// A `ByteSink` over a growable `Vec` — the host's "write the whole file to RAM" backing, so the
/// in-memory GPX→OBCR conversion has somewhere to land (the device uses a FatFs-backed sink).
#[derive(Default)]
struct VecSink(Vec<u8>);

impl ByteSink for VecSink {
    fn write(&mut self, b: &[u8]) -> Result<(), Error> {
        self.0.extend_from_slice(b);
        Ok(())
    }
    fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), Error> {
        let o = off as usize;
        self.0[o..o + b.len()].copy_from_slice(b);
        Ok(())
    }
}

/// Load a route file into `.obcr` bytes: a `.gpx` is converted in memory; a `.obcr` (or anything
/// else that already starts with the OBCR magic) is used as-is.
fn load_obcr(path: &str) -> Vec<u8> {
    let raw = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("error: could not read {path}: {e}");
        std::process::exit(1);
    });

    let is_gpx = Path::new(path).extension().is_some_and(|e| e.eq_ignore_ascii_case("gpx"))
        || raw.starts_with(b"<?xml")
        || raw.windows(4).take(64).any(|w| w == b"<gpx");
    if !is_gpx {
        return raw; // assume already-packed .obcr
    }

    // GPX → OBCR through the same converter the packer/device use, so we detect on the exact
    // decimated geometry that would be stored on the card.
    let name = Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("route");
    let src = SliceSource(&raw);
    let mut sink = VecSink::default();
    obc_route::gpx_to_obcr(&src, name, &mut sink).unwrap_or_else(|e| {
        eprintln!("error: GPX → OBCR conversion failed: {e:?}");
        std::process::exit(1);
    });
    sink.0
}

/// Print the climbs as an aligned table plus a summary line.
fn print_table(name: &str, climbs: &Climbs, total_ascent_m: u32) {
    println!("route: {name}");
    println!(
        "knobs: MIN_GAIN={MIN_GAIN} m  MIN_AVG_GRADE={MIN_AVG_GRADE} %  MAX_DROP={MAX_DROP} m  \
         MAX_FLAT={MAX_FLAT} m  MIN_LEN={MIN_LEN} m"
    );
    println!();
    println!("  #   start_km   end_km   len_km   gain_m   grade_%   top_m");
    println!("  --  --------   ------   ------   ------   -------   -----");
    let mut total_gain = 0u32;
    for (i, c) in climbs.as_slice().iter().enumerate() {
        total_gain += c.gain_m as u32;
        println!(
            "  {:>2}  {:>8.2}  {:>7.2}  {:>7.2}  {:>7}  {:>8}  {:>6}",
            i,
            c.start_m as f64 / 1000.0,
            c.end_m as f64 / 1000.0,
            c.len_m() as f64 / 1000.0,
            c.gain_m,
            c.avg_grade_pct,
            c.top_ele_m,
        );
    }
    println!();
    println!(
        "{} climb(s); detected gain {} m of {} m header ascent ({}%)",
        climbs.len(),
        total_gain,
        total_ascent_m,
        (total_gain * 100).checked_div(total_ascent_m).unwrap_or(0),
    );
}

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: cargo run -p obc-route --example detect_climbs -- <route.gpx | route.obcr>");
        std::process::exit(2);
    };

    let bytes = load_obcr(&path);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap_or_else(|e| {
        eprintln!("error: not a valid OBCR route: {e:?}");
        std::process::exit(1);
    });
    let reader = RouteReader::new(&ridx, &src);

    let climbs = reader.detect_climbs();
    print_table(reader.name(), &climbs, reader.total_ascent_m);
}
