//! The census of the chains of a bake: what the heights by chain depend on, for a measurement of the
//! reads, the cycles and the sizes.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde_json::{json, Value};

use super::area::{area_of, each_area, Area};
use super::chains::{assemble, depths, facts, Arm, Chains, Fact, ReadKind, Segments};
use super::network::Network;
use super::{Classes, Feature, River, Surroundings};
use crate::geometry::{bounds, key, percentile, Boxes};
use crate::graph::components;
use crate::{degrees, Terrain};

const KINDS: [ReadKind; 9] = [
    ReadKind::Side,
    ReadKind::Copy,
    ReadKind::GroundPass,
    ReadKind::End,
    ReadKind::Clearance,
    ReadKind::Under,
    ReadKind::StructurePass,
    ReadKind::Branch,
    ReadKind::Hanger,
];

/// The census of the chains of a bake: the facts of each area job from a `Network` of today's vertices,
/// the chains that they join, and the numbers that the heights by chain depend on.
pub fn chain_census(
    features: &[&Feature],
    context: &[&Feature],
    terrain: &dyn Terrain,
    around: &Surroundings,
    classes: &Classes,
    memory: usize,
) -> Value {
    let clock = std::time::Instant::now();
    let boxes = Boxes::new(&around.rivers.iter().map(|r| bounds(&r.points)).collect::<Vec<_>>());
    let job = |subset: &[&Feature], chosen: &[usize], area: Area| {
        let Some(net) = Network::new(subset, classes) else { return (Vec::new(), 0) };
        let rivers: Vec<&River> =
            boxes.overlapping(bounds(&net.osm_xy)).into_iter().map(|i| &around.rivers[i]).collect();
        (facts(&net, classes, terrain, &rivers, chosen, &|xy| area_of(xy) == area), net.vertex_xy.len())
    };
    let done = each_area(features, context, memory, &job);
    let largest = done.iter().map(|d| d.1).max().unwrap_or(0);
    let areas = done.len();
    let facts: Vec<Fact> = done.into_iter().flat_map(|d| d.0).collect();
    let every: Vec<&Feature> = features.iter().chain(context).copied().collect();
    let mut out = census(&every, facts, terrain);
    out["jobs"] = json!({"areas": areas, "largest_network_vertices": largest});
    out["peak_rss_mb"] = json!(peak_rss_mb());
    out["seconds"] = json!((clock.elapsed().as_secs_f64() * 10.0).round() / 10.0);
    out
}

fn peak_rss_mb() -> usize {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let line = status.lines().find(|l| l.starts_with("VmHWM:"));
    line.and_then(|l| l.split_whitespace().nth(1)?.parse::<usize>().ok()).unwrap_or(0) / 1024
}

/// Kilometres to one decimal; an empty sum of floats is −0.
fn km(metres: f64) -> f64 {
    (metres / 100.0).round() / 10.0 + 0.0
}

/// The count of lengths in metres, their total and their percentiles, in km.
fn dist(metres: &[f64]) -> Value {
    let mut v = metres.to_vec();
    let at = |v: &mut [f64], q: f64| km(percentile(v, q).unwrap_or(0.0));
    json!({"count": v.len(), "km": km(v.iter().sum()), "p50_km": at(&mut v, 50.0), "p90_km": at(&mut v, 90.0),
           "p99_km": at(&mut v, 99.0), "max_km": at(&mut v, 100.0)})
}

