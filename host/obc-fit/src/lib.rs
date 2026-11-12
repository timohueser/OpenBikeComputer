//! Reads a FIT file as a ride or a course, and writes both as FIT.

mod container;

use container::{Message, Value, Writer};

const FILE_ID: u16 = 0;
const SESSION: u16 = 18;
const LAP: u16 = 19;
const RECORD: u16 = 20;
const EVENT: u16 = 21;
const COURSE: u16 = 31;
const COURSE_POINT: u16 = 32;
const ACTIVITY: u16 = 34;

const FILE_ACTIVITY: u8 = 4;
const FILE_COURSE: u8 = 6;
const MANUFACTURER_DEVELOPMENT: u16 = 255;
const EVENT_TIMER: u8 = 0;
const EVENT_SESSION: u8 = 8;
const EVENT_LAP: u8 = 9;
const EVENT_ACTIVITY: u8 = 26;
const EVENT_START: u8 = 0;
const EVENT_STOP: u8 = 1;
const EVENT_STOP_ALL: u8 = 4;

/// The Unix time of the FIT epoch, 1989-12-31 00:00 UTC.
const EPOCH: i64 = 631_065_600;
/// The smallest FIT time that is a date. A course has no date, so its records start here.
const COURSE_START: u32 = 0x1000_0000;
/// The speed of a course that has no time of its own, in m/s.
const COURSE_SPEED: f64 = 5.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sport {
    Road,
    Gravel,
    Mtb,
    Bike,
    Foot,
    Other,
}

impl Sport {
    /// From `sport` and `sub_sport` of a session.
    fn from_fit(sport: Option<i64>, sub_sport: Option<i64>) -> Self {
        const CYCLING: i64 = 2;
        const E_BIKING: i64 = 21;
        match (sport, sub_sport) {
            // road, track_cycling
            (Some(CYCLING | E_BIKING), Some(7 | 13)) => Sport::Road,
            // gravel_cycling, cyclocross, mixed_surface
            (Some(CYCLING | E_BIKING), Some(46 | 11 | 49)) => Sport::Gravel,
            // mountain, downhill, e_bike_mountain
            (Some(CYCLING | E_BIKING), Some(8 | 9 | 47)) => Sport::Mtb,
            (Some(CYCLING | E_BIKING), _) => Sport::Bike,
            // running, walking, hiking
            (Some(1 | 11 | 17), _) => Sport::Foot,
            _ => Sport::Other,
        }
    }

    /// `sport` and `sub_sport` for a session.
    fn to_fit(self) -> (u8, u8) {
        match self {
            Sport::Road => (2, 7),
            Sport::Gravel => (2, 46),
            Sport::Mtb => (2, 8),
            Sport::Bike => (2, 0),
            Sport::Foot => (17, 0),
            Sport::Other => (0, 0),
        }
    }
}

/// One recorded point of a ride. The values are on the steps that FIT stores: whole seconds,
/// 0.2 m of elevation, 1 mm/s of speed and whole degrees Celsius.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// Unix time in seconds.
    pub time: i64,
    /// Latitude in microdegrees.
    pub lat: i32,
    /// Longitude in microdegrees.
    pub lon: i32,
    /// True for the first sample of a ride and for the first sample after a pause.
    pub segment_start: bool,
    /// Metres above sea level.
    pub elevation: Option<f64>,
    /// Metres per second.
    pub speed: Option<f64>,
    pub heart_rate: Option<u8>,
    pub cadence: Option<u8>,
    /// Watts.
    pub power: Option<u16>,
    /// Degrees Celsius.
    pub temperature: Option<i8>,
}

/// A recorded ride. Its first sample always has a position.
#[derive(Clone, Debug, PartialEq)]
pub struct Ride {
    pub sport: Sport,
    pub samples: Vec<Sample>,
}

/// One point of a course line, in microdegrees, with metres above sea level.
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub lat: i32,
    pub lon: i32,
    pub elevation: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Waypoint {
    pub lat: i32,
    pub lon: i32,
    pub name: Option<String>,
}

