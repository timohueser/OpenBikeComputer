//! The storeys over the ground over every cell of the TrailView fixture: storey 0 reads the same with them, each
//! storey is a planar partition, each shell part closes, no parapet or railing stands in a paved face of its
//! storey or in a way of storey 0, no slot of a top is a wall between two deck ways, no bar stands on two decks, no
//! edge stands across the
//! mouth of a way that goes on at another level, something goes on past every open piece, a stub and its deck share
//! their points and heights on their end line, and two neighbouring cells give a storey the same corners and
//! heights on their shared line.
#![cfg(feature = "storeys")]

#[path = "fixture/mod.rs"]
mod fixture;

use std::collections::BTreeMap;

use fixture::{default_dir, Fixture};
use obc_ground_plan::cell::{build_request, build_storeys, CellBuild};
use obc_ground_plan::faces::check::partition;
use obc_ground_plan::faces::heights::height_at;
use obc_ground_plan::faces::records::{self, find};
use obc_ground_plan::faces::{window, EdgeKind, Rules, Surface};
use obc_ground_plan::frame::{metres_per_unit, CellKey, CELL_UNITS};
use obc_ground_plan::graph::Family;
use obc_ground_plan::mesh::shell::{border, closure, mesh_storeys, on_paved, StoreyMesh};
use obc_ground_plan::mesh::{mesh_cell, CellMesh, Reads, Rim, NO_LABEL};
use obc_ground_plan::tile::TileCache;

/// Points per side of the grid at which storey 0 must read the same with the storeys' records.
const GRID: i32 = 32;
/// The probes of what lies past an open piece stand this far to each side of its middle (metres).
const PROBE_M: f64 = 0.3;
/// Storey 0 or another storey goes on past an open piece within this of its top's height (metres).
const GOES_ON_M: f64 = 0.1;
/// Open pieces with water or a bank on both sides (a deck's end whose stub begins past the bank), measured on the
/// fixture at the junction rule's build: the count must not grow.
const OPEN_MAX: usize = 24;

/// The storey of label l's line: 0 for none and for a line of storey 0.
fn storey_of(w: &[u32], l: u32) -> i8 {
    let line = records::label(w, l)[2];
    let f = if line == u32::MAX { 0 } else { w[w[6] as usize + records::LINE * line as usize + 1] };
    if f & records::STOREY != 0 {
        (f >> records::LEG_SHIFT) as i8
    } else {
        0
    }
}

/// The height of a storey mesh's top at its corners at point p, by their label's storey.
fn tops_at(m: &StoreyMesh, w: &[u32], p: [i32; 2]) -> Vec<(i8, f64)> {
    (0..m.corners.len())
        .map(|k| m.corner(k))
        .filter(|&(q, l, off)| q == p && off == 0.0 && l != NO_LABEL)
        .map(|(_, l, _)| (storey_of(w, l), height_at(w, [f64::from(p[0]), f64::from(p[1])], Some(l), f64::NAN).h))
        .collect()
}

