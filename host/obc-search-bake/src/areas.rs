use super::geometry::{envelope, expanded, Entry};
use geo::{BoundingRect, CoordsIter, Distance, Euclidean, Geometry, Intersects, Line, LinesIter, Point, Polygon};
use rstar::{Envelope, RTree, AABB};

struct Part {
    edges: Vec<Line>,
    tree: RTree<Entry>,
    bounds: AABB<[f64; 2]>,
    vertex: Point,
}

impl Part {
    fn new(p: &Polygon) -> Self {
        let edges: Vec<_> = p.lines_iter().collect();
        let tree = RTree::bulk_load(
            edges
                .iter()
                .enumerate()
                .map(|(index, line)| Entry {
                    index,
                    envelope: AABB::from_corners([line.start.x, line.start.y], [line.end.x, line.end.y]),
                })
                .collect(),
        );
        let b = p.bounding_rect().expect("non-empty polygon");
        Self {
            edges,
            tree,
            bounds: AABB::from_corners([b.min().x, b.min().y], [b.max().x, b.max().y]),
            vertex: Point(p.exterior().0[0]),
        }
    }

    fn contains(&self, p: Point) -> bool {
        if !self.bounds.contains_point(&[p.x(), p.y()]) {
            return false;
        }
        let ray = AABB::from_corners([p.x(), p.y()], [self.bounds.upper()[0], p.y()]);
        let mut inside = false;
        for entry in self.tree.locate_in_envelope_intersecting(&ray) {
            let edge = &self.edges[entry.index];
            if edge.intersects(&p) {
                return true;
            }
            let (a, b) = (edge.start, edge.end);
            if (a.y > p.y()) != (b.y > p.y()) && p.x() < a.x + (p.y() - a.y) * (b.x - a.x) / (b.y - a.y) {
                inside = !inside;
            }
        }
        inside
    }

    fn intersects(&self, g: &Geometry) -> bool {
        if g.coords_iter().any(|c| self.contains(Point(c))) || g.intersects(&self.vertex) {
            return true;
        }
        let crossing = |line: Line| {
            let bbox = AABB::from_corners([line.start.x, line.start.y], [line.end.x, line.end.y]);
            self.tree.locate_in_envelope_intersecting(&bbox).any(|e| self.edges[e.index].intersects(&line))
        };
        match g {
            Geometry::Point(_) => false,
            Geometry::LineString(line) => line.lines().any(crossing),
            Geometry::Polygon(p) => p.lines_iter().any(crossing),
            Geometry::MultiPolygon(p) => p.lines_iter().any(crossing),
            _ => unreachable!("OSM geometry is a point, line or area"),
        }
    }
}

pub struct Area {
    parts: Vec<Part>,
    tree: RTree<Entry>,
}

impl Area {
    pub fn new(g: &Geometry) -> Self {
        let parts = match g {
            Geometry::Polygon(p) => vec![Part::new(p)],
            Geometry::MultiPolygon(p) => p.0.iter().map(Part::new).collect(),
            _ => unreachable!("only prepare polygon areas"),
        };
        let tree =
            RTree::bulk_load(parts.iter().enumerate().map(|(index, p)| Entry { index, envelope: p.bounds }).collect());
        Self { parts, tree }
    }

    pub fn contains(&self, p: Point) -> bool {
        self.tree.locate_in_envelope_intersecting(&expanded(p, 0.)).any(|e| self.parts[e.index].contains(p))
    }

    pub fn intersects(&self, g: &Geometry) -> bool {
        self.tree.locate_in_envelope_intersecting(&envelope(g)).any(|e| self.parts[e.index].intersects(g))
    }

    pub fn distance(&self, p: Point) -> f64 {
        if self.contains(p) {
            return 0.;
        }
        let mut best = f64::INFINITY;
        for part in &self.parts {
            if part.bounds.distance_2(&[p.x(), p.y()]) > best * best {
                continue;
            }
            for e in part.tree.nearest_neighbor_iter(&[p.x(), p.y()]) {
                if e.envelope.distance_2(&[p.x(), p.y()]) > best * best {
                    break;
                }
                best = best.min(Euclidean.distance(&p, &part.edges[e.index]));
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::coordinates;
    #[test]
    fn edge_index_matches_geometry_with_holes_touching_edges_and_disjoint_parts() {
        let p = Polygon::new(
            coordinates([[0., 0.], [4., 0.], [4., 4.], [0., 4.], [0., 0.]]),
            vec![coordinates([[1., 1.], [3., 1.], [3., 3.], [1., 3.], [1., 1.]])],
        );
        let g = Geometry::MultiPolygon(geo::MultiPolygon(vec![
            p.clone(),
            Polygon::new(coordinates([[5., 5.], [6., 5.], [6., 6.], [5., 6.], [5., 5.]]), vec![]),
        ]));
        let area = Area::new(&g);
        for x in -1..=14 {
            for y in -1..=14 {
                let point = Point::new(f64::from(x) / 2., f64::from(y) / 2.);
                assert_eq!(area.contains(point), g.intersects(&point));
                let line =
                    Geometry::LineString(coordinates([[point.x(), point.y()], [point.x() + 1., point.y() + 1.]]));
                assert_eq!(area.intersects(&line), g.intersects(&line));
            }
        }
        assert!(area.intersects(&Geometry::Polygon(p)));
        let inside = Geometry::Polygon(Polygon::new(
            coordinates([[0.1, 0.1], [0.2, 0.1], [0.2, 0.2], [0.1, 0.2], [0.1, 0.1]]),
            vec![],
        ));
        assert!(area.intersects(&inside));
        let containing = Geometry::Polygon(Polygon::new(
            coordinates([[-1., -1.], [7., -1.], [7., 7.], [-1., 7.], [-1., -1.]]),
            vec![],
        ));
        assert!(area.intersects(&containing));
    }
}
