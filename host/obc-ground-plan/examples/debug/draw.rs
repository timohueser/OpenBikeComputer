//! The layers of the debug pictures. Each layer draws each cell's own content under a mask of the
//! cell's square, so every cell line shows two independent builds.

use std::collections::BTreeMap;
use std::time::Instant;

use core::f64::consts::PI;

use obc_ground_plan::frame::{CellKey, GlobalPoint, CELL_UNITS};
use obc_ground_plan::sections::Sections;
use obc_ground_plan::tile::{tile_square, Feature, Geometry, Part};
use obc_ground_plan::CellInput;
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Mask, Paint, PathBuilder, Pixmap, Rect, Stroke, StrokeDash, Transform,
};

use crate::{WATERS, WAYS};

/// OSM bridge values of a way that lies on the ground.
const GROUND: [&str; 3] = ["no", "boardwalk", "low_water_crossing"];
/// Level-18 node edges: 2^16 frame units.
const NODE_18: i64 = 1 << 16;

pub struct Canvas {
    pub pixmap: Pixmap,
    x0: f64,
    y0: f64,
    /// Frame units per pixel, and metres per frame unit at the centre.
    upp: f64,
    mpu: f64,
}

pub struct Heights {
    pub values: Vec<Option<f64>>,
    pub unknown: usize,
    pub range: Option<[f64; 2]>,
    pub ns: f64,
}

impl Canvas {
    pub fn new(px: u32, x0: f64, y0: f64, upp: f64, mpu: f64) -> Canvas {
        let mut pixmap = Pixmap::new(px, px).expect("a picture of some pixels");
        pixmap.fill(Color::from_rgba8(235, 235, 230, 255));
        Canvas { pixmap, x0, y0, upp, mpu }
    }

    fn size(&self) -> u32 {
        self.pixmap.width()
    }

    fn px(&self, p: GlobalPoint) -> (f32, f32) {
        (((p.x as f64 - self.x0) / self.upp) as f32, ((p.y as f64 - self.y0) / self.upp) as f32)
    }

    fn at(&self, i: f64, j: f64) -> GlobalPoint {
        GlobalPoint { x: (self.x0 + i * self.upp).round() as i64, y: (self.y0 + j * self.upp).round() as i64 }
    }

    fn metres(&self, m: f64) -> f32 {
        (m / self.mpu / self.upp) as f32
    }

    /// A mask of the closed square [x0, y0, x1, y1] of the frame.
    fn mask(&self, [x0, y0, x1, y1]: [i64; 4]) -> Option<Mask> {
        let (a, b) = (self.px(GlobalPoint { x: x0, y: y0 }), self.px(GlobalPoint { x: x1, y: y1 }));
        let rect = Rect::from_ltrb(
            a.0.max(-1.0),
            a.1.max(-1.0),
            b.0.min(self.size() as f32 + 1.0),
            b.1.min(self.size() as f32 + 1.0),
        )?;
        let mut mask = Mask::new(self.size(), self.size())?;
        mask.fill_path(&PathBuilder::from_rect(rect), FillRule::Winding, false, Transform::identity());
        Some(mask)
    }
}

fn paint(r: u8, g: u8, b: u8, a: u8) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(r, g, b, a);
    paint.anti_alias = true;
    paint
}

fn stroke(width: f32, dashed: bool) -> Stroke {
    Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        dash: dashed.then(|| StrokeDash::new(vec![6.0, 4.0], 0.0)).flatten(),
        ..Stroke::default()
    }
}

/// The cell square of a key.
fn cell_square(key: CellKey) -> [i64; 4] {
    let o = key.origin();
    [o.x, o.y, o.x + CELL_UNITS, o.y + CELL_UNITS]
}

/// Whether a tile's half-open square holds a point: the tile owns it.
pub fn holds(tile: (u32, u32), p: GlobalPoint) -> bool {
    let [x0, y0, x1, y1] = tile_square(tile);
    (x0..x1).contains(&p.x) && (y0..y1).contains(&p.y)
}

