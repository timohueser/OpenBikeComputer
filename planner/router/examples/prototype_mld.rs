//! PROTOTYPE (never merges): a multi-level overlay search (CRP / MLD) over a real package.
//!
//! Usage: prototype_mld PACKAGE PROFILES QUERIES [U1,U2,...]
//! Partitions the junctions by recursive inertial-flow bisection, customizes each profile's
//! cell cliques, and compares random state-to-state queries with the package's own search.
use planner_router::{
    base::{Costs, Graph},
    data::RoutingData,
    search::{Query, Seed, Workspace},
};
use std::{
    cell::Cell,
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
    time::Instant,
};

const INF: u32 = u32::MAX;

// ---------------------------------------------------------------- junction graph
struct Junctions {
    x: Vec<f64>,
    y: Vec<f64>,
    first: Vec<u32>,
    adj: Vec<u32>,
}

fn junction_graph(tail: &[u32], head: &[u32], x: Vec<f64>, y: Vec<f64>) -> Junctions {
    let n = x.len();
    let mut pairs: Vec<(u32, u32)> = Vec::with_capacity(tail.len() * 2);
    for (&a, &b) in tail.iter().zip(head) {
        if a != b {
            pairs.push((a, b));
            pairs.push((b, a));
        }
    }
    pairs.sort_unstable();
    pairs.dedup();
    let mut first = vec![0u32; n + 1];
    for &(a, _) in &pairs {
        first[a as usize + 1] += 1;
    }
    for i in 0..n {
        first[i + 1] += first[i];
    }
    Junctions { x, y, first, adj: pairs.into_iter().map(|p| p.1).collect() }
}

// ---------------------------------------------------------------- inertial flow bisection
struct Local {
    first: Vec<u32>,
    adj: Vec<u32>,
    /// Arc index of the reverse arc.
    rev: Vec<u32>,
}

fn induced(nodes: &[u32], g: &Junctions, local: &mut [u32]) -> Local {
    for (i, &v) in nodes.iter().enumerate() {
        local[v as usize] = i as u32;
    }
    let mut first = vec![0u32; nodes.len() + 1];
    let mut adj = Vec::new();
    for (i, &v) in nodes.iter().enumerate() {
        for &w in &g.adj[g.first[v as usize] as usize..g.first[v as usize + 1] as usize] {
            let l = local[w as usize];
            if l != INF && (l as usize) < nodes.len() && nodes[l as usize] == w {
                adj.push(l);
            }
        }
        first[i + 1] = adj.len() as u32;
    }
    // Reverse arcs: adjacency lists are sorted by global id, so search the partner list.
    let mut rev = vec![0u32; adj.len()];
    for u in 0..nodes.len() {
        for a in first[u] as usize..first[u + 1] as usize {
            let v = adj[a] as usize;
            let row = &adj[first[v] as usize..first[v + 1] as usize];
            let pos = row.iter().position(|&x| x as usize == u).unwrap();
            rev[a] = first[v] + pos as u32;
        }
    }
    for &v in nodes {
        local[v as usize] = INF;
    }
    Local { first, adj, rev }
}

