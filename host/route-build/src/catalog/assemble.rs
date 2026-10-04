//! Orders the main-line members of a route relation into runs of OSM nodes.
//!
//! A port of the ordering logic of the Waymarked Trails route builder
//! (`wmt_db/geometry/route_builder.py` in waymarkedtrails-backend, GPL-3.0, Sarah Hoffmann).
//! Node IDs stand in for coordinates. Only the forward branch of a one-way split enters the line,
//! and appendices are not built.

/// A member way, with its nodes in OSM order.
pub struct Member {
    pub nodes: Vec<i64>,
    pub role: String,
    /// A closed way tagged `junction=roundabout` or `junction=circular`.
    pub roundabout: bool,
}

pub fn is_main(role: &str) -> bool {
    matches!(role, "" | "main" | "forward" | "backward")
}

/// The forward line as runs of connected nodes. A new run starts where the line has a gap.
pub fn main_line(members: &[Member], distance: &dyn Fn(i64, i64) -> f64) -> Vec<Vec<i64>> {
    let ways = members.iter().filter(|m| is_main(&m.role)).map(|m| {
        let closed = m.nodes.first() == m.nodes.last();
        let direction = match m.role.as_str() {
            "forward" => 1,
            "backward" => -1,
            _ if closed && m.roundabout => 1,
            _ => 0,
        };
        Way { nodes: m.nodes.clone(), direction }
    });
    let mut base = Vec::<Seg>::new();
    for way in ways {
        let rest = match base.last_mut() {
            Some(Seg::Ways(ways)) => append(ways, way),
            _ => Some(way),
        };
        if let Some(way) = rest {
            base.push(Seg::Ways(vec![way]));
        }
    }
    let mut mains = Vec::new();
    if base.len() == 1 {
        mains = base;
    } else {
        for (start, end, oneway) in oneway_runs(&base) {
            if oneway && !base[start].roundabout() {
                process_oneways(&mut base, start, end, &mut mains, distance);
            } else {
                mains.extend(base[start..end].iter().cloned());
            }
        }
        flip_order(&mut mains);
        for i in 0..mains.len() {
            if mains[i].roundabout() {
                make_roundabout(&mut mains, i);
            }
        }
        mains = make_linear(mains);
    }
    let mut runs: Vec<Vec<i64>> = Vec::new();
    for seg in &mains {
        seg.forward_ways(&mut |nodes| match runs.last_mut() {
            Some(run) if run.last() == nodes.first() => run.extend(&nodes[1..]),
            _ => runs.push(nodes.to_vec()),
        });
    }
    runs
}

/// Joins runs greedily at their nearest ends, from the first run on, for a relation whose member
/// order jumps. `None` when the runs fork (a run ends inside another run) or when a join is
/// longer than `max_gap`.
pub fn chain(runs: &[Vec<i64>], distance: &dyn Fn(i64, i64) -> f64, max_gap: f64) -> Option<Vec<Vec<i64>>> {
    let inside = |j: usize, node: i64| runs[j].len() > 2 && runs[j][1..runs[j].len() - 1].contains(&node);
    let forks = runs.iter().enumerate().any(|(i, run)| {
        [run[0], run[run.len() - 1]].iter().any(|&end| (0..runs.len()).any(|j| j != i && inside(j, end)))
    });
    if forks {
        return None;
    }
    let mut chain = std::collections::VecDeque::from([runs[0].clone()]);
    let mut rest = runs[1..].to_vec();
    while !rest.is_empty() {
        let (head, tail) = (chain[0][0], chain[chain.len() - 1][chain[chain.len() - 1].len() - 1]);
        // (gap, run, reverse the run, join at the head)
        let mut best = (f64::MAX, 0, false, false);
        for (i, run) in rest.iter().enumerate() {
            let (first, last) = (run[0], run[run.len() - 1]);
            for (gap, reverse, at_head) in [
                (distance(tail, first), false, false),
                (distance(tail, last), true, false),
                (distance(last, head), false, true),
                (distance(first, head), true, true),
            ] {
                if gap < best.0 {
                    best = (gap, i, reverse, at_head);
                }
            }
        }
        let (gap, i, reverse, at_head) = best;
        if gap > max_gap {
            return None;
        }
        let mut run = rest.swap_remove(i);
        if reverse {
            run.reverse();
        }
        if at_head {
            chain.push_front(run);
        } else {
            chain.push_back(run);
        }
    }
    Some(chain.into())
}

