//! Census probe (never committed) for #3651: tunnel ends, bridge chains, level-crossing points of a fixture.
//! tc ends FIXTURE PORTALS.json OUT_DIR   -> tunnel-ends.jsonl, bridges.jsonl, lx-points.jsonl
//! tc lx FIXTURE OUT.jsonl                -> the graph's level crossings of every cell (graph::build)
#![allow(clippy::all)]
#![recursion_limit = "512"]
#[path = "../tests/fixture/mod.rs"]
mod fixture;

use obc_ground_plan::faces::Rules;
use obc_ground_plan::frame::{from_lon_lat, metres_per_unit, GlobalPoint};
use obc_ground_plan::graph::{self, CrossingKind};
use obc_ground_plan::sections::Sections;
use obc_ground_plan::terrain::{pages_from_terrarium, PageKey, StoredPage, UNKNOWN};
use obc_ground_plan::tile::{self, Geometry, Tags};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::Path;

const WAY_KINDS: [&str; 6] = ["tunnel", "gallery", "main-road", "road", "path", "railway"];

// ---------- terrain: the bicubic of `Terrain::height`, over every page of the fixture ----------
struct Ter {
    pages: HashMap<PageKey, StoredPage>,
}
fn spline(t: f64) -> [f64; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [0.5 * (-t + 2.0 * t2 - t3), 0.5 * (2.0 - 5.0 * t2 + 3.0 * t3), 0.5 * (t + 4.0 * t2 - 3.0 * t3), 0.5 * (-t2 + t3)]
}
impl Ter {
    fn load(dir: &Path) -> Ter {
        let mut pages = HashMap::new();
        for z in [12u32, 13] {
            let zd = dir.join(format!("terrain/{z}"));
            let Ok(xs) = std::fs::read_dir(&zd) else { continue };
            for xe in xs.flatten() {
                let x: u32 = xe.file_name().to_string_lossy().parse().unwrap();
                for ye in std::fs::read_dir(xe.path()).unwrap().flatten() {
                    let n = ye.file_name().to_string_lossy().to_string();
                    let y: u32 = n.trim_end_matches(".webp").parse().unwrap();
                    let bytes = std::fs::read(ye.path()).unwrap();
                    let rgba = image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP).unwrap().to_rgba8();
                    let ps = pages_from_terrarium(rgba.as_raw(), rgba.width() as usize, z, x, y).unwrap();
                    for p in ps.into_iter().flatten() {
                        pages.insert(PageKey { level: p.level, x: p.x, y: p.y }, p);
                    }
                }
            }
        }
        Ter { pages }
    }
    fn h(&self, p: GlobalPoint) -> Option<f64> {
        [14u8, 13].iter().find_map(|&l| self.bicubic(l, p))
    }
    fn hf(&self, x: f64, y: f64) -> Option<f64> {
        self.h(GlobalPoint { x: x.round() as i64, y: y.round() as i64 })
    }
    fn bicubic(&self, level: u8, p: GlobalPoint) -> Option<f64> {
        let s = 34 - 8 - u32::from(level);
        let (vx, vy) = (p.x - (1 << (s - 1)), p.y - (1 << (s - 1)));
        let mask = (1i64 << s) - 1;
        let size = (1i64 << s) as f64;
        let (wx, wy) = (spline((vx & mask) as f64 / size), spline((vy & mask) as f64 / size));
        let (tx, ty) = (vx >> s, vy >> s);
        let mut sum = 0.0;
        for (j, wy) in wy.iter().enumerate() {
            for (i, wx) in wx.iter().enumerate() {
                let (x, y) = (tx + i as i64 - 1, ty + j as i64 - 1);
                if x < 0 || y < 0 {
                    return None;
                }
                let pg = self.pages.get(&PageKey { level, x: (x >> 8) as u32, y: (y >> 8) as u32 })?;
                let code = pg.codes[(y & 255) as usize * 256 + (x & 255) as usize];
                if code == UNKNOWN {
                    return None;
                }
                sum += f64::from(pg.offset + f32::from(code) * pg.scale) * wx * wy;
            }
        }
        Some(sum)
    }
}

// ---------- features ----------
#[derive(Clone)]
struct Attr {
    kind: String,
    tags: Tags,
}
#[derive(Clone, Copy)]
struct Seg {
    a: usize, // node ids
    b: usize,
    pa: [f64; 2],
    pb: [f64; 2],
    za: Option<f64>,
    zb: Option<f64>,
    attr: usize,
}
struct Area {
    ring: Vec<[f64; 2]>,
    z: Option<f64>,
    kind: String,
    bbox: [f64; 4],
}

fn mpu(y: f64) -> f64 {
    metres_per_unit(y as i64)
}
fn ll(p: [f64; 2]) -> [f64; 2] {
    fixture::lon_lat(p[0], p[1])
}
fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}
fn ro(v: Option<f64>) -> Value {
    v.map(|v| json!(r2(v))).unwrap_or(Value::Null)
}

fn is_tunnel(a: &Attr) -> bool {
    let t = a.tags.get("tunnel").map(String::as_str);
    a.kind == "tunnel" || a.kind == "gallery" || (t.is_some() && t != Some("no") && WAY_KINDS.contains(&a.kind.as_str()))
}
fn is_bridge(a: &Attr) -> bool {
    matches!(a.tags.get("bridge").map(String::as_str), Some(b) if !["no", "boardwalk", "low_water_crossing"].contains(&b))
}

struct Nodes {
    grid: HashMap<(i64, i64), Vec<usize>>,
    pts: Vec<[f64; 2]>,
}
impl Nodes {
    fn id(&mut self, p: [f64; 2]) -> usize {
        let tol = 0.3 / mpu(p[1]);
        let c = ((p[0] / 512.0).floor() as i64, (p[1] / 512.0).floor() as i64);
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(v) = self.grid.get(&(c.0 + dx, c.1 + dy)) {
                    for &i in v {
                        let q = self.pts[i];
                        if (q[0] - p[0]).hypot(q[1] - p[1]) <= tol {
                            return i;
                        }
                    }
                }
            }
        }
        self.pts.push(p);
        self.grid.entry(c).or_default().push(self.pts.len() - 1);
        self.pts.len() - 1
    }
}

