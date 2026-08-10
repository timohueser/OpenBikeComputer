#if DEBUG
import Foundation
import OBCDomain
import OBCWeather

/// The mock's weather surface (WX13): fixture job rings, a fixture service manifest state and a
/// fixture pending job, so the Weather screens can be driven — and photographed — without a radio,
/// a device or the live weather service.
///
/// Debug-only like the rest of `OBCMock`. The production composition root never builds these: it
/// hands the screens the real history ring, the real engine and the real service client.
public enum WeatherDemoState: String, Sendable, Equatable, CaseIterable {
    /// A device with weather, a healthy service and a recent successful delivery.
    case healthy
    /// A rider who has never had a successful sync — nothing in the ring at all.
    case empty
    /// The baker is behind: a published generation past its staleness deadline, plus a delivery that
    /// still landed (stale rain is refused on the device, not hidden here).
    case stale
    /// A job that keeps failing, with a mixture of ring outcomes and one owed job to retry.
    case failing
    /// Firmware without the weather feature bit.
    case unsupported

    /// Whether this state's device announces `FEATURE_WEATHER`.
    public var deviceSupportsWeather: Bool { self != .unsupported }
}

/// A fixture job-history ring the Weather diagnostics screen reads.
public struct MockWeatherFixtures: Sendable {
    public var history: [WeatherJobHistoryEntry]
    public var pending: WeatherJobPending?
    public var status: WeatherServiceStatus?

    public init(
        history: [WeatherJobHistoryEntry], pending: WeatherJobPending?,
        status: WeatherServiceStatus?
    ) {
        self.history = history
        self.pending = pending
        self.status = status
    }

    /// The fixture set for a demo state, relative to `now`.
    public static func forState(_ state: WeatherDemoState, now: Date = Date()) -> MockWeatherFixtures {
        func entry(
            _ outcome: WeatherJobHistoryEntry.Outcome, _ failure: WeatherJobFailure?,
            phase: WeatherJobPhase, minutesAgo: Double, requestID: UInt32 = 4_182,
            attempts: Int = 1, generation: String? = "20260810T1430Z",
            readMS: Int? = 1_700, uploadMS: Int? = 2_400, bytes: Int? = 43_800,
            noRainMap: NoRainMapReason? = nil
        ) -> WeatherJobHistoryEntry {
            WeatherJobHistoryEntry(
                startedAt: now.addingTimeInterval(-minutesAgo * 60 - 22),
                finishedAt: now.addingTimeInterval(-minutesAgo * 60),
                requestID: requestID, outcome: outcome, failureReason: failure,
                phaseReached: phase, attempts: attempts, bundleByteCount: bytes,
                readConnectedMilliseconds: readMS, uploadConnectedMilliseconds: uploadMS,
                precipitationGeneration: generation, noRainMapReason: noRainMap)
        }

        // Every source that may have painted a cell of the mosaic. There is no per-cell provenance
        // to narrow it to, so all four are credited on every screen that shows any of them.
        let credits = [
            WeatherAttribution(
                text: "Source: Deutscher Wetterdienst (DWD), CC BY 4.0",
                url: "https://creativecommons.org/licenses/by/4.0/", sourceID: "dwd-rv"),
            WeatherAttribution(
                text: "Source: NOAA/NWS MRMS (U.S. Government open data)",
                url: "https://www.nesdis.noaa.gov/about/open-data", sourceID: "us"),
            WeatherAttribution(
                text: "Source: NOAA GFS and NASA GPM IMERG (U.S. Government open data)",
                url: "https://www.nesdis.noaa.gov/about/open-data", sourceID: "gfs"),
        ]

        func status(age: TimeInterval, staleBy: TimeInterval? = nil) -> WeatherServiceStatus {
            WeatherServiceStatus(
                generation: "20260810T1430Z", generatedAt: now.addingTimeInterval(-age),
                observedAt: now, referenceTime: now.addingTimeInterval(-age - 160),
                staleAfter: now.addingTimeInterval(staleBy ?? 5_400),
                nextGenerationExpectedAt: now.addingTimeInterval(900), cellSizeMetres: 1_113,
                frameCount: 9, latestFrameValidAt: now.addingTimeInterval(7_200),
                attributions: credits, skippedFrames: 0)
        }

        switch state {
        case .healthy:
            return MockWeatherFixtures(
                history: [
                    entry(.committed, nil, phase: .uploading, minutesAgo: 94, requestID: 4_180),
                    entry(.superseded, .superseded, phase: .fetching, minutesAgo: 62,
                          requestID: 4_181, uploadMS: nil, bytes: nil),
                    entry(.committed, nil, phase: .uploading, minutesAgo: 61, requestID: 4_181),
                    entry(.committed, nil, phase: .uploading, minutesAgo: 12),
                ],
                pending: nil,
                status: status(age: 200))
        case .empty, .unsupported:
            // A device that never asks has never been answered: an `unsupported` run with delivery
            // rows in it would contradict its own banner.
            return MockWeatherFixtures(history: [], pending: nil, status: status(age: 200))
        case .stale:
            return MockWeatherFixtures(
                history: [
                    entry(.committed, nil, phase: .uploading, minutesAgo: 78),
                    entry(.committed, nil, phase: .uploading, minutesAgo: 26,
                          generation: "20260810T1400Z"),
                ],
                pending: nil,
                status: status(age: 4_000, staleBy: -1_500))
        case .failing:
            return MockWeatherFixtures(
                history: [
                    entry(.committed, nil, phase: .uploading, minutesAgo: 140, requestID: 4_179),
                    entry(.failed, .fetchFailed, phase: .fetching, minutesAgo: 46,
                          requestID: 4_181, attempts: 6, generation: nil, uploadMS: nil,
                          bytes: nil),
                    entry(.failed, .transferCorrupted, phase: .uploading, minutesAgo: 21,
                          requestID: 4_182, attempts: 3, uploadMS: 4_100),
                    // Its own calm outcome, not a failure: the job continued and the row is there
                    // to be readable, not alarming (#1198 review).
                    entry(.agedOut, .agedOut, phase: .bundleReady, minutesAgo: 8,
                          requestID: 4_183, attempts: 2, uploadMS: nil,
                          noRainMap: .expired(staleAfter: now.addingTimeInterval(-2_400))),
                ],
                pending: WeatherJobPending(
                    phase: .bundleReady, requestID: 4_184,
                    startedAt: now.addingTimeInterval(-190), updatedAt: now.addingTimeInterval(-40),
                    attempts: 2, deferrals: 1, retryNotBefore: now.addingTimeInterval(18),
                    bundleByteCount: 42_100),
                status: status(age: 200))
        }
    }
}