/// The open pieces off the cell lines past which nothing goes on: at neither side of the piece's middle does storey
/// 0 hold a paved face or shoulder within `GOES_ON_M` of the top (its records), or a stub's verge or slope, and at its
/// ends no top of another storey lies within `GOES_ON_M`.
fn open_without_ground(key: CellKey, m: &StoreyMesh, w: &[u32]) -> Vec<String> {
    let c = CELL_UNITS as i32;
    let mpu = metres_per_unit(key.origin().y + CELL_UNITS / 2);
    let on_cell_line = |[a, b]: &[[i32; 2]; 2]| (0..2).any(|k| a[k] == b[k] && (a[k] == 0 || a[k] == c));
    let mut out = Vec::new();
    for piece in m.open.iter().filter(|p| !on_cell_line(p)) {
        let [a, b] = *piece;
        let (ta, tb) = (tops_at(m, w, a), tops_at(m, w, b));
        let Some(&(storey, za)) = ta.iter().find(|t| t.0 > 0) else { continue };
        let zb = tb.iter().find(|t| t.0 == storey).map_or(za, |t| t.1);
        let mid = [f64::from(a[0] + b[0]) / 2.0, f64::from(a[1] + b[1]) / 2.0];
        let d = [f64::from(b[0] - a[0]), f64::from(b[1] - a[1])];
        let len = libm::hypot(d[0], d[1]).max(1e-9);
        let n = [-d[1] / len * PROBE_M / mpu, d[0] / len * PROBE_M / mpu];
        // A paved face or shoulder of storey 0 at the top's height, or its verge or slope (the stub's side, under a
        // deck's band); no water, bank or bare terrain.
        let z = (za + zb) / 2.0;
        let ground = [1.0, -1.0].iter().any(|&side| {
            let q = [mid[0] + side * n[0], mid[1] + side * n[1]];
            find(w, q).is_some_and(|g| {
                let s = records::label(w, g)[0] & 0xff;
                let paved = [Surface::Carriageway, Surface::Shoulder, Surface::RailBed, Surface::Crossing];
                let side = [Surface::Verge, Surface::Slope];
                (paved.iter().any(|&x| x as u32 == s) && (height_at(w, q, Some(g), f64::NAN).h - z).abs() <= GOES_ON_M)
                    || side.iter().any(|&x| x as u32 == s)
            })
        });
        let other = |t: &[(i8, f64)], z: f64| t.iter().any(|&(k, h)| k != storey && (h - z).abs() <= GOES_ON_M);
        if !ground && !(other(&ta, za) && other(&tb, zb)) {
            out.push(format!("{key:?}: past the open piece {piece:?} of storey {storey} goes on nothing"));
        }
    }
    out
}

/// An edge face, parapet, railing or post of a storey stands in a paved face of storey 0 where that face lies within
/// this of the deck's top under it (metres): a parapet's height.
const STAND_M: f64 = 1.0;
/// Edge triangles over a stub where it meets its deck at a joint with a bend (the deck's end square to its own way,
/// the stub's to its), and over another way's ground stretch beside a deck (their strokes overlap): measured on the
/// fixture at the junction rule's build. The B6 seams and GP12.8 own them; the counts must not grow.
const JOINT_MAX: usize = 25;
const BESIDE_MAX: usize = 235;

/// The edge faces, parapets, railings and posts of the storeys that stand in a carriageway, rail bed or crossing of
/// storey 0 (its records): a triangle's middle over such a face within `STAND_M` of its deck's top there. Returns
/// those on a way that meets the deck at a junction (none: a deck ends on its mouth's line there, `ground_junctions`),
/// and the counts of those on a stub at its joint and on a way that does not meet the deck.
fn edges_on_ground_ways(key: CellKey, m: &StoreyMesh, b: &CellBuild) -> (Vec<String>, usize, usize) {
    let (g, w) = (&b.graph, &b.records);
    let paved = |l: u32| {
        let s = records::label(w, l)[0] & 0xff;
        [Surface::Carriageway, Surface::RailBed, Surface::Crossing].iter().any(|&x| x as u32 == s)
    };
    let way = |l: u32| {
        let line = records::label(w, l)[2];
        (line != u32::MAX).then(|| w[w[6] as usize + records::LINE * line as usize])
    };
    // Whether ways a and c meet at a junction (three arms or more), and at a joint.
    let both = |arms: &[obc_ground_plan::graph::Arm], a: u32, c: u32| {
        arms.iter().any(|x| x.way == a) && arms.iter().any(|x| x.way == c)
    };
    let junction = |a: u32, c: u32| g.junctions.iter().any(|j| both(&j.arms, a, c));
    let joint = |a: u32, c: u32| g.joints.iter().any(|j| both(&j.arms, a, c));
    let (mut out, mut joints, mut beside) = (Vec::new(), 0, 0);
    for k in 2..6 {
        for t in m.part(k) {
            let c = m.triangles[t].map(|i| m.corner(i as usize));
            let Some(&(_, deck, _)) = c.iter().find(|x| storey_of(w, x.1) > 0) else { continue };
            let q = [0, 1].map(|i| c.iter().map(|x| f64::from(x.0[i])).sum::<f64>() / 3.0);
            let top = height_at(w, q, Some(deck), f64::NAN).h;
            let Some(f) = find(w, q).filter(|&f| paved(f)) else { continue };
            if (height_at(w, q, Some(f), f64::NAN).h - top).abs() >= STAND_M {
                continue;
            }
            match way(f).zip(way(deck)) {
                Some((a, d)) if junction(a, d) => out.push(format!(
                    "{key:?}: a {} triangle at {q:.0?} stands in the mouth of way {a}",
                    shell_part(m, t)
                )),
                Some((a, d)) if joint(a, d) => joints += 1,
                _ => beside += 1,
            }
        }
    }
    (out, joints, beside)
}