/// Whether a point lies inside an area, by the copy of the tile that owns the point: even-odd over its rings.
pub fn inside(f: &Feature, p: GlobalPoint) -> bool {
    let mut odd = false;
    for part in f.parts.iter().filter(|part| holds(part.tile, p)) {
        let n = part.points.len();
        for i in 0..n {
            let (a, b) = (part.points[i], part.points[(i + 1) % n]);
            if (a.y > p.y) != (b.y > p.y) {
                let x = a.x as f64 + (p.y - a.y) as f64 * (b.x - a.x) as f64 / (b.y - a.y) as f64;
                if (p.x as f64) < x {
                    odd = !odd;
                }
            }
        }
    }
    odd
}

/// The height at each pixel centre, from the cell that holds the pixel.
pub fn heights(canvas: &Canvas, cells: &BTreeMap<CellKey, CellInput>) -> Heights {
    let size = canvas.size() as usize;
    let start = Instant::now();
    let values: Vec<Option<f64>> = (0..size * size)
        .map(|n| {
            let p = canvas.at((n % size) as f64 + 0.5, (n / size) as f64 + 0.5);
            cells.get(&CellKey::of(p)).and_then(|cell| cell.terrain.height(p))
        })
        .collect();
    let ns = start.elapsed().as_secs_f64() * 1e9 / values.len() as f64;
    let known = values.iter().flatten();
    let range = known.clone().copied().reduce(f64::min).zip(known.copied().reduce(f64::max)).map(|(a, b)| [a, b]);
    Heights { unknown: values.iter().filter(|h| h.is_none()).count(), values, range, ns }
}

/// The light of a slope (rise per metre east and south) from 315°, 45° up.
pub fn shade(east: f64, south: f64) -> f64 {
    let (azimuth, altitude) = (315.0 * PI / 180.0, PI / 4.0);
    let light =
        [libm::cos(altitude) * libm::sin(azimuth), libm::cos(altitude) * libm::cos(azimuth), libm::sin(altitude)];
    // The normal in (east, north, up).
    let normal = [-east, south, 1.0];
    let length = (normal[0] * normal[0] + normal[1] * normal[1] + 1.0).sqrt();
    ((normal[0] * light[0] + normal[1] * light[1] + normal[2] * light[2]) / length).max(0.0)
}

