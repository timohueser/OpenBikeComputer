use obc_ble::radio_policy::{RadioPolicy, SensorActivity};

#[test]
fn discovery_waits_for_permission_and_resumes_after_usb() {
    let mut policy = RadioPolicy::new(true, true);
    policy.set_discovery(true);
    assert_eq!(policy.sensor_activity(), SensorActivity::Park, "USB is unknown at boot");

    policy.set_usb_inhibited(false);
    assert_eq!(policy.sensor_activity(), SensorActivity::Discover);

    policy.set_usb_inhibited(true);
    assert!(!policy.enabled());
    assert_eq!(policy.sensor_activity(), SensorActivity::Park, "a live scan must stop");
    policy.set_discovery(true);
    assert_eq!(policy.sensor_activity(), SensorActivity::Park, "a repeated request cannot bypass USB");

    policy.set_usb_inhibited(false);
    assert_eq!(policy.sensor_activity(), SensorActivity::Discover);
    policy.set_discovery(false);
    assert_eq!(policy.sensor_activity(), SensorActivity::Connect, "closing the list releases the initiator");
}

#[test]
fn closing_discovery_while_inhibited_does_not_queue_a_scan() {
    let mut policy = RadioPolicy::new(true, false);
    policy.set_discovery(true);
    policy.set_usb_inhibited(true);
    policy.set_discovery(false);
    policy.set_usb_inhibited(false);
    assert_eq!(policy.sensor_activity(), SensorActivity::Connect);

    policy.set_enabled(false);
    policy.set_discovery(true);
    assert_eq!(policy.sensor_activity(), SensorActivity::Park);
    policy.set_discovery(false);
    policy.set_enabled(true);
    assert_eq!(policy.sensor_activity(), SensorActivity::Connect);
}

#[test]
fn rider_switch_and_usb_each_veto_discovery_and_connections() {
    let mut policy = RadioPolicy::new(false, false);
    for discovery in [false, true] {
        policy.set_discovery(discovery);
        assert_eq!(policy.sensor_activity(), SensorActivity::Park);
        policy.set_usb_inhibited(true);
        policy.set_enabled(true);
        assert_eq!(policy.sensor_activity(), SensorActivity::Park, "enabling Bluetooth cannot override USB");
        policy.set_enabled(false);
        policy.set_usb_inhibited(false);
        assert_eq!(policy.sensor_activity(), SensorActivity::Park, "unplugging cannot override the rider");
    }
    policy.set_enabled(true);
    assert!(policy.enabled());
    assert_eq!(policy.sensor_activity(), SensorActivity::Discover);
}
