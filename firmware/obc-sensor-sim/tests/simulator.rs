use obc_ble::{parse_csc_measurement, parse_hr_measurement, parse_power_measurement, CrankCadence};
use obc_sensor_sim::{
    control::Control,
    input::{Button, Press},
    Scenario, Sensor, Simulator,
};

fn crank(sim: &Simulator, now: u64) -> obc_ble::CrankRevs {
    let (bytes, len) = sim.measurement(now);
    match sim.sensor {
        Sensor::Power => parse_power_measurement(&bytes[..len]).unwrap().crank.unwrap(),
        Sensor::Cadence => parse_csc_measurement(&bytes[..len]).unwrap().crank.unwrap(),
        _ => unreachable!(),
    }
}

#[test]
fn profiles_decode_through_the_bike_computer() {
    let mut sim = Simulator::default();
    sim.tick(2000);
    let (bytes, len) = sim.measurement(2000);
    let power = parse_power_measurement(&bytes[..len]).unwrap();
    assert_eq!(power.watts, 200);
    assert_eq!(power.crank.unwrap(), obc_ble::CrankRevs { revs: 3, event_time_1024: 2048 });
    sim.next_sensor(2000);
    sim.tick(4000);
    let (bytes, len) = sim.measurement(4000);
    let cadence = parse_csc_measurement(&bytes[..len]).unwrap();
    assert_eq!(cadence.wheel, None);
    assert_eq!(cadence.crank.unwrap().revs, 3);
    sim.next_sensor(4000);
    let (bytes, len) = sim.measurement(4000);
    let hr = parse_hr_measurement(&bytes[..len]).unwrap();
    assert_eq!((hr.bpm, hr.contact), (120, Some(true)));
    sim.stopped = true;
    let (bytes, len) = sim.measurement(4000);
    let hr = parse_hr_measurement(&bytes[..len]).unwrap();
    assert_eq!((hr.bpm, hr.contact), (0, Some(false)));
}

#[test]
fn crank_events_survive_rollover_stops_and_disconnected_time() {
    let mut sim = Simulator::default();
    sim.next_sensor(0);
    let mut reader = CrankCadence::new();
    sim.tick(62_000);
    reader.update(crank(&sim, 62_000));
    sim.tick(64_000);
    assert_eq!(reader.update(crank(&sim, 64_000)), Some(90));
    let stopped = crank(&sim, 64_000);
    sim.stopped = true;
    sim.tick(80_000);
    assert_eq!(crank(&sim, 80_000), stopped);
    assert_eq!(reader.update(stopped), Some(0));
    sim.stopped = false;
    sim.online = false;
    sim.tick(82_000);
    assert_eq!(crank(&sim, 82_000).revs, stopped.revs + 3);
    sim.tick(43_772_000);
    assert_eq!(crank(&sim, 43_772_000).revs, 98);
}

#[test]
fn last_event_time_is_not_notification_time_at_low_cadence() {
    let mut sim = Simulator::default();
    sim.next_sensor(0);
    for _ in 0..89 {
        sim.adjust(false);
    }
    for now in (20..=60_000).step_by(20) {
        sim.tick(now);
    }
    let event = crank(&sim, 60_000);
    assert_eq!(event, obc_ble::CrankRevs { revs: 1, event_time_1024: 61_440 });
    sim.tick(61_000);
    assert_eq!(crank(&sim, 61_000), event);
}

#[test]
fn scenarios_and_one_unit_controls_work_for_each_sensor() {
    let mut sim = Simulator::default();
    for sensor in [Sensor::Power, Sensor::Cadence, Sensor::HeartRate] {
        assert_eq!(sim.sensor, sensor);
        let base = sim.base();
        sim.adjust(true);
        assert_eq!(sim.base(), base + 1);
        sim.adjust(false);
        sim.next_scenario(0);
        assert_eq!(sim.scenario, Scenario::Ramp);
        assert_eq!(sim.value(0), base / 2);
        assert_eq!(sim.value(20_000), base);
        assert_eq!(sim.value(40_000), base / 2);
        sim.next_scenario(0);
        assert_eq!(sim.value(9_999), base / 2);
        assert_eq!(sim.value(10_000), base);
        for _ in 0..2100 {
            sim.adjust(true);
        }
        assert_eq!(sim.base(), sensor.limit());
        for _ in 0..2100 {
            sim.adjust(false);
        }
        assert_eq!(sim.base(), 0);
        sim.next_sensor(0);
    }
    assert_eq!(sim.base(), 0);
    let factory = [1, 2, 3, 4, 5, 6];
    let addresses = [Sensor::Power, Sensor::Cadence, Sensor::HeartRate].map(|s| s.address(factory));
    assert!(addresses.iter().all(|a| a[5] & 0xc0 == 0xc0));
    assert_ne!(addresses[0], addresses[1]);
    assert_ne!(addresses[1], addresses[2]);
    assert_ne!(addresses[0], addresses[2]);
}

#[test]
fn debounced_hold_repeats_one_unit_and_long_press_does_not_also_click() {
    let mut button = Button::default();
    assert_eq!(button.update(true, 0, true), None);
    assert_eq!(button.update(false, 10, true), None);
    assert_eq!(button.update(true, 20, true), None);
    assert_eq!(button.update(true, 50, true), Some(Press::Step));
    assert_eq!(button.update(true, 549, true), None);
    assert_eq!(button.update(true, 550, true), Some(Press::Step));
    assert_eq!(button.update(true, 610, true), Some(Press::Step));
    assert_eq!(button.update(false, 620, true), None);
    assert_eq!(button.update(false, 650, true), None);
    let mut button = Button::default();
    button.update(true, 0, false);
    button.update(true, 30, false);
    assert_eq!(button.update(true, 1030, false), Some(Press::Long));
    button.update(false, 1100, false);
    assert_eq!(button.update(false, 1130, false), None);
    button.update(true, 1200, false);
    button.update(true, 1230, false);
    button.update(false, 1300, false);
    assert_eq!(button.update(false, 1330, false), Some(Press::Short));
}

#[test]
fn calibration_requires_subscription_and_serializes_until_confirmation() {
    let mut control = Control::default();
    assert_eq!(control.start(&[0x0c], false, true), Err(0xfd));
    assert_eq!(control.start(&[], true, true), Err(0x0d));
    let reply = control.start(&[0x0c], true, true).unwrap();
    assert_eq!(&reply.bytes[..reply.len], &[0x20, 0x0c, 1, 0, 0]);
    assert!(reply.calibrating);
    assert_eq!(control.start(&[0x0c], true, true), Err(0xfe));
    control.confirmed();
    for (request, status) in [(&[0x0c][..], 4), (&[0x0c, 0][..], 3), (&[0xff][..], 2)] {
        let reply = control.start(request, true, false).unwrap();
        assert_eq!(&reply.bytes[..reply.len], &[0x20, request[0], status]);
        control.confirmed();
    }
}