/// A parapet's, railing's or post's corners on two labels differ in height by at most this (metres): a bar stands
/// on one deck.
const BAR_STEP_M: f64 = 0.05;

/// The parapet, railing and post triangles whose corners stand on two labels of the storeys over the ground that lie
/// more than `BAR_STEP_M` apart. (A deck's end takes storey 0's labels: `stub_meets_deck`.)
fn bars_across_decks(key: CellKey, m: &StoreyMesh, w: &[u32]) -> Vec<String> {
    let mut out = Vec::new();
    for k in 3..6 {
        for t in m.part(k) {
            let c = m.triangles[t].map(|i| m.corner(i as usize));
            for (i, j) in [(0, 1), (1, 2), (0, 2)] {
                let (p, la, lb) = (c[i].0, c[i].1, c[j].1);
                if la == lb || storey_of(w, la) == 0 || storey_of(w, lb) == 0 {
                    continue;
                }
                let q = [f64::from(p[0]), f64::from(p[1])];
                let h = |l: u32| height_at(w, q, Some(l), f64::NAN).h;
                if (h(la) - h(lb)).abs() > BAR_STEP_M {
                    out.push(format!(
                        "{key:?}: a {} triangle at {p:?} stands on labels {la} and {lb}",
                        shell_part(m, t)
                    ));
                }
            }
        }
    }
    out
}

/// Where a stub meets its deck (an end line of storey 0's plan, `EdgeKind::DeckEdge`), storey 0's cell mesh and the
/// paved part of the deck's top have the same points, and their heights there differ by less than `SEAM_M`: no paved
/// end point of a top on it is missing from the cell mesh or stands apart from it, and no cell mesh point on it lies
/// inside a paved open piece's span.
const SEAM_M: f64 = 0.001;
/// The seams of `stub_meets_deck`, measured on the fixture at the storeys' build: a deck's end at a bend of its way
/// (its stub leaves at an angle past `records::FLAT_TURN_DEG`), a stub whose face at the end is no paved face or
/// shoulder, and stub points inside a deck's end piece. The count must not grow.
const SEAMS_MAX: usize = 14;

