//! Real FIT files from other devices and services, from the fixture package `fit-captures`.

use obc_fit::{read, write_course, write_ride, File, Sport};

enum Expect {
    /// The Unix time of the first sample, and the sport.
    Ride(i64, Sport),
    Course(&'static str),
}

const CASES: [(&str, Expect); 4] = [
    ("garmin-ride.fit", Expect::Ride(0, Sport::Road)),
    ("wahoo-ride.fit", Expect::Ride(0, Sport::Road)),
    ("strava-ride.fit", Expect::Ride(0, Sport::Road)),
    ("komoot-course.fit", Expect::Course("")),
];

#[test]
fn each_capture_reads_with_its_start_and_sport_and_is_equal_after_write_and_read() {
    for (name, expect) in CASES {
        let file = read(&obc_fixtures::read("fit-captures", name)).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        match (&file, expect) {
            (File::Ride(ride), Expect::Ride(start, sport)) => {
                assert_eq!((ride.samples[0].time, ride.sport), (start, sport), "{name}");
                assert_eq!(read(&write_ride(ride)).as_ref(), Ok(&file), "{name}");
            }
            (File::Course(course), Expect::Course(title)) => {
                assert_eq!(course.name, title, "{name}");
                assert!(course.points.len() > 1 && course.seconds > 0, "{name}");
                assert_eq!(read(&write_course(course)).as_ref(), Ok(&file), "{name}");
            }
            _ => panic!("{name} is the wrong kind of file"),
        }
    }
}