/// A planned route. A FIT course has times, so `seconds` is the time of the whole course.
#[derive(Clone, Debug, PartialEq)]
pub struct Course {
    /// Empty when the file has no name.
    pub name: String,
    pub points: Vec<Point>,
    pub waypoints: Vec<Waypoint>,
    pub seconds: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum File {
    Ride(Ride),
    Course(Course),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    NotFit,
    /// No record can be read.
    Damaged,
    /// The file is neither an activity nor a course.
    Unsupported,
    NoPositions,
}

impl Error {
    pub fn code(self) -> &'static str {
        match self {
            Error::NotFit => "not-fit",
            Error::Damaged => "fit-damaged",
            Error::Unsupported => "fit-unsupported",
            Error::NoPositions => "fit-no-positions",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Error::NotFit => "This is not a FIT file.",
            Error::Damaged => "This FIT file is damaged: no record in it can be read.",
            Error::Unsupported => "This FIT file holds no ride and no course.",
            Error::NoPositions => "This FIT file has no positions, so it has no line.",
        }
    }
}

const SEMICIRCLES_PER_DEGREE: f64 = 2_147_483_648.0 / 180.0;

fn position(message: &Message, lat: u8, lon: u8) -> Option<(i32, i32)> {
    let degrees = |field| message.int(field).map(|v| v as f64 / SEMICIRCLES_PER_DEGREE);
    let (lat, lon) = (degrees(lat)?, degrees(lon)?);
    (lat.abs() <= 90.0 && lon.abs() <= 180.0).then(|| ((lat * 1e6).round() as i32, (lon * 1e6).round() as i32))
}

fn semicircles(microdegrees: i32) -> i32 {
    // i32::MAX is the invalid value, and 180° would round to 2³¹.
    (f64::from(microdegrees) / 1e6 * SEMICIRCLES_PER_DEGREE).round().min(f64::from(i32::MAX - 1)) as i32
}

/// Reads a ride from an activity file, or a course from a course file. A file that is cut or has
/// a wrong CRC gives the records before the damage.
pub fn read(bytes: &[u8]) -> Result<File, Error> {
    let mut file_type = None;
    let mut sport = None;
    let mut name = None;
    let mut records = 0;
    let mut times = (None, None);
    let mut samples = Vec::new();
    let mut waypoints = Vec::new();
    let mut stopped = false;
    let mut segment_start = true;
    container::decode(bytes, |m| match m.global {
        FILE_ID => file_type = file_type.or(m.int(0)),
        SESSION => sport = sport.or(Some(Sport::from_fit(m.int(5), m.int(6)))),
        COURSE => name = name.take().or(m.text(5)),
        // A turn instruction is no waypoint: left, right, straight, the three forks, slight and sharp left and right, u_turn.
        COURSE_POINT if !matches!(m.int(5), Some(6..=8 | 16..=23)) => {
            if let Some((lat, lon)) = position(m, 2, 3) {
                waypoints.push(Waypoint { lat, lon, name: m.text(6) });
            }
        }
        EVENT if m.int(0) == Some(EVENT_TIMER.into()) => match m.int(1) {
            Some(0) => segment_start |= std::mem::take(&mut stopped),
            // stop, stop_all, stop_disable, stop_disable_all
            Some(1 | 4 | 8 | 9) => stopped = true,
            _ => {}
        },
        RECORD => {
            records += 1;
            if let Some(time) = m.time {
                times = (times.0.or(Some(time)), Some(time));
            }
            // A record with no position is dropped, and its segment start moves to the next sample. A
            // ride also drops a record with no time; a course needs no times. `file_id` is the first message.
            let course = file_type == Some(FILE_COURSE.into());
            let (Some(time), Some((lat, lon))) = (m.time.or(course.then_some(0)), position(m, 0, 1)) else { return };
            samples.push(Sample {
                time: i64::from(time) + EPOCH,
                lat,
                lon,
                segment_start: std::mem::take(&mut segment_start),
                elevation: m.int(78).or(m.int(2)).map(|raw| (raw as f64 - 2500.0) / 5.0),
                speed: m.int(73).or(m.int(6)).map(|raw| raw as f64 / 1000.0),
                heart_rate: m.int(3).and_then(|v| v.try_into().ok()),
                cadence: m.int(4).and_then(|v| v.try_into().ok()),
                power: m.int(7).and_then(|v| v.try_into().ok()),
                temperature: m.int(13).and_then(|v| v.try_into().ok()),
            });
        }
        _ => {}
    })?;
    let is_course = match file_type {
        Some(t) if t == i64::from(FILE_ACTIVITY) => false,
        Some(t) if t == i64::from(FILE_COURSE) => true,
        None if records == 0 => return Err(Error::Damaged),
        _ => return Err(Error::Unsupported),
    };
    if records == 0 {
        return Err(Error::Damaged);
    }
    if samples.is_empty() {
        return Err(Error::NoPositions);
    }
    if !is_course {
        return Ok(File::Ride(Ride { sport: sport.unwrap_or(Sport::Other), samples }));
    }
    Ok(File::Course(Course {
        name: name.unwrap_or_default(),
        points: samples.into_iter().map(|s| Point { lat: s.lat, lon: s.lon, elevation: s.elevation }).collect(),
        waypoints,
        seconds: times.1.zip(times.0).map_or(0, |(last, first)| last.saturating_sub(first)),
    }))
}

