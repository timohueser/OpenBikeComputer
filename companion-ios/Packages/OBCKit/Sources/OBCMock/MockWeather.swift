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
    /// The baker is behind: a covering product past its staleness deadline, plus a delivery that
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
            attempts: Int = 1, product: String? = "dwd-rv",
            readMS: Int? = 1_700, uploadMS: Int? = 2_400, bytes: Int? = 43_800
        ) -> WeatherJobHistoryEntry {
            WeatherJobHistoryEntry(
                startedAt: now.addingTimeInterval(-minutesAgo * 60 - 22),
                finishedAt: now.addingTimeInterval(-minutesAgo * 60),
                requestID: requestID, outcome: outcome, failureReason: failure,
                phaseReached: phase, attempts: attempts, bundleByteCount: bytes,
                readConnectedMilliseconds: readMS, uploadConnectedMilliseconds: uploadMS,
                precipitationProductID: product)
        }

        let radar = WeatherServiceProductStatus(
            id: "dwd-rv", tier: .radar, nominalCellMetres: 1_000,
            referenceTime: now.addingTimeInterval(-360), generatedAt: now.addingTimeInterval(-200),
            stalenessDeadline: now.addingTimeInterval(900),
            attribution: WeatherAttribution(
                text: "Source: Deutscher Wetterdienst (DWD), CC BY 4.0",
                url: "https://creativecommons.org/licenses/by/4.0/"),
            frameCount: 9, latestFrameValidAt: now.addingTimeInterval(7_200))
        let model = WeatherServiceProductStatus(
            id: "icon-eu", tier: .model, nominalCellMetres: 6_500,
            referenceTime: now.addingTimeInterval(-2_700), generatedAt: now.addingTimeInterval(-600),
            stalenessDeadline: now.addingTimeInterval(5_400),
            attribution: WeatherAttribution(
                text: "Source: Deutscher Wetterdienst (DWD), CC BY 4.0",
                url: "https://creativecommons.org/licenses/by/4.0/"),
            frameCount: 3, latestFrameValidAt: now.addingTimeInterval(7_200))
        let floor = WeatherServiceProductStatus(
            id: "gfs-imerg", tier: .floor, nominalCellMetres: 27_000,
            referenceTime: now.addingTimeInterval(-9_000), generatedAt: now.addingTimeInterval(-800),
            stalenessDeadline: now.addingTimeInterval(10_800),
            attribution: WeatherAttribution(
                text: "Source: NOAA GFS and NASA GPM IMERG (U.S. Government open data)",
                url: "https://www.nesdis.noaa.gov/about/open-data"),
            frameCount: 4, latestFrameValidAt: now.addingTimeInterval(7_200))

        func status(_ products: [WeatherServiceProductStatus], age: TimeInterval) -> WeatherServiceStatus {
            WeatherServiceStatus(
                generatedAt: now.addingTimeInterval(-age), observedAt: now,
                products: products, skippedProducts: 0)
        }

        switch state {
        case .healthy, .unsupported:
            return MockWeatherFixtures(
                history: [
                    entry(.committed, nil, phase: .uploading, minutesAgo: 94, requestID: 4_180),
                    entry(.superseded, .superseded, phase: .fetching, minutesAgo: 62,
                          requestID: 4_181, uploadMS: nil, bytes: nil),
                    entry(.committed, nil, phase: .uploading, minutesAgo: 61, requestID: 4_181),
                    entry(.committed, nil, phase: .uploading, minutesAgo: 12),
                ],
                pending: nil,
                status: status([radar, model, floor], age: 200))
        case .empty:
            return MockWeatherFixtures(
                history: [], pending: nil, status: status([radar, model, floor], age: 200))
        case .stale:
            let expired = WeatherServiceProductStatus(
                id: radar.id, tier: radar.tier, nominalCellMetres: radar.nominalCellMetres,
                referenceTime: now.addingTimeInterval(-4_200),
                generatedAt: now.addingTimeInterval(-4_000),
                stalenessDeadline: now.addingTimeInterval(-1_500),
                attribution: radar.attribution, frameCount: 9,
                latestFrameValidAt: now.addingTimeInterval(3_000))
            return MockWeatherFixtures(
                history: [
                    entry(.committed, nil, phase: .uploading, minutesAgo: 78, product: "dwd-rv"),
                    entry(.committed, nil, phase: .uploading, minutesAgo: 26, product: "icon-eu"),
                ],
                pending: nil,
                status: status([expired, model, floor], age: 4_000))
        case .failing:
            return MockWeatherFixtures(
                history: [
                    entry(.committed, nil, phase: .uploading, minutesAgo: 140, requestID: 4_179),
                    entry(.failed, .fetchFailed, phase: .fetching, minutesAgo: 46,
                          requestID: 4_181, attempts: 6, product: nil, uploadMS: nil, bytes: nil),
                    entry(.failed, .transferCorrupted, phase: .uploading, minutesAgo: 21,
                          requestID: 4_182, attempts: 3, uploadMS: 4_100),
                    entry(.failed, .agedOut, phase: .bundleReady, minutesAgo: 8,
                          requestID: 4_183, attempts: 2, uploadMS: nil),
                ],
                pending: WeatherJobPending(
                    phase: .bundleReady, requestID: 4_184,
                    startedAt: now.addingTimeInterval(-190), updatedAt: now.addingTimeInterval(-40),
                    attempts: 2, deferrals: 1, retryNotBefore: now.addingTimeInterval(18),
                    bundleByteCount: 42_100),
                status: status([radar, model, floor], age: 200))
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
            precipitationProductID: "dwd-rv"))
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