#[derive(Clone, Debug)]
struct Way {
    nodes: Vec<i64>,
    /// 1: the route uses the way forward only, -1: backward only, 0: both.
    direction: i8,
}

impl Way {
    fn first(&self) -> i64 {
        self.nodes[0]
    }
    fn last(&self) -> i64 {
        self.nodes[self.nodes.len() - 1]
    }
    fn reverse(&mut self) {
        self.nodes.reverse();
        self.direction = -self.direction;
    }
}

#[derive(Clone, Debug)]
enum Seg {
    /// Connected ways of one role and direction.
    Ways(Vec<Way>),
    /// Separate ways for the two directions. Both lists run in the primary direction of the route.
    Split { forward: Vec<Seg>, backward: Vec<Seg>, first: i64, last: i64 },
}

impl Seg {
    fn first(&self) -> i64 {
        match self {
            Seg::Ways(ways) => ways[0].first(),
            Seg::Split { first, .. } => *first,
        }
    }
    fn last(&self) -> i64 {
        match self {
            Seg::Ways(ways) => ways[ways.len() - 1].last(),
            Seg::Split { last, .. } => *last,
        }
    }
    fn direction(&self) -> i8 {
        match self {
            Seg::Ways(ways) => ways[0].direction,
            Seg::Split { .. } => 0,
        }
    }
    fn reversable(&self) -> bool {
        match self {
            Seg::Ways(ways) => ways.len() == 1 && self.first() != self.last(),
            Seg::Split { forward, backward, .. } => forward.len() == 1 && backward.len() == 1,
        }
    }
    fn roundabout(&self) -> bool {
        matches!(self, Seg::Ways(ways) if ways.len() == 1 && ways[0].direction != 0 && ways[0].first() == ways[0].last())
    }
    /// The nodes of a roundabout; empty for any other segment.
    fn ring(&self) -> &[i64] {
        match self {
            Seg::Ways(ways) if self.roundabout() => &ways[0].nodes,
            _ => &[],
        }
    }
    fn reverse(&mut self) {
        match self {
            Seg::Ways(ways) => {
                ways.reverse();
                ways.iter_mut().for_each(Way::reverse);
            }
            Seg::Split { forward, backward, first, last } => {
                std::mem::swap(forward, backward);
                for list in [&mut *forward, &mut *backward] {
                    list.reverse();
                    list.iter_mut().for_each(Seg::reverse);
                }
                *first = forward[0].first();
                *last = forward[forward.len() - 1].last();
            }
        }
    }
    fn forward_ways(&self, visit: &mut dyn FnMut(&[i64])) {
        match self {
            Seg::Ways(ways) => ways.iter().for_each(|way| visit(&way.nodes)),
            Seg::Split { forward, .. } => forward.iter().for_each(|seg| seg.forward_ways(visit)),
        }
    }
}

/// Appends a way to a run of ways, or gives it back.
fn append(ways: &mut Vec<Way>, mut way: Way) -> Option<Way> {
    let (first, last, direction) = (ways[0].first(), ways[ways.len() - 1].last(), ways[0].direction);
    if first == last || way.first() == way.last() || (direction == 0) != (way.direction == 0) {
        return Some(way);
    }
    if way.first() == last {
        if direction != way.direction {
            return Some(way);
        }
    } else if way.last() == last {
        if direction != -way.direction {
            return Some(way);
        }
        way.reverse();
    } else if ways.len() == 1 && way.first() == first && direction == -way.direction {
        ways[0].reverse();
    } else if ways.len() == 1 && way.last() == first && direction == way.direction {
        ways[0].reverse();
        way.reverse();
    } else {
        return Some(way);
    }
    ways.push(way);
    None
}

