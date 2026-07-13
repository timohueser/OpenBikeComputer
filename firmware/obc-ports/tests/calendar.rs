use obc_ports::DateTime;

#[test]
fn calendar_arithmetic_has_no_app_editor_year_policy() {
    let next = DateTime { year: 2099, month: 12, day: 31, hour: 23, minute: 59 }.add_minutes(1);
    assert_eq!(next, DateTime { year: 2100, month: 1, day: 1, hour: 0, minute: 0 });

    let previous = DateTime { year: 2020, month: 1, day: 1, hour: 0, minute: 15 }.with_offset(-30);
    assert_eq!(previous, DateTime { year: 2019, month: 12, day: 31, hour: 23, minute: 45 });
}
