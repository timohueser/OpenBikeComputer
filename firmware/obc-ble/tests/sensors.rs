//! The standard cycling-sensor GATT codecs: byte layouts pinned by hand (flags + LE fields spelled
//! out), the optional-field skip chains walked at every combination, and the tolerance rule —
//! short/garbled frames yield `None`, never a panic. Plus the crank→rpm accumulator's corner cases
//! (coasting, both `u16` wraps, duplicate-event hold, reconnect reset) and a truncation fuzz sweep
//! that hammers every prefix of a valid frame through every parser.

use obc_ble::{
    classify_advertisement, parse_battery_level, parse_csc_measurement, parse_hr_measurement, parse_power_measurement,
    power_crank_feeds_cadence, AdvMatch, CrankCadence, CrankRevs, CscSample, HrSample, PowerSample, SensorKind,
    WheelRevs, UUID_BATTERY_LEVEL, UUID_BATTERY_SERVICE, UUID_CSC_MEASUREMENT, UUID_CSC_SERVICE,
    UUID_CYCLING_POWER_MEASUREMENT, UUID_CYCLING_POWER_SERVICE, UUID_HEART_RATE_SERVICE, UUID_HR_MEASUREMENT,
};

// ---------------------------------------------------------------------------------------------
// UUID constants — pinned so the board crate's `Uuid`s and the scan filter can't drift.
// ---------------------------------------------------------------------------------------------

#[test]
fn uuid_constants() {
    assert_eq!(UUID_HEART_RATE_SERVICE, 0x180D);
    assert_eq!(UUID_CYCLING_POWER_SERVICE, 0x1818);
    assert_eq!(UUID_CSC_SERVICE, 0x1816);
    assert_eq!(UUID_BATTERY_SERVICE, 0x180F);
    assert_eq!(UUID_HR_MEASUREMENT, 0x2A37);
    assert_eq!(UUID_CYCLING_POWER_MEASUREMENT, 0x2A63);
    assert_eq!(UUID_CSC_MEASUREMENT, 0x2A5B);
    assert_eq!(UUID_BATTERY_LEVEL, 0x2A19);
}

// ---------------------------------------------------------------------------------------------
// Heart Rate Measurement (0x2A37)
// ---------------------------------------------------------------------------------------------

#[test]
fn hr_8bit_classic_strap_frame() {
    // The canonical frame: flags 0x00 (8-bit value, no contact feature), 72 bpm.
    assert_eq!(parse_hr_measurement(&[0x00, 0x48]), Some(HrSample { bpm: 72, contact: None }));
}

#[test]
fn hr_16bit_value() {
    // Flag bit 0 set → bpm is u16 LE. 0x0140 = 320 bpm (well past 8-bit range).
    assert_eq!(parse_hr_measurement(&[0x01, 0x40, 0x01]), Some(HrSample { bpm: 320, contact: None }));
}

#[test]
fn hr_contact_bits() {
    // Bit 2 = supported, bit 1 = status.
    // 0b100 = supported, no contact.
    assert_eq!(parse_hr_measurement(&[0b100, 60]), Some(HrSample { bpm: 60, contact: Some(false) }));
    // 0b110 = supported, contact detected.
    assert_eq!(parse_hr_measurement(&[0b110, 60]), Some(HrSample { bpm: 60, contact: Some(true) }));
    // 0b010 = status bit set but NOT supported → we report nothing.
    assert_eq!(parse_hr_measurement(&[0b010, 60]), Some(HrSample { bpm: 60, contact: None }));
}

#[test]
fn hr_rr_bearing_frame_from_garmin_capture() {
    // A real Garmin HRM frame: flags 0x16 = bit1 (status) + bit2 (supported) + bit4 (RR present),
    // 8-bit bpm 65, then two u16 RR intervals we ignore. bpm + contact still read cleanly.
    let frame = [0x16, 0x41, 0x24, 0x03, 0x18, 0x03];
    assert_eq!(parse_hr_measurement(&frame), Some(HrSample { bpm: 65, contact: Some(true) }));
}

#[test]
fn hr_energy_expended_flag_ignored() {
    // Bit 3 (energy expended) present: two extra bytes after the bpm. We still parse bpm/contact.
    let frame = [0b1000, 0x50, 0xE8, 0x03];
    assert_eq!(parse_hr_measurement(&frame), Some(HrSample { bpm: 0x50, contact: None }));
}

