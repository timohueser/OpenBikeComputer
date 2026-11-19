//! Browser FIT import and export: a ride or a course from FIT bytes, and FIT bytes from both.
//!
//! The numbers cross as `Float64Array`s, with NaN for an absent value. A course does not cross as
//! JSON text, because that would link the JSON parser into the module.

use obc_fit::{Course, File, Point, Ride, Sample, Sport, Waypoint};
use wasm_bindgen::prelude::*;

/// A sample crosses as 10 values: Unix time, latitude, longitude, segment start (1 or 0),
/// elevation, speed, heart rate, cadence, power and temperature.
const SAMPLE_VALUES: usize = 10;

const SPORTS: [(Sport, &str); 7] = [
    (Sport::Road, "road"),
    (Sport::Gravel, "gravel"),
    (Sport::Mtb, "mtb"),
    (Sport::Bike, "bike"),
    (Sport::Foot, "foot"),
    (Sport::Other, "other"),
    (Sport::Unknown, "unknown"),
];

fn error(name: &str, message: &str, code: Option<&str>) -> JsValue {
    let error = js_sys::Error::new(message);
    error.set_name(name);
    if let Some(code) = code {
        let _ = js_sys::Reflect::set(&error, &"code".into(), &code.into());
    }
    error.into()
}

fn present(value: f64) -> Option<f64> {
    (!value.is_nan()).then_some(value)
}

fn whole<T: TryFrom<i64>>(value: f64) -> Option<T> {
    present(value).and_then(|v| T::try_from(v.round() as i64).ok())
}

fn nan(value: Option<impl Into<f64>>) -> f64 {
    value.map_or(f64::NAN, Into::into)
}

/// Gives `{ sport, samples }` for a ride, or `{ name, points, waypoints, names, elapsedS }` for a
/// course: points as latitude, longitude and elevation, waypoints as latitude and longitude, and
/// the waypoint names (`undefined` for none). Throws a `FitError` with a `code`.
#[wasm_bindgen]
pub fn obc_fit_read(bytes: &[u8], fallback: &str) -> Result<js_sys::Object, JsValue> {
    let file = obc_fit::read(bytes).map_err(|e| error("FitError", e.message(), Some(e.code())))?;
    let out = js_sys::Object::new();
    let set = |key: &str, value: &JsValue| js_sys::Reflect::set(&out, &key.into(), value);
    let array = |values: Vec<f64>| JsValue::from(js_sys::Float64Array::from(&values[..]));
    match file {
        File::Ride(ride) => {
            let sport = SPORTS.iter().find(|(s, _)| *s == ride.sport).map_or("other", |(_, code)| code);
            let samples = ride.samples.iter().flat_map(|s| {
                [
                    s.time as f64,
                    s.lat.into(),
                    s.lon.into(),
                    f64::from(u8::from(s.segment_start)),
                    nan(s.elevation),
                    nan(s.speed),
                    nan(s.heart_rate),
                    nan(s.cadence),
                    nan(s.power),
                    nan(s.temperature),
                ]
            });
            set("sport", &sport.into())?;
            set("samples", &array(samples.collect()))?;
        }
        File::Course(course) => {
            let name = if course.name.is_empty() { fallback } else { &course.name };
            let points = course.points.iter().flat_map(|p| [p.lat.into(), p.lon.into(), nan(p.elevation)]);
            let waypoints = course.waypoints.iter().flat_map(|w| [w.lat.into(), w.lon.into()]);
            let names: js_sys::Array =
                course.waypoints.iter().map(|w| w.name.as_deref().map_or(JsValue::UNDEFINED, JsValue::from)).collect();
            set("name", &name.into())?;
            set("points", &array(points.collect()))?;
            set("waypoints", &array(waypoints.collect()))?;
            set("names", &names)?;
            set("elapsedS", &course.seconds.into())?;
        }
    }
    Ok(out)
}

#[wasm_bindgen]
pub fn obc_fit_write_ride(sport: &str, samples: &[f64]) -> Result<Vec<u8>, JsValue> {
    let sport = SPORTS.iter().find(|(_, code)| *code == sport).ok_or_else(|| error("Error", "unknown sport", None))?.0;
    let (samples, rest) = samples.as_chunks::<SAMPLE_VALUES>();
    if !rest.is_empty() {
        return Err(error("Error", "a sample has 10 values", None));
    }
    let samples = samples
        .iter()
        .map(|v| Sample {
            time: v[0] as i64,
            lat: v[1].round() as i32,
            lon: v[2].round() as i32,
            segment_start: v[3] != 0.0,
            elevation: present(v[4]),
            speed: present(v[5]),
            heart_rate: whole(v[6]),
            cadence: whole(v[7]),
            power: whole(v[8]),
            temperature: whole(v[9]),
        })
        .collect();
    Ok(obc_fit::write_ride(&Ride { sport, samples }))
}

/// Takes a course in the shape that `obc_fit_read` gives. An empty name is no name.
#[wasm_bindgen]
pub fn obc_fit_write_course(
    name: String,
    points: &[f64],
    waypoints: &[f64],
    names: Vec<String>,
    elapsed_s: f64,
) -> Result<Vec<u8>, JsValue> {
    let (points, rest) = points.as_chunks::<3>();
    let (waypoints, odd) = waypoints.as_chunks::<2>();
    if !rest.is_empty() || !odd.is_empty() || waypoints.len() != names.len() {
        return Err(error("Error", "a point has 3 values, and a waypoint has 2 values and a name", None));
    }
    Ok(obc_fit::write_course(&Course {
        name,
        points: points
            .iter()
            .map(|p| Point { lat: p[0].round() as i32, lon: p[1].round() as i32, elevation: present(p[2]) })
            .collect(),
        waypoints: waypoints
            .iter()
            .zip(names)
            .map(|(w, name)| Waypoint {
                lat: w[0].round() as i32,
                lon: w[1].round() as i32,
                name: (!name.is_empty()).then_some(name),
            })
            .collect(),
        seconds: elapsed_s.round().clamp(0.0, f64::from(u32::MAX)) as u32,
    }))
}
