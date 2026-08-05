use obc_ports::DateTime;

#[test]
fn calendar_arithmetic_has_no_app_editor_year_policy() {
    let next = DateTime { year: 2099, month: 12, day: 31, hour: 23, minute: 59 }.add_minutes(1);
    assert_eq!(next, DateTime { year: 2100, month: 1, day: 1, hour: 0, minute: 0 });

    let previous = DateTime { year: 2020, month: 1, day: 1, hour: 0, minute: 15 }.with_offset(-30);
    assert_eq!(previous, DateTime { year: 2019, month: 12, day: 31, hour: 23, minute: 45 });
}

/// The calendar arithmetic runs over Unix seconds, so it saturates at that window's edges rather
/// than wrapping into a nonsense year. Both edges are far outside the app's own 2020–2099 clamp.
#[test]
fn calendar_arithmetic_saturates_at_the_epoch_window() {
    // The floor: the epoch itself, shifted back, stays put.
    let epoch = DateTime { year: 1970, month: 1, day: 1, hour: 0, minute: 0 };
    assert_eq!(epoch.with_offset(-60), epoch, "no date exists below the epoch to roll back to");
    assert_eq!(epoch.add_minutes(60), DateTime { hour: 1, ..epoch }, "forward from the floor is normal");

    // The ceiling: `u32::MAX` epoch seconds is 2106-02-07 06:28:15, truncated to the minute.
    let ceiling = DateTime { year: 2106, month: 2, day: 7, hour: 6, minute: 28 };
    let far = DateTime { year: 2099, month: 1, day: 1, hour: 0, minute: 0 }.add_minutes(u32::MAX);
    assert_eq!(far, ceiling, "a huge advance pins at the last representable minute");
    assert_eq!(ceiling.add_minutes(1), ceiling, "and stays there");
}