/// The numbers of 2.6 of the slice brief from the facts of a bake.
pub(crate) fn census(every: &[&Feature], mut facts: Vec<Fact>, terrain: &dyn Terrain) -> Value {
    use ReadKind::*;
    facts.sort();
    facts.dedup();
    let built = assemble(every, &facts);
    let segments = Segments::new(every);
    let chains = &built.chains;
    let latlon = |xy: [f64; 2]| degrees(xy);
    let places = |list: &mut dyn Iterator<Item = [f64; 2]>| list.take(20).map(latlon).collect::<Vec<_>>();
    let chain_of = |arm: Arm, by: &[usize]| by[segments.end(arm).0];
    let mut segment_chain = vec![0; segments.part.len()];
    for (c, chain) in chains.iter().enumerate() {
        for &arm in &chain.arms {
            segment_chain[segments.end(arm).0] = c;
        }
    }
    // The first and the last vertex of each chain, and the arm at each end that points into it.
    let ends: Vec<[Arm; 2]> = chains
        .iter()
        .map(|c| {
            let last = segments.end(*c.arms.last().unwrap());
            [c.arms[0], segments.arm((last.0, 1 - last.1))]
        })
        .collect();

    let osm_segments = segments.part.len();
    let total_m: f64 = (0..osm_segments).map(|s| segments.metres(s)).sum();
    let nodes: usize =
        (0..osm_segments).map(|s| (segments.metres(s) / 2.0).ceil() as usize).sum::<usize>() + segments.parts.len();
    let count_kind =
        |kind: ReadKind| facts.iter().filter(|f| matches!(f, Fact::Read { kind: k, .. } if *k == kind)).count();
    let fact_reads: serde_json::Map<String, Value> =
        KINDS.iter().map(|&k| (format!("{k:?}"), json!(count_kind(k)))).collect();

    let ground: Vec<usize> = (0..chains.len()).filter(|&c| !chains[c].structure).collect();
    let structures: Vec<usize> = (0..chains.len()).filter(|&c| chains[c].structure).collect();
    let lengths = |list: &[usize]| list.iter().map(|&c| chains[c].metres).collect::<Vec<_>>();
    let mut by_class: BTreeMap<usize, Vec<f64>> = BTreeMap::new();
    for &c in &ground {
        by_class.entry(chains[c].rule).or_default().push(chains[c].metres);
    }
    let selfs: HashSet<usize> = built.reads.iter().filter(|r| r.0 == r.1).map(|r| r.0).collect();
    let mut longest = ground.clone();
    longest.sort_by(|&a, &b| chains[b].metres.total_cmp(&chains[a].metres));
    let longest: Vec<Value> = longest
        .iter()
        .take(20)
        .map(|&c| {
            let [a, b] = ends[c];
            json!({"km": km(chains[c].metres), "class": chains[c].rule, "from": latlon(segments.point(a)),
                   "to": latlon(segments.point(b))})
        })
        .collect();
    let closed: Vec<usize> = ground.iter().copied().filter(|&c| chains[c].closed).collect();

    // Structures: their ends, fixed by an End or a Branch read or as a bridge end that meets no way.
    let mut occurrences: HashMap<[u64; 2], u32> = HashMap::new();
    for (_, _, points, _) in &segments.parts {
        points.iter().for_each(|&p| *occurrences.entry(key(p)).or_default() += 1);
    }
    let mut end_reads: HashMap<Arm, (ReadKind, Arm)> = HashMap::new();
    for fact in &facts {
        if let Fact::Read { kind: kind @ (End | Branch), reader, from } = *fact {
            let held = end_reads.entry(reader).or_insert((kind, from));
            if kind == End {
                *held = (kind, from);
            }
        }
    }
    let bridge = |arm: Arm| {
        let f = segments.feature(segments.end(arm).0);
        f.free && !f.tunnel
    };
    let mut asked = Vec::new();
    let mut fixed_ends = [0usize; 3];
    // Per structure chain with two fixed ends: its points (end, end) and how each height is found.
    enum Height {
        Terrain([f64; 2]),
        Line(usize, [f64; 2]),
    }
    let mut shafts_of: Vec<(usize, [Height; 2])> = Vec::new();
    for &c in structures.iter().filter(|&&c| !chains[c].closed) {
        let height = |arm: Arm| -> Option<Height> {
            let at = segments.point(arm);
            match end_reads.get(&arm) {
                Some((End, _)) => Some(Height::Terrain(at)),
                Some((_, from)) => Some(Height::Line(chain_of(*from, &segment_chain), at)),
                None if bridge(arm) && occurrences[&key(at)] == 1 => Some(Height::Terrain(at)),
                None => None,
            }
        };
        let [a, b] = ends[c].map(height);
        fixed_ends[usize::from(a.is_some()) + usize::from(b.is_some())] += 1;
        if let (Some(a), Some(b)) = (a, b) {
            shafts_of.push((c, [a, b]));
        }
    }
    // The terrain at every end of a chain, and metres along each chain to its vertices.
    for e in &ends {
        asked.extend(e.map(|arm| segments.point(arm)));
    }
    let heights = terrain.heights(&asked);
    let end_height = |c: usize, k: usize| heights[2 * c + k];
    let along = |c: usize, at: [f64; 2]| -> f64 {
        let mut s = 0.0;
        for &arm in &chains[c].arms {
            if segments.point(arm) == at {
                return s;
            }
            s += segments.metres(segments.end(arm).0);
        }
        s
    };
    let terrain_at = |p: [f64; 2]| terrain.height(p);
    let shaft_list: Vec<usize> = shafts_of
        .iter()
        .filter(|(c, hs)| {
            let h = hs.each_ref().map(|h| match h {
                Height::Terrain(p) => terrain_at(*p),
                Height::Line(main, p) => {
                    let (h0, h1) = (end_height(*main, 0), end_height(*main, 1));
                    let share = along(*main, *p) / chains[*main].metres.max(1e-9);
                    h0 + (h1 - h0) * share
                }
            });
            (h[1] - h[0]).abs() > chains[*c].metres
        })
        .map(|(c, _)| *c)
        .collect();
    let mixed = structures
        .iter()
        .filter(|&&c| {
            let kinds: HashSet<bool> =
                chains[c].arms.iter().map(|&a| segments.feature(segments.end(a).0).tunnel).collect();
            kinds.len() == 2
        })
        .count();
    let mut pass_after = vec![Vec::new(); chains.len()];
    for &(r, f, kind) in &built.reads {
        if kind == StructurePass {
            pass_after[r].push(f);
        }
    }
    let pass_order = components(&pass_after.iter().map(Vec::as_slice).collect::<Vec<_>>());
    let pass_depth = depths(&pass_order, &pass_after);
    let mut stacked: Vec<(usize, usize)> = pass_order
        .iter()
        .zip(&pass_depth)
        .filter(|(_, &d)| d >= 2)
        .flat_map(|(members, &d)| members.iter().map(move |&c| (d, c)))
        .collect();
    stacked.sort_by(|a, b| b.cmp(a));

    let edges = |kind: ReadKind| built.reads.iter().filter(|r| r.2 == kind).count();
    let reads: serde_json::Map<String, Value> = KINDS.iter().map(|&k| (format!("{k:?}"), json!(edges(k)))).collect();
    // Copies: the pairs of chains, those whose rows read in both directions, and the metres that a row reads.
    let mut copy_ways: HashMap<(usize, usize), [bool; 2]> = HashMap::new();
    let mut copied: HashSet<usize> = HashSet::new();
    for fact in &facts {
        if let Fact::Read { kind: Copy, reader, from } = *fact {
            let (r, f) = (chain_of(reader, &segment_chain), chain_of(from, &segment_chain));
            if r != f {
                copy_ways.entry((r.min(f), r.max(f))).or_default()[usize::from(r > f)] = true;
                copied.insert(segments.end(reader).0);
            }
        }
    }
    let switching = copy_ways.values().filter(|w| w[0] && w[1]).count();

    // The order: components by depth, and the cycles of reads.
    let mut by_depth = vec![0; built.depth.iter().max().map_or(0, |d| d + 1)];
    for (members, &d) in built.order.iter().zip(&built.depth) {
        by_depth[d] += members.len();
    }
    let mut cycles: Vec<(f64, &Vec<usize>)> = built
        .order
        .iter()
        .filter(|m| m.len() >= 2 || selfs.contains(&m[0]))
        .map(|m| (m.iter().map(|&c| chains[c].metres).sum(), m))
        .collect();
    cycles.sort_by(|a, b| b.0.total_cmp(&a.0));
    let chain_m: f64 = chains.iter().map(|c| c.metres).sum();
    let cycle_m: f64 = cycles.iter().map(|c| c.0).sum();

    // The closure of 1,000 chains: every chain that it reads, and they read, and so on.
    let step = (chains.len() / 1000).max(1);
    let (mut closure_n, mut closure_m) = (Vec::new(), Vec::new());
    for start in (0..chains.len()).step_by(step).take(1000) {
        let (mut seen, mut stack) = (HashSet::from([start]), vec![start]);
        while let Some(c) = stack.pop() {
            for &f in &built.after_of(c) {
                if seen.insert(f) {
                    stack.push(f);
                }
            }
        }
        closure_n.push(seen.len() as f64);
        closure_m.push(seen.iter().map(|&c| chains[c].metres).sum::<f64>());
    }
    let spread = |v: &mut Vec<f64>, scale: &dyn Fn(f64) -> f64| {
        json!({"p50": scale(percentile(v, 50.0).unwrap_or(0.0)), "p90": scale(percentile(v, 90.0).unwrap_or(0.0)),
               "p99": scale(percentile(v, 99.0).unwrap_or(0.0)), "max": scale(percentile(v, 100.0).unwrap_or(0.0))})
    };
    let start_of = |c: usize| segments.point(chains[c].arms[0]);
    // The kinds of the reads inside each cycle ("-self": a chain on itself), and the largest cycles.
    let mut by_kinds: BTreeMap<String, (usize, usize, f64)> = BTreeMap::new();
    let mut largest = Vec::new();
    for (m, members) in &cycles {
        let set: HashSet<usize> = members.iter().copied().collect();
        let kinds: BTreeSet<String> = built
            .reads
            .iter()
            .filter(|r| set.contains(&r.0) && set.contains(&r.1))
            .map(|r| format!("{:?}{}", r.2, if r.0 == r.1 { "-self" } else { "" }))
            .collect();
        let name = kinds.into_iter().collect::<Vec<_>>().join("+");
        let entry = by_kinds.entry(name.clone()).or_default();
        *entry = (entry.0 + 1, entry.1 + members.len(), entry.2 + m);
        if largest.len() < 10 {
            let rules: BTreeSet<usize> = members.iter().map(|&c| chains[c].rule).collect();
            largest.push(json!({"km": km(*m), "chains": members.len(), "kinds": name, "rules": rules,
                                "at": latlon(start_of(members[0]))}));
        }
    }
    let cycle_kinds: serde_json::Map<String, Value> =
        by_kinds.into_iter().map(|(k, (n, c, m))| (k, json!({"count": n, "chains": c, "km": km(m)}))).collect();
    json!({
        "cycle_kinds": cycle_kinds, "largest_cycles": largest,
        "ways": segments.parts.len(), "osm_segments": osm_segments, "km": km(total_m),
        "nodes_2m": nodes, "store_bytes": 4 * nodes,
        "facts": {
            "through": facts.iter().filter(|f| matches!(f, Fact::Through { .. })).count(),
            "link": facts.iter().filter(|f| matches!(f, Fact::Link { .. })).count(),
            "read": fact_reads, "bytes": facts.len() * std::mem::size_of::<Fact>(), "conflicts": built.conflicts,
        },
        "ground": {
            "chains": dist(&lengths(&ground)),
            "by_class": by_class.iter().map(|(rule, m)| (rule.to_string(), dist(m))).collect::<serde_json::Map<_, _>>(),
            "closed": {"count": closed.len(), "km": km(closed.iter().map(|&c| chains[c].metres).sum())},
            "self_reads": ground.iter().filter(|c| selfs.contains(c)).count(),
            "longest": longest,
        },
        "structures": {
            "chains": dist(&lengths(&structures)), "mixed": mixed,
            "fixed_ends": {"0": fixed_ends[0], "1": fixed_ends[1], "2": fixed_ends[2]},
            "shafts": shaft_list.len(), "shaft_places": places(&mut shaft_list.iter().map(|&c| start_of(c))),
            "passes": edges(StructurePass), "stack_depth_max": pass_depth.iter().max().copied().unwrap_or(0),
            "stack_places": places(&mut stacked.iter().map(|&(_, c)| start_of(c))),
        },
        "reads": reads,
        "copies": {"pairs": copy_ways.len(), "switching_pairs": switching,
                   "km": km(copied.iter().map(|&s| segments.metres(s)).sum())},
        "order": {
            "components": built.order.len(), "depth_max": by_depth.len().saturating_sub(1), "chains_by_depth": by_depth,
            "cycles": {
                "count": cycles.len(), "chains": cycles.iter().map(|c| c.1.len()).sum::<usize>(), "km": km(cycle_m),
                "largest_chains": cycles.iter().map(|c| c.1.len()).max().unwrap_or(0),
                "largest_km": km(cycles.first().map_or(0.0, |c| c.0)),
                "share_of_km": ((cycle_m / chain_m.max(1e-9)) * 1e4).round() / 1e4 + 0.0,
                "places": places(&mut cycles.iter().map(|c| start_of(c.1[0]))),
            },
        },
        "closure": {"sampled": closure_n.len(), "chains": spread(&mut closure_n, &|v| v),
                    "km": spread(&mut closure_m, &km)},
    })
}

impl Chains {
    /// The chains that chain c reads.
    fn after_of(&self, c: usize) -> Vec<usize> {
        let from = self.reads.partition_point(|r| r.0 < c);
        self.reads[from..].iter().take_while(|r| r.0 == c).map(|r| r.1).collect()
    }
}