#[test]
fn hr_short_buffers_return_none() {
    assert_eq!(parse_hr_measurement(&[]), None); // no flags
    assert_eq!(parse_hr_measurement(&[0x00]), None); // 8-bit format but no bpm byte
    assert_eq!(parse_hr_measurement(&[0x01, 0x40]), None); // 16-bit format but only one bpm byte
}

// ---------------------------------------------------------------------------------------------
// Cycling Power Measurement (0x2A63)
// ---------------------------------------------------------------------------------------------

#[test]
fn cps_power_only_frame() {
    // flags 0x0000, power 200 W. No optional fields.
    let frame = [0x00, 0x00, 0xC8, 0x00];
    assert_eq!(parse_power_measurement(&frame), Some(PowerSample { watts: 200, crank: None }));
}

#[test]
fn cps_power_plus_crank() {
    // flags bit5 (crank) only, power 250 W, crank revs 100, event time 2048.
    let mut frame = vec![0x20, 0x00, 0xFA, 0x00];
    frame.extend_from_slice(&100u16.to_le_bytes());
    frame.extend_from_slice(&2048u16.to_le_bytes());
    assert_eq!(
        parse_power_measurement(&frame),
        Some(PowerSample { watts: 250, crank: Some(CrankRevs { revs: 100, event_time_1024: 2048 }) })
    );
}

#[test]
fn cps_full_skip_chain() {
    // bits 0 (balance) + 2 (torque) + 4 (wheel) + 5 (crank) all set — the parser must skip
    // 1 + 2 + 6 bytes before reading crank.
    let flags: u16 = (1 << 0) | (1 << 2) | (1 << 4) | (1 << 5);
    let mut frame = vec![];
    frame.extend_from_slice(&flags.to_le_bytes());
    frame.extend_from_slice(&(-30i16).to_le_bytes()); // negative watts (regen)
    frame.push(0x7F); // pedal power balance (1 B)
    frame.extend_from_slice(&500u16.to_le_bytes()); // accumulated torque (2 B)
    frame.extend_from_slice(&12345u32.to_le_bytes()); // wheel revs (4 B)
    frame.extend_from_slice(&9000u16.to_le_bytes()); // wheel event time (2 B)
    frame.extend_from_slice(&777u16.to_le_bytes()); // crank revs
    frame.extend_from_slice(&4096u16.to_le_bytes()); // crank event time
    assert_eq!(
        parse_power_measurement(&frame),
        Some(PowerSample { watts: -30, crank: Some(CrankRevs { revs: 777, event_time_1024: 4096 }) })
    );
}

#[test]
fn cps_negative_watts() {
    let frame = [0x00, 0x00, 0x9C, 0xFF]; // 0xFF9C = -100
    assert_eq!(parse_power_measurement(&frame), Some(PowerSample { watts: -100, crank: None }));
}

#[test]
fn cps_short_at_every_boundary() {
    // The full-skip-chain frame, truncated at each byte, must never yield a crank read past the end.
    let flags: u16 = (1 << 0) | (1 << 2) | (1 << 4) | (1 << 5);
    let mut frame = vec![];
    frame.extend_from_slice(&flags.to_le_bytes());
    frame.extend_from_slice(&250i16.to_le_bytes());
    frame.push(0x7F);
    frame.extend_from_slice(&500u16.to_le_bytes());
    frame.extend_from_slice(&12345u32.to_le_bytes());
    frame.extend_from_slice(&9000u16.to_le_bytes());
    frame.extend_from_slice(&777u16.to_le_bytes());
    frame.extend_from_slice(&4096u16.to_le_bytes());

    // Every prefix shorter than the whole frame must be None (crank incomplete) — and never panic.
    for n in 0..frame.len() {
        assert_eq!(parse_power_measurement(&frame[..n]), None, "prefix len {n} should be None");
    }
    // The complete frame parses.
    assert!(parse_power_measurement(&frame).is_some());

    // Mandatory head alone (no optional flags) needs exactly 4 bytes.
    assert_eq!(parse_power_measurement(&[0x00, 0x00, 0x01]), None);
    assert_eq!(parse_power_measurement(&[0x00, 0x00, 0x01, 0x00]), Some(PowerSample { watts: 1, crank: None }));
}

