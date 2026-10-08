//! Lines in microdegrees: the nearest point of a segment, points along a line, and the lossless
//! road page columns with coordinate deltas and elevation bit deltas.
use crate::{
    model::{Point, Road, NO_ELEVATION},
    package::ROADS_PER_PAGE,
    storage,
};
use serde::{Deserialize, Serialize};

/// Metres per microdegree of latitude.
pub const METRES_PER_UDEG: f64 = 0.111195;

/// The point of the segment `a`–`b` nearest to `p`: its parameter from `a` (0) to `b` (1), and its
/// distance in metres. The plane is scaled at the latitude of `p`.
pub fn project(p: Point, a: Point, b: Point) -> (f64, f64) {
    let scale = (p.lat as f64 * 1e-6).to_radians().cos() * METRES_PER_UDEG;
    let xy = |q: Point| [(q.lon - p.lon) as f64 * scale, (q.lat - p.lat) as f64 * METRES_PER_UDEG];
    let (a, b) = (xy(a), xy(b));
    let d = [b[0] - a[0], b[1] - a[1]];
    let t = (-(a[0] * d[0] + a[1] * d[1]) / (d[0] * d[0] + d[1] * d[1]).max(f64::MIN_POSITIVE)).clamp(0.0, 1.0);
    (t, (a[0] + d[0] * t).hypot(a[1] + d[1] * t))
}

/// The point at parameter `t` of the segment `a`–`b`, rounded to microdegrees. An end is itself,
/// with its own elevation; between the ends, the elevation is unknown where an end's is unknown.
pub fn lerp(a: Point, b: Point, t: f64) -> Point {
    if t <= 0.0 {
        return a;
    }
    if t >= 1.0 {
        return b;
    }
    let mix = |a: f64, b: f64| a + (b - a) * t;
    Point {
        lat: mix(a.lat as f64, b.lat as f64).round() as i32,
        lon: mix(a.lon as f64, b.lon as f64).round() as i32,
        elevation: if a.elevation == NO_ELEVATION || b.elevation == NO_ELEVATION {
            NO_ELEVATION
        } else {
            mix(a.elevation as f64, b.elevation as f64) as f32
        },
    }
}

/// The metres along `line` at each of its points.
pub fn cumulative(line: &[Point]) -> Vec<f64> {
    let mut along = Vec::with_capacity(line.len());
    along.push(0.0);
    for pair in line.windows(2) {
        along.push(along[along.len() - 1] + pair[0].distance(pair[1]));
    }
    along
}

/// The segment from point `k` to point `k + 1` of a line of two or more points, whose `cumulative`
/// lengths are `along`, that holds the point `at` metres along it, and the parameter of that point.
pub fn locate(along: &[f64], at: f64) -> (usize, f64) {
    let i = along.partition_point(|&a| a < at).clamp(1, along.len() - 1);
    (i - 1, ((at - along[i - 1]) / (along[i] - along[i - 1]).max(f64::MIN_POSITIVE)).clamp(0.0, 1.0))
}

/// The point `at` metres along a line, as for `locate`.
pub fn at(line: &[Point], along: &[f64], at: f64) -> Point {
    let (k, t) = locate(along, at);
    lerp(line[k], line[k + 1], t)
}

/// The part of a line from `from` to `to` metres along it, as for `at`, without repeated points.
pub fn cut(line: &[Point], along: &[f64], from: f64, to: f64) -> Vec<Point> {
    let inner = line.iter().zip(along).filter(|&(_, &a)| a > from && a < to).map(|(p, _)| *p);
    let mut part: Vec<Point> = Vec::new();
    for point in std::iter::once(at(line, along, from)).chain(inner).chain([at(line, along, to)]) {
        if part.last().is_none_or(|p| p.lat != point.lat || p.lon != point.lon) {
            part.push(point);
        }
    }
    part
}

#[derive(Serialize, Deserialize)]
struct Columns {
    roads: Vec<Road>,
    lengths: Vec<u32>,
    latitude: Vec<i32>,
    longitude: Vec<i32>,
    elevation: Vec<u32>,
}