fn fit_time(unix: i64) -> u32 {
    (unix - EPOCH).clamp(0, i64::from(u32::MAX - 1)) as u32
}

fn metres(a: (i32, i32), b: (i32, i32)) -> f64 {
    const METRES_PER_MICRODEGREE: f64 = 0.111_32;
    let cos = ((f64::from(a.0) + f64::from(b.0)) / 2.0 / 1e6).to_radians().cos();
    let north = (f64::from(b.0) - f64::from(a.0)) * METRES_PER_MICRODEGREE;
    let east = (f64::from(b.1) - f64::from(a.1)) * METRES_PER_MICRODEGREE * cos;
    north.hypot(east)
}

fn altitude(elevation: Option<f64>) -> Option<Value<'static>> {
    let raw = (elevation? * 5.0 + 2500.0).round();
    (0.0..65535.0).contains(&raw).then_some(Value::U16(raw as u16))
}

/// Writes `file_id` and returns the writer.
/// `serial` tells files apart that have the same type and creation time.
fn start_file(file_type: u8, start: u32, serial: u32) -> Writer {
    let mut writer = Writer::default();
    writer.message(
        FILE_ID,
        &[
            (0, Value::Enum(file_type)),
            (1, Value::U16(MANUFACTURER_DEVELOPMENT)),
            (2, Value::U16(0)),
            (3, Value::U32z(serial.max(1))),
            (4, Value::U32(start)),
        ],
    );
    writer
}

/// FNV-1a over the name, the positions and the time of a course, so that the same course gives the same bytes.
fn hash(course: &Course) -> u32 {
    let points = course.points.iter().map(|p| (p.lat, p.lon));
    let positions = points.chain(course.waypoints.iter().map(|w| (w.lat, w.lon)));
    let bytes =
        course.name.bytes().chain(positions.flat_map(|(lat, lon)| [lat.to_le_bytes(), lon.to_le_bytes()].concat()));
    let bytes = bytes.chain(course.seconds.to_le_bytes());
    bytes.fold(0x811C_9DC5, |hash, byte| (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193))
}

fn timer(writer: &mut Writer, time: u32, event_type: u8) {
    writer.message(EVENT, &[(253, Value::U32(time)), (0, Value::Enum(EVENT_TIMER)), (1, Value::Enum(event_type))]);
}