/// Runs of one-way or two-way segments as `(start, end, oneway)`. A roundabout is a run alone.
fn oneway_runs(segs: &[Seg]) -> Vec<(usize, usize, bool)> {
    let mut runs = Vec::new();
    let mut start = 0;
    let mut oneway = None;
    for (i, seg) in segs.iter().enumerate() {
        if oneway != Some(seg.direction() != 0) || seg.roundabout() {
            if start < i {
                runs.push((start, i, oneway.unwrap()));
            }
            start = i;
            oneway = Some(seg.direction() != 0);
        }
        if seg.roundabout() {
            runs.push((i, i + 1, seg.direction() != 0));
            start = i + 1;
            oneway = None;
        }
    }
    if let Some(oneway) = oneway {
        runs.push((start, segs.len(), oneway));
    }
    runs
}

fn process_oneways(base: &mut [Seg], start: usize, end: usize, out: &mut Vec<Seg>, distance: &dyn Fn(i64, i64) -> f64) {
    let mut starts = Vec::new();
    if let Some(prev) = start.checked_sub(1).map(|i| &base[i]) {
        if prev.roundabout() {
            starts.extend(prev.ring());
        } else {
            starts.push(prev.last());
            if prev.reversable() && (start < 2 || prev.first() != base[start - 2].last()) {
                starts.push(prev.first());
            }
        }
    }
    let mut ends = Vec::new();
    if let Some(next) = base.get(end) {
        if next.roundabout() {
            ends.extend(next.ring());
        } else {
            ends.push(next.first());
            if next.reversable() && base.get(end + 1).is_none_or(|after| next.last() != after.first()) {
                ends.push(next.last());
            }
        }
    }
    if end - start > 1 {
        return directional(&mut base[start..end], &starts, &ends, out);
    }
    let Seg::Ways(ways) = &base[start] else { unreachable!("base segments are runs of ways") };
    out.push(if ways[0].first() == ways[ways.len() - 1].last() {
        circular(ways, &starts, &ends, distance)
    } else {
        simple_split(ways, &starts, &ends)
    });
}

/// Splits a closed run of one-way ways at the most convenient place.
fn circular(ways: &[Way], starts: &[i64], ends: &[i64], distance: &dyn Fn(i64, i64) -> f64) -> Seg {
    let n = ways.len();
    if n < 2 {
        return Seg::Ways(ways.to_vec());
    }
    let pick = |points: &[i64]| {
        (1..n).find(|&i| points.contains(&ways[i].first())).unwrap_or_else(|| {
            let nearest = |i: usize| points.iter().map(|&p| distance(p, ways[i].first())).fold(f64::MAX, f64::min);
            (1..n).min_by(|&a, &b| nearest(a).total_cmp(&nearest(b))).unwrap()
        })
    };
    let first = ways[0].first();
    let at = if starts.contains(&first) && !ends.is_empty() {
        pick(ends)
    } else if ends.contains(&first) && !starts.is_empty() {
        pick(starts)
    } else {
        n / 2
    };
    let (mut forward, mut backward) = (Seg::Ways(ways[..at].to_vec()), Seg::Ways(ways[at..].to_vec()));
    if ways[0].direction == 1 {
        backward.reverse();
    } else {
        forward.reverse();
    }
    split(forward, backward)
}

fn split(forward: Seg, backward: Seg) -> Seg {
    let (first, last) = (forward.first(), forward.last());
    Seg::Split { forward: vec![forward], backward: vec![backward], first, last }
}

/// Splits a U-shaped run at `at`; `open_at_start` puts the open end of the U toward the start.
fn u_split(ways: &[Way], at: usize, open_at_start: bool) -> Seg {
    if at == 0 || at >= ways.len() {
        return Seg::Ways(ways.to_vec());
    }
    let (head, tail) = (Seg::Ways(ways[..at].to_vec()), Seg::Ways(ways[at..].to_vec()));
    let (mut forward, mut backward) =
        if (ways[0].direction > 0) == open_at_start { (head, tail) } else { (tail, head) };
    if ways[0].direction > 0 {
        backward.reverse();
    } else {
        forward.reverse();
    }
    split(forward, backward)
}

