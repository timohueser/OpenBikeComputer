//! `requests`: the request bytes of fixture cells, with their native digests and build times, for
//! check 7 (`builder/ground-plan-wasm/check.mjs`) and the timing page. A cell whose far-only build
//! gives another far block than its full build, or whose far block breaks `far_matches_near`, is an
//! error; a cell whose far block the crate refuses is noted and counted.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use obc_ground_plan::cell::{build_cell, far_request, STAGES};
use obc_ground_plan::faces::records::check::far_matches_near;
use obc_ground_plan::frame::CellKey;
use obc_ground_plan::terrain::Terrain;
use obc_ground_plan::tile::TileCache;
use obc_ground_plan::transport::{fnv, Flat, Request};
use serde_json::{json, Value};

use crate::fixture::{self, Fixture};

/// The side of the square about a place whose cells a place names.
const PLACE_M: f64 = 128.0;

/// The fixture files of a cell's inputs as the viewer loads them: feature tiles (gzip) and terrain
/// tiles (WebP), for the timing page's `prepare`.
fn sources(dir: &Path, key: CellKey) -> Value {
    let path = |p: PathBuf| p.is_file().then(|| p.display().to_string());
    let tiles: Vec<Value> = (key.feature_tiles().into_iter())
        .filter_map(|(c, r)| Some(json!([c, r, path(dir.join(format!("features/13/{c}/{r}.bin")))?])))
        .collect();
    let mut stored: Vec<(u32, u32, u32)> =
        Terrain::wanted(key).into_iter().map(|p| (p.tile().0, p.tile().1, p.tile().2)).collect();
    stored.sort();
    stored.dedup();
    let terrain: Vec<Value> = (stored.into_iter())
        .filter_map(|(z, x, y)| Some(json!([z, x, y, path(dir.join(format!("terrain/{z}/{x}/{y}.webp")))?])))
        .collect();
    json!({"tiles": tiles, "terrain": terrain})
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

pub fn run() -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let (mut out, mut places, mut all, mut runs) = (None::<PathBuf>, Vec::new(), false, 5usize);
    let mut views = root.join("builder/web/tests/browser/trailview/road-views-top.json");
    let mut dir = fixture::default_dir();
    let mut args = std::env::args().skip(2);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--out" => out = Some(value()?.into()),
            "--places" => places = value()?.split(',').map(String::from).collect(),
            "--all" => all = true,
            "--runs" => runs = value()?.parse().map_err(|_| "--runs: a count")?,
            "--views" => views = value()?.into(),
            "--fixture" => dir = value()?.into(),
            _ => return Err(format!("requests: unknown option {arg}")),
        }
    }
    let out = out.ok_or("requests --out DIR (--places NAMES | --all) [--runs N]")?;
    if all == !places.is_empty() || runs == 0 {
        return Err("requests: give --places or --all, and --runs of 1 or more".into());
    }
    let mut fix = Fixture::open(&dir);
    let mut cells: BTreeMap<CellKey, Vec<String>> = BTreeMap::new();
    if all {
        cells.extend(fix.cells().into_iter().map(|k| (k, Vec::new())));
    }
    for (name, lat, lon) in crate::named_places(&views, &places)? {
        for key in crate::square_cells(lat, lon, PLACE_M) {
            cells.entry(key).or_default().push(name.clone());
        }
    }
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let rules = obc_ground_plan::faces::Rules::embedded();
    let start = Instant::now();
    let mut clock = || start.elapsed().as_secs_f64() * 1e3;
    let fixture_dir = std::fs::canonicalize(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let (mut index, mut refused, mut seen) = (Vec::new(), 0, BTreeMap::<&str, usize>::new());
    for (key, names) in cells {
        let bytes = fix.request(key).encode();
        let file = format!("{}-{}.bin", key.x, key.y);
        crate::write(&out.join(&file), &bytes)?;
        let builds = (0..runs)
            .map(|_| build_cell(&bytes, rules, &mut clock).map_err(|e| format!("cell {}/{}: {e}", key.x, key.y)))
            .collect::<Result<Vec<_>, _>>()?;
        let last = &builds[runs - 1];
        let fail = |e: String| format!("cell {}/{}: {e}", key.x, key.y);
        let digest = Flat::of(last).digest(last).map_err(fail)?;
        // The far-only build as `build_cell` does a full one: from the bytes, with a new tile cache.
        let mut far_ms = Vec::with_capacity(runs);
        for _ in 0..runs {
            let start = clock();
            let alone = far_request(Request::decode(&bytes).map_err(fail)?, &mut TileCache::new(4), rules);
            far_ms.push(clock() - start);
            if alone != last.far {
                return Err(fail("the far-only build gives another far block".into()));
            }
        }
        let (far, far_bytes, counts) = match &last.far {
            Ok(words) => {
                let (faults, counts) = far_matches_near(&last.records, words, key);
                if !faults.is_empty() {
                    return Err(fail(format!(
                        "{} far block faults: {:?}",
                        faults.len(),
                        &faults[..faults.len().min(4)]
                    )));
                }
                counts.iter().for_each(|(&case, &n)| *seen.entry(case).or_default() += n);
                let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
                (fnv(&bytes), bytes.len(), json!(counts))
            }
            Err(e) => {
                refused += 1;
                eprintln!("cell {}/{}: the far block is refused: {e}", key.x, key.y);
                (format!("refused: {e}"), 0, Value::Null)
            }
        };
        let times: Vec<&Vec<f64>> = builds.iter().map(|b| &b.times).collect();
        let mut native: serde_json::Map<String, Value> = (STAGES.iter().enumerate())
            .map(|(i, stage)| (stage.to_string(), json!(median(times.iter().map(|t| t[i]).collect()))))
            .collect();
        native.insert("total".into(), json!(median(times.iter().map(|t| t.iter().sum()).collect())));
        native.insert("far".into(), json!(median(far_ms)));
        index.push(json!({
            "cell": [key.x, key.y], "file": file, "places": names, "request": fnv(&bytes), "digest": digest,
            "far": far, "far_bytes": far_bytes, "far_check": counts, "native": native, "sources": sources(&fixture_dir, key),
        }));
    }
    crate::write(&out.join("index.json"), serde_json::to_string_pretty(&index).unwrap().as_bytes())?;
    let folder = std::fs::canonicalize(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("{} requests -> {}", index.len(), out.join("index.json").display());
    println!("far blocks: {refused} refused; the check saw {seen:?}");
    println!("timing page: http://<host>:5173/plan-timing.html?requests=/@fs{}/", folder.display());
    Ok(())
}