fn stub_meets_deck(key: CellKey, ground: &CellMesh, m: &StoreyMesh, b: &CellBuild) -> Vec<String> {
    let w = &b.records;
    let plan = &b.faces;
    let ends: Vec<[[f64; 2]; 2]> = (plan.edges.iter().filter(|e| e.kind == EdgeKind::DeckEdge))
        .map(|e| [plan.vertices[e.a as usize], plan.vertices[e.b as usize]].map(|v| v.map(f64::from)))
        .collect();
    // On an end line: within one unit of it, between its ends.
    let on = |p: [i32; 2]| {
        let q = p.map(f64::from);
        ends.iter().any(|[a, c]| {
            let d = [c[0] - a[0], c[1] - a[1]];
            let l2 = (d[0] * d[0] + d[1] * d[1]).max(1e-9);
            let t = ((q[0] - a[0]) * d[0] + (q[1] - a[1]) * d[1]) / l2;
            let off = ((q[0] - a[0]) * d[1] - (q[1] - a[1]) * d[0]).abs() / l2.sqrt();
            (0.0..=1.0).contains(&t) && off <= 1.0
        })
    };
    // Per point, the heights of its corners: the stub's face and those beside it on the line.
    let mut cell: BTreeMap<[i32; 2], Vec<f64>> = BTreeMap::new();
    for k in 0..ground.corners.len() {
        let (p, l, terrain) = ground.corner(k);
        if !terrain && l != NO_LABEL && on(p) {
            cell.entry(p).or_default().push(height_at(w, p.map(f64::from), Some(l), f64::NAN).h);
        }
    }
    // The heights of a top's paved corners (a carriageway, rail bed, crossing or shoulder) at a point.
    let paved_tops = |p: [i32; 2]| -> Vec<f64> {
        (0..m.corners.len())
            .map(|k| m.corner(k))
            .filter(|&(q, l, off)| q == p && off == 0.0 && l != NO_LABEL)
            .filter(|&(_, l, _)| {
                let s = records::label(w, l)[0] & 0xff;
                [Surface::Carriageway, Surface::Shoulder, Surface::RailBed, Surface::Crossing]
                    .iter()
                    .any(|&x| x as u32 == s)
            })
            .map(|(_, l, _)| height_at(w, p.map(f64::from), Some(l), f64::NAN).h)
            .collect()
    };
    let mut out = Vec::new();
    let mut spans: Vec<[[i32; 2]; 2]> = Vec::new();
    for &[a, c] in m.open.iter().filter(|[a, c]| on(*a) && on(*c)) {
        let (ta, tc) = (paved_tops(a), paved_tops(c));
        if !ta.is_empty() && !tc.is_empty() {
            spans.push([a, c]);
        }
        for (p, tops) in [(a, ta), (c, tc)] {
            match cell.get(&p) {
                _ if tops.is_empty() => {}
                None => out.push(format!("{key:?}: the deck's end point {p:?} is no point of storey 0's mesh")),
                Some(h) if !tops.iter().any(|t| h.iter().any(|h| (h - t).abs() < SEAM_M)) => {
                    out.push(format!("{key:?}: at {p:?} storey 0 stands at {h:.4?} m, the deck at {tops:.4?} m"))
                }
                _ => {}
            }
        }
    }
    for &p in cell.keys() {
        let inside = spans.iter().any(|[a, c]| {
            let between = |i: usize| (a[i].min(c[i])..=a[i].max(c[i])).contains(&p[i]);
            p != *a && p != *c && between(0) && between(1)
        });
        if inside {
            out.push(format!("{key:?}: storey 0's point {p:?} on an end line is no point of the deck's paved end"));
        }
    }
    out
}

/// A slot of a top between two deck ways stands no higher than a parapet and the cross-slope of a deck: higher,
/// each deck has its own edge (`mesh::shell`).
const DECK_STEP_M: f64 = 1.5;

/// The slots of the tops between two deck ways higher than `DECK_STEP_M`, by the crate's height function.
fn walls_between_decks(key: CellKey, m: &StoreyMesh, b: &CellBuild) -> Vec<String> {
    let w = &b.records;
    let deck = |l: u32| {
        let line = records::label(w, l)[2];
        let way = (line != u32::MAX).then(|| w[w[6] as usize + records::LINE * line as usize]);
        way.filter(|&x| b.storeys.way(x).is_some())
    };
    let h = |k: u32| {
        let (p, l, _) = m.corner(k as usize);
        height_at(w, [f64::from(p[0]), f64::from(p[1])], Some(l), 500.0).h
    };
    (m.slots[..m.top_slots as usize].iter())
        .filter(|s| deck(m.corner(s[0] as usize).1).zip(deck(m.corner(s[3] as usize).1)).is_some_and(|(a, c)| a != c))
        .filter(|s| (h(s[0]) - h(s[3])).abs().max((h(s[1]) - h(s[2])).abs()) > DECK_STEP_M)
        .map(|s| format!("{key:?}: the slot {s:?} is a wall between two deck ways"))
        .collect()
}

