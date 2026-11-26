//! The open stretch of a tunnel way whose opening lies inside it: from the end vertex of the way to the opening, the
//! way is a ground way.

use super::curve::{insert, u_at};
use super::level_tags::assemble;
use super::points::{key, norm, vertex_station, Plan};
use super::{Family, Level, Part, Sample, Station};
use crate::frame::{CellKey, GlobalPoint};

/// An opening of a tunnel that lies `shift_m` metres along its way from the way's end vertex `way` (the vertex as the
/// zoom-13 tile holds it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OpenEnd {
    pub way: GlobalPoint,
    pub shift_m: f64,
}

/// The station `shift_m` metres along the part's curve from its first vertex (`from_end` false) or from its last.
fn station_from(part: &Part, index: usize, shift_m: f64, from_end: bool) -> Station {
    let n = part.spans.len();
    let mut left = shift_m;
    for k in 0..n {
        let span = if from_end { n - 1 - k } else { k };
        let len = part.spans[span].len_m;
        if left <= len {
            let s = if from_end { len - left } else { left };
            let st = Station { part: index as u32, span: span as u32, u: u_at(&part.spans[span], s) };
            return norm(part, st);
        }
        left -= len;
    }
    vertex_station(part, index, if from_end { 0 } else { n })
}

/// The plan with the stretch of each tunnel piece from an end vertex that `ends` names to its opening on the ground:
/// the cut is a sample of the part (`curve::insert`, as `level_tags::split` cuts at an end line), so the ground
/// stretch crosses, joins and takes its cut and fill as the ground way that it is, and meets the tunnel at a joint.
pub(crate) fn open(cell: CellKey, parts: &[Part], samples: &mut [Vec<Sample>], plan: Plan, ends: &[OpenEnd]) -> Plan {
    if ends.is_empty() {
        return plan;
    }
    // Per part, its stretches on the ground as (lo, hi) in station order, from the vertex to the cut.
    let mut ground: Vec<Vec<(Station, Station)>> = vec![Vec::new(); parts.len()];
    for (i, part) in parts.iter().enumerate() {
        if part.level != Level::Tunnel || part.family == Family::Water {
            continue;
        }
        let n = part.spans.len();
        for end in ends.iter().filter(|e| e.shift_m > 0.0) {
            let at = [end.way.x, end.way.y];
            for from_end in [false, true] {
                let v = if from_end { n } else { 0 };
                if part.vertices[v].pos != at {
                    continue;
                }
                let cut = insert(part, &mut samples[i], station_from(part, i, end.shift_m, from_end), cell);
                let vertex = vertex_station(part, i, v);
                ground[i].push(if from_end { (cut, vertex) } else { (vertex, cut) });
            }
        }
    }
    if ground.iter().all(Vec::is_empty) {
        return plan;
    }
    let place = |st: &Station| f64::from(st.span) + st.u;
    let mut out = Vec::with_capacity(plan.pieces.len() + 2);
    for piece in &plan.pieces {
        let stretches = &ground[piece.part];
        if stretches.is_empty() || piece.level != Level::Tunnel || piece.synthetic {
            out.push(piece.clone());
            continue;
        }
        let mut cuts: Vec<Station> = stretches.iter().flat_map(|&(a, b)| [a, b]).collect();
        cuts.retain(|c| key(c) > key(&piece.from) && key(c) < key(&piece.to));
        cuts.sort_by(|a, b| key(a).partial_cmp(&key(b)).unwrap());
        cuts.dedup();
        let cuts: Vec<(Station, f64)> =
            [piece.from].into_iter().chain(cuts).chain([piece.to]).map(|st| (st, place(&st))).collect();
        let on_ground = |m: f64| stretches.iter().any(|(a, b)| place(a) <= m && m <= place(b));
        assemble(piece, &cuts, |m| Some(if on_ground(m) { Level::Ground } else { piece.level }), &mut out);
    }
    Plan { pieces: out, ..plan }
}