#[test]
fn cps_declared_fields_must_be_present_even_without_crank() {
    // flags bit4 (wheel) only, NO crank — a frame truncated inside the declared wheel field is
    // garbled: the mandatory head must not be trusted either. "Short buffer at any point → None."
    let flags: u16 = 1 << 4;
    let mut frame = vec![];
    frame.extend_from_slice(&flags.to_le_bytes());
    frame.extend_from_slice(&220i16.to_le_bytes());
    frame.extend_from_slice(&12345u32.to_le_bytes()); // wheel revs
    frame.extend_from_slice(&9000u16.to_le_bytes()); // wheel event time

    // Truncated right after the mandatory head (declared wheel data entirely missing) → None,
    // and every partial-wheel truncation too.
    for n in 4..frame.len() {
        assert_eq!(parse_power_measurement(&frame[..n]), None, "prefix len {n} should be None");
    }
    // The complete wheel-only frame parses — with no crank data to surface.
    assert_eq!(parse_power_measurement(&frame), Some(PowerSample { watts: 220, crank: None }));
}

#[test]
fn cps_crank_after_only_wheel() {
    // Only bits 4 (wheel) + 5 (crank): the parser skips 6 wheel bytes then reads crank.
    let flags: u16 = (1 << 4) | (1 << 5);
    let mut frame = vec![];
    frame.extend_from_slice(&flags.to_le_bytes());
    frame.extend_from_slice(&180i16.to_le_bytes());
    frame.extend_from_slice(&1u32.to_le_bytes()); // wheel revs
    frame.extend_from_slice(&2u16.to_le_bytes()); // wheel event time
    frame.extend_from_slice(&55u16.to_le_bytes()); // crank revs
    frame.extend_from_slice(&66u16.to_le_bytes()); // crank event time
    assert_eq!(
        parse_power_measurement(&frame),
        Some(PowerSample { watts: 180, crank: Some(CrankRevs { revs: 55, event_time_1024: 66 }) })
    );
}

// ---------------------------------------------------------------------------------------------
// CSC Measurement (0x2A5B)
// ---------------------------------------------------------------------------------------------

#[test]
fn csc_empty_flags() {
    assert_eq!(parse_csc_measurement(&[0x00]), Some(CscSample { wheel: None, crank: None }));
}

#[test]
fn csc_crank_only() {
    // flags bit1 = crank present. revs 200, event time 1500.
    let mut frame = vec![0b10];
    frame.extend_from_slice(&200u16.to_le_bytes());
    frame.extend_from_slice(&1500u16.to_le_bytes());
    assert_eq!(
        parse_csc_measurement(&frame),
        Some(CscSample { wheel: None, crank: Some(CrankRevs { revs: 200, event_time_1024: 1500 }) })
    );
}

#[test]
fn csc_wheel_only() {
    // flags bit0 = wheel present. revs 0x0001_0000 (u32), event time 800.
    let mut frame = vec![0b01];
    frame.extend_from_slice(&65_536u32.to_le_bytes());
    frame.extend_from_slice(&800u16.to_le_bytes());
    assert_eq!(
        parse_csc_measurement(&frame),
        Some(CscSample { wheel: Some(WheelRevs { revs: 65_536, event_time_1024: 800 }), crank: None })
    );
}

#[test]
fn csc_wheel_and_crank() {
    // Both flags: wheel (6 B) then crank (4 B).
    let mut frame = vec![0b11];
    frame.extend_from_slice(&10u32.to_le_bytes()); // wheel revs
    frame.extend_from_slice(&20u16.to_le_bytes()); // wheel event time
    frame.extend_from_slice(&30u16.to_le_bytes()); // crank revs
    frame.extend_from_slice(&40u16.to_le_bytes()); // crank event time
    assert_eq!(
        parse_csc_measurement(&frame),
        Some(CscSample {
            wheel: Some(WheelRevs { revs: 10, event_time_1024: 20 }),
            crank: Some(CrankRevs { revs: 30, event_time_1024: 40 }),
        })
    );
}

