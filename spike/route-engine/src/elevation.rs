//! Host-only sampling of local Copernicus geographic Float32 DSM tiles.
use crate::model::{Graph, Point, NO_ELEVATION};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use tiff::{
    decoder::{Decoder, DecodingResult},
    tags::Tag,
};

pub fn apply_dem(graph: &mut Graph, directory: &Path) -> Result<(), String> {
    let mut tiles: BTreeMap<(i32, i32), Vec<(usize, usize)>> = BTreeMap::new();
    for (road_id, road) in graph.roads.iter().enumerate() {
        for (point_id, point) in road.shape.iter().enumerate() {
            tiles
                .entry((point.lat.div_euclid(1_000_000), point.lon.div_euclid(1_000_000)))
                .or_default()
                .push((road_id, point_id));
        }
    }
    let mut missing = 0usize;
    for ((lat, lon), samples) in tiles {
        let path = directory.join(format!(
            "Copernicus_DSM_COG_10_{}{:02}_00_{}{:03}_00_DEM.tif",
            if lat >= 0 { 'N' } else { 'S' },
            lat.abs(),
            if lon >= 0 { 'E' } else { 'W' },
            lon.abs()
        ));
        if !path.exists() {
            missing += samples.len();
            continue;
        }
        let mut decoder =
            Decoder::new(BufReader::new(File::open(&path).map_err(|e| e.to_string())?)).map_err(|e| e.to_string())?;
        let scale = decoder.get_tag_f64_vec(Tag::ModelPixelScaleTag).map_err(|e| e.to_string())?;
        let tie = decoder.get_tag_f64_vec(Tag::ModelTiepointTag).map_err(|e| e.to_string())?;
        let (width, height) = decoder.dimensions().map_err(|e| e.to_string())?;
        if scale.len() < 2 || tie.len() < 6 || scale[0] <= 0.0 || scale[1] <= 0.0 {
            return Err("DEM has unsupported georeferencing".into());
        }
        let DecodingResult::F32(data) = decoder.read_image().map_err(|e| e.to_string())? else {
            return Err("DEM must contain Float32 elevations".into());
        };
        for (road, index) in samples {
            let point = &mut graph.roads[road].shape[index];
            let x = ((point.lon as f64 * 1e-6 - tie[3]) / scale[0] + tie[0]).round() as i64;
            let y = ((tie[4] - point.lat as f64 * 1e-6) / scale[1] + tie[1]).round() as i64;
            if x < 0 || y < 0 || x >= width as i64 || y >= height as i64 {
                missing += 1;
                continue;
            }
            let value = data[y as usize * width as usize + x as usize];
            if !value.is_finite() || !(-500.0..=9000.0).contains(&value) {
                missing += 1;
                continue;
            }
            point.elevation = value.round() as i16;
        }
    }
    for road in &mut graph.roads {
        (road.ascent_m, road.descent_m) = relief(&road.shape);
        if let Some(point) = road.shape.first() {
            graph.points[road.from as usize].elevation = point.elevation;
        }
        if let Some(point) = road.shape.last() {
            graph.points[road.to as usize].elevation = point.elevation;
        }
    }
    graph.warnings.retain(|w| !w.starts_with("No DEM applied;"));
    graph.warnings.push(format!("DSM sampled at source geometry vertices; unsmoothed DSM, no bridge/tunnel correction; {missing} samples missing"));
    Ok(())
}

fn relief(points: &[Point]) -> (u32, u32) {
    let (mut up, mut down) = (0, 0);
    let mut anchor = None;
    for p in points {
        if p.elevation == NO_ELEVATION {
            anchor = None;
            continue;
        }
        if let Some(previous) = anchor {
            let delta: i32 = p.elevation as i32 - previous;
            up += delta.max(0) as u32;
            down += (-delta).max(0) as u32;
        }
        anchor = Some(p.elevation as i32);
    }
    (up, down)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_samples_do_not_bridge_elevation_gaps() {
        let p = |elevation| Point { elevation, ..Point::default() };
        assert_eq!(relief(&[p(10), p(11), p(14), p(NO_ELEVATION), p(100), p(95)]), (4, 5));
    }
}
