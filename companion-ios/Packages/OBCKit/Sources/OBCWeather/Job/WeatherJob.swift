import Foundation

/// Where a job stands, persisted after every externally visible edge so a suspension, process
/// death or CoreBluetooth relaunch resumes exactly where the last edge left off — never earlier
/// (re-doing paid work), never later (claiming work that didn't happen).
public enum WeatherJobPhase: String, Codable, Equatable, Sendable {
    /// A Weather Request advertisement was seen; the context read has not completed yet. There is
    /// nothing worth persisting about this phase beyond its existence — a relaunch here starts the
    /// read over, because the read *is* the checkpointable event.
    case readingContext
    /// The context is persisted; the fetch/build (manifest → corridor tiles → MET hourly → OBCW)
    /// is owed. BLE is disconnected for the whole of this phase.
    case fetching
    /// The finished bundle bytes are persisted; only the upload leg is owed. A relaunch here goes
    /// straight to reconnect + upload without touching the network.
    case bundleReady
    /// The upload leg is in flight. The transfer is atomic from the engine's view (the device
    /// commits or it doesn't), and an interrupted upload restarts from `bundleReady`'s bytes —
    /// uploads restart, not resume, and a duplicate answers `committed` (§11.6).
    case uploading
}

/// Why a job (or one attempt of it) failed — the WX13-visible vocabulary. String-backed so the
/// history ring persists it without inventing a second encoding.
public enum WeatherJobFailure: String, Codable, Equatable, Sendable {
    /// The device raised a request with no GPS fix and this build has no substitute location.
    /// Surfaced honestly rather than fetched-for-nowhere; the device will re-raise once it has a
    /// fix (or the rider opens Weather outdoors).
    case noPosition
    /// The hourly provider (or the network under it) failed — there is no bundle without it.
    case fetchFailed
    /// The context read leg failed (scan/connect/read/decode).
    case contextReadFailed
    /// The upload leg failed on the link (drop, timeout, radio off).
    case uploadFailed
    /// The device refused the bytes as not-a-bundle (§11.5 `error`) — a producer bug to surface.
    case bundleRejected
    /// The bundle could not be built (builder policy, oversize, malformed inputs).
    case buildFailed
    /// The job exceeded its attempt budget and was abandoned to the device's ladder.
    case attemptsExhausted
    /// A newer device request superseded this job before it could finish.
    case superseded
}

/// The persisted job — everything a relaunched process needs to finish the exchange.
public struct WeatherJobRecord: Codable, Equatable, Sendable {
    public var id: UUID
    public var phase: WeatherJobPhase
    /// The checkpointed context read. `nil` only in `.readingContext`.
    public var snapshot: WeatherDeviceRequestSnapshot?
    /// The finished OBCW bytes (≤ 64 KiB by producer policy). `nil` until `.bundleReady`.
    public var bundleBytes: Data?
    /// The generation stamped into those bytes — kept beside them so a later context read can
    /// compare against what the device then holds without re-decoding the container.
    public var bundleGeneration: UInt32?
    /// The bundle's geographic window in microdegrees (south, west, north, east) — the
    /// "materially changed request" check: a rider who has left this window needs a rebuild.
    public var bundleWindow: [Int64]?
    /// When the bundle was built — a bundle past ``WeatherJobEngine/Configuration/bundleMaxAge``
    /// is rebuilt rather than uploaded as yesterday's weather.
    public var bundleBuiltAt: Date?
    /// Diagnostics snapshots for the eventual history entry (no coordinates).
    public var precipitationProductID: String?
    public var noRainMapReason: String?
    /// Completed attempts that ended in a retryable failure.
    public var attempts: Int
    public var startedAt: Date
    public var updatedAt: Date
    /// Retry cooldown: `resume()` will not act before this. A fresh device discovery overrides it —
    /// the device asking again *is* the ladder, and the phone must not out-stubborn it.
    public var notBefore: Date?

    public init(
        id: UUID = UUID(),
        phase: WeatherJobPhase = .readingContext,
        snapshot: WeatherDeviceRequestSnapshot? = nil,
        bundleBytes: Data? = nil,
        bundleGeneration: UInt32? = nil,
        bundleWindow: [Int64]? = nil,
        bundleBuiltAt: Date? = nil,
        precipitationProductID: String? = nil,
        noRainMapReason: String? = nil,
        attempts: Int = 0,
        startedAt: Date,
        updatedAt: Date,
        notBefore: Date? = nil
    ) {
        self.id = id
        self.phase = phase
        self.snapshot = snapshot
        self.bundleBytes = bundleBytes
        self.bundleGeneration = bundleGeneration
        self.bundleWindow = bundleWindow
        self.bundleBuiltAt = bundleBuiltAt
        self.precipitationProductID = precipitationProductID
        self.noRainMapReason = noRainMapReason
        self.attempts = attempts
        self.startedAt = startedAt
        self.updatedAt = updatedAt
        self.notBefore = notBefore
    }

    /// Whether the persisted bundle still answers `snapshot` — same-window containment. The window
    /// is stored as `[s, w, n, e]`; a rider outside it has materially moved on.
    public func bundleCovers(latitudeMicrodegrees: Int64, longitudeMicrodegrees: Int64) -> Bool {
        guard let bundleWindow, bundleWindow.count == 4 else { return false }
        return latitudeMicrodegrees >= bundleWindow[0] && latitudeMicrodegrees <= bundleWindow[2]
            && longitudeMicrodegrees >= bundleWindow[1] && longitudeMicrodegrees <= bundleWindow[3]
    }
}

/// The single-job checkpoint store. One job at a time by design: the device holds one pending
/// request and one bundle slot pair, so a second concurrent job could only race the first for the
/// same singleton object.
public protocol WeatherJobStore: Sendable {
    func load() -> WeatherJobRecord?
    func save(_ record: WeatherJobRecord)
    func clear()
}

/// The file-backed checkpoint: one JSON document in Application Support, written atomically so a
/// crash mid-save leaves the previous checkpoint intact rather than a torn one.
public final class FileWeatherJobStore: WeatherJobStore, @unchecked Sendable {
    private let fileURL: URL
    private let queue = DispatchQueue(label: "com.openbikecomputer.weather.jobstore")

    public init(fileURL: URL) {
        self.fileURL = fileURL
    }

    /// The standard location: `Application Support/OBCWeather/job.json`.
    public static func standard() -> FileWeatherJobStore {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("OBCWeather", isDirectory: true)
        try? FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
        return FileWeatherJobStore(fileURL: base.appendingPathComponent("job.json"))
    }

    public func load() -> WeatherJobRecord? {
        queue.sync {
            guard let data = try? Data(contentsOf: fileURL) else { return nil }
            // An unreadable checkpoint (schema change, torn write that somehow landed) is a fresh
            // start, not a crash loop: the device's ladder re-raises the request.
            return try? decoder().decode(WeatherJobRecord.self, from: data)
        }
    }

    public func save(_ record: WeatherJobRecord) {
        queue.sync {
            guard let data = try? encoder().encode(record) else { return }
            try? data.write(to: fileURL, options: .atomic)
        }
    }

    public func clear() {
        queue.sync { try? FileManager.default.removeItem(at: fileURL) }
    }

    private func encoder() -> JSONEncoder {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .secondsSince1970
        return encoder
    }

    private func decoder() -> JSONDecoder {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .secondsSince1970
        return decoder
    }
}
