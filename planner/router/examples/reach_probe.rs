//! Throwaway probe for issue #3254: what does a one-direction search with a riding-time limit cost?
//! Usage: reach_probe PACKAGE_DIRECTORY LON LAT [PROFILE]
use planner_router::{
    data::RoutingData,
    model::{Pace, Point, BIKE, NO_ELEVATION},
    snap::Policy,
};
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashSet},
    time::Instant,
};

type Heap = BinaryHeap<Reverse<(u64, u32)>>;

/// Plain Dijkstra over the profile cost from the seeds, up to `ceiling`. Returns the settle order.
fn sweep(
    graph: &planner_router::base::Graph,
    costs: &planner_router::base::Costs,
    seeds: &[(u32, u64)],
    ceiling: u64,
    cost: &mut [u64],
    parent: &mut [u32],
) -> (Vec<u32>, usize, usize) {
    cost.fill(u64::MAX);
    parent.fill(u32::MAX);
    let mut heap = Heap::new();
    for &(node, c) in seeds {
        if c < cost[node as usize] {
            cost[node as usize] = c;
            heap.push(Reverse((c, node)));
        }
    }
    let (mut order, mut relaxed, mut peak) = (Vec::new(), 0usize, 0usize);
    while let Some(Reverse((c, node))) = heap.pop() {
        if cost[node as usize] != c {
            continue;
        }
        if c > ceiling {
            break;
        }
        order.push(node);
        for arc in graph.first[node as usize] as usize..graph.first[node as usize + 1] as usize {
            let weight = costs.arc(graph, arc);
            if weight == u64::MAX {
                continue;
            }
            relaxed += 1;
            let to = graph.head[arc];
            let next = c + weight;
            if next < cost[to as usize] {
                cost[to as usize] = next;
                parent[to as usize] = node;
                heap.push(Reverse((next, to)));
                peak = peak.max(heap.len());
            }
        }
    }
    (order, relaxed, peak)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (path, lon, lat) = (&args[1], args[2].parse::<f64>()?, args[3].parse::<f64>()?);
    let metric = args.get(4).map_or("touring", String::as_str);
    let t = Instant::now();
    let mut package = planner_router::open(std::path::Path::new(path))?;
    let budget = package.default_budget();
    package.set_memory_budget(budget);
    let prepared = package.prepared(metric)?;
    let profile = package.profile(metric)?.clone();
    let (graph, costs) = (&*prepared.graph, &prepared.costs);
    let n = graph.nodes();
    println!("open + prepare {metric}: {:.2} s, road states {n}, arcs {}", t.elapsed().as_secs_f64(), graph.head.len());

    let point = Point { lat: (lat * 1e6).round() as i32, lon: (lon * 1e6).round() as i32, elevation: NO_ELEVATION };
    let found = package.snap(point, metric, Policy::default())?;
    let mut seeds = Vec::new();
    let mut seed_fraction = Vec::new();
    for c in &found.retained {
        let e = package.endpoint(metric, c.position.road)?;
        let Some(cost) = e.cost else { continue };
        seeds.push((c.position.road, cost.total() - cost.prefix(c.position.fraction)?));
        seed_fraction.push((c.position.road, c.position.fraction));
    }
    println!("seeds: {}", seeds.len());

    let (mut cost, mut parent) = (vec![u64::MAX; n], vec![u32::MAX; n]);
    // A: the whole package, three runs.
    let mut order = Vec::new();
    for run in 0..3 {
        let t = Instant::now();
        let (o, relaxed, peak) = sweep(graph, costs, &seeds, u64::MAX, &mut cost, &mut parent);
        let s = t.elapsed().as_secs_f64();
        println!(
            "A run {run}: settled {} of {n} in {:.3} s = {:.2} M labels/s, relaxed {relaxed}, heap peak {peak}",
            o.len(),
            s,
            o.len() as f64 / s / 1e6
        );
        order = o;
    }

    // B: moving seconds, length and points of every settled road, from its geometry (what a route answer does).
    let pace = Pace::default();
    let t = Instant::now();
    let (mut secs, mut metres, mut points) = (vec![0f32; n], vec![0u32; n], vec![0u32; n]);
    let mut twin = vec![(0i64, 0u32, 0u32); n];
    for &road in &order {
        let (s, m, p, key) = package.with_road(road, |r| {
            let walk = profile.mode(r) != BIKE;
            let s: f64 = r.shape.windows(2).map(|w| pace.segment_seconds(w[0], w[1], walk, profile.mtb_pace)).sum();
            (s as f32, r.length_m, r.shape.len() as u32, (r.way, r.from.min(r.to), r.from.max(r.to)))
        })?;
        (secs[road as usize], metres[road as usize], points[road as usize], twin[road as usize]) = (s, m, p, key);
    }
    let b = t.elapsed().as_secs_f64();
    println!("B: seconds of {} roads from geometry in {:.2} s = {:.2} M roads/s", order.len(), b, order.len() as f64 / b / 1e6);

    // Riding time at the end of each road, along the tree of the cost search.
    let mut time = vec![f32::MAX; n];
    for &road in &order {
        let r = road as usize;
        time[r] = match parent[r] {
            u32::MAX => {
                let f = seed_fraction.iter().find(|s| s.0 == road).map_or(0.0, |s| s.1);
                secs[r] * (1.0 - f as f32)
            }
            p => time[p as usize] + secs[r],
        };
    }
    println!("farthest road of the package: {:.1} h", order.iter().map(|&r| time[r as usize]).fold(0f32, f32::max) / 3600.0);

    // C: for each riding time, what is in reach and how far the cost search must go to find all of it.
    for hours in [1.0f32, 2.0, 3.0, 4.0, 6.0, 8.0] {
        let limit = hours * 3600.0;
        let (mut states, mut last, mut roads, mut km, mut pts) = (0usize, 0usize, HashSet::new(), 0f64, 0u64);
        for (i, &road) in order.iter().enumerate() {
            let r = road as usize;
            if time[r] <= limit {
                states += 1;
                last = i;
                if roads.insert(twin[r]) {
                    km += metres[r] as f64 / 1000.0;
                    pts += points[r] as u64;
                }
            }
        }
        // How far the cost search goes to find 99% and 99.9% of the states in reach.
        let (mut seen, mut at99, mut at999) = (0usize, 0usize, 0usize);
        for (i, &road) in order.iter().enumerate() {
            if time[road as usize] <= limit {
                seen += 1;
                if at99 == 0 && seen * 100 >= states * 99 {
                    at99 = i + 1;
                }
                if at999 == 0 && seen * 1000 >= states * 999 {
                    at999 = i + 1;
                }
            }
        }
        println!("  {hours} h: 99% of them after {at99} settled ({:.2}x), 99.9% after {at999} ({:.2}x)", at99 as f64 / states.max(1) as f64, at999 as f64 / states.max(1) as f64);
        let ceiling = cost[order[last] as usize];
        let (mut c2, mut p2) = (vec![u64::MAX; n], vec![u32::MAX; n]);
        let mut best = f64::MAX;
        let mut settled = 0;
        for _ in 0..3 {
            let t = Instant::now();
            let (o, ..) = sweep(graph, costs, &seeds, ceiling, &mut c2, &mut p2);
            best = best.min(t.elapsed().as_secs_f64());
            settled = o.len();
        }
        println!(
            "C {hours} h: {states} states in reach ({:.0}% of package), {} roads, {:.0} km, {pts} points | search to the last of them settles {settled} states ({:.2}x) in {:.0} ms | labels {:.0} MB at 16 B",
            states as f64 / n as f64 * 100.0,
            roads.len(),
            km,
            settled as f64 / states.max(1) as f64,
            best * 1000.0,
            settled as f64 * 16.0 / 1e6
        );
    }
    Ok(())
}