/// Unit-capacity Dinic between the node sets `s` and `t`. Returns the cut and the source side.
fn min_cut(l: &Local, n: usize, s: &[u32], t: &[u32]) -> (usize, Vec<bool>) {
    let mut flow = vec![0i8; l.adj.len()];
    let mut kind = vec![0u8; n]; // 1 source, 2 sink
    for &v in s {
        kind[v as usize] = 1;
    }
    for &v in t {
        kind[v as usize] = 2;
    }
    let mut level = vec![u32::MAX; n];
    let mut cursor = vec![0u32; n];
    let mut queue = Vec::with_capacity(n);
    let mut total = 0usize;
    loop {
        level.fill(u32::MAX);
        queue.clear();
        for &v in s {
            level[v as usize] = 0;
            queue.push(v);
        }
        let mut head = 0;
        let mut reached = false;
        while head < queue.len() {
            let u = queue[head] as usize;
            head += 1;
            if kind[u] == 2 {
                reached = true;
                continue;
            }
            for a in l.first[u] as usize..l.first[u + 1] as usize {
                let v = l.adj[a] as usize;
                if level[v] == u32::MAX && flow[a] < 1 {
                    level[v] = level[u] + 1;
                    queue.push(v as u32);
                }
            }
        }
        if !reached {
            let side = level.iter().map(|&d| d != u32::MAX).collect();
            return (total, side);
        }
        for u in 0..n {
            cursor[u] = l.first[u];
        }
        // Blocking flow by iterative DFS from each source.
        let mut stack: Vec<(u32, u32)> = Vec::new(); // (node, arc used to enter)
        for &root in s {
            loop {
                stack.clear();
                stack.push((root, u32::MAX));
                let mut found = false;
                while let Some(&(u, _)) = stack.last() {
                    let u = u as usize;
                    if kind[u] == 2 {
                        found = true;
                        break;
                    }
                    let mut advanced = false;
                    while cursor[u] < l.first[u + 1] {
                        let a = cursor[u] as usize;
                        let v = l.adj[a] as usize;
                        if flow[a] < 1 && level[v] == level[u] + 1 && kind[v] != 1 {
                            stack.push((v as u32, a as u32));
                            advanced = true;
                            break;
                        }
                        cursor[u] += 1;
                    }
                    if !advanced {
                        level[u] = u32::MAX; // dead end
                        stack.pop();
                        if let Some(&(p, _)) = stack.last() {
                            cursor[p as usize] += 1;
                        }
                    }
                }
                if !found {
                    break;
                }
                for &(_, a) in &stack[1..] {
                    flow[a as usize] += 1;
                    flow[l.rev[a as usize] as usize] -= 1;
                }
                total += 1;
            }
        }
    }
}

/// Splits `nodes` into two parts with the smallest of four inertial-flow cuts.
fn bisect(nodes: &[u32], g: &Junctions, local: &mut [u32]) -> (Vec<u32>, Vec<u32>, usize) {
    let l = induced(nodes, g, local);
    let n = nodes.len();
    let k = ((n as f64) * 0.25).floor().max(1.0) as usize;
    let mut best: Option<(usize, f64, Vec<bool>)> = None;
    for dir in 0..4 {
        let angle = dir as f64 * std::f64::consts::PI / 4.0;
        let (c, s) = (angle.cos(), angle.sin());
        let mut order: Vec<u32> = (0..n as u32).collect();
        let key = |i: u32| g.x[nodes[i as usize] as usize] * c + g.y[nodes[i as usize] as usize] * s;
        order.sort_unstable_by(|&a, &b| key(a).partial_cmp(&key(b)).unwrap());
        let (src, snk) = (&order[..k], &order[n - k..]);
        let (cut, side) = min_cut(&l, n, src, snk);
        let a = side.iter().filter(|&&b| b).count();
        let balance = a.min(n - a) as f64 / n as f64;
        if best.as_ref().is_none_or(|b| (cut, -balance) < (b.0, -b.1)) {
            best = Some((cut, balance, side));
        }
    }
    let (cut, _, side) = best.unwrap();
    let (mut a, mut b) = (Vec::new(), Vec::new());
    for (i, &v) in nodes.iter().enumerate() {
        if side[i] {
            a.push(v)
        } else {
            b.push(v)
        }
    }
    (a, b, cut)
}

/// A bisection tree: leaves hold at most `leaf` junctions. Returns the leaf order and the
/// (start, end, children) of every tree node, root first.
struct Tree {
    order: Vec<u32>,
    nodes: Vec<(u32, u32, Option<(usize, usize)>)>,
}

fn build_tree(nodes: Vec<u32>, g: &Junctions, leaf: usize, local: &mut Vec<u32>, depth: usize) -> Tree {
    // Splits the top levels in parallel threads.
    if nodes.len() <= leaf {
        let len = nodes.len() as u32;
        return Tree { order: nodes, nodes: vec![(0, len, None)] };
    }
    let (a, b, _) = bisect(&nodes, g, local);
    if a.is_empty() || b.is_empty() {
        let len = nodes.len() as u32;
        return Tree { order: nodes, nodes: vec![(0, len, None)] };
    }
    let (ta, tb) = if depth < 3 {
        std::thread::scope(|scope| {
            let left = scope.spawn(|| {
                let mut local = vec![INF; g.x.len()];
                build_tree(a, g, leaf, &mut local, depth + 1)
            });
            let tb = {
                let mut local = vec![INF; g.x.len()];
                build_tree(b, g, leaf, &mut local, depth + 1)
            };
            (left.join().unwrap(), tb)
        })
    } else {
        (build_tree(a, g, leaf, local, depth + 1), build_tree(b, g, leaf, local, depth + 1))
    };
    let shift = ta.order.len() as u32;
    let mut order = ta.order;
    order.extend(tb.order);
    let mut out = vec![(0, order.len() as u32, Some((1, 1 + ta.nodes.len())))];
    let base_a = 1;
    let base_b = 1 + ta.nodes.len();
    for (s, e, c) in ta.nodes {
        out.push((s, e, c.map(|(x, y)| (x + base_a, y + base_a))));
    }
    for (s, e, c) in tb.nodes {
        out.push((s + shift, e + shift, c.map(|(x, y)| (x + base_b, y + base_b))));
    }
    Tree { order, nodes: out }
}