/// A hillshade with contours every 1 m (thin) and 5 m (dark); magenta where no height is known.
pub fn terrain(canvas: &mut Canvas, heights: &[Option<f64>]) {
    let size = canvas.size() as usize;
    let pixel_m = canvas.upp * canvas.mpu;
    let h = |i: usize, j: usize| heights[j.min(size - 1) * size + i.min(size - 1)];
    let data = canvas.pixmap.data_mut();
    for j in 0..size {
        for i in 0..size {
            let rgb = match h(i, j) {
                None => [255, 0, 255],
                Some(here) => {
                    let at = |di: isize, dj: isize| {
                        h((i as isize + di).max(0) as usize, (j as isize + dj).max(0) as usize).unwrap_or(here)
                    };
                    let east = (at(1, 0) - at(-1, 0)) / (2.0 * pixel_m);
                    let south = (at(0, 1) - at(0, -1)) / (2.0 * pixel_m);
                    let light = 0.35 + 0.65 * shade(east, south);
                    let mut rgb = [205.0 * light, 215.0 * light, 190.0 * light];
                    let differs =
                        |step: f64| [at(1, 0), at(0, 1)].iter().any(|&n| (n / step).floor() != (here / step).floor());
                    let darken = if differs(5.0) {
                        0.45
                    } else if differs(1.0) {
                        0.8
                    } else {
                        1.0
                    };
                    rgb.iter_mut().for_each(|c| *c *= darken);
                    rgb.map(|c| c.clamp(0.0, 255.0) as u8)
                }
            };
            let p = 4 * (j * size + i);
            data[p..p + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }
}

/// The points of a polyline in pixels.
fn path(canvas: &Canvas, points: &[GlobalPoint], close: bool) -> Option<tiny_skia::Path> {
    let mut builder = PathBuilder::new();
    for (i, &p) in points.iter().enumerate() {
        let (x, y) = canvas.px(p);
        if i == 0 {
            builder.move_to(x, y)
        } else {
            builder.line_to(x, y)
        }
    }
    if close {
        builder.close();
    }
    builder.finish()
}

/// The runs of consecutive owned segments of a line part.
fn owned_runs(part: &Part) -> Vec<&[GlobalPoint]> {
    let mut runs = Vec::new();
    let mut start = None;
    for i in 0..=part.own.len() {
        match (start, part.own.get(i).copied().unwrap_or(false)) {
            (None, true) => start = Some(i),
            (Some(s), false) => {
                runs.push(&part.points[s..=i]);
                start = None;
            }
            _ => {}
        }
    }
    runs
}

fn kind_colour(kind: &str) -> (u8, u8, u8) {
    match kind {
        "main-road" => (200, 90, 20),
        "road" => (120, 100, 30),
        "path" => (170, 40, 40),
        "railway" => (40, 40, 40),
        "tunnel" | "gallery" => (110, 50, 150),
        "river" | "stream" => (30, 80, 200),
        _ => (90, 90, 90),
    }
}

/// The ways as chords and bands, the sidewalks, water, areas and points.
pub fn inputs(canvas: &mut Canvas, cells: &BTreeMap<CellKey, CellInput>, sections: &Sections) {
    for (&key, cell) in cells {
        let Some(mask) = canvas.mask(cell_square(key)) else { continue };
        // Areas: each copy under the cell and its own tile's square.
        for f in cell.features.iter().filter(|f| f.geometry == Geometry::Area) {
            let mut polygons: Vec<Vec<&Part>> = Vec::new();
            for part in &f.parts {
                match polygons.last_mut() {
                    Some(polygon) if !part.outer => polygon.push(part),
                    _ => polygons.push(vec![part]),
                }
            }
            for polygon in polygons {
                let [a, b] = [cell_square(key), tile_square(polygon[0].tile)];
                let both = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
                if both[0] >= both[2] || both[1] >= both[3] {
                    continue;
                }
                let Some(clip) = canvas.mask(both) else { continue };
                let mut builder = PathBuilder::new();
                for ring in &polygon {
                    if let Some(ring) = path(canvas, &ring.points, true) {
                        builder.push_path(&ring);
                    }
                }
                let Some(shape) = builder.finish() else { continue };
                let fill = match f.kind.as_str() {
                    "building" | "building-part" => Some(paint(130, 130, 130, 200)),
                    "lake" | "river-area" | "sea" => Some(paint(90, 140, 220, 170)),
                    _ => None,
                };
                match fill {
                    Some(fill) => {
                        canvas.pixmap.fill_path(&shape, &fill, FillRule::EvenOdd, Transform::identity(), Some(&clip))
                    }
                    None => canvas.pixmap.stroke_path(
                        &shape,
                        &paint(40, 110, 40, 220),
                        &stroke(1.0, false),
                        Transform::identity(),
                        Some(&clip),
                    ),
                }
            }
        }
        let lines: Vec<&Feature> = cell.features.iter().filter(|f| f.geometry == Geometry::Line).collect();
        // Bands: every shoulder, lighter, under every carriageway; water as its half-width.
        for pass in 0..2 {
            for f in &lines {
                let kind = f.kind.as_str();
                let band = match (pass, WAYS.contains(&kind), WATERS.contains(&kind)) {
                    (0, true, _) => {
                        let s = sections.way(kind, &f.tags, false);
                        (s.half_m + s.shoulder_m, paint(250, 245, 200, 110))
                    }
                    (1, true, _) => (sections.way(kind, &f.tags, false).half_m, paint(120, 120, 120, 120)),
                    (1, _, true) => (sections.water(kind, &f.tags).half_m, paint(60, 120, 230, 130)),
                    _ => continue,
                };
                for part in &f.parts {
                    for run in owned_runs(part) {
                        if let Some(line) = path(canvas, run, false) {
                            let width = 2.0 * canvas.metres(band.0);
                            canvas.pixmap.stroke_path(
                                &line,
                                &band.1,
                                &stroke(width, false),
                                Transform::identity(),
                                Some(&mask),
                            );
                        }
                    }
                }
            }
        }
        // Sidewalks: a line at the outer edge of each side that `walk` marks.
        for f in lines.iter().filter(|f| WAYS.contains(&f.kind.as_str())) {
            let s = sections.way(&f.kind, &f.tags, false);
            let offset = canvas.metres(s.half_m + s.sidewalk_m);
            for part in &f.parts {
                let Some(plan) = &part.plan else { continue };
                for (i, &own) in part.own.iter().enumerate() {
                    if !own || plan.walk[i] == 0 {
                        continue;
                    }
                    let ((ax, ay), (bx, by)) = (canvas.px(part.points[i]), canvas.px(part.points[i + 1]));
                    let length = ((bx - ax) * (bx - ax) + (by - ay) * (by - ay)).sqrt().max(1e-6);
                    // Left of the direction, with y down the picture.
                    let (lx, ly) = ((by - ay) / length, -(bx - ax) / length);
                    for (bit, sign) in [(1u8, 1.0f32), (2, -1.0)] {
                        if plan.walk[i] & bit == 0 {
                            continue;
                        }
                        let (dx, dy) = (sign * lx * offset, sign * ly * offset);
                        let mut builder = PathBuilder::new();
                        builder.move_to(ax + dx, ay + dy);
                        builder.line_to(bx + dx, by + dy);
                        if let Some(edge) = builder.finish() {
                            canvas.pixmap.stroke_path(
                                &edge,
                                &paint(200, 30, 120, 255),
                                &stroke(1.0, false),
                                Transform::identity(),
                                Some(&mask),
                            );
                        }
                    }
                }
            }
        }
        // Chords, dashed on a bridge or in a tunnel, and their vertices.
        for f in &lines {
            let (r, g, b) = kind_colour(&f.kind);
            let bridge = f.tags.get("bridge").is_some_and(|v| !GROUND.contains(&v.as_str()));
            let tunnel = f.kind == "tunnel" || f.tags.get("tunnel").is_some_and(|v| v != "no");
            let width = if WAYS.contains(&f.kind.as_str()) || WATERS.contains(&f.kind.as_str()) { 1.5 } else { 1.0 };
            for part in &f.parts {
                for run in owned_runs(part) {
                    if let Some(line) = path(canvas, run, false) {
                        canvas.pixmap.stroke_path(
                            &line,
                            &paint(r, g, b, 255),
                            &stroke(width, bridge || tunnel),
                            Transform::identity(),
                            Some(&mask),
                        );
                    }
                }
                for (v, &p) in part.points.iter().enumerate() {
                    let touches = part.own.get(v).copied().unwrap_or(false) || v > 0 && part.own[v - 1];
                    if !touches {
                        continue;
                    }
                    let (x, y) = canvas.px(p);
                    let side = part.plan.as_ref().is_some_and(|plan| plan.side[v] != 0);
                    let mark = if side {
                        Rect::from_xywh(x - 2.5, y - 2.5, 5.0, 5.0).map(PathBuilder::from_rect)
                    } else {
                        PathBuilder::from_circle(x, y, 1.6)
                    };
                    if let Some(mark) = mark {
                        canvas.pixmap.fill_path(
                            &mark,
                            &paint(r, g, b, 255),
                            FillRule::Winding,
                            Transform::identity(),
                            Some(&mask),
                        );
                    }
                }
            }
        }
        // Points.
        for f in cell.features.iter().filter(|f| f.geometry == Geometry::Point) {
            for part in &f.parts {
                for (&p, &own) in part.points.iter().zip(&part.own) {
                    let (x, y) = canvas.px(p);
                    if let (true, Some(dot)) = (own, PathBuilder::from_circle(x, y, 2.5)) {
                        canvas.pixmap.fill_path(
                            &dot,
                            &paint(20, 20, 20, 255),
                            FillRule::Winding,
                            Transform::identity(),
                            Some(&mask),
                        );
                    }
                }
            }
        }
    }
}

/// The cell lines, the margin of the centre's cell, level-18 node lines, the centre and a 10 m bar.
pub fn cells(canvas: &mut Canvas, keys: &[CellKey], centre: GlobalPoint) {
    let size = canvas.size() as f32;
    let line = |canvas: &mut Canvas, a: (f32, f32), b: (f32, f32), colour: Paint, width: f32, dashed: bool| {
        let mut builder = PathBuilder::new();
        builder.move_to(a.0, a.1);
        builder.line_to(b.0, b.1);
        if let Some(path) = builder.finish() {
            canvas.pixmap.stroke_path(&path, &colour, &stroke(width, dashed), Transform::identity(), None);
        }
    };
    let (x0, y0) = (canvas.x0 as i64, canvas.y0 as i64);
    let (x1, y1) = (x0 + (size as f64 * canvas.upp) as i64, y0 + (size as f64 * canvas.upp) as i64);
    let mut x = x0.div_euclid(NODE_18) * NODE_18;
    while x <= x1 {
        let (px, _) = canvas.px(GlobalPoint { x, y: y0 });
        line(canvas, (px, 0.0), (px, size), paint(0, 0, 0, 40), 1.0, false);
        x += NODE_18;
    }
    let mut y = y0.div_euclid(NODE_18) * NODE_18;
    while y <= y1 {
        let (_, py) = canvas.px(GlobalPoint { x: x0, y });
        line(canvas, (0.0, py), (size, py), paint(0, 0, 0, 40), 1.0, false);
        y += NODE_18;
    }
    for &key in keys {
        let [a, b, c, d] = cell_square(key);
        let (p, q) = (canvas.px(GlobalPoint { x: a, y: b }), canvas.px(GlobalPoint { x: c, y: d }));
        if let Some(rect) = Rect::from_ltrb(p.0, p.1, q.0, q.1) {
            canvas.pixmap.stroke_path(
                &PathBuilder::from_rect(rect),
                &paint(0, 0, 0, 200),
                &stroke(1.5, false),
                Transform::identity(),
                None,
            );
        }
    }
    let [a, b, c, d] = CellKey::of(centre).grown();
    let (p, q) = (canvas.px(GlobalPoint { x: a, y: b }), canvas.px(GlobalPoint { x: c, y: d }));
    if let Some(rect) = Rect::from_ltrb(p.0, p.1, q.0, q.1) {
        canvas.pixmap.stroke_path(
            &PathBuilder::from_rect(rect),
            &paint(220, 0, 0, 220),
            &stroke(2.0, true),
            Transform::identity(),
            None,
        );
    }
    let (cx, cy) = canvas.px(centre);
    line(canvas, (cx - 12.0, cy), (cx + 12.0, cy), paint(0, 0, 0, 255), 2.0, false);
    line(canvas, (cx, cy - 12.0), (cx, cy + 12.0), paint(0, 0, 0, 255), 2.0, false);
    let bar = canvas.metres(10.0);
    let (left, bottom) = (20.0, size - 20.0);
    line(canvas, (left, bottom), (left + bar, bottom), paint(0, 0, 0, 255), 4.0, false);
    line(canvas, (left, bottom - 8.0), (left, bottom + 2.0), paint(0, 0, 0, 255), 2.0, false);
    line(canvas, (left + bar, bottom - 8.0), (left + bar, bottom + 2.0), paint(0, 0, 0, 255), 2.0, false);
}
