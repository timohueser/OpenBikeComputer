import Foundation

/// The phone's current time + local UTC offset, stamped onto the device's trusted
/// wall clock on every connect via `setClock` (spec §4.4 cmd 5, epic #638). The
/// device has no RTC; this is one of exactly two sources (GPS the other) that mark
/// its clock *trusted* for the boot — the safety gate the retention sweep needs
/// before it deletes anything.
public struct WallClockSample: Equatable, Sendable {
    /// The phone's current time in **unix seconds** (UTC). The device sets its
    /// wall-clock set-point from this; expiry arithmetic is pure UTC.
    public var utcSeconds: UInt32
    /// The phone's current **local UTC offset in minutes**, DST already applied
    /// (`+02:00` → `120`). The device holds no timezone tables — the offset only
    /// shifts the *displayed* hour; it is persisted and refreshed every connect.
    public var offsetMinutes: Int16

    public init(utcSeconds: UInt32, offsetMinutes: Int16) {
        self.utcSeconds = utcSeconds
        self.offsetMinutes = offsetMinutes
    }

    /// Sample the phone's clock **now** (the connect-time default): `Date` →
    /// unix seconds and `TimeZone.current.secondsFromGMT()` → minutes (DST
    /// already folded in). Times before 2020-01-01 or offsets past ±840 min are
    /// clamped into the wire's valid range so a bogus host clock still encodes to
    /// something the device accepts rather than rejecting the whole prologue.
    public init(date: Date = Date(), timeZone: TimeZone = .current) {
        let seconds = date.timeIntervalSince1970
        // Spec §4.4: `utc < 1577836800` (2020-01-01) is rejected device-side.
        let clampedSeconds = min(max(seconds, 1_577_836_800), Double(UInt32.max))
        utcSeconds = UInt32(clampedSeconds)
        let minutes = timeZone.secondsFromGMT(for: date) / 60
        offsetMinutes = Int16(min(max(minutes, -840), 840))
    }
}