/// The edge faces, parapets, railings and posts of a storey across the mouth of an arm that goes on at another
/// level, at a junction or joint of one of its deck ways: the arm's centre line from the point out to its half-width
/// and `edge_band_m`. A deck ends open there; a corner round would stand across the arm.
fn edges_in_mouths(key: CellKey, m: &StoreyMesh, b: &CellBuild, band_m: f64) -> Vec<String> {
    let (g, w) = (&b.graph, &b.records);
    let mpu = metres_per_unit(key.origin().y + CELL_UNITS / 2);
    let storey = |x: u32| b.storeys.way(x).map_or(0, |d| d.storey);
    let lines = || w[w[6] as usize..w[7] as usize].chunks(records::LINE);
    let half_m = |x: u32| {
        let ground = |l: &&[u32]| l[0] == x && l[1] & (records::STOREY | records::CEILING) == 0;
        lines().find(ground).map(|l| f64::from(f32::from_bits(l[11])))
    };
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let meets = |p: [f64; 2], q: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        let (d1, d2) = (cross(p, q, a), cross(p, q, b));
        let (d3, d4) = (cross(a, b, p), cross(a, b, q));
        d1 * d2 <= 0.0 && d3 * d4 <= 0.0 && (d1 != 0.0 || d2 != 0.0)
    };
    let shell: Vec<usize> = (2..6).flat_map(|k| m.part(k)).collect();
    let mut out = Vec::new();
    let points = g.junctions.iter().map(|j| (j.pos, &j.arms[..])).chain(g.joints.iter().map(|j| (j.pos, &j.arms[..])));
    for (pos, arms) in points {
        for deck in arms.iter().map(|a| storey(a.way)).filter(|&k| k > 0) {
            for a in arms.iter().filter(|a| g.ways[a.way as usize].family != Family::Water && storey(a.way) != deck) {
                let Some(half) = half_m(a.way) else { continue };
                let len = libm::hypot(a.heading[0], a.heading[1]).max(1e-9);
                let reach = (half + band_m) / mpu / len;
                let p = pos.map(f64::from);
                let q = [p[0] + a.heading[0] * reach, p[1] + a.heading[1] * reach];
                for &t in &shell {
                    let tri = m.triangles[t];
                    let c = tri.map(|k| m.corner(k as usize).0.map(f64::from));
                    if storey_of(w, m.corner(tri[0] as usize).1) == deck
                        && (0..3).any(|i| meets(p, q, c[i], c[(i + 1) % 3]))
                    {
                        let of: Vec<(u32, i8)> = arms.iter().map(|x| (x.way, storey(x.way))).collect();
                        out.push(format!(
                            "{key:?}: a {} triangle of storey {deck} stands across way {} at {pos:?} (arms, storeys: {of:?})",
                            shell_part(m, t),
                            a.way
                        ));
                    }
                }
            }
        }
    }
    out
}

fn shell_part(m: &StoreyMesh, t: usize) -> &'static str {
    (0..obc_ground_plan::mesh::shell::PARTS.len())
        .find(|&k| m.part(k).contains(&t))
        .map_or("?", |k| obc_ground_plan::mesh::shell::PARTS[k])
}

