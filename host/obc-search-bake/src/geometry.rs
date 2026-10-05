use geo::{
    BoundingRect, Centroid, Coord, CoordsIter, Geometry, InteriorPoint, Intersects, LineString, MultiPolygon, Point,
    Polygon,
};
use rstar::{PointDistance, RTreeObject, AABB};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub struct Entry {
    pub index: usize,
    pub envelope: AABB<[f64; 2]>,
}

impl RTreeObject for Entry {
    type Envelope = AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

impl PointDistance for Entry {
    fn distance_2(&self, point: &[f64; 2]) -> f64 {
        self.envelope.distance_2(point)
    }
}

pub fn envelope(g: &Geometry) -> AABB<[f64; 2]> {
    let b = g.bounding_rect().expect("non-empty geometry");
    AABB::from_corners([b.min().x, b.min().y], [b.max().x, b.max().y])
}

pub fn center(g: &Geometry) -> Point {
    let p = match g {
        Geometry::LineString(line) => {
            let total: f64 = line.lines().map(|s| distance2(Point(s.start), Point(s.end)).sqrt()).sum();
            let mut remaining = total / 2.;
            let mut p = Point(line.0[0]);
            for s in line.lines() {
                let length = distance2(Point(s.start), Point(s.end)).sqrt();
                if length >= remaining && length > 0. {
                    let t = remaining / length;
                    p = Point::new(s.start.x + t * (s.end.x - s.start.x), s.start.y + t * (s.end.y - s.start.y));
                    break;
                }
                remaining -= length;
            }
            Some(p)
        }
        Geometry::Polygon(poly) => surface_point(poly).map(|(p, _)| p),
        Geometry::MultiPolygon(polys) => polys.0.iter().filter_map(surface_point).reduce(widest).map(|(p, _)| p),
        _ => g.centroid(),
    }
    .expect("non-empty geometry");
    Point::new((p.x() * 1e7).round() / 1e7, (p.y() * 1e7).round() / 1e7)
}

fn surface_point(poly: &Polygon) -> Option<(Point, f64)> {
    let bounds = poly.bounding_rect()?;
    let middle = (bounds.min().y + bounds.max().y) / 2.;
    let mut lower = bounds.min().y;
    let mut upper = bounds.max().y;
    for c in poly.coords_iter() {
        if c.y <= middle {
            lower = lower.max(c.y);
        } else {
            upper = upper.min(c.y);
        }
    }
    let y = (lower + upper) / 2.;
    let mut xs = Vec::new();
    for ring in std::iter::once(poly.exterior()).chain(poly.interiors()) {
        for edge in ring.lines() {
            if (edge.start.y <= y && edge.end.y > y) || (edge.end.y <= y && edge.start.y > y) {
                xs.push(edge.start.x + (y - edge.start.y) * (edge.end.x - edge.start.x) / (edge.end.y - edge.start.y));
            }
        }
    }
    xs.sort_by(f64::total_cmp);
    xs.chunks_exact(2)
        .map(|x| (Point::new((x[0] + x[1]) / 2., y), x[1] - x[0]))
        .reduce(widest)
        .or_else(|| poly.interior_point().map(|p| (p, 0.)))
}

fn widest(a: (Point, f64), b: (Point, f64)) -> (Point, f64) {
    if b.1 > a.1 {
        b
    } else {
        a
    }
}

pub fn bounds_geometry(g: &Geometry) -> Geometry {
    let b = envelope(g);
    let (a, z) = (b.lower(), b.upper());
    if a == z {
        Point::new(a[0], a[1]).into()
    } else if a[0] == z[0] || a[1] == z[1] {
        coordinates([a, z]).into()
    } else {
        geo::Rect::new(Coord { x: a[0], y: a[1] }, Coord { x: z[0], y: z[1] }).to_polygon().into()
    }
}

pub fn expanded(p: Point, radius: f64) -> AABB<[f64; 2]> {
    AABB::from_corners([p.x() - radius, p.y() - radius], [p.x() + radius, p.y() + radius])
}

pub fn distance2(a: Point, b: Point) -> f64 {
    (a.x() - b.x()).powi(2) + (a.y() - b.y()).powi(2)
}

// Join by node identity. Missing members and unclosed rings invalidate the area.
pub fn rings(mut parts: Vec<Vec<i64>>) -> Option<Vec<Vec<i64>>> {
    let mut endpoints: BTreeMap<i64, BTreeSet<usize>> = BTreeMap::new();
    for (i, part) in parts.iter().enumerate() {
        if part.len() < 2 {
            return None;
        }
        endpoints.entry(part[0]).or_default().insert(i);
        endpoints.entry(*part.last()?).or_default().insert(i);
    }
    let mut used = vec![false; parts.len()];
    let mut result = Vec::new();
    for start in 0..parts.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut ring = std::mem::take(&mut parts[start]);
        while ring.first() != ring.last() {
            let end = *ring.last()?;
            let next = *endpoints.get(&end)?.iter().find(|&&i| !used[i])?;
            used[next] = true;
            let mut part = std::mem::take(&mut parts[next]);
            if part[0] != end {
                part.reverse();
            }
            ring.extend(part.into_iter().skip(1));
        }
        if ring.len() < 4 {
            return None;
        }
        result.push(ring);
    }
    Some(result)
}

pub fn polygons(outer: Vec<LineString>, inner: Vec<LineString>) -> Option<Geometry> {
    let mut polygons: Vec<_> = outer.into_iter().map(|ring| Polygon::new(ring, vec![])).collect();
    for hole in inner {
        let point = Point(*hole.0.first()?);
        let parent = polygons.iter_mut().find(|p| p.intersects(&point))?;
        parent.interiors_push(hole);
    }
    if polygons.is_empty() {
        return None;
    }
    Some(Geometry::MultiPolygon(MultiPolygon(polygons)))
}

pub fn coordinates(points: impl IntoIterator<Item = [f64; 2]>) -> LineString {
    LineString(points.into_iter().map(|[x, y]| Coord { x, y }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn assembly_keeps_holes_and_rejects_incomplete_boundaries() {
        assert_eq!(rings(vec![vec![1, 2], vec![3, 2], vec![3, 4, 1]]), Some(vec![vec![1, 2, 3, 4, 1]]));
        assert_eq!(rings(vec![vec![1, 2], vec![2, 3]]), None);
        let g = polygons(
            vec![coordinates([[0., 0.], [4., 0.], [4., 4.], [0., 4.], [0., 0.]])],
            vec![coordinates([[1., 1.], [3., 1.], [3., 3.], [1., 3.], [1., 1.]])],
        )
        .unwrap();
        assert!(g.intersects(&Point::new(0.5, 0.5)));
        assert!(!g.intersects(&Point::new(2., 2.)));
        assert!(g.intersects(&center(&g)));
        assert_eq!(center(&g), Point::new(0.5, 2.));
    }
}
