//! Two arms side by side (Rooms in the spec): per metre of an arm the state of each other arm near it, the runs of
//! metres in which two arms lie with each other and why each ends, the lanes that turn away, and the arms that lie
//! over one another.

use std::collections::BTreeMap;

use super::super::{Input, Tube};
use super::axes::Axes;
use super::MERGE_M;

/// What another arm is at a metre of an arm, tested in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum State {
    /// Its foot lies past its openings: it is no tunnel there.
    Open,
    Apart,
    /// One lies over the other by the lower one's least height and a roof.
    Over,
    /// The two axes lead one way, or against each other, within `pair_max_deg`.
    With,
    Across,
}

/// A metre of an arm against another: its metres along the arm, the other's state, the metres along the other at the
/// foot, and the height of the other's floor over the arm's.
#[derive(Clone, Copy, Debug)]
pub(super) struct Metre {
    pub s: f64,
    pub state: State,
    pub foot: f64,
    pub rise: f64,
}

/// Per arm and other arm, the metres of the arm at which the other lies near, in order.
pub(super) type Metres = BTreeMap<(usize, usize), Vec<Metre>>;

/// The metres of each arm that `spans` holds (from opening to opening), every whole metre, against each other such arm
/// near it, with the heights of the road solver.
pub(super) fn metres(arms: &[Tube], spans: &[Option<[f64; 2]>], input: &Input) -> Metres {
    let b = &input.rules.bake;
    let (wall, roof) = (2.0 * b.shell_m, b.shell_m + b.slab_m);
    let cos = b.pair_max_deg.to_radians().cos();
    let axes = Axes::new(arms);
    let widest = arms.iter().map(|t| t.half).fold(0.0, f64::max);
    let mut out: Metres = BTreeMap::new();
    for (a, arm) in arms.iter().enumerate() {
        let Some([s0, s1]) = spans[a] else { continue };
        let steps = (s1 - s0).ceil().max(1.0) as usize;
        for k in 0..=steps {
            let s = s0 + (s1 - s0) * k as f64 / steps as f64;
            let (q, dir) = (arm.curve.at(s).0, arm.curve.dir(s));
            for f in axes.near(q.p, a, arm.half + widest + wall) {
                let Some([o0, o1]) = spans[f.tube] else { continue };
                let other = &arms[f.tube];
                let rise = f.z - q.z;
                let lower = if rise > 0.0 { arm } else { other };
                let state = if f.past.is_some() || f.s < o0 - 1e-6 || f.s > o1 + 1e-6 {
                    State::Open
                } else if f.d - arm.half - other.half >= wall - MERGE_M {
                    State::Apart
                } else if rise.abs() >= input.rules.groups[lower.group].least_height_m + roof {
                    State::Over
                } else if (dir[0] * f.dir[0] + dir[1] * f.dir[1]).abs() >= cos {
                    State::With
                } else {
                    State::Across
                };
                out.entry((a, f.tube)).or_default().push(Metre { s, state, foot: f.s, rise });
            }
        }
    }
    out
}

/// Why a run ends on one side: an arm opens there, the next metres are apart or over (a seam), or they are across
/// (a lane turns away).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum End {
    Opens,
    Seam,
    Turns,
}

/// A run of two arms: the arms, its metres along each at its two ends, and why it ends there. Where an arm opens, the
/// run ends at the openings of both arms on that side.
#[derive(Clone, Copy, Debug)]
pub(super) struct Run {
    pub a: usize,
    pub b: usize,
    pub sa: [f64; 2],
    pub sb: [f64; 2],
    pub ends: [End; 2],
}

/// Two arms over one another: the upper, its metres there, the lower, its metres there, and whether only the lower's
/// section goes lower (else its floor: they cross).
pub(super) type Over = (usize, [f64; 2], usize, [f64; 2], bool);