/// Splits an open run of one-way ways where it touches its neighbours, when that is conclusive.
fn simple_split(ways: &[Way], starts: &[i64], ends: &[i64]) -> Seg {
    let n = ways.len();
    if n <= 1 || starts.iter().any(|p| ends.contains(p)) {
        return Seg::Ways(ways.to_vec());
    }
    // The way index where a run touches a start or end point; `n - 1` with `true` is its last node.
    let mut front = Vec::new();
    let mut back = Vec::new();
    for (i, way) in ways.iter().enumerate() {
        if starts.contains(&way.first()) {
            front.push((i, false));
        }
        if ends.contains(&way.first()) {
            back.push((i, false));
        }
    }
    let last = ways[n - 1].last();
    if starts.contains(&last) {
        front.push((n - 1, true));
    }
    if ends.contains(&last) {
        back.push((n - 1, true));
    }
    let at_end = |list: &[(usize, bool)]| list.last().is_some_and(|c| c.1);
    let index = |c: (usize, bool)| c.0;
    if let Some(&f0) = front.first() {
        if f0 == (0, false) {
            if at_end(&front) || (!back.is_empty() && !at_end(&back)) {
                return u_split(ways, back.first().map_or(n / 2, |&b| index(b)), true);
            }
            return Seg::Ways(ways.to_vec());
        }
        if at_end(&front) {
            return match back.first() {
                Some(&b) if !b.1 && b.0 > 0 => u_split(ways, b.0, true),
                _ => Seg::Ways(ways.to_vec()),
            };
        }
    }
    if let Some(&b0) = back.first() {
        if b0 == (0, false) {
            if at_end(&back) || (!front.is_empty() && !at_end(&front)) {
                return u_split(ways, front.first().map_or(n / 2, |&f| index(f)), false);
            }
            return Seg::Ways(ways.to_vec());
        }
        if at_end(&back) {
            return match front.first() {
                Some(&f) => u_split(ways, index(f), false),
                None => Seg::Ways(ways.to_vec()),
            };
        }
        return u_split(ways, index(b0), true);
    }
    match front.first() {
        Some(&f) => u_split(ways, index(f), false),
        None => Seg::Ways(ways.to_vec()),
    }
}

/// Sorts a run of one-way segments into forward and backward branches that meet again.
fn directional(segs: &mut [Seg], starts: &[i64], ends: &[i64], out: &mut Vec<Seg>) {
    let mut first_point = segs
        .iter()
        .find_map(|s| {
            if starts.contains(&s.first()) {
                Some(s.first())
            } else if s.reversable() && starts.contains(&s.last()) {
                Some(s.last())
            } else {
                None
            }
        })
        .unwrap_or(segs[0].first());
    let key = |direction: i8| usize::from(direction < 0);
    let mut endpoints = [first_point; 2];
    let mut branches: [Vec<Seg>; 2] = [Vec::new(), Vec::new()];
    for i in 0..segs.len() {
        let (head, rest) = segs[i..].split_first_mut().unwrap();
        let seg = head;
        let direction = seg.direction();
        if ends.contains(&endpoints[key(direction)]) {
            if seg.reversable() {
                seg.reverse();
            }
        } else if seg.first() != endpoints[key(direction)] && seg.reversable() && !ends.contains(&seg.last()) {
            if seg.last() == endpoints[key(-direction)] || ends.contains(&seg.first()) {
                seg.reverse();
            } else {
                // Look ahead for a hint on the direction.
                for next in rest.iter() {
                    if seg.direction() == next.direction() {
                        if seg.last() == next.first() {
                            break;
                        }
                        if next.reversable() && seg.first() == next.last() {
                            seg.reverse();
                            break;
                        }
                    } else {
                        if seg.first() == next.first() {
                            seg.reverse();
                            break;
                        }
                        if next.reversable() && seg.last() == next.last() {
                            break;
                        }
                    }
                }
            }
        }
        let k = key(seg.direction());
        branches[k].push(seg.clone());
        endpoints[k] = seg.last();
        if endpoints[0] == endpoints[1] && branches.iter().all(|b| !b.is_empty()) {
            let [forward, backward] = std::mem::take(&mut branches);
            out.push(Seg::Split { forward, backward, first: first_point, last: endpoints[0] });
            first_point = endpoints[0];
        }
    }
    let [forward, backward] = branches;
    match (forward.is_empty(), backward.is_empty()) {
        (false, false) => {
            let last = if ends.contains(&endpoints[1]) { endpoints[1] } else { endpoints[0] };
            out.push(Seg::Split { forward, backward, first: first_point, last });
        }
        (false, true) => out.extend(forward),
        _ => out.extend(backward),
    }
}

