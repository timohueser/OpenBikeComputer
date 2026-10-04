use super::{
    geometry::{coordinates, envelope, Entry},
    input::Input,
};
use flate2::read::GzDecoder;
use geo::{BoundingRect, Geometry, Intersects, Point, Polygon};
use rstar::RTree;
use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
};

struct Country {
    code: String,
    area: f64,
    geometry: Geometry,
}

pub struct Countries {
    entries: Vec<Country>,
    tree: RTree<Entry>,
}

// The static country grid is data. Parse its EWKB rows without executing SQL.
fn polygon(bytes: &[u8]) -> Option<Geometry> {
    let mut input = bytes;
    let geometry = parse(&mut input, 0)?;
    input.is_empty().then_some(geometry)
}

fn parse(input: &mut &[u8], depth: u8) -> Option<Geometry> {
    if depth > 1 {
        return None;
    }
    let mut endian = [0; 1];
    input.read_exact(&mut endian).ok()?;
    let little = match endian[0] {
        1 => true,
        0 => false,
        _ => return None,
    };
    let u32_ = |input: &mut &[u8]| -> Option<u32> {
        let mut b = [0; 4];
        input.read_exact(&mut b).ok()?;
        Some(if little { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    };
    let kind = u32_(input)?;
    if kind & 0x20000000 != 0 && u32_(input)? != 4326 {
        return None;
    }
    let count = u32_(input)?;
    if count as usize > input.len() / 4 {
        return None;
    }
    if kind & 0x1fffffff == 6 {
        let polygons: Option<Vec<_>> = (0..count)
            .map(|_| match parse(input, depth + 1)? {
                Geometry::Polygon(p) => Some(p),
                _ => None,
            })
            .collect();
        return Some(Geometry::MultiPolygon(geo::MultiPolygon(polygons?)));
    }
    if kind & 0x1fffffff != 3 {
        return None;
    }
    if count == 0 {
        return Some(Geometry::Polygon(Polygon::new(geo::LineString::new(vec![]), vec![])));
    }
    let mut rings = Vec::new();
    for _ in 0..count {
        let n = u32_(input)? as usize;
        if n < 4 || n > input.len() / 16 {
            return None;
        }
        let mut points = Vec::new();
        for _ in 0..n {
            let mut x = [0; 8];
            let mut y = [0; 8];
            input.read_exact(&mut x).ok()?;
            input.read_exact(&mut y).ok()?;
            let x = if little { f64::from_le_bytes(x) } else { f64::from_be_bytes(x) };
            let y = if little { f64::from_le_bytes(y) } else { f64::from_be_bytes(y) };
            if !x.is_finite() || !y.is_finite() || x.abs() > 180. || y.abs() > 90. {
                return None;
            }
            points.push([x, y]);
        }
        if points.first() != points.last() {
            return None;
        }
        rings.push(coordinates(points));
    }
    let outer = rings.remove(0);
    Some(Geometry::Polygon(Polygon::new(outer, rings)))
}

impl Countries {
    pub fn read(path: Option<&Path>, input: &Input) -> Result<Self, Box<dyn std::error::Error>> {
        let mut entries = Vec::new();
        if let Some(path) = path {
            let bounds = input.features.iter().map(|f| envelope(&f.geometry)).reduce(|a, b| {
                use rstar::Envelope;
                a.merged(&b)
            });
            for line in BufReader::new(GzDecoder::new(File::open(path)?)).lines() {
                let line = line?;
                let columns: Vec<_> = line.split('\t').collect();
                if columns.len() != 3 || columns[0].len() != 2 {
                    continue;
                }
                let hex = columns[2];
                if !hex.is_ascii() || hex.len() % 2 != 0 {
                    return Err("Invalid country EWKB".into());
                }
                let bytes: Result<Vec<_>, _> =
                    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16)).collect();
                let geometry = polygon(&bytes?).ok_or("Invalid country polygon")?;
                if geometry.bounding_rect().is_none() {
                    continue;
                }
                if bounds.is_some_and(|b| {
                    use rstar::Envelope;
                    b.intersects(&envelope(&geometry))
                }) {
                    entries.push(Country { code: columns[0].into(), area: columns[1].parse()?, geometry });
                }
            }
            if entries.is_empty() {
                return Err("Country grid does not cover the input".into());
            }
        }
        let tree = RTree::bulk_load(
            entries.iter().enumerate().map(|(index, c)| Entry { index, envelope: envelope(&c.geometry) }).collect(),
        );
        Ok(Self { entries, tree })
    }

    pub fn at(&self, p: Point) -> Option<&str> {
        self.tree
            .locate_in_envelope_intersecting(&super::geometry::expanded(p, 0.))
            .map(|e| &self.entries[e.index])
            .filter(|c| c.geometry.intersects(&p))
            .min_by(|a, b| a.area.total_cmp(&b.area).then(a.code.cmp(&b.code)))
            .map(|c| c.code.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn country_grid_accepts_multipolygons_and_empty_areas_but_rejects_truncated_rows() {
        let mut area = vec![1];
        area.extend(3_u32.to_le_bytes());
        area.extend(1_u32.to_le_bytes());
        area.extend(5_u32.to_le_bytes());
        for [x, y] in [[0_f64, 0.], [1., 0.], [1., 1.], [0., 1.], [0., 0.]] {
            area.extend(x.to_le_bytes());
            area.extend(y.to_le_bytes());
        }
        let mut multi = vec![1];
        multi.extend(0x20000006_u32.to_le_bytes());
        multi.extend(4326_u32.to_le_bytes());
        multi.extend(1_u32.to_le_bytes());
        multi.extend(&area);
        assert!(polygon(&multi).unwrap().intersects(&Point::new(0.5, 0.5)));
        assert!(polygon(&multi[..multi.len() - 1]).is_none());
        let empty = [1, 3, 0, 0, 0, 0, 0, 0, 0];
        assert!(polygon(&empty).unwrap().bounding_rect().is_none());
    }
}