/// A spatial index of segments by 64 m squares (units 2^15).
struct SegIndex {
    g: HashMap<(i64, i64), Vec<usize>>,
}
const SQ: f64 = 32768.0;
impl SegIndex {
    fn new(segs: &[Seg], keep: impl Fn(&Seg) -> bool) -> SegIndex {
        let mut g: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, s) in segs.iter().enumerate().filter(|(_, s)| keep(s)) {
            let (x0, x1) = (s.pa[0].min(s.pb[0]), s.pa[0].max(s.pb[0]));
            let (y0, y1) = (s.pa[1].min(s.pb[1]), s.pa[1].max(s.pb[1]));
            for cx in (x0 / SQ).floor() as i64..=(x1 / SQ).floor() as i64 {
                for cy in (y0 / SQ).floor() as i64..=(y1 / SQ).floor() as i64 {
                    g.entry((cx, cy)).or_default().push(i);
                }
            }
        }
        SegIndex { g }
    }
    fn near(&self, p: [f64; 2], r: f64) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        for cx in ((p[0] - r) / SQ).floor() as i64..=((p[0] + r) / SQ).floor() as i64 {
            for cy in ((p[1] - r) / SQ).floor() as i64..=((p[1] + r) / SQ).floor() as i64 {
                if let Some(v) = self.g.get(&(cx, cy)) {
                    out.extend(v);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

fn seg_dist(a: [f64; 2], b: [f64; 2], q: [f64; 2]) -> (f64, f64) {
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = (d[0] * d[0] + d[1] * d[1]).max(1e-9);
    let t = (((q[0] - a[0]) * d[0] + (q[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0);
    ((q[0] - a[0] - t * d[0]).hypot(q[1] - a[1] - t * d[1]), t)
}
fn cross(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> Option<(f64, f64)> {
    let r = [b[0] - a[0], b[1] - a[1]];
    let s = [d[0] - c[0], d[1] - c[1]];
    let den = r[0] * s[1] - r[1] * s[0];
    if den.abs() < 1e-9 {
        return None;
    }
    let t = ((c[0] - a[0]) * s[1] - (c[1] - a[1]) * s[0]) / den;
    let u = ((c[0] - a[0]) * r[1] - (c[1] - a[1]) * r[0]) / den;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some((t, u))
}
fn inside(ring: &[[f64; 2]], q: [f64; 2]) -> bool {
    let mut c = false;
    let n = ring.len();
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        if (a[1] > q[1]) != (b[1] > q[1]) && q[0] < (b[0] - a[0]) * (q[1] - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
    }
    c
}
fn lerp(a: Option<f64>, b: Option<f64>, t: f64) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a + t * (b - a)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        _ => None,
    }
}

/// A polyline walked from a node: points, heights, metres along, and the segments used.
struct Walk {
    pts: Vec<[f64; 2]>,
    z: Vec<Option<f64>>,
    s: Vec<f64>,
    segs: Vec<usize>,
    nodes: Vec<usize>,
}
impl Walk {
    fn at(&self, s: f64) -> Option<([f64; 2], [f64; 2], Option<f64>)> {
        if self.pts.len() < 2 || s > *self.s.last().unwrap() + 1e-6 {
            return None;
        }
        let i = self.s.windows(2).position(|w| s <= w[1] + 1e-9).unwrap_or(self.s.len() - 2);
        let l = (self.s[i + 1] - self.s[i]).max(1e-9);
        let t = ((s - self.s[i]) / l).clamp(0.0, 1.0);
        let (a, b) = (self.pts[i], self.pts[i + 1]);
        let p = [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])];
        let d = [(b[0] - a[0]), (b[1] - a[1])];
        let n = d[0].hypot(d[1]).max(1e-9);
        Some((p, [d[0] / n, d[1] / n], lerp(self.z[i], self.z[i + 1], t)))
    }
    fn len(&self) -> f64 {
        *self.s.last().unwrap_or(&0.0)
    }
}

/// From node `n` along the segments that `ok` allows, first `first`, at each node the straightest one; up to `max` m.
fn walk(segs: &[Seg], adj: &[Vec<usize>], n: usize, first: usize, ok: &dyn Fn(usize) -> bool, max: f64) -> Walk {
    let mut w = Walk { pts: vec![], z: vec![], s: vec![], segs: vec![], nodes: vec![n] };
    let (mut node, mut seg) = (n, first);
    let mut seen = HashSet::new();
    loop {
        let sg = &segs[seg];
        let (p0, z0, p1, z1, next) =
            if sg.a == node { (sg.pa, sg.za, sg.pb, sg.zb, sg.b) } else { (sg.pb, sg.zb, sg.pa, sg.za, sg.a) };
        if w.pts.is_empty() {
            w.pts.push(p0);
            w.z.push(z0);
            w.s.push(0.0);
        }
        let l = (p1[0] - p0[0]).hypot(p1[1] - p0[1]) * mpu(p0[1]);
        w.pts.push(p1);
        w.z.push(z1);
        w.s.push(w.s.last().unwrap() + l);
        w.segs.push(seg);
        w.nodes.push(next);
        seen.insert(seg);
        if *w.s.last().unwrap() >= max {
            break;
        }
        let dir = [p1[0] - p0[0], p1[1] - p0[1]];
        let dl = dir[0].hypot(dir[1]).max(1e-9);
        let best = adj[next]
            .iter()
            .filter(|&&k| !seen.contains(&k) && ok(k))
            .map(|&k| {
                let s2 = &segs[k];
                let q = if s2.a == next { s2.pb } else { s2.pa };
                let d2 = [q[0] - p1[0], q[1] - p1[1]];
                let c = (dir[0] * d2[0] + dir[1] * d2[1]) / (dl * d2[0].hypot(d2[1]).max(1e-9));
                (k, c)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((k, c)) if c > 0.0 => {
                node = next;
                seg = k;
            }
            _ => break,
        }
    }
    w
}

fn network(a: &Attr) -> String {
    if let Some(r) = a.tags.get("railway") {
        if !a.tags.contains_key("highway") {
            return format!("rail:{r}");
        }
    }
    let h = a.tags.get("highway").map(String::as_str).unwrap_or("?");
    match h {
        "motorway" | "motorway_link" | "trunk" | "trunk_link" => "motorway/trunk".into(),
        "service" | "track" => "service/track".into(),
        "footway" | "cycleway" | "path" | "steps" | "bridleway" | "pedestrian" => "path".into(),
        _ => "road".into(),
    }
}
fn paved(a: &Attr) -> &'static str {
    let s = a.tags.get("surface").map(String::as_str);
    const P: [&str; 10] =
        ["asphalt", "concrete", "paved", "paving_stones", "sett", "cobblestone", "concrete:plates", "chipseal", "metal", "concrete:lanes"];
    match s {
        Some(s) if P.contains(&s) => "paved",
        Some(_) => "unpaved",
        None => {
            if a.tags.contains_key("railway") && !a.tags.contains_key("highway") {
                "rail"
            } else {
                match a.tags.get("highway").map(String::as_str) {
                    Some("track" | "path" | "bridleway") => "unpaved?",
                    Some("footway" | "cycleway" | "steps" | "pedestrian") => "paved?",
                    _ => "paved?",
                }
            }
        }
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a[1].as_str() {
        "ends" => ends(&a),
        "lx" => lx(&a),
        "pt" => ends(&a),
        "grid" => ends(&a),
        _ => panic!("mode"),
    }
}

fn lx(a: &[String]) {
    let dir = Path::new(&a[2]);
    let mut fix = fixture::Fixture::open(dir);
    let cells: Vec<_> = (fix.cells().into_iter())
        .filter(|k| k.feature_tiles().iter().all(|&(c, r)| fixture::feature_tile(dir, c, r).is_some()))
        .collect();
    eprintln!("cells {}", cells.len());
    let rules = Rules::embedded();
    let lines = fix.map(&cells, |key, input| {
        let g = graph::build(key, &input, &rules.sections);
        let o = g.cell.origin();
        let mut out = Vec::new();
        for c in g.crossings.iter().filter(|c| c.owned && c.kind == CrossingKind::LevelCrossing) {
            let p = fixture::lon_lat((o.x + i64::from(c.pos[0])) as f64, (o.y + i64::from(c.pos[1])) as f64);
            let fe = |st: &graph::Station| &input.features[g.parts[st.part as usize].feature.0 as usize];
            let (fo, fu) = (fe(&c.over), fe(&c.under));
            let tag = |f: &obc_ground_plan::tile::Feature| {
                ["highway", "railway", "waterway"].iter().find_map(|k| f.tags.get(*k).map(|v| format!("{k}={v}"))).unwrap_or_default()
            };
            out.push(json!({"lon": p[0], "lat": p[1], "source": format!("{:?}", c.source), "angle": r2(c.angle_rad.to_degrees()),
                "over_kind": fo.kind, "over": tag(fo), "under_kind": fu.kind, "under": tag(fu),
                "over_tags": fo.tags, "under_tags": fu.tags}));
        }
        out
    });
    let mut f = std::fs::File::create(&a[3]).unwrap();
    let mut n = 0;
    for l in lines.into_iter().flatten() {
        writeln!(f, "{l}").unwrap();
        n += 1;
    }
    eprintln!("level crossings {n}");
}

fn ends(a: &[String]) {
    let dir = Path::new(&a[2]);
    let fix = fixture::Fixture::open(dir);
    let [west, south, east, north] = fix.bounds;
    let (nw, se) = (from_lon_lat(west, north), from_lon_lat(east, south));
    let in_box = |p: [f64; 2]| p[0] >= nw.x as f64 && p[0] <= se.x as f64 && p[1] >= nw.y as f64 && p[1] <= se.y as f64;
    let portals: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&a[3]).unwrap()).unwrap();
    let out = Path::new(&a[4]);
    let sections = Sections::embedded();

    // Read every tile; dedup segments, areas and points across tile copies.
    let mut attrs: Vec<Attr> = vec![];
    let mut attr_id: HashMap<String, usize> = HashMap::new();
    let mut nodes = Nodes { grid: HashMap::new(), pts: vec![] };
    let mut segs: Vec<Seg> = vec![];
    let mut seg_seen: HashSet<(i64, i64, i64, i64, usize)> = HashSet::new();
    let mut areas: Vec<Area> = vec![];
    let mut area_seen = HashSet::new();
    let mut points: Vec<(Attr, [f64; 2])> = vec![];
    let mut point_seen = HashSet::new();
    let fdir = dir.join("features/13");
    for ce in std::fs::read_dir(&fdir).unwrap().flatten() {
        let c: u32 = ce.file_name().to_string_lossy().parse().unwrap();
        for re in std::fs::read_dir(ce.path()).unwrap().flatten() {
            let r: u32 = re.file_name().to_string_lossy().trim_end_matches(".bin").parse().unwrap();
            let bytes = fixture::feature_tile(dir, c, r).unwrap();
            for f in tile::features(&bytes, c, r).unwrap() {
                match f.geometry {
                    Geometry::Line if WAY_KINDS.contains(&f.kind.as_str()) || f.kind == "river" || f.kind == "stream" => {
                        let key = format!("{}|{:?}", f.kind, f.tags);
                        let ai = *attr_id.entry(key).or_insert_with(|| {
                            attrs.push(Attr { kind: f.kind.clone(), tags: f.tags.clone() });
                            attrs.len() - 1
                        });
                        for p in &f.parts {
                            for i in 0..p.points.len().saturating_sub(1) {
                                let (pa, pb) = (p.points[i], p.points[i + 1]);
                                let k = if (pa.x, pa.y) <= (pb.x, pb.y) { (pa.x, pa.y, pb.x, pb.y, ai) } else { (pb.x, pb.y, pa.x, pa.y, ai) };
                                if pa == pb || !seg_seen.insert(k) {
                                    continue;
                                }
                                let z = |j: usize| p.heights.as_ref().and_then(|h| h[j]);
                                let (fa, fb) = ([pa.x as f64, pa.y as f64], [pb.x as f64, pb.y as f64]);
                                let (na, nb) = (nodes.id(fa), nodes.id(fb));
                                segs.push(Seg { a: na, b: nb, pa: fa, pb: fb, za: z(i), zb: z(i + 1), attr: ai });
                            }
                        }
                    }
                    Geometry::Area if ["building", "building-part", "river-area", "lake"].contains(&f.kind.as_str()) => {
                        for p in f.parts.iter().filter(|p| p.outer && p.points.len() >= 3) {
                            let k = (p.points[0], p.points[1], p.points.len(), f.kind.clone());
                            if !area_seen.insert(k) {
                                continue;
                            }
                            let ring: Vec<[f64; 2]> = p.points.iter().map(|q| [q.x as f64, q.y as f64]).collect();
                            let bbox = [
                                ring.iter().map(|q| q[0]).fold(f64::MAX, f64::min),
                                ring.iter().map(|q| q[1]).fold(f64::MAX, f64::min),
                                ring.iter().map(|q| q[0]).fold(f64::MIN, f64::max),
                                ring.iter().map(|q| q[1]).fold(f64::MIN, f64::max),
                            ];
                            let z = p.heights.as_ref().and_then(|h| h.iter().flatten().cloned().reduce(f64::min));
                            areas.push(Area { ring, z, kind: f.kind.clone(), bbox });
                        }
                    }
                    Geometry::Point if f.kind == "level-crossing" => {
                        for p in &f.parts {
                            for q in &p.points {
                                if point_seen.insert((q.x, q.y)) {
                                    points.push((Attr { kind: f.kind.clone(), tags: f.tags.clone() }, [q.x as f64, q.y as f64]));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    eprintln!("segs {} nodes {} areas {} lx points {}", segs.len(), nodes.pts.len(), areas.len(), points.len());
    let ter = Ter::load(dir);
    eprintln!("pages {}", ter.pages.len());

    let mut adj: Vec<Vec<usize>> = vec![vec![]; nodes.pts.len()];
    for (i, s) in segs.iter().enumerate() {
        adj[s.a].push(i);
        adj[s.b].push(i);
    }
    let water = |s: &Seg| attrs[s.attr].kind == "river" || attrs[s.attr].kind == "stream";
    let tun = |i: usize| !water(&segs[i]) && is_tunnel(&attrs[segs[i].attr]);
    let brg = |i: usize| !water(&segs[i]) && !tun(i) && is_bridge(&attrs[segs[i].attr]);
    let ground = |i: usize| !water(&segs[i]) && !tun(i) && !brg(i);

    // Components of tunnel segments and of bridge segments.
    fn comps(n: usize, segs: &[Seg], adj: &[Vec<usize>], keep: &dyn Fn(usize) -> bool) -> Vec<usize> {
        let mut comp = vec![usize::MAX; n];
        let mut next = 0;
        for i in 0..n {
            if comp[i] != usize::MAX || !keep(i) {
                continue;
            }
            let mut st = vec![i];
            comp[i] = next;
            while let Some(k) = st.pop() {
                for nd in [segs[k].a, segs[k].b] {
                    for &j in &adj[nd] {
                        if comp[j] == usize::MAX && keep(j) {
                            comp[j] = next;
                            st.push(j);
                        }
                    }
                }
            }
            next += 1;
        }
        comp
    }
    let tcomp = comps(segs.len(), &segs, &adj, &tun);
    let bcomp = comps(segs.len(), &segs, &adj, &brg);
    let seg_len = |s: &Seg| (s.pb[0] - s.pa[0]).hypot(s.pb[1] - s.pa[1]) * mpu(s.pa[1]);
    let mut clen: HashMap<usize, f64> = HashMap::new();
    let mut cout: HashSet<usize> = HashSet::new();
    for (i, s) in segs.iter().enumerate().filter(|(i, _)| tun(*i)) {
        *clen.entry(tcomp[i]).or_default() += seg_len(s);
        if !in_box(s.pa) || !in_box(s.pb) {
            cout.insert(tcomp[i]);
        }
    }

    let idx_ground = SegIndex::new(&segs, |s| !water(s) && !is_tunnel(&attrs[s.attr]) && !is_bridge(&attrs[s.attr]));
    let idx_bridge = SegIndex::new(&segs, |s| !water(s) && !is_tunnel(&attrs[s.attr]) && is_bridge(&attrs[s.attr]));
    let idx_all = SegIndex::new(&segs, |_| true);
    let mut bgrid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, ar) in areas.iter().enumerate() {
        for cx in (ar.bbox[0] / SQ).floor() as i64..=(ar.bbox[2] / SQ).floor() as i64 {
            for cy in (ar.bbox[1] / SQ).floor() as i64..=(ar.bbox[3] / SQ).floor() as i64 {
                bgrid.entry((cx, cy)).or_default().push(i);
            }
        }
    }
    let portal_pts: Vec<[f64; 2]> = portals
        .iter()
        .map(|p| {
            let g = from_lon_lat(p["lon"].as_f64().unwrap(), p["lat"].as_f64().unwrap());
            [g.x as f64, g.y as f64]
        })
        .collect();

    if a[1] == "pt" {
        let mut fo = std::fs::File::create(out.join("pts.jsonl")).unwrap();
        let lln = |p: [f64; 2]| { let q = ll(p); json!([q[1], q[0]]) };
        for line in std::fs::read_to_string(&a[5]).unwrap().lines().filter(|l| !l.trim().is_empty()) {
            let f: Vec<&str> = line.split_whitespace().collect();
            let (id, ty, lat, lon) = (f[0], f[1], f[2].parse::<f64>().unwrap(), f[3].parse::<f64>().unwrap());
            let g = from_lon_lat(lon, lat);
            let q = [g.x as f64, g.y as f64];
            let m = mpu(q[1]);
            let head = |from: [f64; 2], to: [f64; 2]| ((to[0] - from[0]).atan2(-(to[1] - from[1])).to_degrees() + 360.0) % 360.0;
            if ty == "lx" {
                let best = idx_ground.near(q, 5.0 / m).into_iter()
                    .filter(|&j| { let aj = &attrs[segs[j].attr]; ground(j) && !(aj.tags.contains_key("railway") && !aj.tags.contains_key("highway")) })
                    .map(|j| (j, seg_dist(segs[j].pa, segs[j].pb, q))).min_by(|x, y| x.1 .0.total_cmp(&y.1 .0));
                let Some((j, (d, t))) = best else { writeln!(fo, "{}", json!({"id": id, "error": "no way"})).unwrap(); continue };
                let sj = segs[j];
                let len = seg_len(&sj);
                let okw = |k: usize| ground(k) && !(attrs[segs[k].attr].tags.contains_key("railway") && !attrs[segs[k].attr].tags.contains_key("highway"));
                let wa = walk(&segs, &adj, sj.a, j, &okw, t * len + 40.0);
                let wb = walk(&segs, &adj, sj.b, j, &okw, (1.0 - t) * len + 40.0);
                let pz = lerp(sj.za, sj.zb, t);
                let mut sides = vec![];
                for (w, s0) in [(&wa, t * len), (&wb, (1.0 - t) * len)] {
                    // From 25 m before P on the other side, looking at P: the point 25 m beyond P on this walk.
                    if let Some((p, _, z)) = w.at(s0 + 25.0) {
                        sides.push(json!({"at": lln(p), "z": ro(z), "ter": ro(ter.hf(p[0], p[1])), "heading": r2(head(p, q))}));
                    }
                }
                let aj = &attrs[sj.attr];
                writeln!(fo, "{}", json!({"id": id, "type": ty, "d": r2(d * m), "class": aj.tags.get("highway"), "z": ro(pz), "ter": ro(ter.hf(q[0], q[1])), "sides": sides})).unwrap();
            } else {
                // A bridge end: the nearest node of a bridge segment, the ground way that meets it.
                let best = idx_bridge.near(q, 5.0 / m).into_iter().flat_map(|j| [(j, segs[j].a, segs[j].pa), (j, segs[j].b, segs[j].pb)])
                    .filter(|(_, n, _)| adj[*n].iter().any(|&k| ground(k)))
                    .min_by(|x, y| ((x.2[0] - q[0]).hypot(x.2[1] - q[1])).total_cmp(&(y.2[0] - q[0]).hypot(y.2[1] - q[1])));
                let Some((j, n, p0)) = best else { writeln!(fo, "{}", json!({"id": id, "error": "no end"})).unwrap(); continue };
                let sj = segs[j];
                let into = { let o = if sj.a == n { sj.pb } else { sj.pa }; let d = [o[0] - p0[0], o[1] - p0[1]]; let l = d[0].hypot(d[1]); [d[0] / l, d[1] / l] };
                let zn = if sj.a == n { sj.za } else { sj.zb };
                let gk = adj[n].iter().copied().filter(|&k| ground(k)).max_by(|&x, &y| {
                    let dir = |k: usize| { let s2 = &segs[k]; let o = if s2.a == n { s2.pb } else { s2.pa }; let d = [o[0] - p0[0], o[1] - p0[1]]; -(d[0] * into[0] + d[1] * into[1]) / d[0].hypot(d[1]) };
                    dir(x).total_cmp(&dir(y))
                }).unwrap();
                let w = walk(&segs, &adj, n, gk, &|k| ground(k), 45.0);
                let aj = &attrs[sj.attr];
                let ws = sections.way(&aj.kind, &aj.tags, false);
                let mut v = json!({"id": id, "type": ty, "end": lln(p0), "z": ro(zn), "ter": ro(ter.hf(p0[0], p0[1])), "class": aj.tags.get("highway").or(aj.tags.get("railway")), "half": ws.half_m,
                    "into_heading": r2(head(p0, [p0[0] + into[0], p0[1] + into[1]]))});
                if let Some((p, t, z)) = w.at(30f64.min(w.len())) {
                    let nrm = [-t[1], t[0]];
                    let u = (ws.half_m + 4.0) / m;
                    let side = [p[0] + nrm[0] * u, p[1] + nrm[1] * u];
                    v["approach"] = json!({"at": lln(p), "z": ro(z), "ter": ro(ter.hf(p[0], p[1])), "heading": r2(head(p, p0)),
                        "beside": lln(side), "beside_ter": ro(ter.hf(side[0], side[1])), "beside_heading": r2(head(side, p0))});
                }
                // Side view: 25 m square to the bridge, from the side with the lower terrain, 6 m along into the bridge.
                let c = [p0[0] + into[0] * 6.0 / m, p0[1] + into[1] * 6.0 / m];
                let nrm = [-into[1], into[0]];
                let mut opts = vec![];
                for sg in [1.0, -1.0] {
                    let sp = [c[0] + sg * nrm[0] * 25.0 / m, c[1] + sg * nrm[1] * 25.0 / m];
                    opts.push(json!({"at": lln(sp), "ter": ro(ter.hf(sp[0], sp[1])), "heading": r2(head(sp, p0))}));
                }
                v["side"] = json!(opts);
                writeln!(fo, "{v}").unwrap();
            }
        }
        return;
    }
    // ---------- Part A ----------
    let mut fa = std::fs::File::create(out.join("tunnel-ends.jsonl")).unwrap();
    let mut n_ends = 0;
    for nd in 0..nodes.pts.len() {
        let tsegs: Vec<usize> = adj[nd].iter().copied().filter(|&i| tun(i)).collect();
        if tsegs.len() != 1 || !in_box(nodes.pts[nd]) {
            continue;
        }
        let t0 = tsegs[0];
        let at = &attrs[segs[t0].attr];
        let ws = sections.way(&at.kind, &at.tags, false);
        let (half, outer, bore) = (ws.half_m, ws.half_m + ws.shoulder_m.max(ws.sidewalk_m), ws.bore_m);
        let comp = tcomp[t0];
        let chain = clen[&comp];
        let w = walk(&segs, &adj, nd, t0, &|i| tun(i), chain + 1.0);
        let p0 = nodes.pts[nd];
        let m = mpu(p0[1]);
        // Heading into the tube over the first 10 m.
        let (into, head) = {
            let q = w.at(10f64.min(w.len())).map(|x| x.0).unwrap_or(p0);
            let d = [q[0] - p0[0], q[1] - p0[1]];
            let l = d[0].hypot(d[1]).max(1e-9);
            let d = [d[0] / l, d[1] / l];
            (d, (d[0].atan2(-d[1]).to_degrees() + 360.0) % 360.0)
        };
        let floor0 = w.z.first().cloned().flatten();
        let ll0 = ll(p0);
        // Least terrain across the tube minus floor at s.
        let least = |s: f64| -> Option<(f64, f64)> {
            let (p, t, z) = w.at(s)?;
            let z = z?;
            let n = [-t[1], t[0]];
            let u = outer / m;
            let hs = [ter.hf(p[0], p[1])?, ter.hf(p[0] + n[0] * u, p[1] + n[1] * u)?, ter.hf(p[0] - n[0] * u, p[1] - n[1] * u)?];
            Some((hs.iter().cloned().fold(f64::MAX, f64::min) - z, z))
        };
        let prof: Vec<Value> = [0.0, 5.0, 10.0, 20.0, 30.0, 50.0, 100.0].iter().map(|&s| ro(least(s).map(|x| x.0))).collect();
        let lim = 300f64.min(chain / 2.0).min(w.len());
        let mut d_cover: Option<f64> = None;
        let mut s = 0.0;
        while s <= lim {
            if let Some((d, _)) = least(s) {
                if d >= bore + 0.5 {
                    d_cover = Some(s);
                    break;
                }
            }
            s += 1.0;
        }
        // The same at the centre line only.
        let mut d_cover_c: Option<f64> = None;
        let mut s = 0.0;
        while s <= lim {
            if let Some((p, _, Some(z))) = w.at(s) {
                if ter.hf(p[0], p[1]).is_some_and(|t| t - z >= bore + 0.5) {
                    d_cover_c = Some(s);
                    break;
                }
            }
            s += 1.0;
        }
        // The same with the bore_least room (for comparison with the bake's portal rule).
        let mut d_least: Option<f64> = None;
        let mut s = 0.0;
        while s <= lim {
            if let Some((d, _)) = least(s) {
                if d >= ws.bore_least_m + ws.roof_least_m {
                    d_least = Some(s);
                    break;
                }
            }
            s += 1.0;
        }
        let (mut out_n, mut tot_n, mut worst) = (0, 0, 0.0f64);
        let mut s = 0.0;
        while s <= w.len() {
            if let Some((d, _)) = least(s) {
                tot_n += 1;
                if d < bore {
                    out_n += 1;
                    worst = worst.max(bore - d);
                }
            }
            s += 1.0;
        }
        // What meets the end node.
        let others: Vec<usize> = adj[nd].iter().copied().filter(|&i| i != t0).collect();
        let meets_ground = others.iter().filter(|&&i| ground(i)).count();
        let meets_bridge = others.iter().filter(|&&i| brg(i)).count();
        let meets_water = others.iter().filter(|&&i| water(&segs[i])).count();
        let meets = if meets_ground > 0 {
            "ground"
        } else if meets_bridge > 0 {
            "bridge"
        } else if meets_water > 0 {
            "water"
        } else {
            "nothing"
        };
        // Approach: the straightest non-tunnel way leading away from the tube.
        let lead = others
            .iter()
            .filter(|&&i| !water(&segs[i]))
            .map(|&i| {
                let s2 = &segs[i];
                let q = if s2.a == nd { s2.pb } else { s2.pa };
                let d = [q[0] - p0[0], q[1] - p0[1]];
                (i, -(d[0] * into[0] + d[1] * into[1]) / d[0].hypot(d[1]).max(1e-9))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1));
        let ow = lead.map(|(i, _)| walk(&segs, &adj, nd, i, &|k| !tun(k) && !water(&segs[k]), 40.0));
        let mut appr = vec![];
        for &d in &[0.0, 10.0, 20.0, 30.0] {
            let (p, t, z) = match &ow {
                Some(ow) => match ow.at(d) {
                    Some(x) => x,
                    None => {
                        appr.push(json!(null));
                        continue;
                    }
                },
                None => {
                    let u = d / m;
                    ([p0[0] - into[0] * u, p0[1] - into[1] * u], [-into[0], -into[1]], floor0)
                }
            };
            let n = [-t[1], t[0]];
            let u = (half + 2.0) / m;
            let c = ter.hf(p[0], p[1]);
            let l = ter.hf(p[0] + n[0] * u, p[1] + n[1] * u);
            let r = ter.hf(p[0] - n[0] * u, p[1] - n[1] * u);
            let rel = |v: Option<f64>| match (v, z) {
                (Some(v), Some(z)) => json!(r2(v - z)),
                _ => Value::Null,
            };
            appr.push(json!([rel(c), rel(l), rel(r)]));
        }
        let cross_slope = {
            let n = [-into[1], into[0]];
            let u = (half + 2.0) / m;
            match (ter.hf(p0[0] + n[0] * u, p0[1] + n[1] * u), ter.hf(p0[0] - n[0] * u, p0[1] - n[1] * u)) {
                (Some(l), Some(r)) => Some((l - r) / (2.0 * (half + 2.0))),
                _ => None,
            }
        };
        // On top: ground ways crossing the tube's centre line; buildings over the tube's centre.
        let wset: HashSet<usize> = w.segs.iter().copied().collect();
        let wnodes: HashSet<usize> = w.nodes.iter().copied().collect();
        let mut tops = vec![];
        let top_lim = w.len().min(300.0);
        for k in 0..w.pts.len() - 1 {
            if w.s[k] > top_lim {
                break;
            }
            let (a0, b0) = (w.pts[k], w.pts[k + 1]);
            let mid = [(a0[0] + b0[0]) / 2.0, (a0[1] + b0[1]) / 2.0];
            let r = (b0[0] - a0[0]).hypot(b0[1] - a0[1]) / 2.0 + 10.0;
            for j in idx_ground.near(mid, r) {
                let sj = &segs[j];
                if wset.contains(&j) || wnodes.contains(&sj.a) || wnodes.contains(&sj.b) {
                    continue;
                }
                if let Some((t, u)) = cross(a0, b0, sj.pa, sj.pb) {
                    let s = w.s[k] + t * (w.s[k + 1] - w.s[k]);
                    let zf = lerp(w.z[k], w.z[k + 1], t);
                    let zt = lerp(sj.za, sj.zb, u);
                    let aj = &attrs[sj.attr];
                    let cls = aj.tags.get("highway").or(aj.tags.get("railway")).cloned().unwrap_or_default();
                    let room = match (zt, zf) {
                        (Some(a), Some(b)) => Some(a - b),
                        _ => None,
                    };
                    tops.push(json!({"what": "way", "kind": aj.kind, "class": cls, "paved": paved(aj), "s": r2(s), "room": ro(room)}));
                }
            }
        }
        let mut bseen = HashSet::new();
        let mut s = 0.0;
        while s <= top_lim {
            if let Some((p, _, zf)) = w.at(s) {
                let c = ((p[0] / SQ).floor() as i64, (p[1] / SQ).floor() as i64);
                for &bi in bgrid.get(&c).map(|v| v.as_slice()).unwrap_or(&[]) {
                    let ar = &areas[bi];
                    if !ar.kind.starts_with("building") || bseen.contains(&bi) {
                        continue;
                    }
                    if p[0] >= ar.bbox[0] && p[0] <= ar.bbox[2] && p[1] >= ar.bbox[1] && p[1] <= ar.bbox[3] && inside(&ar.ring, p) {
                        bseen.insert(bi);
                        let base = ar.z.or_else(|| ter.hf(p[0], p[1]));
                        let room = match (base, zf) {
                            (Some(a), Some(b)) => Some(a - b),
                            _ => None,
                        };
                        tops.push(json!({"what": "building", "kind": ar.kind, "s": r2(s), "room": ro(room)}));
                    }
                }
            }
            s += 1.0;
        }
        // Bridge decks over the end within 15 m (not sharing the end node).
        let mut decks = vec![];
        for j in idx_bridge.near(p0, 15.0 / m) {
            let sj = &segs[j];
            if sj.a == nd || sj.b == nd {
                continue;
            }
            let (d, t) = seg_dist(sj.pa, sj.pb, p0);
            if d * m <= 15.0 {
                let zt = lerp(sj.za, sj.zb, t);
                if let (Some(zt), Some(zf)) = (zt, floor0) {
                    if zt > zf + 2.0 {
                        let aj = &attrs[sj.attr];
                        decks.push(json!({"d": r2(d * m), "over": r2(zt - zf), "class": aj.tags.get("highway").or(aj.tags.get("railway"))}));
                    }
                }
            }
        }
        let census = portal_pts
            .iter()
            .enumerate()
            .map(|(i, q)| (i, (q[0] - p0[0]).hypot(q[1] - p0[1]) * m))
            .filter(|x| x.1 <= 5.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|x| x.0);
        let lv = at.tags.get("levels").cloned();
        // PROTO grid: along the straight line of `into` (s > 0 in, s < 0 out), floor z and terrain at
        // lateral offsets -16..=16 m (positive = LEFT of the way into the tube, as tunnels.ts).
        let left = [into[1], -into[0]];
        let mut gin = vec![];
        let mut gout = vec![];
        if a[1] == "grid" {
            for si in -40i32..=60 {
                let s = f64::from(si);
                let z = if s >= 0.0 {
                    w.at(s).and_then(|x| x.2)
                } else {
                    match &ow {
                        Some(ow) => ow.at(-s).and_then(|x| x.2),
                        None => None,
                    }
                };
                let c = [p0[0] + into[0] * s / m, p0[1] + into[1] * s / m];
                let hs: Vec<Value> = (-16i32..=16)
                    .map(|k| {
                        let u = f64::from(k) / m;
                        ter.hf(c[0] + left[0] * u, c[1] + left[1] * u).map(|v| json!((v * 10.0).round() / 10.0)).unwrap_or(Value::Null)
                    })
                    .collect();
                let rowv = json!([z.map(|v| (v * 100.0).round() / 100.0), hs]);
                if s >= 0.0 {
                    gin.push(rowv)
                } else {
                    gout.push(rowv)
                }
            }
            gout.reverse();
        }
        let row = json!({
            "gin": gin, "gout": gout,
            "node": nd, "lat": ll0[1], "lon": ll0[0], "x": p0[0], "y": p0[1], "heading": r2(head), "into": into, "census": census,
            "comp": comp, "network": network(at), "kind": at.kind, "highway": at.tags.get("highway"), "railway": at.tags.get("railway"),
            "class": ws.class, "half": r2(half), "outer": r2(outer), "bore": bore, "bore_least": ws.bore_least_m, "roof_least": ws.roof_least_m,
            "oneway": at.tags.get("oneway"), "lanes": at.tags.get("lanes"), "surface": at.tags.get("surface"), "paved": paved(at),
            "tunnel": at.tags.get("tunnel"), "layer": at.tags.get("layer"), "width": at.tags.get("width"), "levels": lv,
            "tracks": at.tags.get("tracks"), "gauge": at.tags.get("gauge"),
            "chain": r2(chain), "chain_cut": cout.contains(&comp), "walk": r2(w.len()),
            "meets": meets, "meets_ground": meets_ground, "meets_bridge": meets_bridge, "meets_water": meets_water, "degree": adj[nd].len(),
            "floor0": ro(floor0), "prof": prof, "d_cover": d_cover, "d_cover_c": d_cover_c, "d_least": d_least, "lim": r2(lim),
            "out_share": if tot_n > 0 { r2(out_n as f64 / tot_n as f64) } else { -1.0 }, "worst": r2(worst),
            "approach": appr, "approach_way": lead.map(|(i, _)| { let aj = &attrs[segs[i].attr]; format!("{}:{}", aj.kind, aj.tags.get("highway").or(aj.tags.get("railway")).cloned().unwrap_or_default()) }),
            "cross_slope": ro(cross_slope), "tops": tops, "decks": decks,
        });
        writeln!(fa, "{row}").unwrap();
        n_ends += 1;
    }
    eprintln!("tunnel ends {n_ends}");

    // ---------- Part C: bridge chains ----------
    let mut fb = std::fs::File::create(out.join("bridges.jsonl")).unwrap();
    let mut by: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..segs.len() {
        if bcomp[i] != usize::MAX {
            by.entry(bcomp[i]).or_default().push(i);
        }
    }
    for (c, list) in by {
        let len: f64 = list.iter().map(|&i| seg_len(&segs[i])).sum();
        let at = &attrs[segs[list[0]].attr];
        let mid = segs[list[0]].pa;
        if !in_box(mid) {
            continue;
        }
        let set: HashSet<usize> = list.iter().copied().collect();
        let mut over: BTreeMap<String, u32> = BTreeMap::new();
        for &i in &list {
            let si = &segs[i];
            let mp = [(si.pa[0] + si.pb[0]) / 2.0, (si.pa[1] + si.pb[1]) / 2.0];
            let r = (si.pb[0] - si.pa[0]).hypot(si.pb[1] - si.pa[1]) / 2.0 + 10.0;
            for j in idx_all.near(mp, r) {
                let sj = &segs[j];
                if set.contains(&j) || sj.a == si.a || sj.a == si.b || sj.b == si.a || sj.b == si.b {
                    continue;
                }
                if cross(si.pa, si.pb, sj.pa, sj.pb).is_some() {
                    let aj = &attrs[sj.attr];
                    let key = if water(sj) {
                        aj.kind.clone()
                    } else if aj.tags.contains_key("railway") && !aj.tags.contains_key("highway") {
                        "rail".into()
                    } else if is_bridge(aj) {
                        "bridge-way".into()
                    } else {
                        format!("way:{}", network(aj))
                    };
                    *over.entry(key).or_default() += 1;
                }
            }
            for ar in areas.iter().filter(|ar| !ar.kind.starts_with("building")) {
                if mp[0] < ar.bbox[0] || mp[0] > ar.bbox[2] || mp[1] < ar.bbox[1] || mp[1] > ar.bbox[3] {
                    continue;
                }
                if inside(&ar.ring, mp) {
                    *over.entry(ar.kind.clone()).or_default() += 1;
                }
            }
        }
        // Ends of the chain: nodes with one bridge segment; what meets there.
        let mut ends_v = vec![];
        let mut ns: HashMap<usize, u32> = HashMap::new();
        for &i in &list {
            *ns.entry(segs[i].a).or_default() += 1;
            *ns.entry(segs[i].b).or_default() += 1;
        }
        for (&nd, &k) in ns.iter().filter(|x| *x.1 == 1) {
            let g = adj[nd].iter().filter(|&&i| ground(i)).count();
            let t = adj[nd].iter().filter(|&&i| tun(i)).count();
            let p = ll(nodes.pts[nd]);
            let _ = k;
            ends_v.push(json!({"lat": p[1], "lon": p[0], "ground": g, "tunnel": t, "degree": adj[nd].len()}));
        }
        let p = ll(mid);
        writeln!(fb, "{}", json!({"comp": c, "lat": p[1], "lon": p[0], "network": network(at), "kind": at.kind, "class": at.tags.get("highway").or(at.tags.get("railway")),
            "bridge": at.tags.get("bridge"), "layer": at.tags.get("layer"), "len": r2(len), "over": over, "ends": ends_v, "segs": list.len()})).unwrap();
    }

    // ---------- Part B: level-crossing points ----------
    let mut fl = std::fs::File::create(out.join("lx-points.jsonl")).unwrap();
    for (at, p) in &points {
        if !in_box(*p) {
            continue;
        }
        let m = mpu(p[1]);
        let mut near = vec![];
        for j in idx_all.near(*p, 3.0 / m) {
            let sj = &segs[j];
            let (d, _) = seg_dist(sj.pa, sj.pb, *p);
            if d * m <= 1.0 && !water(sj) {
                let aj = &attrs[sj.attr];
                near.push(json!({"kind": aj.kind, "class": aj.tags.get("highway").or(aj.tags.get("railway")), "rail": aj.tags.contains_key("railway") && !aj.tags.contains_key("highway"),
                    "bridge": is_bridge(aj), "tunnel": is_tunnel(aj), "d": r2(d * m)}));
            }
        }
        let q = ll(*p);
        writeln!(fl, "{}", json!({"lat": q[1], "lon": q[0], "tags": at.tags, "near": near})).unwrap();
    }
    eprintln!("done");
}
