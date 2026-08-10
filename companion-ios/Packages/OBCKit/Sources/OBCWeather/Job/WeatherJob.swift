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
    /// The upload leg failed on the link: a drop, a timeout, the radio going away. The bytes are
    /// untouched and the retry re-sends them.
    case uploadFailed
    /// The bytes arrived corrupted (§11.5 `crcMismatch`): the wire damaged a *correct* bundle, so
    /// the retry re-sends the same bytes.
    ///
    /// Deliberately **not** folded into ``uploadFailed``. The engine treats the two alike — both
    /// keep the bundle, both re-upload — but they are different field stories, and the ring is
    /// where that difference has to survive: a run of `uploadFailed` says "this link keeps
    /// dropping", a run of `transferCorrupted` says "this link stays up and mangles bytes", which
    /// is a radio/PHY problem the drop label would hide (#1227 follow-up).
    case transferCorrupted
    /// The device refused the bytes as not-a-bundle (§11.5 `error`) — a producer bug to surface.
    case bundleRejected
    /// The device could not take the bundle *right now* — it answered `busy` / `storageFull` /
    /// `notFound`, or the phone's own transfer slot was still held by a foreground transfer when
    /// the budget ran out. Says nothing about the bytes: they are kept and the next trigger
    /// re-uploads them, and it does not spend one of the request's attempts.
    case deviceUnavailable
    /// The bundle could not be built (builder policy, oversize, malformed inputs).
    case buildFailed
    /// The job exceeded its attempt budget and was abandoned to the device's ladder.
    case attemptsExhausted
    /// A newer device request superseded this job before it could finish — or the device had
    /// already taken a bundle at or past this one's generation, so these bytes answered a question
    /// somebody else had answered.
    case superseded
    /// Time, not another request, ended it: a checkpoint that outlived ``WeatherJobEngine``'s
    /// `jobLifetime`, or a built bundle that sat past `bundleMaxAge` while the app slept.
    ///
    /// Split out of ``superseded`` and ``attemptsExhausted`` (#1227 follow-up), which both used to
    /// absorb it and both told the rider something false: nothing superseded a job the app simply
    /// slept through, and a job that aged out on its first attempt exhausted nothing.
    case agedOut
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
    /// Deferrals: waits the *device* (or a foreground transfer) asked for, which do not spend an
    /// attempt. Counted anyway so a permanently-full device cannot loop for the whole job
    /// lifetime — past the attempt budget a deferral degrades into an ordinary attempt.
    public var deferrals: Int = 0
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
        deferrals: Int = 0,
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
        self.deferrals = deferrals
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
///
/// **This file holds the rider's coordinate** — the one place in WX9 that does, and only because
/// a relaunched process must be able to finish a fetch it already paid a connection for. Three
/// rules follow from that, and they are the store's job, not the engine's:
///
/// - **Never in a backup.** The containing directory is marked excluded, so a coordinate cannot
///   ride an iCloud/iTunes backup off the phone and outlive the job by years.
/// - **Data protection is explicit, not inherited.** The class is
///   `completeUntilFirstUserAuthentication`, written down here rather than taken as the platform
///   default — and deliberately *not* `complete`: the whole point of the standing watch is a
///   background wake with the phone **locked**, and `complete` would make the checkpoint
///   unreadable exactly then, turning every locked-phone request into a silent failure.
/// - **It expires on its own.** `load()` refuses (and deletes) a checkpoint past ``lifetime``, so
///   the coordinate is gone at the horizon even if the engine never runs again. That horizon is
///   the engine's `jobLifetime`: past it the job was going to be dropped anyway, and the trade is
///   one diagnostics row for a coordinate that does not linger.
public final class FileWeatherJobStore: WeatherJobStore, @unchecked Sendable {
    private let fileURL: URL
    private let lifetime: TimeInterval
    private let now: @Sendable () -> Date
    private let queue = DispatchQueue(label: "com.openbikecomputer.weather.jobstore")

    public init(
        fileURL: URL,
        lifetime: TimeInterval = 2 * 3_600,
        now: @escaping @Sendable () -> Date = Date.init
    ) {
        self.fileURL = fileURL
        self.lifetime = lifetime
        self.now = now
    }

    /// The standard location: `Application Support/OBCWeather/job.json`.
    public static func standard() -> FileWeatherJobStore {
        FileWeatherJobStore(fileURL: standardDirectory().appendingPathComponent("job.json"))
    }

    /// `Application Support/OBCWeather/`, created and marked backup-excluded. Shared with the
    /// history ring, which is coordinate-free but has no business in a backup either.
    static func standardDirectory() -> URL {
        var base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("OBCWeather", isDirectory: true)
        try? FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? base.setResourceValues(values)
        return base
    }

    public func load() -> WeatherJobRecord? {
        queue.sync {
            guard let data = try? Data(contentsOf: fileURL) else { return nil }
            // An unreadable checkpoint (schema change, torn write that somehow landed) is a fresh
            // start, not a crash loop: the device's ladder re-raises the request.
            guard let record = try? decoder().decode(WeatherJobRecord.self, from: data) else {
                return nil
            }
            guard now().timeIntervalSince(record.startedAt) <= lifetime else {
                try? FileManager.default.removeItem(at: fileURL)
                return nil
            }
            return record
        }
    }

    public func save(_ record: WeatherJobRecord) {
        queue.sync {
            guard let data = try? encoder().encode(record) else { return }
            var options: Data.WritingOptions = [.atomic]
            #if os(iOS)
            options.insert(.completeFileProtectionUntilFirstUserAuthentication)
            #endif
            try? data.write(to: fileURL, options: options)
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
