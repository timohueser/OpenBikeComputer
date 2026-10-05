use super::{
    features::Input,
    geometry::{coordinates, envelope, Entry},
};
use flate2::read::GzDecoder;
use geo::{Area, BoundingRect, Geometry, Point, Polygon};
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
    current: bool,
}

pub struct Countries {
    entries: Vec<Country>,
    tree: RTree<Entry>,
    prepared: Vec<super::areas::Area>,
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
                    entries.push(Country {
                        code: columns[0].into(),
                        area: columns[1].parse()?,
                        geometry,
                        current: false,
                    });
                }
            }
            if entries.is_empty() {
                return Err("Country grid does not cover the input".into());
            }
        }
        for f in &input.features {
            if f.tag("boundary") != "administrative"
                || f.tag("admin_level") != "2"
                || !matches!(f.source, osmpbfreader::OsmId::Relation(_))
                || !matches!(f.geometry, Geometry::Polygon(_) | Geometry::MultiPolygon(_))
            {
                continue;
            }
            let code = ["ISO3166-1:alpha2", "ISO3166-1"]
                .into_iter()
                .map(|key| f.tag(key))
                .find(|code| code.len() == 2 && code.bytes().all(|b| b.is_ascii_alphabetic()));
            if let Some(code) = code {
                entries.push(Country {
                    code: code.to_ascii_lowercase(),
                    area: f.geometry.unsigned_area(),
                    geometry: f.geometry.clone(),
                    current: true,
                });
            }
        }
        Ok(Self::new(entries))
    }

    fn new(entries: Vec<Country>) -> Self {
        let tree = RTree::bulk_load(
            entries.iter().enumerate().map(|(index, c)| Entry { index, envelope: envelope(&c.geometry) }).collect(),
        );
        let prepared = entries.iter().map(|c| super::areas::Area::new(&c.geometry)).collect();
        Self { entries, tree, prepared }
    }

    pub fn at(&self, p: Point) -> Option<&str> {
        let matches: Vec<_> = self
            .tree
            .locate_in_envelope_intersecting(&super::geometry::expanded(p, 0.))
            .filter(|e| self.prepared[e.index].contains(p))
            .map(|e| &self.entries[e.index])
            .collect();
        let current: std::collections::BTreeSet<_> =
            matches.iter().filter(|c| c.current).map(|c| c.code.as_str()).collect();
        if current.len() == 1 {
            return current.into_iter().next();
        }
        matches
            .iter()
            .filter(|c| !c.current && (current.is_empty() || current.contains(c.code.as_str())))
            .min_by(|a, b| a.area.total_cmp(&b.area).then(a.code.cmp(&b.code)))
            .map(|c| c.code.as_str())
            .or_else(|| current.into_iter().next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::Intersects;
    #[test]
    fn current_country_boundaries_precede_the_fallback_grid() {
        let geometry: Geometry =
            Polygon::new(coordinates([[0., 0.], [1., 0.], [1., 1.], [0., 1.], [0., 0.]]), vec![]).into();
        let entries = vec![
            Country { code: "old".into(), area: 0.5, geometry: geometry.clone(), current: false },
            Country { code: "new".into(), area: 1., geometry: geometry.clone(), current: true },
        ];
        let countries = Countries::new(entries);
        assert_eq!(countries.at(Point::new(0.5, 0.5)), Some("new"));
        assert_eq!(countries.at(Point::new(2., 2.)), None);
    }
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