/// An in-memory history ring over fixture rows.
public final class MockWeatherHistoryStore: WeatherJobHistoryStore, @unchecked Sendable {
    private let lock = NSLock()
    private var stored: [WeatherJobHistoryEntry]

    public init(_ entries: [WeatherJobHistoryEntry]) { stored = entries }

    public func append(_ entry: WeatherJobHistoryEntry) { lock.withLock { stored.append(entry) } }
    public func entries() -> [WeatherJobHistoryEntry] { lock.withLock { stored } }
}

/// A job control that models the one thing the screen can ask of a job: finish it now. A retry
/// succeeds (the checkpoint clears), because the interesting failure paths are already in the ring.
public final class MockWeatherJobControl: WeatherJobControlling, @unchecked Sendable {
    private let lock = NSLock()
    private var job: WeatherJobPending?
    private let history: MockWeatherHistoryStore
    private let now: @Sendable () -> Date

    public init(
        pending: WeatherJobPending?, history: MockWeatherHistoryStore,
        now: @escaping @Sendable () -> Date = Date.init
    ) {
        self.job = pending
        self.history = history
        self.now = now
    }

    public func pendingJob() async -> WeatherJobPending? { lock.withLock { job } }

    public func retryNow() async {
        let finished: WeatherJobPending? = lock.withLock {
            defer { job = nil }
            return job
        }
        guard let finished else { return }
        history.append(WeatherJobHistoryEntry(
            startedAt: finished.startedAt, finishedAt: now(), requestID: finished.requestID,
            outcome: .committed, failureReason: nil, phaseReached: .uploading,
            attempts: finished.attempts, bundleByteCount: finished.bundleByteCount,
            readConnectedMilliseconds: 1_600, uploadConnectedMilliseconds: 2_300,
            precipitationGeneration: "20260810T1430Z"))
    }
}

/// A service-status provider over a fixture manifest state; `nil` models an outage.
public struct MockWeatherServiceStatus: WeatherServiceStatusProviding {
    private let status: WeatherServiceStatus?

    public init(status: WeatherServiceStatus?) { self.status = status }

    public func serviceStatus(now: Date) async throws -> WeatherServiceStatus {
        guard let status else { throw WeatherManifestError.malformed }
        return status
    }
}
#endif
