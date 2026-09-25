//! Build a bounded, deterministic finished ride for device development.

use std::{env, fs, path::Path};

use obc_elevation::DeadBand;
use obc_formats::{
    bike::BikeType,
    ride::{encode_footer, Footer, FOOTER_LEN},
    track::{encode_record, RECORD_LEN},
};
use obc_ports::TrackPoint;
use obc_replay::Track;

const MAX_POINTS: usize = 20_000;
const MAX_GPX_BYTES: u64 = 8 * 1024 * 1024;
const EARTH_M: f64 = 6_371_000.0;

fn main() -> Result<(), String> {
    let args: Vec<_> = env::args().collect();
    if args.len() != 5 {
        return Err("usage: gpx_to_ride INPUT.gpx OUTPUT.obcr START_UNIX_S RIDE_NAME".into());
    }
    let start_time = args[3].parse::<u32>().map_err(|_| "START_UNIX_S must be a Unix timestamp in seconds")?;
    let input = Path::new(&args[1]);
    if fs::metadata(input).map_err(|e| e.to_string())?.len() > MAX_GPX_BYTES {
        return Err(format!("GPX exceeds {MAX_GPX_BYTES} bytes"));
    }
    let track = Track::load(input)?;
    let bytes = build_ride(&track, start_time, &args[4])?;
    let output = Path::new(&args[2]);
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(output, &bytes).map_err(|e| e.to_string())?;
    println!("{}: {} samples, {} bytes", output.display(), track.points.len(), bytes.len());
    Ok(())
}

fn build_ride(track: &Track, start_time: u32, name: &str) -> Result<Vec<u8>, String> {
    let points = &track.points;
    if !(2..=MAX_POINTS).contains(&points.len()) {
        return Err(format!("ride needs 2..={MAX_POINTS} track points"));
    }
    let mut distance = vec![0.0; points.len()];
    let mut climb = DeadBand::<f32>::new();
    for (i, p) in points.iter().enumerate() {
        let ele = p.ele.ok_or("every point needs elevation")?;
        if !ele.is_finite() || !(i16::MIN as f32..=i16::MAX as f32).contains(&ele) {
            return Err("elevation is outside the ride sample range".into());
        }
        if !p.t.is_finite() || p.t < 0.0 || p.t * 1000.0 > u32::MAX as f64 {
            return Err("time is outside the ride sample range".into());
        }
        if i > 0 {
            if p.t <= points[i - 1].t {
                return Err("track times must increase".into());
            }
            distance[i] = distance[i - 1] + ground_distance(&points[i - 1], p);
        }
        climb.push(ele);
    }
    if distance.last().unwrap().round() > u32::MAX as f64
        || climb.ascent().round() > u16::MAX as f32
        || climb.descent().round() > u16::MAX as f32
    {
        return Err("ride totals exceed footer range".into());
    }

    let mut bytes = Vec::with_capacity(points.len() * RECORD_LEN + FOOTER_LEN);
    let mut moving_s = 0.0;
    let mut moving_m = 0.0;
    let mut hr_sum = 0.0;
    let mut cadence_sum = 0.0;
    let mut power_sum = 0.0;
    let mut energy_j = 0.0;
    let mut max_hr = 0;
    let mut max_power = 0;
    let mut hr = 100.0;
    for (i, p) in points.iter().enumerate() {
        let segment = if i == 0 { 1 } else { i };
        let dt = points[segment].t - points[segment - 1].t;
        let speed = (distance[segment] - distance[segment - 1]) / dt;
        let lo = i.saturating_sub(3);
        let hi = (i + 3).min(points.len() - 1);
        let span = distance[hi] - distance[lo];
        let grade = if span > 1.0 { (points[hi].ele.unwrap() - points[lo].ele.unwrap()) as f64 / span } else { 0.0 };
        let coasting = speed < 0.8 || (grade < -0.035 && speed > 4.0);
        let wave = (p.t / 37.0).sin() * 8.0 + (p.t / 13.0).sin() * 4.0;
        let power =
            if coasting { 0 } else { (135.0 + speed * 8.0 + grade * 1600.0 + wave).round().clamp(70.0, 360.0) as u16 };
        let cadence = if coasting { 0 } else { (80.0 - grade * 90.0 + wave / 4.0).round().clamp(60.0, 105.0) as u8 };
        let target_hr = if coasting { 105.0 } else { 95.0 + power as f64 * 0.2 };
        if i > 0 {
            hr += (target_hr - hr) * (1.0 - (-dt / 45.0).exp());
        }
        let hr_bpm = hr.round().clamp(70.0, 185.0) as u8;
        max_hr = max_hr.max(hr_bpm);
        max_power = max_power.max(power);
        let sample = TrackPoint {
            lon: p.lon,
            lat: p.lat,
            ele: p.ele.unwrap().round() as i16,
            t_ms: (p.t * 1000.0).round() as u32,
            segment_start: i == 0,
            hr: Some(hr_bpm),
            cadence: Some(cadence),
            power: Some(power),
        };
        bytes.extend_from_slice(&encode_record(&sample));
        if i > 0 && speed >= 0.8 {
            moving_s += dt;
            moving_m += distance[i] - distance[i - 1];
            hr_sum += hr_bpm as f64 * dt;
            cadence_sum += cadence as f64 * dt;
            power_sum += power as f64 * dt;
            energy_j += power as f64 * dt;
        }
    }

    if moving_s == 0.0 {
        return Err("ride has no moving intervals".into());
    }
    let avg = |sum: f64| (sum / moving_s).round();
    let mut footer = Footer::new(
        name,
        start_time,
        distance.last().unwrap().round() as u32,
        moving_s.round() as u32,
        (moving_m / moving_s * 100.0).round() as u16,
        climb.ascent().round() as u16,
        points.len() as u32,
        Some(avg(hr_sum) as u8),
        Some(max_hr),
        Some(avg(cadence_sum) as u8),
        Some(avg(power_sum) as u16),
        Some(max_power),
    );
    footer.descent_m = climb.descent().round() as u16;
    footer.energy_kj = Some((energy_j / 1000.0).round() as u32);
    footer.bike = BikeType::Road;
    bytes.extend_from_slice(&encode_footer(&footer));
    Ok(bytes)
}