// ---------------------------------------------------------------- overlay
struct Level {
    /// Cell of each junction.
    cell: Vec<u32>,
    cells: usize,
    /// Per cell: entry states (cut roads arriving in the cell) and exit states (cut roads leaving).
    entries_first: Vec<u32>,
    entries: Vec<u32>,
    exits_first: Vec<u32>,
    exits: Vec<u32>,
    /// Position of a state within its head cell's entries, and within its tail cell's exits.
    entry_pos: HashMap<u32, u32>,
    exit_pos: HashMap<u32, u32>,
    /// Offset of each cell's matrix (entries x exits, row-major).
    offset: Vec<u64>,
}

struct Overlay {
    tail: Vec<u32>,
    head: Vec<u32>,
    levels: Vec<Level>,
}

fn overlay(tail: Vec<u32>, head: Vec<u32>, cells: Vec<Vec<u32>>) -> Overlay {
    let states = tail.len();
    let mut levels = Vec::new();
    for cell in cells {
        let count = *cell.iter().max().unwrap() as usize + 1;
        let mut ent: Vec<Vec<u32>> = vec![Vec::new(); count];
        let mut ext: Vec<Vec<u32>> = vec![Vec::new(); count];
        for r in 0..states {
            let (a, b) = (cell[tail[r] as usize], cell[head[r] as usize]);
            if a != b {
                ent[b as usize].push(r as u32);
                ext[a as usize].push(r as u32);
            }
        }
        let mut entry_pos = HashMap::new();
        let mut exit_pos = HashMap::new();
        let (mut entries_first, mut entries, mut exits_first, mut exits, mut offset) =
            (vec![0u32], Vec::new(), vec![0u32], Vec::new(), vec![0u64]);
        for c in 0..count {
            for (i, &r) in ent[c].iter().enumerate() {
                entry_pos.insert(r, i as u32);
            }
            for (i, &r) in ext[c].iter().enumerate() {
                exit_pos.insert(r, i as u32);
            }
            entries.extend(&ent[c]);
            exits.extend(&ext[c]);
            entries_first.push(entries.len() as u32);
            exits_first.push(exits.len() as u32);
            offset.push(offset[c] + (ent[c].len() * ext[c].len()) as u64);
        }
        levels.push(Level { cell, cells: count, entries_first, entries, exits_first, exits, entry_pos, exit_pos, offset });
    }
    Overlay { tail, head, levels }
}

impl Level {
    fn entries(&self, c: usize) -> &[u32] {
        &self.entries[self.entries_first[c] as usize..self.entries_first[c + 1] as usize]
    }
    fn exits(&self, c: usize) -> &[u32] {
        &self.exits[self.exits_first[c] as usize..self.exits_first[c + 1] as usize]
    }
}

/// Dijkstra scratch indexed by state id.
struct Scratch {
    dist: Vec<u64>,
    parent: Vec<(u32, u8)>,
    touched: Vec<u32>,
    heap: BinaryHeap<Reverse<(u64, u32)>>,
}
impl Scratch {
    fn new(states: usize) -> Self {
        Self { dist: vec![u64::MAX; states], parent: vec![(INF, 0); states], touched: Vec::new(), heap: BinaryHeap::new() }
    }
    fn reset(&mut self) {
        for &v in &self.touched {
            self.dist[v as usize] = u64::MAX;
        }
        self.touched.clear();
        self.heap.clear();
    }
    fn relax(&mut self, v: u32, d: u64, parent: (u32, u8), push: bool) {
        if d < self.dist[v as usize] {
            if self.dist[v as usize] == u64::MAX {
                self.touched.push(v);
            }
            self.dist[v as usize] = d;
            self.parent[v as usize] = parent;
            if push {
                self.heap.push(Reverse((d, v)));
            }
        }
    }
}