/// Orients each reversable segment by its neighbours.
fn flip_order(segs: &mut [Seg]) {
    for i in 0..segs.len() {
        if !segs[i].reversable() || segs[i].roundabout() {
            continue;
        }
        let (first, last) = (segs[i].first(), segs[i].last());
        let prev = i.checked_sub(1).map(|p| &segs[p]);
        let next = segs.get(i + 1);
        let touches = |seg: &Seg, point: i64, end: i64| end == point || seg.ring().contains(&point);
        if prev.is_some_and(|p| touches(p, first, p.last())) {
            continue;
        }
        if next.is_some_and(|n| touches(n, last, n.first()) || (n.reversable() && last == n.last())) {
            continue;
        }
        if prev.is_some_and(|p| touches(p, last, p.last()))
            || next.is_some_and(|n| touches(n, first, n.first()) || (n.reversable() && first == n.last()))
        {
            segs[i].reverse();
        }
    }
}

/// Replaces a roundabout with the arcs that the route uses in each direction.
fn make_roundabout(segs: &mut [Seg], at: usize) {
    let Seg::Ways(ways) = &segs[at] else { return };
    let mut way = ways[0].clone();
    if way.direction == -1 {
        way.reverse();
    }
    let points = &way.nodes;
    let find = |point: i64| points.iter().position(|&p| p == point);
    let prev = at.checked_sub(1).map(|p| &segs[p]);
    let next = segs.get(at + 1);
    let arc = |forward: bool| {
        let from = prev.and_then(|p| {
            find(match p {
                Seg::Split { forward: f, backward: b, .. } => (if forward { f } else { b }).last().unwrap().last(),
                _ => p.last(),
            })
        });
        let to = next.and_then(|n| {
            if n.roundabout() {
                n.ring().iter().find_map(|&p| find(p))
            } else {
                find(match n {
                    Seg::Split { forward: f, backward: b, .. } => (if forward { f } else { b })[0].first(),
                    _ => n.first(),
                })
            }
        });
        let from = from.unwrap_or(if to != Some(0) { 0 } else { points.len() / 2 });
        let to = to.unwrap_or(if points.len() / 2 == from { 0 } else { points.len() / 2 });
        // The backward arc runs from the next segment to the previous one.
        let (enter, leave) = if forward { (from, to) } else { (to, from) };
        if enter < leave {
            points[enter..=leave].to_vec()
        } else {
            points[enter..].iter().chain(&points[1..=leave]).copied().collect()
        }
    };
    let forward = Way { nodes: arc(true), direction: 1 };
    let mut backward = Way { nodes: arc(false), direction: 1 };
    backward.reverse();
    let (first, last) = (forward.first(), forward.last());
    segs[at] =
        Seg::Split { forward: vec![Seg::Ways(vec![forward])], backward: vec![Seg::Ways(vec![backward])], first, last };
}