/// The runs of the arms (stretches of metres `with` each other, holes shorter than `room_join_m` closed), the arms over
/// one another (stretches of metres over or across that no run holds, read from the metres of both arms), and the
/// count of runs shorter than `vertex_gap_m`, which are crossings.
pub(super) fn runs(metres: &Metres, spans: &[Option<[f64; 2]>], input: &Input) -> (Vec<Run>, Vec<Over>, usize) {
    let b = &input.rules.bake;
    let (mut out, mut over, mut short) = (Vec::new(), Vec::new(), 0);
    for (&(a, o), list) in metres {
        let [s0, s1] = spans[a].expect("an arm with metres");
        // The metres between two of an arm's metres.
        let step = (s1 - s0) / (s1 - s0).ceil().max(1.0) + 1e-6;
        let next = |i: usize, side: usize| {
            let j = if side == 0 { i.checked_sub(1) } else { (i + 1 < list.len()).then_some(i + 1) };
            j.filter(|&j| (list[j].s - list[i].s).abs() <= step)
        };
        let why = |i: usize, side: usize| {
            if (list[i].s - [s0, s1][side]).abs() < 1e-6 {
                return End::Opens;
            }
            match next(i, side).map(|j| list[j].state) {
                Some(State::Open) => End::Opens,
                Some(State::Across) => End::Turns,
                _ => End::Seam,
            }
        };
        let mut groups: Vec<[usize; 2]> = Vec::new();
        for i in (0..list.len()).filter(|&i| list[i].state == State::With) {
            match groups.last_mut() {
                Some(g) if list[i].s - list[g[1]].s <= b.room_join_m => g[1] = i,
                _ => groups.push([i, i]),
            }
        }
        let mut held = vec![false; list.len()];
        let mut next_to = vec![false; list.len()];
        for g in groups {
            let (first, last) = (list[g[0]], list[g[1]]);
            if last.s - first.s < b.vertex_gap_m {
                short += usize::from(a < o);
                continue;
            }
            (g[0]..=g[1]).for_each(|i| held[i] = true);
            [next(g[0], 0), next(g[1], 1)].into_iter().flatten().for_each(|j| next_to[j] = true);
            // Each run once: as the arm of the lower id reads it.
            if a > o {
                continue;
            }
            let mut ends = [why(g[0], 0), why(g[1], 1)];
            let (mut sa, mut sb) = ([first.s, last.s], [first.foot, last.foot]);
            let forward = last.foot >= first.foot;
            let [o0, o1] = spans[o].expect("an arm with metres");
            // The run ends where either arm's own metres stop seeing the other with it: at a corner of one arm, the
            // other's foot stays at the corner. Their next metre past it tells why.
            let theirs = metres.get(&(o, a)).map_or(&[][..], Vec::as_slice);
            let their_step = (o1 - o0) / (o1 - o0).ceil().max(1.0) + 1e-6;
            for side in 0..2 {
                if ends[side] == End::Opens {
                    continue;
                }
                let seen = |m: &&Metre| m.state == State::With && m.foot >= first.s - step && m.foot <= last.s + step;
                let with = theirs.iter().enumerate().filter(|(_, m)| seen(m));
                let extreme = if side == 1 {
                    with.max_by(|x, y| x.1.foot.total_cmp(&y.1.foot))
                } else {
                    with.min_by(|x, y| x.1.foot.total_cmp(&y.1.foot))
                };
                let Some((j, m)) = extreme else { continue };
                if (side == 1 && m.foot < sa[1]) || (side == 0 && m.foot > sa[0]) {
                    (sa[side], sb[side]) = (m.foot, m.s);
                }
                let away = (side == 1) == forward;
                let k = if away { (j + 1 < theirs.len()).then_some(j + 1) } else { j.checked_sub(1) };
                match k.filter(|&k| (theirs[k].s - m.s).abs() <= their_step).map(|k| theirs[k].state) {
                    Some(State::Across) => ends[side] = End::Turns,
                    Some(State::Open) => ends[side] = End::Opens,
                    _ => {}
                }
            }
            for side in (0..2).filter(|&side| ends[side] == End::Opens) {
                sa[side] = [s0, s1][side];
                sb[side] = if (side == 1) == forward { o1 } else { o0 };
            }
            out.push(Run { a, b: o, sa, sb, ends });
        }
        // The stretches of metres over or across that no run holds: by height where they are all over, else they
        // cross; next to a run only its metres over count, as a lane that turns away is in no room.
        let mut i = 0;
        while i < list.len() {
            let touch = |j: usize| !held[j] && !matches!(list[j].state, State::Apart | State::Open);
            if !touch(i) {
                i += 1;
                continue;
            }
            let mut k = i;
            while k + 1 < list.len() && touch(k + 1) && list[k + 1].s - list[k].s <= step {
                k += 1;
            }
            let stretch = &list[i..=k];
            let beside = (i..=k).any(|j| next_to[j]);
            let any_over = stretch.iter().any(|m| m.state == State::Over);
            let all_over = stretch.iter().all(|m| m.state == State::Over);
            if !beside || any_over {
                let least = stretch.iter().min_by(|x, y| x.rise.abs().total_cmp(&y.rise.abs())).expect("a metre");
                let feet = stretch
                    .iter()
                    .fold([f64::INFINITY, f64::NEG_INFINITY], |r, m| [r[0].min(m.foot), r[1].max(m.foot)]);
                let own = [stretch[0].s, stretch[stretch.len() - 1].s];
                over.push(if least.rise > 0.0 {
                    (o, feet, a, own, beside || all_over)
                } else {
                    (a, own, o, feet, beside || all_over)
                });
            }
            i = k + 1;
        }
    }
    (out, over, short)
}