/// One search inside cell `c` of level `level` (1-based) from `from`. It uses level-0 arcs for
/// level 1, else the cliques of level - 1. Exit states of the cell are relaxed, not expanded.
fn cell_search(
    o: &Overlay,
    cliques: &[Vec<u32>],
    graph: &Graph,
    costs: &Costs,
    level: usize,
    c: u32,
    from: u32,
    s: &mut Scratch,
    stop: Option<u32>,
) {
    let lv = &o.levels[level - 1];
    s.reset();
    s.relax(from, 0, (INF, 0), true);
    while let Some(Reverse((d, v))) = s.heap.pop() {
        if d != s.dist[v as usize] {
            continue;
        }
        if Some(v) == stop {
            return;
        }
        let h = o.head[v as usize] as usize;
        if lv.cell[h] != c {
            continue; // an exit
        }
        if level == 1 {
            for a in graph.first[v as usize] as usize..graph.first[v as usize + 1] as usize {
                let w = costs.arc(graph, a);
                if w != u64::MAX {
                    s.relax(graph.head[a], d + w, (v, 1), true);
                }
            }
        } else {
            let low = &o.levels[level - 2];
            let sub = low.cell[h] as usize;
            let p = low.entry_pos[&v] as usize;
            let nx = low.exits(sub).len();
            let base = low.offset[sub] as usize + p * nx;
            for (q, &y) in low.exits(sub).iter().enumerate() {
                let w = cliques[level - 2][base + q];
                if w != INF {
                    s.relax(y, d + w as u64, (v, level as u8), true);
                }
            }
        }
    }
}