/// Sorts the segments into one line when that is possible; otherwise keeps the member order.
fn make_linear(segs: Vec<Seg>) -> Vec<Seg> {
    let mut lists: Vec<Vec<Seg>> = Vec::new();
    for seg in segs.iter().cloned() {
        match lists.last_mut() {
            Some(list) if list[list.len() - 1].last() == seg.first() => list.push(seg),
            _ => lists.push(vec![seg]),
        }
    }
    while lists.len() > 1 {
        let mut current = lists.pop().unwrap();
        let (head, tail) = (current[0].first(), current[current.len() - 1].last());
        if let Some(list) = lists.iter_mut().find(|l| l[l.len() - 1].last() == head || l[0].first() == tail) {
            if list[list.len() - 1].last() == head {
                list.extend(current);
            } else {
                current.append(list);
                *list = current;
            }
            continue;
        }
        let Some(list) = lists.iter_mut().find(|l| l[l.len() - 1].last() == tail || l[0].first() == head) else {
            return segs;
        };
        current.reverse();
        current.iter_mut().for_each(Seg::reverse);
        if list[list.len() - 1].last() == tail {
            list.extend(current);
        } else {
            current.append(list);
            *list = current;
        }
    }
    lists.pop().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn way(nodes: &[i64], role: &str) -> Member {
        Member { nodes: nodes.to_vec(), role: role.into(), roundabout: false }
    }
    fn line(members: &[Member]) -> Vec<Vec<i64>> {
        main_line(members, &|a, b| (a - b).abs() as f64)
    }

    #[test]
    fn keeps_member_order_and_sorts_only_into_one_line() {
        // Reversed and out of order, but one line in the end.
        assert_eq!(line(&[way(&[1, 2], ""), way(&[3, 2], "main"), way(&[3, 4], "")]), [vec![1, 2, 3, 4]]);
        assert_eq!(line(&[way(&[1, 2], ""), way(&[3, 4], ""), way(&[2, 3], "")]), [vec![1, 2, 3, 4]]);
        // No single line: the member order stays, with a gap.
        assert_eq!(line(&[way(&[1, 2], ""), way(&[8, 9], ""), way(&[2, 3], "")]), [vec![1, 2], vec![8, 9], vec![2, 3]]);
        // An excursion is not in the main line; an out-and-back spur turns at its tip.
        assert_eq!(
            line(&[way(&[1, 2], ""), way(&[2, 7], "excursion"), way(&[2, 5], ""), way(&[2, 5], ""), way(&[2, 3], "")]),
            [vec![1, 2, 5, 2, 3]]
        );
    }

    #[test]
    fn jumping_member_order_joins_at_nearest_ends_but_a_fork_does_not() {
        // The nodes lie on a line; the distance is the difference of the IDs.
        let distance = |a: i64, b: i64| (a - b).abs() as f64;
        let runs = [vec![10, 20], vec![50, 40], vec![21, 30], vec![9, 0]];
        assert_eq!(chain(&runs, &distance, 10.0).unwrap(), [vec![0, 9], vec![10, 20], vec![21, 30], vec![40, 50]]);
        assert_eq!(chain(&runs, &distance, 5.0), None, "a join of 10 m is too long");
        // Small jumps in a shuffled order: the chain does not retrace itself.
        let shuffled = [vec![0, 9], vec![21, 30], vec![10, 20], vec![31, 40]];
        assert_eq!(chain(&shuffled, &distance, 10.0).unwrap(), [vec![0, 9], vec![10, 20], vec![21, 30], vec![31, 40]]);
        // The second run starts inside the first: a branch.
        assert_eq!(chain(&[vec![10, 20, 30], vec![20, 25]], &distance, 10.0), None);
    }

    #[test]
    fn one_way_split_and_roundabout_follow_the_forward_branch() {
        // Two one-way carriageways between 2 and 5.
        let split = [way(&[1, 2], ""), way(&[2, 3, 5], "forward"), way(&[2, 4, 5], "backward"), way(&[5, 6], "")];
        assert_eq!(line(&split), [vec![1, 2, 3, 5, 6]]);
        let ring = Member { nodes: vec![10, 11, 12, 13, 10], role: String::new(), roundabout: true };
        assert_eq!(line(&[way(&[1, 10], ""), ring, way(&[12, 20], "")]), [vec![1, 10, 11, 12, 20]]);
    }
}
