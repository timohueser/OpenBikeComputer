import Foundation

/// The phone's current time and local UTC offset, stamped onto the device's trusted wall clock
/// on every connect. The device has no RTC: this and GPS are the only two sources that mark its
/// clock trusted for a boot.
public struct WallClockSample: Equatable, Sendable {
    /// The phone's current time in unix seconds (UTC).
    public var utcSeconds: UInt32
    /// The phone's current local UTC offset in minutes, DST already applied (`+02:00` is
    /// `120`). The device holds no timezone tables, so the offset only shifts the displayed hour.
    public var offsetMinutes: Int16

    public init(utcSeconds: UInt32, offsetMinutes: Int16) {
        self.utcSeconds = utcSeconds
        self.offsetMinutes = offsetMinutes
    }

    /// Sample the phone's clock now. A time before 2020-01-01, or an offset past 840 minutes,
    /// is clamped into the wire's valid range, so a bogus host clock still encodes to something
    /// the device accepts instead of failing the whole prologue.
    public init(date: Date = Date(), timeZone: TimeZone = .current) {
        let seconds = date.timeIntervalSince1970
        // The device rejects `utc < 1577836800` (2020-01-01).
        let clampedSeconds = min(max(seconds, 1_577_836_800), Double(UInt32.max))
        utcSeconds = UInt32(clampedSeconds)
        let minutes = timeZone.secondsFromGMT(for: date) / 60
        offsetMinutes = Int16(min(max(minutes, -840), 840))
    }
}
