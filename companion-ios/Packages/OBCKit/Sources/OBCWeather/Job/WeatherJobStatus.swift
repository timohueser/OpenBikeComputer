import Foundation

/// The job that is *currently owed*, as a screen may see it (WX13).
///
/// A projection of ``WeatherJobRecord``, not the record itself, and the reason is the coordinate.
/// The record carries the rider's position — it must, so a relaunched process can finish a fetch it
/// already paid a connection for — and the moment a view model could hold one, a screen, a log line
/// or a support export could hold one too. This type has no field a coordinate could ride in, the
/// same construction rule ``WeatherJobHistoryEntry`` is built on, so the coordinate stays where it
/// was designed to live and die: the checkpoint file.
public struct WeatherJobPending: Equatable, Sendable {
    public var phase: WeatherJobPhase
    public var requestID: UInt32
    public var startedAt: Date
    public var updatedAt: Date
    /// Completed attempts that ended in a retryable failure.
    public var attempts: Int
    /// Waits the device (or a foreground transfer) asked for; they do not spend an attempt.
    public var deferrals: Int
    /// The cooldown a `.resume` honours. A rider's *Retry now* waives it; the device re-advertising
    /// overrides it.
    public var retryNotBefore: Date?
    /// Bytes of the built bundle, once there is one.
    public var bundleByteCount: Int?

    public init(
        phase: WeatherJobPhase, requestID: UInt32, startedAt: Date, updatedAt: Date,
        attempts: Int, deferrals: Int, retryNotBefore: Date? = nil, bundleByteCount: Int? = nil
    ) {
        self.phase = phase
        self.requestID = requestID
        self.startedAt = startedAt
        self.updatedAt = updatedAt
        self.attempts = attempts
        self.deferrals = deferrals
        self.retryNotBefore = retryNotBefore
        self.bundleByteCount = bundleByteCount
    }

    init(record: WeatherJobRecord) {
        self.init(
            phase: record.phase,
            requestID: record.snapshot?.requestID ?? 0,
            startedAt: record.startedAt,
            updatedAt: record.updatedAt,
            attempts: record.attempts,
            deferrals: record.deferrals,
            retryNotBefore: record.notBefore,
            bundleByteCount: record.bundleBytes?.count)
    }
}

/// What a diagnostics/settings screen may do to the running job: look, and ask it to try again now.
///
/// Deliberately two methods. Everything else the job does is the device's to start — the phone
/// cannot raise a weather request on the device's behalf, so there is no "fetch weather now" here
/// to offer, and offering one would be a button that lies.
public protocol WeatherJobControlling: Sendable {
    /// The job the checkpoint currently owes, coordinate-free; `nil` when nothing is owed.
    func pendingJob() async -> WeatherJobPending?
    /// Finish the owed job now, waiving the local retry cooldown. A no-op when nothing is owed.
    func retryNow() async
}

extension WeatherJobEngine: WeatherJobControlling {
    public func pendingJob() -> WeatherJobPending? {
        pendingRecord().map(WeatherJobPending.init(record:))
    }

    public func retryNow() async {
        await kick(.userRetry)
    }
}
