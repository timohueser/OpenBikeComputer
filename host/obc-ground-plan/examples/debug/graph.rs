//! The `graph` layer of the debug pictures, the junction census and the build bench of the way graph.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use obc_ground_plan::debug as palette;
use obc_ground_plan::frame::{metres_per_unit, CellKey, GlobalPoint};
use obc_ground_plan::graph::{build, CrossingKind, Family, Level, SampleKind, WayGraph};
use obc_ground_plan::sections::Sections;
use serde_json::{json, Value};
use tiny_skia::{FillRule, PathBuilder, Rect, Stroke, StrokeDash, Transform};

use crate::draw::{cell_square, paint, path, stroke, Canvas};
use crate::fixture::{feature_tile, Fixture};

fn rgb((r, g, b): palette::Rgb, a: u8) -> tiny_skia::Paint<'static> {
    paint(r, g, b, a)
}

/// 5 × 7 glyphs, a row per byte, the high bit of the 5 at the left.
const GLYPHS: [(CrossingKind, char, [u8; 7]); 9] = [
    (CrossingKind::Deck, 'D', [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e]),
    (CrossingKind::Culvert, 'C', [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e]),
    (CrossingKind::Ford, 'F', [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10]),
    (CrossingKind::PipeInStones, 'P', [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10]),
    (CrossingKind::SteppingStones, 'S', [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e]),
    (CrossingKind::Footbridge, 'B', [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e]),
    (CrossingKind::LevelCrossing, 'L', [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f]),
    (CrossingKind::Stacked, 'X', [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11]),
    (CrossingKind::Bore, 'T', [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04]),
];

/// The levels of each cell, under the cell's mask: the runs of each way with a state by colour (DECK
/// blue, BORE violet, OPEN grey, HIDDEN red), wider by storey; the end lines black (A), white
/// (portal) or red (closed), thicker where merged.
pub fn levels(canvas: &mut Canvas, graphs: &BTreeMap<CellKey, WayGraph>) {
    use obc_ground_plan::graph::{EndKind, State};
    for (&key, g) in graphs {
        let Some(mask) = canvas.mask(cell_square(key)) else { continue };
        let o = key.origin();
        let global = |p: [i32; 2]| GlobalPoint { x: o.x + i64::from(p[0]), y: o.y + i64::from(p[1]) };
        for r in &g.levels.runs {
            let way = &g.ways[r.way as usize];
            let inside = |s: &&obc_ground_plan::graph::Sample| {
                (s.span, s.u) >= (r.from.span, r.from.u) && (s.span, s.u) <= (r.to.span, r.to.u)
            };
            let samples = &g.samples[way.samples.start as usize..way.samples.end as usize];
            let points: Vec<GlobalPoint> = samples.iter().filter(inside).map(|s| global(s.pos)).collect();
            let colour = match r.state {
                State::Deck => paint(30, 90, 230, 200),
                State::Bore => paint(140, 40, 200, 200),
                State::Open => paint(120, 120, 120, 200),
                State::Hidden => paint(230, 30, 30, 200),
            };
            let storey = r.sheet.map_or(0, |s| g.levels.sheets[s as usize].storey.unsigned_abs());
            if let Some(p) = path(canvas, &points, false) {
                let width = 6.0 + 3.0 * f32::from(storey);
                canvas.pixmap.stroke_path(&p, &colour, &stroke(width, false), Transform::identity(), Some(&mask));
            }
        }
        for l in &g.levels.end_lines {
            let colour = match l.kind {
                EndKind::A => paint(0, 0, 0, 255),
                EndKind::Portal => paint(255, 255, 255, 255),
                EndKind::Closed => paint(230, 30, 30, 255),
            };
            let width = if l.ways.len() > 1 { 4.0 } else { 2.0 };
            if let Some(p) = path(canvas, &[global(l.ends[0]), global(l.ends[1])], false) {
                canvas.pixmap.stroke_path(&p, &colour, &stroke(width, false), Transform::identity(), Some(&mask));
            }
        }
    }
}