pub fn encode(roads: &[Road]) -> Result<Vec<u8>, String> {
    if roads.len() > ROADS_PER_PAGE as usize {
        return Err("Too many roads in a geometry page".into());
    }
    let mut page = Columns {
        roads: roads.to_vec(),
        lengths: Vec::new(),
        latitude: Vec::new(),
        longitude: Vec::new(),
        elevation: Vec::new(),
    };
    let (mut lat, mut lon, mut elevation) = (0i32, 0i32, 0u32);
    for road in &mut page.roads {
        page.lengths.push(u32::try_from(road.shape.len()).map_err(|_| "Too many points")?);
        for point in std::mem::take(&mut road.shape) {
            page.latitude.push(point.lat.checked_sub(lat).ok_or("Latitude delta overflow")?);
            page.longitude.push(point.lon.checked_sub(lon).ok_or("Longitude delta overflow")?);
            page.elevation.push(point.elevation.to_bits() ^ elevation);
            (lat, lon, elevation) = (point.lat, point.lon, point.elevation.to_bits());
        }
    }
    storage::encode(&page)
}

pub fn decode(bytes: &[u8]) -> Result<Vec<Road>, String> {
    let mut page: Columns = storage::decode(bytes)?;
    let points = page
        .lengths
        .iter()
        .try_fold(0usize, |sum, &length| sum.checked_add(length as usize))
        .ok_or("Too many geometry points")?;
    if page.roads.len() > ROADS_PER_PAGE as usize
        || page.roads.len() != page.lengths.len()
        || page.roads.iter().any(|road| !road.shape.is_empty())
        || page.latitude.len() != points
        || page.longitude.len() != points
        || page.elevation.len() != points
    {
        return Err("Invalid geometry columns".into());
    }
    let (mut lat, mut lon, mut elevation, mut index) = (0i32, 0i32, 0u32, 0usize);
    for (road, length) in page.roads.iter_mut().zip(page.lengths) {
        for _ in 0..length {
            lat = lat.checked_add(page.latitude[index]).ok_or("Latitude overflow")?;
            lon = lon.checked_add(page.longitude[index]).ok_or("Longitude overflow")?;
            elevation ^= page.elevation[index];
            road.shape.push(Point { lat, lon, elevation: f32::from_bits(elevation) });
            index += 1;
        }
    }
    Ok(page.roads)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Surface;

    #[test]
    fn every_vertex_keeps_its_own_height_next_to_an_unknown_one() {
        let line: Vec<Point> = [(0, 10.0), (1_000, NO_ELEVATION), (2_000, 30.0), (3_000, 40.0)]
            .map(|(lon, elevation)| Point { lat: 0, lon, elevation })
            .to_vec();
        let along = cumulative(&line);
        assert_eq!(cut(&line, &along, 0.0, along[3]), line);
        assert_eq!(cut(&line, &along, along[2], along[3]), line[2..]);
        assert_eq!(at(&line, &along, (along[1] + along[2]) / 2.0).elevation, NO_ELEVATION);
    }

    #[test]
    fn geometry_preserves_fields_and_float_bits_and_rejects_invalid_columns() {
        let road = Road {
            from: 2,
            to: 9,
            way: -42,
            reversed: true,
            length_m: 100,
            ascent_m: 3,
            descent_m: 7,
            surface: Surface::Rough,
            class: 4,
            access: 7,
            difficulty: 2,
            hiking_difficulty: Some(3),
            structure: true,
            shape: vec![
                Point { lat: -85_000_000, lon: 180_000_000, elevation: NO_ELEVATION },
                Point { lat: 85_000_000, lon: -180_000_000, elevation: -0.0 },
                Point { lat: 0, lon: 0, elevation: f32::from_bits(0x43fe_cafe) },
            ],
        };
        let roads = vec![road.clone(), road];
        let bytes = encode(&roads).unwrap();
        assert_eq!(postcard::to_allocvec(&roads).unwrap(), postcard::to_allocvec(&decode(&bytes).unwrap()).unwrap());
        let mut columns: Columns = storage::decode(&bytes).unwrap();
        columns.longitude.pop();
        assert!(decode(&storage::encode(&columns).unwrap()).is_err());
        columns.longitude.push(0);
        columns.longitude[1] = i32::MAX;
        assert!(decode(&storage::encode(&columns).unwrap()).is_err());
    }
}