#[test]
fn csc_short_at_every_boundary() {
    let mut frame = vec![0b11];
    frame.extend_from_slice(&10u32.to_le_bytes());
    frame.extend_from_slice(&20u16.to_le_bytes());
    frame.extend_from_slice(&30u16.to_le_bytes());
    frame.extend_from_slice(&40u16.to_le_bytes());
    for n in 0..frame.len() {
        assert_eq!(parse_csc_measurement(&frame[..n]), None, "prefix len {n} should be None");
    }
    assert!(parse_csc_measurement(&frame).is_some());
    assert_eq!(parse_csc_measurement(&[]), None); // no flags byte at all
}

// ---------------------------------------------------------------------------------------------
// Battery Level (0x2A19)
// ---------------------------------------------------------------------------------------------

#[test]
fn battery_level() {
    assert_eq!(parse_battery_level(&[0]), Some(0));
    assert_eq!(parse_battery_level(&[78]), Some(78));
    assert_eq!(parse_battery_level(&[100]), Some(100));
    assert_eq!(parse_battery_level(&[250]), Some(100)); // out-of-range clamps to 100
    assert_eq!(parse_battery_level(&[42, 0, 0]), Some(42)); // trailing bytes ignored
    assert_eq!(parse_battery_level(&[]), None);
}

// ---------------------------------------------------------------------------------------------
// CrankCadence accumulator
// ---------------------------------------------------------------------------------------------

fn crank(revs: u16, t: u16) -> CrankRevs {
    CrankRevs { revs, event_time_1024: t }
}

#[test]
fn cadence_first_sample_has_no_delta() {
    let mut c = CrankCadence::new();
    assert_eq!(c.update(crank(10, 1000)), None); // no baseline yet
}

#[test]
fn cadence_steady_90_rpm() {
    // 90 rpm = 1.5 rev/s → 1 rev every 1/1.5 s = 682.67 ticks of 1/1024 s. Use exact math: over
    // 3 revs the event time advances 2048 ticks (2 s) → 3 rev / 2 s * 60 = 90 rpm.
    let mut c = CrankCadence::new();
    assert_eq!(c.update(crank(0, 0)), None);
    assert_eq!(c.update(crank(3, 2048)), Some(90));
    assert_eq!(c.update(crank(6, 4096)), Some(90));
    assert_eq!(c.update(crank(9, 6144)), Some(90));
}

#[test]
fn cadence_u16_rev_wrap() {
    // Revs wrap from near-max across zero; wrapping_sub gives the true small delta.
    let mut c = CrankCadence::new();
    assert_eq!(c.update(crank(65_534, 0)), None);
    // +3 revs (65534 -> 1 wraps) over 2048 ticks (2 s) → 90 rpm.
    assert_eq!(c.update(crank(1, 2048)), Some(90));
}

#[test]
fn cadence_u16_time_wrap() {
    // Event time wraps; wrapping_sub gives the true small Δt.
    let mut c = CrankCadence::new();
    assert_eq!(c.update(crank(0, 65_534)), None);
    // Δt = 1024 - 65534 wraps... choose so Δt = 2048 ticks: 65534 + 2048 = 67582 -> 2046 (mod 65536).
    // +3 revs over 2048 ticks → 90 rpm.
    assert_eq!(c.update(crank(3, 2046)), Some(90));
}

#[test]
fn cadence_coasting_returns_zero() {
    // Sensor keeps notifying with unchanged revs (and frozen event time) → 0 rpm.
    let mut c = CrankCadence::new();
    assert_eq!(c.update(crank(50, 5000)), None);
    assert_eq!(c.update(crank(50, 5000)), Some(0));
    assert_eq!(c.update(crank(50, 5000)), Some(0));
    // Then pedaling resumes and computes cleanly against the coasting baseline.
    assert_eq!(c.update(crank(53, 7048)), Some(90));
}

#[test]
fn cadence_duplicate_event_holds() {
    // Revs advanced but event time didn't move → can't compute → None, and the baseline is held.
    let mut c = CrankCadence::new();
    assert_eq!(c.update(crank(10, 1000)), None);
    assert_eq!(c.update(crank(13, 1000)), None); // Δt == 0, Δrevs > 0 → hold
                                                 // Next good frame computes against the held baseline (revs 10 @ t 1000), not the garbled one:
                                                 // +3 revs over 2048 ticks → 90 rpm.
    assert_eq!(c.update(crank(13, 3048)), Some(90));
}