fn customize(o: &Overlay, graph: &Graph, costs: &Costs) -> (Vec<Vec<u32>>, Vec<f64>) {
    let states = o.tail.len();
    let mut cliques: Vec<Vec<u32>> = Vec::new();
    let mut times = Vec::new();
    for level in 1..=o.levels.len() {
        let t = Instant::now();
        let lv = &o.levels[level - 1];
        let mut out = vec![INF; *lv.offset.last().unwrap() as usize];
        let threads = 4;
        // Interleave cells over threads for balance.
        let results: Vec<Vec<(usize, Vec<u32>)>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..threads)
                .map(|tid| {
                    let cliques = &cliques;
                    scope.spawn(move || {
                        let mut s = Scratch::new(states);
                        let mut done = Vec::new();
                        for c in (tid..lv.cells).step_by(threads) {
                            let (ent, ext) = (lv.entries(c), lv.exits(c));
                            let mut m = vec![INF; ent.len() * ext.len()];
                            for (p, &e) in ent.iter().enumerate() {
                                cell_search(o, cliques, graph, costs, level, c as u32, e, &mut s, None);
                                for (q, &x) in ext.iter().enumerate() {
                                    let d = s.dist[x as usize];
                                    if d != u64::MAX {
                                        assert!(d < INF as u64, "clique cost overflows u32");
                                        m[p * ext.len() + q] = d as u32;
                                    }
                                }
                            }
                            done.push((c, m));
                        }
                        done
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for (c, m) in results.into_iter().flatten() {
            out[lv.offset[c] as usize..lv.offset[c + 1] as usize].copy_from_slice(&m);
        }
        cliques.push(out);
        times.push(t.elapsed().as_secs_f64());
    }
    (cliques, times)
}

// ---------------------------------------------------------------- query
#[derive(Default)]
struct Stats {
    settled: usize,
    level0: usize,
    clique_rows: usize,
    clique_bytes: usize,
    cells: [std::collections::HashSet<u32>; 8],
    /// Settled states and relaxed arcs by the level of the arcs they used (0: road arcs).
    by_level: [(usize, usize); 8],
}

struct Side {
    labels: HashMap<u32, (u64, u32, u8)>,
    heap: BinaryHeap<Reverse<(u64, u32)>>,
}

fn query(
    o: &Overlay,
    cliques: &[Vec<u32>],
    graph: &Graph,
    costs: &Costs,
    source: u32,
    target: u32,
    stats: &mut Stats,
) -> Option<(u64, Vec<(u32, u8)>)> {
    let levels = o.levels.len();
    // Open cells: the head of the source and the tail of the target.
    let open: Vec<[u32; 2]> =
        o.levels.iter().map(|lv| [lv.cell[o.head[source as usize] as usize], lv.cell[o.tail[target as usize] as usize]]).collect();
    let level_of = |j: u32| -> usize {
        for l in 0..levels {
            let c = o.levels[l].cell[j as usize];
            if open[l].contains(&c) {
                return l;
            }
        }
        levels
    };
    let mut sides = [
        Side { labels: HashMap::new(), heap: BinaryHeap::new() },
        Side { labels: HashMap::new(), heap: BinaryHeap::new() },
    ];
    sides[0].labels.insert(source, (0, INF, 0));
    sides[0].heap.push(Reverse((0, source)));
    sides[1].labels.insert(target, (0, INF, 0));
    sides[1].heap.push(Reverse((0, target)));
    let mut best = u64::MAX;
    let mut meeting = INF;
    if source == target {
        return Some((0, vec![(source, 0)]));
    }
    loop {
        for side in &mut sides {
            while let Some(&Reverse((d, v))) = side.heap.peek() {
                if side.labels[&v].0 == d {
                    break;
                }
                side.heap.pop();
            }
        }
        let top: [u64; 2] = std::array::from_fn(|i| sides[i].heap.peek().map_or(u64::MAX, |r| r.0 .0));
        if top[0] == u64::MAX || top[1] == u64::MAX || top[0].saturating_add(top[1]) >= best {
            break;
        }
        let dir = usize::from(top[1] < top[0]);
        let Reverse((d, v)) = sides[dir].heap.pop().unwrap();
        stats.settled += 1;
        let mut relax = |sides: &mut [Side; 2], w: u32, nd: u64, kind: u8| {
            let e = sides[dir].labels.entry(w).or_insert((u64::MAX, INF, 0));
            if nd < e.0 {
                *e = (nd, v, kind);
                sides[dir].heap.push(Reverse((nd, w)));
                if let Some(&(od, _, _)) = sides[1 - dir].labels.get(&w) {
                    if nd + od < best {
                        best = nd + od;
                        meeting = w;
                    }
                }
            }
        };
        if dir == 0 {
            let l = level_of(o.head[v as usize]);
            stats.by_level[l].0 += 1;
            if l == 0 {
                stats.level0 += 1;
                stats.by_level[0].1 += (graph.first[v as usize + 1] - graph.first[v as usize]) as usize;
                for a in graph.first[v as usize] as usize..graph.first[v as usize + 1] as usize {
                    let w = costs.arc(graph, a);
                    if w != u64::MAX {
                        relax(&mut sides, graph.head[a], d + w, 1);
                    }
                }
            } else {
                let lv = &o.levels[l - 1];
                let c = lv.cell[o.head[v as usize] as usize] as usize;
                let p = *lv.entry_pos.get(&v).expect("forward overlay state is an entry") as usize;
                let nx = lv.exits(c).len();
                let base = lv.offset[c] as usize + p * nx;
                stats.clique_rows += 1;
                stats.clique_bytes += nx * 4;
                stats.by_level[l].1 += nx;
                stats.cells[l].insert(c as u32);
                for (q, &y) in lv.exits(c).iter().enumerate() {
                    let w = cliques[l - 1][base + q];
                    if w != INF {
                        relax(&mut sides, y, d + w as u64, (l + 1) as u8);
                    }
                }
            }
        } else {
            let l = level_of(o.tail[v as usize]);
            stats.by_level[l].0 += 1;
            if l == 0 {
                stats.level0 += 1;
                stats.by_level[0].1 += (graph.reverse_first[v as usize + 1] - graph.reverse_first[v as usize]) as usize;
                for i in graph.reverse_first[v as usize] as usize..graph.reverse_first[v as usize + 1] as usize {
                    let a = graph.reverse_arc(i);
                    let w = costs.arc(graph, a);
                    if w != u64::MAX {
                        relax(&mut sides, graph.reverse_tail[i], d + w, 1);
                    }
                }
            } else {
                let lv = &o.levels[l - 1];
                let c = lv.cell[o.tail[v as usize] as usize] as usize;
                let q = *lv.exit_pos.get(&v).expect("backward overlay state is an exit") as usize;
                let nx = lv.exits(c).len();
                stats.clique_rows += 1;
                stats.clique_bytes += lv.entries(c).len() * 4;
                stats.by_level[l].1 += lv.entries(c).len();
                stats.cells[l].insert(c as u32);
                for (p, &e) in lv.entries(c).iter().enumerate() {
                    let w = cliques[l - 1][lv.offset[c] as usize + p * nx + q];
                    if w != INF {
                        relax(&mut sides, e, d + w as u64, (l + 1) as u8);
                    }
                }
            }
        }
    }
    if meeting == INF {
        return None;
    }
    // The overlay path: (state, kind of the arc that reached it).
    let mut path = Vec::new();
    let mut v = meeting;
    loop {
        let (_, p, k) = sides[0].labels[&v];
        path.push((v, k));
        if p == INF {
            break;
        }
        v = p;
    }
    path.reverse();
    let mut v = meeting;
    loop {
        let (_, p, k) = sides[1].labels[&v];
        if p == INF {
            break;
        }
        path.push((p, k));
        v = p;
    }
    Some((best, path))
}

/// Replaces each clique arc with its states, recursively. Returns the state sequence.
fn unpack(o: &Overlay, cliques: &[Vec<u32>], graph: &Graph, costs: &Costs, path: &[(u32, u8)], s: &mut Scratch) -> Vec<u32> {
    let mut out = vec![path[0].0];
    for pair in path.windows(2) {
        let (from, (to, kind)) = (pair[0].0, pair[1]);
        expand(o, cliques, graph, costs, from, to, kind as usize, s, &mut out);
    }
    out
}

fn expand(o: &Overlay, cliques: &[Vec<u32>], graph: &Graph, costs: &Costs, from: u32, to: u32, kind: usize, s: &mut Scratch, out: &mut Vec<u32>) {
    if kind <= 1 {
        out.push(to);
        return;
    }
    let level = kind - 1;
    let c = o.levels[level - 1].cell[o.head[from as usize] as usize];
    cell_search(o, cliques, graph, costs, level, c, from, s, Some(to));
    let mut chain = Vec::new();
    let mut v = to;
    while v != from {
        let (p, k) = s.parent[v as usize];
        chain.push((v, k));
        v = p;
    }
    chain.reverse();
    let mut prev = from;
    for (v, k) in chain {
        expand(o, cliques, graph, costs, prev, v, k as usize, s, out);
        prev = v;
    }
}

fn path_cost(graph: &Graph, costs: &Costs, states: &[u32]) -> Option<u64> {
    let mut total = 0;
    for w in states.windows(2) {
        let row = graph.first[w[0] as usize] as usize..graph.first[w[0] as usize + 1] as usize;
        let a = row.clone().find(|&a| graph.head[a] == w[1])?;
        let c = costs.arc(graph, a);
        if c == u64::MAX {
            return None;
        }
        total += c;
    }
    Some(total)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let selection = planner_router::open(std::path::Path::new(&args[1]))?;
    let profiles: Vec<&str> = args[2].split(',').collect();
    let queries: usize = args[3].parse()?;
    let sizes: Vec<usize> =
        args.get(4).map_or(vec![1 << 9, 1 << 12, 1 << 15, 1 << 18], |s| s.split(',').map(|v| v.parse().unwrap()).collect());
    let min_km: f64 = args.get(5).map_or(0.0, |v| v.parse().unwrap());

    let t = Instant::now();
    let first = selection.prepared(profiles[0])?;
    let graph = &first.graph;
    let states = graph.nodes();
    let mut remap: HashMap<u32, u32> = HashMap::new();
    let (mut tail, mut head, mut x, mut y) = (vec![0u32; states], vec![0u32; states], Vec::new(), Vec::new());
    let mut point = |id: u32, p: planner_router::model::Point, x: &mut Vec<f64>, y: &mut Vec<f64>| -> u32 {
        *remap.entry(id).or_insert_with(|| {
            x.push(p.lon as f64 * (p.lat as f64 * 1e-6).to_radians().cos());
            y.push(p.lat as f64);
            (x.len() - 1) as u32
        })
    };
    let mut lat_lon = vec![(0i32, 0i32); states];
    for r in 0..states {
        let (f, to, a, b) = selection.with_road(r as u32, |road| (road.from, road.to, road.shape[0], *road.shape.last().unwrap()))?;
        tail[r] = point(f, a, &mut x, &mut y);
        head[r] = point(to, b, &mut x, &mut y);
        lat_lon[r] = (b.lat, b.lon);
    }
    let junctions = x.len();
    let g = junction_graph(&tail, &head, x, y);
    eprintln!(
        "load {:.1}s: {} states, {} turns, {} junctions, {} junction edges",
        t.elapsed().as_secs_f64(),
        states,
        graph.head.len(),
        junctions,
        g.adj.len() / 2
    );

    let t = Instant::now();
    let mut local = vec![INF; junctions];
    let tree = build_tree((0..junctions as u32).collect(), &g, sizes[0], &mut local, 0);
    eprintln!("partition {:.1}s: {} tree nodes", t.elapsed().as_secs_f64(), tree.nodes.len());
    // Level cells: the maximal tree nodes no larger than each threshold.
    let mut cells = Vec::new();
    for &u in &sizes {
        let mut cell = vec![0u32; junctions];
        let mut count = 0u32;
        let mut stack = vec![0usize];
        while let Some(i) = stack.pop() {
            let (s, e, c) = tree.nodes[i];
            if (e - s) as usize <= u || c.is_none() {
                for &j in &tree.order[s as usize..e as usize] {
                    cell[j as usize] = count;
                }
                count += 1;
            } else {
                let (a, b) = c.unwrap();
                stack.push(b);
                stack.push(a);
            }
        }
        cells.push(cell);
    }
    let o = overlay(tail, head, cells);
    for (l, lv) in o.levels.iter().enumerate() {
        let cut = lv.entries.len();
        let mut sizes: Vec<usize> = (0..lv.cells).map(|c| lv.entries(c).len().max(lv.exits(c).len())).collect();
        sizes.sort_unstable();
        eprintln!(
            "level {}: {} cells, {} cut roads ({:.2}% of states), boundary per cell median {} p90 {} max {}, matrix entries {} ({:.1} MB as u32, {:.2} B/state)",
            l + 1,
            lv.cells,
            cut,
            100.0 * cut as f64 / states as f64,
            sizes[sizes.len() / 2],
            sizes[sizes.len() * 9 / 10],
            sizes.last().unwrap(),
            lv.offset.last().unwrap(),
            *lv.offset.last().unwrap() as f64 * 4.0 / 1e6,
            *lv.offset.last().unwrap() as f64 * 4.0 / states as f64
        );
    }

    // Random reachable pairs, the same for every profile.
    let mut seed = 7u64;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let km_of = |s: u32, t: u32| {
        let (a, b) = (lat_lon[s as usize], lat_lon[t as usize]);
        planner_router::model::Point { lat: a.0, lon: a.1, elevation: 0.0 }
            .distance(planner_router::model::Point { lat: b.0, lon: b.1, elevation: 0.0 })
            / 1000.0
    };
    let pairs: Vec<(u32, u32)> = (0..queries * 2000)
        .map(|_| ((rand() % states as u64) as u32, (rand() % states as u64) as u32))
        .filter(|&(s, t)| km_of(s, t) >= min_km)
        .take(queries * 3)
        .collect();

    for profile in profiles {
        let prepared = selection.prepared(profile)?;
        let (graph, costs) = (&prepared.graph, &prepared.costs);
        let t = Instant::now();
        let (cliques, times) = customize(&o, graph, costs);
        let bytes: usize = cliques.iter().map(|c| c.len() * 4).sum();
        let finite: usize = cliques.iter().map(|c| c.iter().filter(|&&v| v != INF).count()).sum();
        let max = cliques.iter().flat_map(|c| c.iter()).filter(|&&v| v != INF).max().copied().unwrap_or(0);
        eprintln!(
            "{profile}: customize {:.1}s (levels {:?}), cliques {:.1} MB, {:.1}% finite, max {}",
            t.elapsed().as_secs_f64(),
            times.iter().map(|t| format!("{t:.1}")).collect::<Vec<_>>(),
            bytes as f64 / 1e6,
            100.0 * finite as f64 / (bytes / 4) as f64,
            max
        );
        for (l, c) in cliques.iter().enumerate() {
            let raw: Vec<u8> = c.iter().flat_map(|v| v.to_le_bytes()).collect();
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            std::io::Write::write_all(&mut z, &raw).unwrap();
            let zipped = z.finish().unwrap().len();
            let small = c.iter().filter(|&&v| v != INF && v < 65535).count();
            let finite = c.iter().filter(|&&v| v != INF).count().max(1);
            eprintln!(
                "  level {}: {:.2} MB raw ({:.2} B/state), zlib {:.2} MB, {:.1}% of finite values fit u16",
                l + 1,
                raw.len() as f64 / 1e6,
                raw.len() as f64 / states as f64,
                zipped as f64 / 1e6,
                100.0 * small as f64 / finite as f64
            );
        }
        let mut workspace = Workspace::default();
        let mut scratch = Scratch::new(states);
        let mut done = 0;
        println!("profile,km,cost,mld_settled,mld_level0,mld_rows,mld_clique_kb,mld_cells,mld_ms,unpack_ms,path_states,alt_settled,alt_ms,dij_settled,dij_ms,path_cells1,levels");
        for &(s, t) in &pairs {
            if done == queries {
                break;
            }
            if costs.roads.get(s as usize) == u64::MAX || costs.roads.get(t as usize) == u64::MAX {
                continue;
            }
            let mut stats = Stats::default();
            let clock = Instant::now();
            let found = query(&o, &cliques, graph, costs, s, t, &mut stats);
            let mld_ms = clock.elapsed().as_secs_f64() * 1e3;
            let Some((cost, path)) = found else { continue };
            let clock = Instant::now();
            let states_path = unpack(&o, &cliques, graph, costs, &path, &mut scratch);
            let unpack_ms = clock.elapsed().as_secs_f64() * 1e3;
            assert_eq!(path_cost(graph, costs, &states_path), Some(cost), "unpacked path cost");
            let run = |guided: bool, ws: &mut Workspace| -> (u64, usize, f64) {
                let count = Cell::new(0usize);
                let cancelled = || {
                    count.set(count.get() + 1);
                    false
                };
                let starts = [Seed { node: s, cost: 0, choice: 0 }];
                let ends = [Seed { node: t, cost: 0, choice: 0 }];
                let potential = if guided { prepared.guide.as_ref().map(|g| g.potential(&starts, &ends).unwrap()) } else { None };
                let clock = Instant::now();
                let found = ws
                    .run_with_potential(
                        graph,
                        costs,
                        Query { starts: &starts, ends: &ends, ceiling: u64::MAX, max_roads: 10_000_000, heap_bytes: 1 << 32, cancelled: &cancelled },
                        potential.as_ref(),
                    )
                    .unwrap()
                    .unwrap();
                (found.cost, count.get() * 1024, clock.elapsed().as_secs_f64() * 1e3)
            };
            let (alt_cost, alt_settled, alt_ms) = run(true, &mut workspace);
            assert_eq!(alt_cost, cost, "MLD cost differs from the package search");
            let (dij_settled, dij_ms) = if done % 5 == 0 { let r = run(false, &mut workspace); (r.1, r.2) } else { (0, 0.0) };
            let km = km_of(s, t);
            let cells: usize = stats.cells.iter().map(|c| c.len()).sum();
            let path_cells1 = {
                let mut c: Vec<u32> = states_path.iter().map(|&v| o.levels[0].cell[o.head[v as usize] as usize]).collect();
                c.sort_unstable();
                c.dedup();
                c.len()
            };
            let levels: Vec<String> = stats.by_level[..=o.levels.len()].iter().map(|(a, b)| format!("{a}/{b}")).collect();
            println!(
                "{profile},{km:.1},{cost},{},{},{},{:.1},{cells},{mld_ms:.3},{unpack_ms:.3},{},{alt_settled},{alt_ms:.2},{dij_settled},{dij_ms:.2},{path_cells1},{}",
                stats.settled,
                stats.level0,
                stats.clique_rows,
                stats.clique_bytes as f64 / 1024.0,
                states_path.len(),
                levels.join(" ")
            );
            done += 1;
        }
    }
    Ok(())
}