/// The graph of each cell, drawn under the cell's mask: samples by family (dashed on a deck, dotted in
/// a tunnel, a tick at a cell line), links red, synthetic decks orange, host arms thick and guest arms
/// thin for 8 m, joints as dots, ends as rings, a crossing as its letter.
pub fn layer(canvas: &mut Canvas, graphs: &BTreeMap<CellKey, WayGraph>) {
    for (&key, g) in graphs {
        let Some(mask) = canvas.mask(cell_square(key)) else { continue };
        let o = key.origin();
        let global = |p: [i32; 2]| GlobalPoint { x: o.x + i64::from(p[0]), y: o.y + i64::from(p[1]) };
        let synthetic: BTreeSet<u32> = g.decks.iter().filter(|d| d.synthetic).flat_map(|d| d.ways.clone()).collect();
        for way in &g.ways {
            let samples = &g.samples[way.samples.start as usize..way.samples.end as usize];
            let points: Vec<GlobalPoint> = samples.iter().map(|s| global(s.pos)).collect();
            let (r, gr, b) = match (way.link, synthetic.contains(&way.id)) {
                (Some(_), _) => palette::LINK,
                (_, true) => palette::SYNTHETIC_DECK,
                _ => palette::family_colour(way.family),
            };
            let mut line = Stroke { width: 2.5, ..stroke(2.5, false) };
            line.dash = match way.level {
                Level::Deck => StrokeDash::new(vec![7.0, 4.0], 0.0),
                Level::Tunnel => StrokeDash::new(vec![1.5, 3.5], 0.0),
                Level::Ground => None,
            };
            if let Some(p) = path(canvas, &points, false) {
                canvas.pixmap.stroke_path(&p, &paint(r, gr, b, 230), &line, Transform::identity(), Some(&mask));
            }
            for s in samples.iter().filter(|s| s.kind == SampleKind::CellLine) {
                let (x, y) = canvas.px(global(s.pos));
                let (nx, ny) = (-s.tangent[1] as f32 * 6.0, s.tangent[0] as f32 * 6.0);
                let mut tick = PathBuilder::new();
                tick.move_to(x - nx, y - ny);
                tick.line_to(x + nx, y + ny);
                if let Some(t) = tick.finish() {
                    canvas.pixmap.stroke_path(
                        &t,
                        &paint(0, 0, 0, 255),
                        &stroke(1.5, false),
                        Transform::identity(),
                        Some(&mask),
                    );
                }
            }
        }
        let arm_m = canvas.metres(8.0);
        for j in &g.junctions {
            let (x, y) = canvas.px(global(j.pos));
            for arm in &j.arms {
                let mut a = PathBuilder::new();
                a.move_to(x, y);
                a.line_to(x + arm.heading[0] as f32 * arm_m, y + arm.heading[1] as f32 * arm_m);
                let Some(a) = a.finish() else { continue };
                let (width, colour) =
                    if arm.host { (7.0, rgb(palette::JUNCTION, 170)) } else { (3.0, rgb(palette::GUEST_ARM, 200)) };
                canvas.pixmap.stroke_path(&a, &colour, &stroke(width, false), Transform::identity(), Some(&mask));
            }
            if let Some(dot) = PathBuilder::from_circle(x, y, 4.0) {
                canvas.pixmap.fill_path(
                    &dot,
                    &rgb(palette::JUNCTION, 255),
                    FillRule::Winding,
                    Transform::identity(),
                    Some(&mask),
                );
            }
        }
        for j in &g.joints {
            let (x, y) = canvas.px(global(j.pos));
            if let Some(dot) = PathBuilder::from_circle(x, y, 3.0) {
                canvas.pixmap.fill_path(
                    &dot,
                    &rgb(palette::JOINT, 255),
                    FillRule::Winding,
                    Transform::identity(),
                    Some(&mask),
                );
            }
        }
        for e in &g.ends {
            let (x, y) = canvas.px(global(e.pos));
            if let Some(ring) = PathBuilder::from_circle(x, y, 6.0) {
                canvas.pixmap.stroke_path(
                    &ring,
                    &rgb(palette::END, 255),
                    &stroke(2.0, false),
                    Transform::identity(),
                    Some(&mask),
                );
            }
        }
        for c in &g.crossings {
            let (x, y) = canvas.px(global(c.pos));
            let glyph = GLYPHS.iter().find(|(k, ..)| *k == c.kind).map(|g| g.2).unwrap_or([0; 7]);
            let cell = 3.0;
            if let Some(back) = Rect::from_xywh(x - 3.5 * cell, y - 4.5 * cell, 7.0 * cell, 9.0 * cell) {
                canvas.pixmap.fill_rect(back, &rgb(palette::CROSSING, 230), Transform::identity(), Some(&mask));
            }
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..5 {
                    if bits >> (4 - col) & 1 == 1 {
                        let px = x - 2.5 * cell + col as f32 * cell;
                        let py = y - 3.5 * cell + row as f32 * cell;
                        if let Some(r) = Rect::from_xywh(px, py, cell, cell) {
                            canvas.pixmap.fill_rect(r, &paint(0, 0, 0, 255), Transform::identity(), Some(&mask));
                        }
                    }
                }
            }
        }
    }
}

