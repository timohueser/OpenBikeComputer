//! TEMPORARY probe (copied into tests/ to run, never committed). MAP_LAT, MAP_LON, MAP_SIZE (m),
//! MAP_STEP (m), MAP_OUT: rows [i, j, h, h, h, t, rule] over the square; h null where the rule is not
//! slope (or with MAP_ALL, terrain only).
#[path = "fixture/mod.rs"]
mod fixture;
use fixture::{default_dir, Fixture};
use obc_ground_plan::faces::heights::check::terrain_at;
use obc_ground_plan::faces::heights::height_at;
use obc_ground_plan::faces::records::{self, find};
use obc_ground_plan::faces::Rules;
use obc_ground_plan::frame::{from_lon_lat, metres_per_unit, CellKey};
use obc_ground_plan::graph;

#[test]
fn zz_map() {
    let Ok(lat) = std::env::var("MAP_LAT") else { return };
    let lat: f64 = lat.parse().unwrap();
    let lon: f64 = std::env::var("MAP_LON").unwrap().parse().unwrap();
    let size: f64 = std::env::var("MAP_SIZE").unwrap_or("20".into()).parse().unwrap();
    let step: f64 = std::env::var("MAP_STEP").unwrap_or("0.1".into()).parse().unwrap();
    let all = std::env::var("MAP_ALL").is_ok();
    let out = std::env::var("MAP_OUT").unwrap();
    let c = from_lon_lat(lon, lat);
    let key = CellKey::of(c);
    let mut fix = Fixture::open(&default_dir());
    let rules = Rules::embedded();
    let res = fix.map(&[key], |key, input| {
        let graph = graph::build(key, &input, &rules.sections);
        let words = records::build(&graph, &input, rules, key, 0, true).near.words;
        let o = key.origin();
        let mpu = metres_per_unit(c.y);
        let n = (size / step) as i64;
        let mut rows = Vec::new();
        for j in 0..n {
            for i in 0..n {
                let q = [(c.x - o.x) as f64 + ((i - n / 2) as f64 * step) / mpu, (c.y - o.y) as f64 + ((j - n / 2) as f64 * step) / mpu];
                let label = find(&words, q);
                let rule = label.map_or(0, |id| records::label(&words, id)[0] >> 24);
                let Some(t) = terrain_at(&input.terrain, key, q) else { continue };
                if rule != 1 && !(all && rule != 0) {
                    rows.push(format!("[{i},{j},null,null,null,{t:.4},{rule}]"));
                    continue;
                }
                let h = height_at(&words, q, label, t).h;
                rows.push(format!("[{i},{j},{h:.4},{h:.4},{h:.4},{t:.4},{rule}]"));
            }
        }
        (n, rows)
    });
    let (n, rows) = &res[0];
    std::fs::write(out, format!("{{\"n\":{n},\"rows\":[{}]}}", rows.join(","))).unwrap();
}
