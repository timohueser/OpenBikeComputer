use obc_ports::{
    AltimeterSource, Button, ButtonEvent, CadenceSource, ClockSource, CompassSource, DateTime, Fix, FuelGauge, GpsTime,
    HeartRateSource, InputEvent, InputSource, LocationSource, PowerSource, SettingsStore, TemperatureSource,
    TrackError, TrackPoint, TrackSink,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FakeSettings {
    brightness: u8,
}

#[derive(Default)]
struct FakeSettingsStore(Option<FakeSettings>);

impl SettingsStore for FakeSettingsStore {
    type Value = FakeSettings;

    fn load(&mut self) -> Option<Self::Value> {
        self.0
    }

    fn save(&mut self, value: &Self::Value) -> Result<(), obc_ports::SettingsSaveError> {
        self.0 = Some(*value);
        Ok(())
    }
}

struct FakeLocation(Option<Fix>);

impl LocationSource for FakeLocation {
    fn poll(&mut self) -> Option<Fix> {
        self.0.take()
    }
}

#[derive(Default)]
struct FakeSensors;

impl AltimeterSource for FakeSensors {
    fn poll(&mut self) -> Option<f32> {
        Some(512.5)
    }
}

impl TemperatureSource for FakeSensors {
    fn poll(&mut self) -> Option<f32> {
        Some(18.0)
    }
}

impl ClockSource for FakeSensors {
    fn poll(&mut self) -> Option<GpsTime> {
        Some(GpsTime { utc: DateTime { year: 2026, month: 7, day: 13, hour: 9, minute: 30 }, second: 12 })
    }
}

impl CompassSource for FakeSensors {
    fn poll(&mut self) -> Option<f32> {
        Some(270.0)
    }
}

impl FuelGauge for FakeSensors {
    fn poll(&mut self) -> Option<u8> {
        Some(73)
    }
}

impl HeartRateSource for FakeSensors {
    fn poll(&mut self) -> Option<u16> {
        Some(140)
    }
}

impl PowerSource for FakeSensors {
    fn poll(&mut self) -> Option<u16> {
        Some(240)
    }
}

impl CadenceSource for FakeSensors {
    fn poll(&mut self) -> Option<u8> {
        Some(88)
    }
}

struct FakeInput(Option<InputEvent>);

impl InputSource for FakeInput {
    fn poll(&mut self) -> Option<InputEvent> {
        self.0.take()
    }
}

#[derive(Default)]
struct FakeTrack(Option<TrackPoint>);

impl TrackSink for FakeTrack {
    fn record(&mut self, point: TrackPoint) -> Result<(), TrackError> {
        self.0 = Some(point);
        Ok(())
    }
}

#[test]
fn independent_fakes_implement_sensor_input_and_track_ports() {
    let fix = Fix::at(47_000_000, 8_000_000);
    let mut location = FakeLocation(Some(fix));
    assert_eq!(location.poll(), Some(fix));
    assert_eq!(location.poll(), None);

    let mut sensors = FakeSensors;
    assert_eq!(AltimeterSource::poll(&mut sensors), Some(512.5));
    assert_eq!(TemperatureSource::poll(&mut sensors), Some(18.0));
    assert_eq!(ClockSource::poll(&mut sensors).unwrap().second, 12);
    assert_eq!(CompassSource::poll(&mut sensors), Some(270.0));
    assert_eq!(FuelGauge::poll(&mut sensors), Some(73));
    assert_eq!(HeartRateSource::poll(&mut sensors), Some(140));
    assert_eq!(PowerSource::poll(&mut sensors), Some(240));
    assert_eq!(CadenceSource::poll(&mut sensors), Some(88));

    let edge = InputEvent::Button(ButtonEvent::Down(Button::Encoder));
    let mut input = FakeInput(Some(edge));
    assert_eq!(input.poll(), Some(edge));

    let point = TrackPoint {
        lon: 8_000_000,
        lat: 47_000_000,
        ele: 512,
        t_ms: 1_000,
        segment_start: true,
        hr: Some(140),
        cadence: Some(88),
        power: Some(240),
    };
    let mut track = FakeTrack::default();
    track.record(point).unwrap();
    assert_eq!(track.0, Some(point));
}

#[test]
fn independent_fake_implements_settings_port() {
    let mut store = FakeSettingsStore::default();
    assert_eq!(store.load(), None);

    let settings = FakeSettings { brightness: 73 };
    store.save(&settings);
    assert_eq!(store.load(), Some(settings));
}