/// The fixture's cells whose feature tiles it holds.
pub fn fixture_cells(fixture: &Fixture, dir: &std::path::Path) -> Vec<CellKey> {
    (fixture.cells().into_iter())
        .filter(|k| k.feature_tiles().iter().all(|&(c, r)| feature_tile(dir, c, r).is_some()))
        .collect()
}

/// The distance of q outside the convex hull of the points: 0 inside.
fn outside_hull(points: &[[f64; 2]], q: [f64; 2]) -> f64 {
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut sorted = points.to_vec();
    sorted.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    let mut hull: Vec<[f64; 2]> = Vec::new();
    for order in [sorted.clone(), sorted.iter().rev().copied().collect()] {
        let start = hull.len();
        for p in order {
            while hull.len() >= start + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    let n = hull.len();
    if n >= 3 && (0..n).all(|i| cross(hull[i], hull[(i + 1) % n], q) >= 0.0) {
        return 0.0;
    }
    let distance = |a: [f64; 2], b: [f64; 2]| {
        let (d, v) = ([b[0] - a[0], b[1] - a[1]], [q[0] - a[0], q[1] - a[1]]);
        let length2 = d[0] * d[0] + d[1] * d[1];
        let t = if length2 > 0.0 { ((v[0] * d[0] + v[1] * d[1]) / length2).clamp(0.0, 1.0) } else { 0.0 };
        ((v[0] - t * d[0]) * (v[0] - t * d[0]) + (v[1] - t * d[1]) * (v[1] - t * d[1])).sqrt()
    };
    (0..n).map(|i| distance(hull[i], hull[(i + 1) % n])).fold(f64::INFINITY, f64::min)
}

/// Per water span that starts in its cell: how far, in metres, its curve leaves the convex hull of
/// its chord and the chords before and after it; how far it runs past its chord's ends; and its
/// largest distance from its chord over the chord's length.
fn water_overshoot(g: &WayGraph) -> Vec<([f64; 3], GlobalPoint)> {
    let mut out = Vec::new();
    for way in g.ways.iter().filter(|w| w.family == Family::Water) {
        let part = &g.parts[way.part as usize];
        let v = &part.vertices;
        let last = if way.to.u > 0.0 { way.to.span + 1 } else { way.to.span };
        for k in way.from.span as usize..last as usize {
            let p0 = v[k].pos;
            if !g.cell.contains(GlobalPoint { x: p0[0], y: p0[1] }) {
                continue;
            }
            let rel = |i: usize| [(v[i].pos[0] - p0[0]) as f64, (v[i].pos[1] - p0[1]) as f64];
            let near: Vec<[f64; 2]> = (k.saturating_sub(1)..=(k + 2).min(v.len() - 1)).map(rel).collect();
            let chord = rel(k + 1);
            let c = (chord[0] * chord[0] + chord[1] * chord[1]).sqrt();
            let unit = [chord[0] / c, chord[1] / c];
            let mpu = metres_per_unit(p0[1]);
            let mut worst = [0.0f64; 3];
            for j in 0..=64 {
                let q = obc_ground_plan::graph::point(part, k, j as f64 / 64.0).d;
                let along = q[0] * unit[0] + q[1] * unit[1];
                let side = (unit[0] * q[1] - unit[1] * q[0]).abs();
                worst[0] = worst[0].max(outside_hull(&near, q) * mpu);
                worst[1] = worst[1].max((-along).max(along - c).max(0.0) * mpu);
                worst[2] = worst[2].max(side / c);
            }
            out.push((worst, GlobalPoint { x: p0[0], y: p0[1] }));
        }
    }
    out
}

/// Every owned junction of the fixture by (host classes, guest class), the host rules, junction
/// pairs closer than 2 m and 10 m, and the counters of `GraphStats` summed over the cells.
pub fn census(graphs: &BTreeMap<CellKey, WayGraph>, sections: &Sections) -> Value {
    let name = |g: &WayGraph, way: u32| {
        let w = &g.ways[way as usize];
        sections.rank_name(w.family == Family::Water, w.class as usize).to_string()
    };
    let mut kinds: BTreeMap<(String, String), u32> = BTreeMap::new();
    let mut rules: BTreeMap<String, u32> = BTreeMap::new();
    let (mut close_2, mut close_10, mut implicit, mut owned) = (0, 0, 0, 0);
    let mut stats: BTreeMap<String, u64> = BTreeMap::new();
    let mut crossings: BTreeMap<String, u32> = BTreeMap::new();
    let mut owned_points = [0usize; 2];
    let mut overshoot: Vec<([f64; 3], GlobalPoint)> = Vec::new();
    for g in graphs.values() {
        for j in g.junctions.iter().filter(|j| j.owned) {
            owned += 1;
            implicit += u32::from(j.implicit);
            *rules.entry(format!("{:?}", j.host_rule)).or_default() += 1;
            let mut hosts: Vec<String> = j.arms.iter().filter(|a| a.host).map(|a| name(g, a.way)).collect();
            hosts.sort();
            for arm in j.arms.iter().filter(|a| !a.host) {
                *kinds.entry((hosts.join("+"), name(g, arm.way))).or_default() += 1;
            }
        }
        for (i, a) in g.junctions.iter().enumerate() {
            for b in &g.junctions[i + 1..] {
                let lower = if a.pos <= b.pos { a } else { b };
                let (dx, dy) = ((a.pos[0] - b.pos[0]) as f64, (a.pos[1] - b.pos[1]) as f64);
                let d = (dx * dx + dy * dy).sqrt() * metres_per_unit(g.cell.origin().y);
                if lower.owned && d < 10.0 {
                    close_10 += 1;
                    close_2 += u32::from(d < 2.0);
                }
            }
        }
        for (key, value) in g.stats.counters() {
            *stats.entry(key.to_string()).or_default() += u64::from(value);
        }
        for (&(kind, source), &n) in &g.stats.crossings {
            *stats.entry(format!("crossings {kind:?} {source:?}")).or_default() += u64::from(n);
        }
        for c in g.crossings.iter().filter(|c| c.owned) {
            *crossings.entry(format!("{:?} {:?}", c.kind, c.source)).or_default() += 1;
        }
        overshoot.extend(water_overshoot(g));
        owned_points[0] += g.joints.iter().filter(|j| j.owned).count();
        owned_points[1] += g.ends.iter().filter(|e| e.owned).count();
    }
    let worst_hull = overshoot.iter().max_by(|a, b| a.0[0].total_cmp(&b.0[0])).map(|o| crate::lon_lat(o.1));
    let spread = |n: usize| {
        let mut sorted: Vec<f64> = overshoot.iter().map(|o| o.0[n]).collect();
        sorted.sort_by(f64::total_cmp);
        let at = |q: f64| sorted.get(((sorted.len().max(1) - 1) as f64 * q) as usize).copied().unwrap_or(0.0);
        json!({"max": at(1.0), "p99": at(0.99), "over_0_5_m": sorted.iter().filter(|&&d| d > 0.5).count()})
    };
    let mut rows: Vec<_> = kinds.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    json!({
        "cells": graphs.len(),
        "water_spans": overshoot.len(),
        "water_outside_hull_worst_lon_lat": worst_hull,
        "water_outside_hull_m": spread(0),
        "water_past_ends_m": spread(1),
        "water_bulge_per_chord": spread(2),
        "owned_junctions": owned,
        "owned_joints": owned_points[0],
        "owned_ends": owned_points[1],
        "owned_crossings": crossings,
        "implicit_junctions": implicit,
        "host_rules": rules,
        "junction_pairs_under_2_m": close_2,
        "junction_pairs_under_10_m": close_10,
        "by_hosts_and_guest": rows.iter().map(|((h, g), n)| json!({"hosts": h, "guest": g, "count": n})).collect::<Vec<_>>(),
        "stats_summed_over_cells_each_its_work_area": stats,
    })
}

/// The heap that a graph holds, in bytes.
fn heap(g: &WayGraph) -> usize {
    use obc_ground_plan::graph::{Arm, Crossing, Deck, End, Joint, Junction, Part, Sample, Span, Vertex, Way};
    use std::mem::size_of;
    let arms: usize = g.junctions.iter().map(|j| j.arms.capacity()).sum::<usize>() * size_of::<Arm>();
    let parts: usize = (g.parts.iter())
        .map(|p| p.vertices.capacity() * size_of::<Vertex>() + p.spans.capacity() * size_of::<Span>())
        .sum();
    let decks: usize = g.decks.iter().map(|d| d.ways.capacity() * 4).sum();
    arms + parts
        + decks
        + g.parts.capacity() * size_of::<Part>()
        + g.ways.capacity() * size_of::<Way>()
        + g.samples.capacity() * size_of::<Sample>()
        + g.junctions.capacity() * size_of::<Junction>()
        + g.joints.capacity() * size_of::<Joint>()
        + g.ends.capacity() * size_of::<End>()
        + g.crossings.capacity() * size_of::<Crossing>()
        + g.decks.capacity() * size_of::<Deck>()
}

/// The build time of stages 1 to 6 per fixture cell (the median of `runs`, and the first run, cold
/// for that cell's data), and over the cells the median, p95 and max of both, with the samples,
/// junctions, crossings and heap of each graph.
pub fn bench(fixture: &mut Fixture, cells: &[CellKey], sections: &Sections, runs: usize) -> Value {
    let mut rows = Vec::new();
    let mut times = Vec::new();
    let mut firsts = Vec::new();
    let mut heaps = Vec::new();
    for &key in cells {
        let input = fixture.cell(key);
        let mut ms: Vec<f64> = (0..runs)
            .map(|_| {
                let start = Instant::now();
                std::hint::black_box(build(
                    key,
                    &input,
                    sections,
                    &obc_ground_plan::faces::Rules::embedded().structures,
                ));
                start.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        firsts.push(ms[0]);
        let first = ms[0];
        ms.sort_by(f64::total_cmp);
        let g = build(key, &input, sections, &obc_ground_plan::faces::Rules::embedded().structures);
        let median = ms[ms.len() / 2];
        times.push(median);
        heaps.push(heap(&g));
        rows.push(json!({
            "cell": [key.x, key.y],
            "ms": median,
            "first_ms": first,
            "samples": g.samples.len(),
            "junctions": g.junctions.len(),
            "crossings": g.crossings.len(),
            "heap_bytes": heap(&g),
        }));
    }
    let quantiles = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        let at = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize];
        json!({"median": at(0.5), "p95": at(0.95), "max": at(1.0)})
    };
    json!({
        "cells": cells.len(),
        "runs_per_cell": runs,
        "ms": quantiles(times),
        "first_ms": quantiles(firsts),
        "heap_bytes_max": heaps.iter().max(),
        "per_cell": rows,
    })
}

/// The owned points and crossings of the graphs inside the picture, at their pixels, with the class,
/// level, direction, host and room of each arm: what the picture shows, as text.
pub fn points(canvas: &Canvas, graphs: &BTreeMap<CellKey, WayGraph>, sections: &Sections) -> Value {
    let size = canvas.pixmap.width() as f32;
    let mut out = Vec::new();
    for g in graphs.values() {
        let o = g.cell.origin();
        let px = |p: [i32; 2]| canvas.px(GlobalPoint { x: o.x + i64::from(p[0]), y: o.y + i64::from(p[1]) });
        let seen = |(x, y): (f32, f32)| (0.0..size).contains(&x) && (0.0..size).contains(&y);
        let arm = |a: &obc_ground_plan::graph::Arm| {
            let w = &g.ways[a.way as usize];
            let class = sections.rank_name(w.family == Family::Water, w.class as usize);
            let link = w.link.map_or(String::new(), |l| format!(" {l:?}"));
            format!("{class}{link} {:?} dir {} host {} room {:.1}", w.level, a.dir, a.host, a.room_m)
        };
        let at = |p: (f32, f32)| [p.0.round(), p.1.round()];
        for j in g.junctions.iter().filter(|j| j.owned && seen(px(j.pos))) {
            let arms: Vec<String> = j.arms.iter().map(arm).collect();
            out.push(json!({"junction": at(px(j.pos)), "rule": format!("{:?}", j.host_rule), "implicit": j.implicit, "arms": arms}));
        }
        for j in g.joints.iter().filter(|j| j.owned && seen(px(j.pos))) {
            let arms: Vec<String> = j.arms.iter().map(arm).collect();
            out.push(
                json!({"joint": at(px(j.pos)), "turn_deg": j.turn_rad.to_degrees(), "stroke": j.stroke, "arms": arms}),
            );
        }
        for e in g.ends.iter().filter(|e| e.owned && seen(px(e.pos))) {
            out.push(json!({"end": at(px(e.pos)), "arm": arm(&e.arm)}));
        }
        for c in g.crossings.iter().filter(|c| c.owned && seen(px(c.pos))) {
            let kind = format!("{:?} {:?}", c.kind, c.source);
            out.push(json!({"crossing": at(px(c.pos)), "kind": kind, "angle_deg": c.angle_rad.to_degrees()}));
        }
    }
    Value::Array(out)
}