#[test]
fn the_storeys_close_stand_on_their_sheets_meet_their_neighbours_and_leave_storey_0_as_it_was() {
    let mut fix = Fixture::open(&default_dir());
    let rules = Rules::embedded();
    let mut cache = TileCache::new(16);
    let mut faults = Vec::new();
    let mut built: BTreeMap<CellKey, (CellBuild, StoreyMesh)> = BTreeMap::new();
    let (mut torn, mut seams, mut joints, mut beside, mut open_ends) = (0, Vec::new(), 0, 0, Vec::new());
    for key in fix.cells() {
        let without = build_request(fix.request(key), &mut cache, rules, &mut || 0.0).unwrap();
        let with = build_storeys(fix.request(key), &mut cache, rules, &mut || 0.0).unwrap();
        assert_eq!(without.faces, with.faces, "{key:?}: storey 0's faces");
        assert_eq!(without.far, with.far, "{key:?}: the far block");
        if with.storeys.plans.is_empty() {
            assert_eq!(without.records, with.records, "{key:?}: a cell with no deck has the records of storey 0 alone");
            continue;
        }
        // Storey 0 reads the same label and height at a grid of points.
        let c = f64::from(CELL_UNITS as i32);
        for j in 0..GRID {
            for i in 0..GRID {
                let q = [(f64::from(i) + 0.37) * c / f64::from(GRID), (f64::from(j) + 0.61) * c / f64::from(GRID)];
                let (a, b) = (find(&without.records, q), find(&with.records, q));
                let (ha, hb) = (height_at(&without.records, q, a, 500.0).h, height_at(&with.records, q, b, 500.0).h);
                if a != b || ha.to_bits() != hb.to_bits() {
                    faults.push(format!(
                        "{key:?} at {q:?}: storey 0 reads {a:?} {ha} without and {b:?} {hb} with storeys"
                    ));
                }
            }
        }
        for plan in &with.storeys.plans {
            faults.extend(
                partition(plan, window(key, 0.0)).into_iter().map(|f| format!("{key:?} storey {}: {f:?}", plan.level)),
            );
        }
        let mesh = mesh_storeys(key, &with.storeys, &with.records, &rules.structures).unwrap();
        assert_eq!(mesh.stats.unlabelled, 0, "{key:?}: a sheet face has no label");
        faults.extend(closure(&mesh));
        faults.extend(on_paved(&mesh, &with.storeys, &with.records));
        faults.extend(walls_between_decks(key, &mesh, &with));
        faults.extend(edges_in_mouths(key, &mesh, &with, rules.structures.edge_band_m));
        open_ends.extend(open_without_ground(key, &mesh, &with.records));
        faults.extend(bars_across_decks(key, &mesh, &with.records));
        let (in_mouths, at_joints, by) = edges_on_ground_ways(key, &mesh, &with);
        faults.extend(in_mouths);
        (joints, beside) = (joints + at_joints, beside + by);
        let rim = Rim::of(&with.faces);
        match mesh_cell(&with.faces, &with.records, &Reads::of(&with.faces, &[rim]), &mut || 0.0) {
            Ok(ground) => seams.extend(stub_meets_deck(key, &ground, &mesh, &with)),
            Err(e) => faults.push(format!("{key:?}: storey 0's mesh: {e:?}")),
        }
        torn += mesh.stats.torn;
        built.insert(key, (with, mesh));
    }
    for (&key, (b, m)) in &built {
        for (east, other) in [(true, CellKey { x: key.x + 1, y: key.y }), (false, CellKey { x: key.x, y: key.y + 1 })] {
            if let Some((ob, om)) = built.get(&other) {
                faults.extend(border((m, &b.records), (om, &ob.records), east));
            }
        }
    }
    let triangles: u32 = built.values().map(|(_, m)| m.triangles.len() as u32).sum();
    for f in &faults {
        println!("FAULT {f}");
    }
    seams.sort();
    seams.dedup();
    println!(
        "{} cells with storeys, {triangles} storey triangles, {torn} torn pieces, {} faults",
        built.len(),
        faults.len()
    );
    println!("{} seams where a stub meets its deck: {seams:#?}", seams.len());
    println!("{joints} edge triangles on a stub at its joint, {beside} on a ground way beside a deck");
    println!("{} open pieces with nothing past them: {open_ends:#?}", open_ends.len());
    assert!(open_ends.len() <= OPEN_MAX, "{} open pieces with nothing past them", open_ends.len());
    assert!(joints <= JOINT_MAX && beside <= BESIDE_MAX, "{joints} and {beside} edge triangles on ground ways");
    assert!(seams.len() <= SEAMS_MAX, "{} seams where a stub meets its deck, more than {SEAMS_MAX}", seams.len());
    assert!(built.len() > 10, "the fixture's decks");
    assert!(faults.is_empty(), "{} faults: {:#?}", faults.len(), &faults[..faults.len().min(20)]);
}