/// The lanes that turn away (per lane its arm, its metres at the turn and where it is a room again): at a run's end
/// where the next metres are across, of its two arms the narrower, then the higher id. Its arm is a room again at the
/// first metre `vertex_gap_m` or more past the turn where it lies with or across no arm, or at its opening.
pub(super) fn turned(
    found: &[Run],
    metres: &Metres,
    (arms, live): (&[Tube], &[Option<[f64; 2]>]),
    input: &Input,
) -> Vec<(usize, f64, f64)> {
    let b = &input.rules.bake;
    // Per arm the metres at which it lies with or across another arm.
    let mut touch: Vec<Vec<f64>> = vec![Vec::new(); arms.len()];
    for (&(a, _), list) in metres {
        touch[a].extend(list.iter().filter(|m| matches!(m.state, State::With | State::Across)).map(|m| m.s));
    }
    for list in &mut touch {
        list.sort_by(f64::total_cmp);
        list.dedup();
    }
    let mut turned: Vec<(usize, f64, f64)> = Vec::new();
    for r in found {
        for side in (0..2).filter(|&side| r.ends[side] == End::Turns) {
            let narrower = (arms[r.a].half, std::cmp::Reverse(r.a)) <= (arms[r.b].half, std::cmp::Reverse(r.b));
            let forward = r.sb[1] >= r.sb[0];
            let (t, x, away) = if narrower {
                (r.a, r.sa[side], if side == 1 { 1.0 } else { -1.0 })
            } else {
                (r.b, r.sb[side], if (side == 1) == forward { 1.0 } else { -1.0 })
            };
            let [s0, s1] = live[t].expect("an arm of a run");
            let steps = (s1 - s0).ceil().max(1.0) as usize;
            let mut beyond = (0..=steps)
                .map(|k| s0 + (s1 - s0) * k as f64 / steps as f64)
                .filter(|&s| away * (s - x) >= b.vertex_gap_m - 1e-9)
                .collect::<Vec<f64>>();
            if away < 0.0 {
                beyond.reverse();
            }
            let touches = |s: f64| touch[t].binary_search_by(|v| v.total_cmp(&s)).is_ok();
            let again = beyond.into_iter().find(|&s| !touches(s)).unwrap_or(if away > 0.0 { s1 } else { s0 });
            turned.push((t, x, again));
        }
    }
    turned
}