#[test]
fn cadence_clamps_to_255() {
    let mut c = CrankCadence::new();
    assert_eq!(c.update(crank(0, 0)), None);
    // 100 revs over 1024 ticks (1 s) → 6000 rpm, clamped to 255.
    assert_eq!(c.update(crank(100, 1024)), Some(255));
}

#[test]
fn cadence_reset_on_reconnect() {
    let mut c = CrankCadence::new();
    assert_eq!(c.update(crank(10, 1000)), None);
    assert_eq!(c.update(crank(13, 3048)), Some(90));
    // Disconnect: drop the baseline so a reconnect (counters possibly reset on the sensor) doesn't
    // compute a wild delta across the gap.
    c.reset();
    assert_eq!(c.update(crank(500, 40_000)), None); // treated as a fresh first sample
    assert_eq!(c.update(crank(503, 42_048)), Some(90));
}

// ---------------------------------------------------------------------------------------------
// Fuzz-ish tolerance sweep: no truncation of any valid frame may panic.
// ---------------------------------------------------------------------------------------------

#[test]
fn no_prefix_of_any_frame_panics() {
    // A representative valid frame for each parser, plus their every-byte-flipped variants, run
    // through all parsers at every prefix length. Parsers must return (Some or None), never panic.
    let frames: &[&[u8]] = &[
        &[0x16, 0x41, 0x24, 0x03, 0x18, 0x03], // HR + RR
        &[0x3F, 0x00, 0xFA, 0x00, 0x7F, 0xF4, 0x01, 0x39, 0x30, 0x00, 0x00, 0x28, 0x23, 0x09, 0x03, 0x10, 0x00, 0x10], // CPS all flags
        &[0x03, 0x0A, 0x00, 0x00, 0x00, 0x14, 0x00, 0x1E, 0x00, 0x28, 0x00], // CSC both
        &[0x64],                                                             // battery
    ];
    for frame in frames {
        for byte in 0..=255u16 {
            // Mutate the flags byte(s) too, to exercise unusual skip chains.
            let mut m = frame.to_vec();
            if !m.is_empty() {
                m[0] = byte as u8;
            }
            if m.len() > 1 {
                m[1] = (byte >> 3) as u8;
            }
            for n in 0..=m.len() {
                let s = &m[..n];
                let _ = parse_hr_measurement(s);
                let _ = parse_power_measurement(s);
                let _ = parse_csc_measurement(s);
                let _ = parse_battery_level(s);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Scan-side classification (SE6, #713): AD-structure walk → SensorKind + local name, cadence
// arbitration, and the mapping to service / measurement UUIDs.
// ---------------------------------------------------------------------------------------------

/// Build one AD structure `[len][type][payload…]`.
fn ad(ty: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![(payload.len() + 1) as u8, ty];
    v.extend_from_slice(payload);
    v
}

#[test]
fn kind_uuid_mapping() {
    assert_eq!(SensorKind::HeartRate.service_uuid(), UUID_HEART_RATE_SERVICE);
    assert_eq!(SensorKind::HeartRate.measurement_uuid(), UUID_HR_MEASUREMENT);
    assert_eq!(SensorKind::Power.service_uuid(), UUID_CYCLING_POWER_SERVICE);
    assert_eq!(SensorKind::Power.measurement_uuid(), UUID_CYCLING_POWER_MEASUREMENT);
    assert_eq!(SensorKind::Cadence.service_uuid(), UUID_CSC_SERVICE);
    assert_eq!(SensorKind::Cadence.measurement_uuid(), UUID_CSC_MEASUREMENT);
}

#[test]
fn classify_hr_strap_with_complete_name() {
    // Flags (0x01), complete 16-bit UUID list with HR (0x03), complete local name (0x09).
    let mut adv = ad(0x01, &[0x06]);
    adv.extend(ad(0x03, &0x180Du16.to_le_bytes()));
    adv.extend(ad(0x09, b"Polar H10"));
    assert_eq!(classify_advertisement(&adv), Some(AdvMatch { kind: SensorKind::HeartRate, name: Some("Polar H10") }));
}

#[test]
fn classify_prefers_hr_over_power_over_cadence() {
    // A device that advertises all three services classifies as HR (highest priority).
    let mut all = Vec::new();
    all.extend(ad(0x03, &0x1816u16.to_le_bytes())); // CSC first on the wire
    all.extend(ad(0x03, &0x1818u16.to_le_bytes())); // then power
    all.extend(ad(0x03, &0x180Du16.to_le_bytes())); // then HR
    assert_eq!(classify_advertisement(&all).unwrap().kind, SensorKind::HeartRate);

    // Power meter that also exposes CSC crank data → Power, not Cadence.
    let mut pm = Vec::new();
    pm.extend(ad(0x02, &0x1816u16.to_le_bytes()));
    pm.extend(ad(0x02, &0x1818u16.to_le_bytes()));
    assert_eq!(classify_advertisement(&pm).unwrap().kind, SensorKind::Power);

    // A pure CSC sensor → Cadence.
    let csc = ad(0x03, &0x1816u16.to_le_bytes());
    assert_eq!(classify_advertisement(&csc).unwrap().kind, SensorKind::Cadence);
}

#[test]
fn classify_uuid_list_with_multiple_entries() {
    // One 0x03 structure carrying two UUIDs: an unrelated one then power.
    let mut payload = Vec::new();
    payload.extend_from_slice(&0x180Fu16.to_le_bytes()); // battery service (ignored)
    payload.extend_from_slice(&0x1818u16.to_le_bytes()); // power
    let adv = ad(0x03, &payload);
    assert_eq!(classify_advertisement(&adv).unwrap().kind, SensorKind::Power);
}

#[test]
fn classify_prefers_complete_name_over_shortened() {
    // Shortened name appears first, complete second — complete wins regardless of order.
    let mut adv = ad(0x08, b"Wahoo");
    adv.extend(ad(0x03, &0x1818u16.to_le_bytes()));
    adv.extend(ad(0x09, b"Wahoo KICKR"));
    assert_eq!(classify_advertisement(&adv).unwrap().name, Some("Wahoo KICKR"));

    // Only a shortened name present → that is used.
    let mut short = ad(0x03, &0x180Du16.to_le_bytes());
    short.extend(ad(0x08, b"HRM"));
    assert_eq!(classify_advertisement(&short).unwrap().name, Some("HRM"));
}

#[test]
fn classify_no_sensor_service_is_none() {
    // Flags + a name + only the battery service → not a sensor we pair with.
    let mut adv = ad(0x01, &[0x06]);
    adv.extend(ad(0x03, &0x180Fu16.to_le_bytes()));
    adv.extend(ad(0x09, b"Some Phone"));
    assert_eq!(classify_advertisement(&adv), None);
    // Empty / zero advertisement.
    assert_eq!(classify_advertisement(&[]), None);
    assert_eq!(classify_advertisement(&[0x00]), None);
}

#[test]
fn classify_tolerates_truncation() {
    // A structure claiming more bytes than remain ends the walk without panicking — but the HR
    // service seen before the runt is still reported.
    let mut adv = ad(0x03, &0x180Du16.to_le_bytes());
    adv.push(0x05); // len=5 but no bytes follow
    adv.push(0x09);
    let m = classify_advertisement(&adv).unwrap();
    assert_eq!(m.kind, SensorKind::HeartRate);

    // Invalid-UTF8 name yields a match with no name rather than an error.
    let mut bad = ad(0x03, &0x180Du16.to_le_bytes());
    bad.extend(ad(0x09, &[0xFF, 0xFE, 0x80]));
    assert_eq!(classify_advertisement(&bad), Some(AdvMatch { kind: SensorKind::HeartRate, name: None }));
}

#[test]
fn classify_ad_truncation_fuzz_never_panics() {
    // Every prefix of a rich multi-structure advertisement must return cleanly.
    let mut adv = ad(0x01, &[0x06]);
    adv.extend(ad(0x03, &0x180Du16.to_le_bytes()));
    adv.extend(ad(0x02, &0x1818u16.to_le_bytes()));
    adv.extend(ad(0x09, b"Sensor XYZ"));
    for n in 0..=adv.len() {
        let _ = classify_advertisement(&adv[..n]);
    }
}

#[test]
fn cadence_arbitration_dedicated_wins() {
    // A saved dedicated cadence sensor takes the quantity — the power meter's crank data must not.
    assert!(!power_crank_feeds_cadence(true));
    // No dedicated cadence sensor → the power meter's crank data fills cadence.
    assert!(power_crank_feeds_cadence(false));
}
