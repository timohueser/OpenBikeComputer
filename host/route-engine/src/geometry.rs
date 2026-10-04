//! Lossless road geometry columns with coordinate deltas and elevation bit deltas.
use crate::{
    model::{Point, Road},
    package::ROADS_PER_PAGE,
    storage,
};
use serde::{Deserialize, Serialize};

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
    use crate::model::{Surface, NO_ELEVATION};

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
