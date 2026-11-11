//! The layer `crossings`: the water crossings' zones (outlined violet) and each crossing's letter; and per
//! place `crossings.json`: the owned crossings with type, source, place, the water's width and the way's
//! class, and the counters of the constructions.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use tiny_skia::{PathBuilder, Transform};

use crate::draw::{cell_square, paint, path, stroke, Canvas};
use crate::lon_lat;
use obc_ground_plan::debug::CROSSINGS;
use obc_ground_plan::faces::Rules;
use obc_ground_plan::frame::{CellKey, GlobalPoint};
use obc_ground_plan::graph::{self, Level};
use obc_ground_plan::water_crossings::CrossingStats;
use obc_ground_plan::CellInput;

pub fn layer(canvas: &mut Canvas, cells: &BTreeMap<CellKey, CellInput>, rules: &Rules) -> Value {
    let mut owned = Vec::new();
    let mut total = CrossingStats::default();
    for (&key, input) in cells {
        let g = graph::build(key, input, &rules.sections);
        let wc = obc_ground_plan::faces::water_crossings(&g, input, rules, key);
        let o = key.origin();
        let at = |p: [i32; 2]| GlobalPoint { x: o.x + i64::from(p[0]), y: o.y + i64::from(p[1]) };
        let mask = canvas.mask(cell_square(key));
        let ring = |canvas: &mut Canvas, r: &[[i32; 2]], rgb: (u8, u8, u8)| {
            let pts: Vec<GlobalPoint> = r.iter().map(|&p| at(p)).collect();
            if let Some(p) = path(canvas, &pts, true) {
                canvas.pixmap.stroke_path(
                    &p,
                    &paint(rgb.0, rgb.1, rgb.2, 255),
                    &stroke(2.0, true),
                    Transform::identity(),
                    None,
                );
            }
        };
        for z in &wc.zones {
            for r in &z.rings {
                ring(canvas, r, (170, 90, 230));
            }
        }
        for c in &g.crossings {
            let letter = CROSSINGS.iter().find(|x| x.0 == c.kind).map_or('?', |x| x.1);
            let (x, y) = canvas.px(at(c.pos));
            if let Some(p) = PathBuilder::from_circle(x, y, 4.0) {
                canvas.pixmap.fill_path(
                    &p,
                    &paint(0, 0, 0, 200),
                    tiny_skia::FillRule::Winding,
                    Transform::identity(),
                    mask.as_ref(),
                );
            }
            if !c.owned {
                continue;
            }
            let (lon, lat) = lon_lat(at(c.pos));
            let way = g.way_at(&c.over, Level::Ground).or(g.way_at(&c.over, Level::Deck));
            let water = g.way_at(&c.under, Level::Ground).or(g.way_at(&c.under, Level::Tunnel));
            let feature = |w: u32| &input.features[g.parts[g.ways[w as usize].part as usize].feature.0 as usize];
            owned.push(json!({
                "type": format!("{:?}", c.kind),
                "letter": letter.to_string(),
                "source": format!("{:?}", c.source),
                "lon": lon,
                "lat": lat,
                "w": water.map(|s| rules.sections.water(&feature(s).kind, &feature(s).tags).width_m),
                "class": way.map(|w| rules.sections.way(&feature(w).kind, &feature(w).tags, false).class.to_string()),
                "angle_deg": c.angle_rad.to_degrees(),
                "way_heading": way.map(|w| heading(g.samples[g.sample_near(w, c.pos) as usize].tangent)),
                "water_heading": water.map(|s| heading(g.samples[g.sample_near(s, c.pos) as usize].tangent)),
            }));
        }
        total = add(total, wc.stats);
    }
    json!({"crossings": owned, "counters": total.counters().iter().map(|(n, v)| (n.to_string(), json!(v))).collect::<serde_json::Map<_, _>>()})
}

/// LOOK STUDY: a compass heading (degrees) of a tangent (x east, y south).
fn heading(t: [f64; 2]) -> f64 {
    t[0].atan2(-t[1]).to_degrees().rem_euclid(360.0)
}

fn add(mut a: CrossingStats, b: CrossingStats) -> CrossingStats {
    a.zones += b.zones;
    a.chain_ends += b.chain_ends;
    a.water_ends += b.water_ends;
    a.joins += b.joins;
    a
}
