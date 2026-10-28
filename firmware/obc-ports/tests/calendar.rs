use obc_ports::DateTime;

#[test]
fn calendar_arithmetic_has_no_app_editor_year_policy() {
    let next = DateTime { year: 2099, month: 12, day: 31, hour: 23, minute: 59 }.add_minutes(1);
    assert_eq!(next, DateTime { year: 2100, month: 1, day: 1, hour: 0, minute: 0 });

    let previous = DateTime { year: 2020, month: 1, day: 1, hour: 0, minute: 15 }.with_offset(-30);
    assert_eq!(previous, DateTime { year: 2019, month: 12, day: 31, hour: 23, minute: 45 });
}

/// The calendar arithmetic runs over Unix seconds, so it saturates at that window's edges instead
/// of wrapping into a nonsense year. Both edges are far outside the app's own 2020–2099 clamp.
#[test]
fn calendar_arithmetic_saturates_at_the_epoch_window() {
    let epoch = DateTime { year: 1970, month: 1, day: 1, hour: 0, minute: 0 };
    assert_eq!(epoch.with_offset(-60), epoch, "no date exists below the epoch to roll back to");
    assert_eq!(epoch.add_minutes(60), DateTime { hour: 1, ..epoch }, "forward from the floor is normal");

    // The ceiling: `u32::MAX` epoch seconds is 2106-02-07 06:28:15, truncated to the minute.
    let ceiling = DateTime { year: 2106, month: 2, day: 7, hour: 6, minute: 28 };
    let far = DateTime { year: 2099, month: 1, day: 1, hour: 0, minute: 0 }.add_minutes(u32::MAX);
    assert_eq!(far, ceiling, "a huge advance pins at the last representable minute");
    assert_eq!(ceiling.add_minutes(1), ceiling, "and stays there");
}

fn dt(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> DateTime {
    DateTime { year, month, day, hour, minute }
}

/// References computed independently with `date -u +%s`.
#[test]
fn unix_seconds_match_reference_timestamps_both_ways() {
    for (stamp, secs) in [
        (dt(2020, 1, 1, 0, 0), 1_577_836_800),
        (dt(2024, 2, 29, 12, 30), 1_709_209_800),
        (dt(2026, 7, 2, 9, 33), 1_782_984_780),
        (dt(2026, 12, 31, 23, 59), 1_798_761_540),
        (dt(2099, 12, 31, 23, 59), 4_102_444_740),
    ] {
        assert_eq!(stamp.to_unix(), secs, "to_unix {stamp:?}");
        assert_eq!(DateTime::from_unix(secs), stamp, "from_unix {secs}");
    }
    for stamp in [dt(1970, 1, 1, 0, 0), dt(2000, 2, 29, 12, 34), dt(2026, 7, 5, 9, 41), dt(2038, 1, 19, 3, 14)] {
        assert_eq!(DateTime::from_unix(stamp.to_unix()), stamp, "round trip {stamp:?}");
    }
    assert_eq!(DateTime::from_unix(59), dt(1970, 1, 1, 0, 0), "seconds truncate to the minute, never round");
}

#[test]
fn february_length_follows_the_gregorian_leap_rule() {
    for (year, days) in [(2024, 29), (2025, 28), (2000, 29), (2100, 28)] {
        assert_eq!(DateTime::month_len(year, 2), days, "February {year}");
    }
}

/// Long advances are checked against `datetime.timedelta`: they separate a correct bulk carry from
/// a day-at-a-time walk that loses a leap day.
#[test]
fn add_minutes_carries_into_reference_dates() {
    const DAY: u32 = 24 * 60;
    let base = dt(2025, 6, 29, 14, 40);
    for (start, mins, expected) in [
        (base, 0, base),
        (base, 5, dt(2025, 6, 29, 14, 45)),
        (base, 25, dt(2025, 6, 29, 15, 5)),
        (dt(2025, 6, 29, 23, 59), 1, dt(2025, 6, 30, 0, 0)),
        (dt(2025, 6, 30, 23, 0), 120, dt(2025, 7, 1, 1, 0)),
        (dt(2025, 12, 31, 23, 59), 1, dt(2026, 1, 1, 0, 0)),
        (dt(2024, 2, 28, 0, 0), DAY, dt(2024, 2, 29, 0, 0)),
        (dt(2025, 2, 28, 0, 0), DAY, dt(2025, 3, 1, 0, 0)),
        (dt(2024, 2, 27, 0, 0), 3 * DAY, dt(2024, 3, 1, 0, 0)),
        (dt(2100, 2, 28, 0, 0), DAY, dt(2100, 3, 1, 0, 0)),
        (dt(2000, 2, 28, 0, 0), DAY, dt(2000, 2, 29, 0, 0)),
        (dt(2025, 1, 1, 0, 0), 400 * DAY, dt(2026, 2, 5, 0, 0)),
        (dt(2024, 2, 29, 12, 0), 366 * DAY, dt(2025, 3, 1, 12, 0)),
        (base, 1_000_000, dt(2027, 5, 25, 1, 20)),
    ] {
        assert_eq!(start.add_minutes(mins), expected, "{start:?} + {mins} min");
    }
    assert_eq!(base.add_minutes(5_000).add_minutes(7_777), base.add_minutes(12_777), "split == whole");
}

/// The UTC-to-local shift rolls the date both ways across midnight, months, years and leap days.
#[test]
fn with_offset_rolls_into_reference_dates_and_inverts() {
    let new_year = dt(2025, 1, 1, 0, 15);
    let mid = dt(2025, 6, 29, 10, 0);
    for (utc, offset, local) in [
        (dt(2025, 6, 29, 23, 30), 0, dt(2025, 6, 29, 23, 30)),
        (dt(2025, 6, 29, 23, 30), 15, dt(2025, 6, 29, 23, 45)),
        (dt(2025, 6, 29, 23, 30), 60, dt(2025, 6, 30, 0, 30)),
        (dt(2025, 6, 29, 0, 30), -45, dt(2025, 6, 28, 23, 45)),
        (dt(2025, 7, 1, 0, 0), -60, dt(2025, 6, 30, 23, 0)),
        (new_year, -30, dt(2024, 12, 31, 23, 45)),
        (dt(2024, 3, 1, 0, 30), -60, dt(2024, 2, 29, 23, 30)),
        (dt(2025, 3, 1, 0, 0), -1, dt(2025, 2, 28, 23, 59)),
        (dt(2025, 12, 31, 23, 59), 1, dt(2026, 1, 1, 0, 0)),
        (mid, 14 * 60, dt(2025, 6, 30, 0, 0)),
        (mid, -12 * 60, dt(2025, 6, 28, 22, 0)),
    ] {
        assert_eq!(utc.with_offset(offset), local, "{utc:?} at {offset:+} min");
    }
    for offset in [1i16, -1, 60, -60, 840, -720, 1439, -1439] {
        for utc in [mid, new_year] {
            assert_eq!(utc.with_offset(offset).with_offset(-offset), utc, "round trip at {offset} from {utc:?}");
        }
    }
}