fn ground_distance(a: &obc_replay::TrackPoint, b: &obc_replay::TrackPoint) -> f64 {
    let lat1 = (a.lat as f64 * 1e-6).to_radians();
    let lat2 = (b.lat as f64 * 1e-6).to_radians();
    let dlat = lat2 - lat1;
    let dlon = ((b.lon - a.lon) as f64 * 1e-6).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_M * h.sqrt().asin()
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_formats::{
        ride::{checked_object_len, decode_footer, FOOTER_LEN},
        track::{decode_record, RECORD_LEN},
    };
    use obc_replay::TrackPoint as GpxPoint;

    #[test]
    fn preserves_geometry_and_produces_a_finished_climb_and_descent() {
        let elevations = [200.0, 205.0, 215.0, 225.0, 225.0, 215.0, 205.0, 200.0];
        let track = Track {
            points: elevations
                .iter()
                .enumerate()
                .map(|(i, &ele)| GpxPoint {
                    lat: 48_000_000,
                    lon: 8_000_000 + i as i32 * 1_000,
                    ele: Some(ele),
                    t: i as f64 * 15.0,
                })
                .collect(),
        };
        let bytes = build_ride(&track, 1_790_236_800, "Kandel (simulated)").unwrap();
        let footer = decode_footer(bytes[bytes.len() - FOOTER_LEN..].try_into().unwrap()).unwrap();
        assert_eq!(bytes.len() as u64, checked_object_len(track.points.len() as u32).unwrap());
        assert_eq!(footer.point_count, track.points.len() as u32);
        assert_eq!(footer.name(), "Kandel (simulated)");
        assert_eq!(footer.start_time, 1_790_236_800);
        assert!(footer.climb_m >= 20 && footer.descent_m >= 20);
        let records: Vec<_> = bytes[..bytes.len() - FOOTER_LEN]
            .chunks_exact(RECORD_LEN)
            .map(|b| decode_record(b.try_into().unwrap()))
            .collect();
        for (source, sample) in track.points.iter().zip(&records) {
            assert_eq!((sample.lat, sample.lon, sample.ele), (source.lat, source.lon, source.ele.unwrap() as i16));
        }
        assert_eq!(records[0].t_ms, 0);
        assert_eq!(records[7].t_ms, 105_000);
        assert!(records[1].power.unwrap() > records[6].power.unwrap());
        assert_eq!((records[6].power, records[6].cadence), (Some(0), Some(0)));
        assert!(records[6].hr.unwrap() > 105, "heart rate should lag the descent");
        assert_eq!(build_ride(&track, 1_790_236_800, "Kandel (simulated)").unwrap(), bytes);
    }

    #[test]
    fn rejects_missing_altitude_and_out_of_order_time() {
        let mut track = Track {
            points: vec![
                GpxPoint { lat: 48_000_000, lon: 8_000_000, ele: Some(200.0), t: 0.0 },
                GpxPoint { lat: 48_000_000, lon: 8_001_000, ele: None, t: 10.0 },
            ],
        };
        assert!(build_ride(&track, 1, "test").is_err());
        track.points[1].ele = Some(200.0);
        track.points[1].t = 0.0;
        assert!(build_ride(&track, 1, "test").is_err());
    }
}