/// Writes an activity file. A pause between two segments is a timer stop and a timer start.
pub fn write_ride(ride: &Ride) -> Vec<u8> {
    let start = ride.samples.first().map_or(0, |s| fit_time(s.time));
    let end = ride.samples.last().map_or(0, |s| fit_time(s.time));
    let mut writer = start_file(FILE_ACTIVITY, start, start);
    let (mut distance, mut timer_seconds) = (0.0, 0);
    let mut previous: Option<&Sample> = None;
    for sample in &ride.samples {
        let time = fit_time(sample.time);
        match previous {
            Some(p) if !sample.segment_start => {
                distance += metres((p.lat, p.lon), (sample.lat, sample.lon));
                timer_seconds += time.saturating_sub(fit_time(p.time));
            }
            Some(p) => {
                timer(&mut writer, fit_time(p.time), EVENT_STOP_ALL);
                timer(&mut writer, time, EVENT_START);
            }
            None => timer(&mut writer, time, EVENT_START),
        }
        previous = Some(sample);
        let mut fields = vec![
            (253, Value::U32(time)),
            (0, Value::I32(semicircles(sample.lat))),
            (1, Value::I32(semicircles(sample.lon))),
            (5, Value::U32((distance * 100.0).round() as u32)),
        ];
        fields.extend(altitude(sample.elevation).map(|v| (2, v)));
        let speed = sample.speed.map(|v| (v * 1000.0).round()).filter(|v| (0.0..65535.0).contains(v));
        fields.extend(speed.map(|v| (6, Value::U16(v as u16))));
        fields.extend(sample.heart_rate.filter(|&v| v != u8::MAX).map(|v| (3, Value::U8(v))));
        fields.extend(sample.cadence.filter(|&v| v != u8::MAX).map(|v| (4, Value::U8(v))));
        fields.extend(sample.power.filter(|&v| v != u16::MAX).map(|v| (7, Value::U16(v))));
        fields.extend(sample.temperature.filter(|&v| v != i8::MAX).map(|v| (13, Value::I8(v))));
        writer.message(RECORD, &fields);
    }
    timer(&mut writer, end, EVENT_STOP_ALL);
    let elapsed_ms = Value::U32(end.saturating_sub(start).saturating_mul(1000));
    let timer_ms = Value::U32(timer_seconds.saturating_mul(1000));
    let distance = Value::U32((distance * 100.0).round() as u32);
    let (sport, sub_sport) = ride.sport.to_fit();
    let summary = |event| {
        [
            (254, Value::U16(0)),
            (253, Value::U32(end)),
            (0, Value::Enum(event)),
            (1, Value::Enum(EVENT_STOP)),
            (2, Value::U32(start)),
            (7, elapsed_ms),
            (8, timer_ms),
            (9, distance),
        ]
    };
    writer.message(LAP, &summary(EVENT_LAP));
    let mut session = summary(EVENT_SESSION).to_vec();
    session.extend([(5, Value::Enum(sport)), (6, Value::Enum(sub_sport)), (25, Value::U16(0)), (26, Value::U16(1))]);
    writer.message(SESSION, &session);
    writer.message(
        ACTIVITY,
        &[
            (253, Value::U32(end)),
            (0, timer_ms),
            (1, Value::U16(1)),
            (2, Value::Enum(0)),
            (3, Value::Enum(EVENT_ACTIVITY)),
            (4, Value::Enum(EVENT_STOP)),
        ],
    );
    writer.finish()
}

/// Writes a course file. The time of each record is its share of `seconds` by distance; with
/// `seconds` 0, the course runs at 5 m/s. Each waypoint becomes a course point at the nearest
/// point of the line.
pub fn write_course(course: &Course) -> Vec<u8> {
    let mut distances = Vec::with_capacity(course.points.len());
    let mut total = 0.0;
    for (i, p) in course.points.iter().enumerate() {
        if let Some(q) = i.checked_sub(1).map(|i| &course.points[i]) {
            total += metres((q.lat, q.lon), (p.lat, p.lon));
        }
        distances.push(total);
    }
    let seconds = if course.seconds > 0 { f64::from(course.seconds) } else { total / COURSE_SPEED };
    let time = |d: f64| COURSE_START + if total > 0.0 { (seconds * d / total).round() as u32 } else { 0 };
    let centimetres = |d: f64| Value::U32((d * 100.0).round() as u32);
    let end = time(total);
    let ends = (course.points.first(), course.points.last());

    let mut writer = start_file(FILE_COURSE, COURSE_START, hash(course));
    // Sport 2 is cycling.
    let mut fields = vec![(4, Value::Enum(2))];
    if !course.name.is_empty() {
        fields.push((5, Value::Text(&course.name)));
    }
    writer.message(COURSE, &fields);
    let mut lap = vec![
        (253, Value::U32(COURSE_START)),
        (2, Value::U32(COURSE_START)),
        (7, Value::U32((end - COURSE_START).saturating_mul(1000))),
        (8, Value::U32((end - COURSE_START).saturating_mul(1000))),
        (9, centimetres(total)),
    ];
    if let (Some(first), Some(last)) = ends {
        lap.extend([
            (3, Value::I32(semicircles(first.lat))),
            (4, Value::I32(semicircles(first.lon))),
            (5, Value::I32(semicircles(last.lat))),
            (6, Value::I32(semicircles(last.lon))),
        ]);
    }
    writer.message(LAP, &lap);
    timer(&mut writer, COURSE_START, EVENT_START);
    let nearest: Vec<_> = course
        .waypoints
        .iter()
        .map(|w| {
            (0..course.points.len()).min_by(|&a, &b| {
                let to = |i: usize| metres((w.lat, w.lon), (course.points[i].lat, course.points[i].lon));
                to(a).total_cmp(&to(b))
            })
        })
        .collect();
    let mut index = 0;
    for (i, (p, &d)) in course.points.iter().zip(&distances).enumerate() {
        let mut fields = vec![
            (253, Value::U32(time(d))),
            (0, Value::I32(semicircles(p.lat))),
            (1, Value::I32(semicircles(p.lon))),
            (5, centimetres(d)),
        ];
        fields.extend(altitude(p.elevation).map(|v| (2, v)));
        writer.message(RECORD, &fields);
        // A course point follows the record that it is nearest to, so the course points are in the order of distance.
        for w in course.waypoints.iter().zip(&nearest).filter(|(_, n)| **n == Some(i)).map(|(w, _)| w) {
            let mut fields = vec![
                (254, Value::U16(index)),
                (1, Value::U32(time(d))),
                (2, Value::I32(semicircles(w.lat))),
                (3, Value::I32(semicircles(w.lon))),
                (4, centimetres(d)),
                (5, Value::Enum(0)),
            ];
            if let Some(name) = &w.name {
                fields.push((6, Value::Text(name)));
            }
            writer.message(COURSE_POINT, &fields);
            index += 1;
        }
    }
    timer(&mut writer, end, EVENT_STOP_ALL);
    writer.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(time: i64, lat: i32, segment_start: bool) -> Sample {
        Sample {
            time,
            lat,
            lon: 8_100_000,
            segment_start,
            elevation: None,
            speed: None,
            heart_rate: None,
            cadence: None,
            power: None,
            temperature: None,
        }
    }

    fn ride(count: i64) -> Ride {
        let samples = (0..count).map(|i| sample(1_760_000_000 + i, 46_500_000 + i as i32 * 10, i == 0)).collect();
        Ride { sport: Sport::Road, samples }
    }

    #[test]
    fn a_ride_is_equal_after_write_and_read() {
        let full = Sample {
            elevation: Some(2164.4),
            speed: Some(7.25),
            heart_rate: Some(142),
            cadence: Some(88),
            power: Some(231),
            temperature: Some(-3),
            ..sample(1_760_000_001, -33_865_143, false)
        };
        let samples = vec![
            sample(1_760_000_000, 46_571_234, true),
            full,
            Sample { elevation: Some(-12.2), ..sample(1_760_000_400, 46_571_300, true) },
            sample(1_760_000_405, 46_571_400, false),
        ];
        for sport in [Sport::Road, Sport::Gravel, Sport::Mtb, Sport::Bike, Sport::Foot, Sport::Other] {
            let ride = Ride { sport, samples: samples.clone() };
            assert_eq!(read(&write_ride(&ride)), Ok(File::Ride(ride)));
        }
    }

    #[test]
    fn a_course_is_equal_after_write_and_read() {
        let point = |lat, elevation| Point { lat, lon: 8_400_000, elevation };
        let course = Course {
            name: "Grimsel und Furka".into(),
            points: vec![point(46_560_000, Some(1708.2)), point(46_570_000, None), point(46_580_000, Some(2165.0))],
            waypoints: vec![
                Waypoint { lat: 46_570_010, lon: 8_400_000, name: Some("Wasser".into()) },
                Waypoint { lat: 46_580_000, lon: 8_400_000, name: None },
            ],
            seconds: 5400,
        };
        assert_eq!(read(&write_course(&course)), Ok(File::Course(course.clone())));
        let File::Course(timeless) = read(&write_course(&Course { seconds: 0, ..course })).unwrap() else { panic!() };
        // 2.2 km at 5 m/s.
        assert_eq!(timeless.seconds, 445);
    }

    #[test]
    fn a_cut_file_or_a_wrong_crc_gives_the_records_before_the_damage() {
        let ride = ride(100);
        let mut bytes = write_ride(&ride);
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        assert_eq!(read(&bytes), Ok(File::Ride(ride.clone())));
        // The cut is inside a record. The lap and the session come after the records, so the sport is lost.
        bytes.truncate(bytes.len() / 2);
        let File::Ride(cut) = read(&bytes).unwrap() else { panic!() };
        assert!((40..60).contains(&cut.samples.len()));
        assert_eq!(cut, Ride { sport: Sport::Other, samples: ride.samples[..cut.samples.len()].to_vec() });
    }

    #[test]
    fn each_error_has_its_code() {
        let file = |file_type, record: &[(u8, Value)]| {
            let mut writer = start_file(file_type, 1000, 1);
            if !record.is_empty() {
                writer.message(RECORD, record);
            }
            writer.finish()
        };
        let no_position = [(253, Value::U32(1000)), (3, Value::U8(120))];
        assert_eq!(read(b"<?xml version=\"1.0\"?><gpx/>"), Err(Error::NotFit));
        assert_eq!(read(&file(FILE_ACTIVITY, &[])), Err(Error::Damaged));
        let cut = write_ride(&ride(3));
        assert_eq!(read(&cut[..40]), Err(Error::Damaged));
        // File type 5 is a workout.
        assert_eq!(read(&file(5, &no_position)), Err(Error::Unsupported));
        assert_eq!(read(&file(FILE_ACTIVITY, &no_position)), Err(Error::NoPositions));
        assert_eq!(read(&file(FILE_COURSE, &no_position)), Err(Error::NoPositions));
    }

    #[test]
    fn a_course_skips_turn_instructions_and_keeps_records_with_no_time() {
        let mut writer = start_file(FILE_COURSE, 1000, 1);
        writer.message(RECORD, &[(0, Value::I32(1)), (1, Value::I32(0))]);
        writer.message(RECORD, &[(0, Value::I32(2)), (1, Value::I32(0))]);
        // Course point types 7 (right) and 23 (u_turn) are turns; 3 is water.
        let point =
            |kind, name| [(2, Value::I32(1)), (3, Value::I32(0)), (5, Value::Enum(kind)), (6, Value::Text(name))];
        for (kind, name) in [(7, "Right"), (3, "Well"), (23, "Turn")] {
            writer.message(COURSE_POINT, &point(kind, name));
        }
        let File::Course(course) = read(&writer.finish()).unwrap() else { panic!() };
        assert_eq!((course.points.len(), course.seconds), (2, 0));
        assert_eq!(course.waypoints, [Waypoint { lat: 0, lon: 0, name: Some("Well".into()) }]);
    }

    #[test]
    fn two_courses_have_two_serial_numbers() {
        let serial = |name: &str| {
            let point = Point { lat: 46_560_000, lon: 8_400_000, elevation: None };
            let course = Course { name: name.into(), points: vec![point], waypoints: vec![], seconds: 0 };
            let mut serial = None;
            container::decode(&write_course(&course), |m| serial = serial.or(m.int(3).filter(|_| m.global == FILE_ID)))
                .unwrap();
            serial.unwrap()
        };
        assert_eq!(serial("Furka"), serial("Furka"));
        assert_ne!(serial("Furka"), serial("Grimsel"));
    }

    #[test]
    fn a_dropped_record_gives_its_segment_start_to_the_next_sample() {
        let mut writer = start_file(FILE_ACTIVITY, 1000, 1);
        let at = |time, lat: i32| [(253, Value::U32(time)), (0, Value::I32(lat)), (1, Value::I32(0))];
        writer.message(RECORD, &[(253, Value::U32(999))]);
        writer.message(RECORD, &at(1000, 1));
        timer(&mut writer, 1000, EVENT_STOP_ALL);
        timer(&mut writer, 1100, EVENT_START);
        writer.message(RECORD, &[(253, Value::U32(1100))]);
        writer.message(RECORD, &at(1101, 2));
        writer.message(RECORD, &at(1102, 3));
        let File::Ride(ride) = read(&writer.finish()).unwrap() else { panic!() };
        let starts: Vec<_> = ride.samples.iter().map(|s| (s.time - EPOCH, s.segment_start)).collect();
        assert_eq!(starts, [(1000, true), (1101, true), (1102, false)]);
    }
}
