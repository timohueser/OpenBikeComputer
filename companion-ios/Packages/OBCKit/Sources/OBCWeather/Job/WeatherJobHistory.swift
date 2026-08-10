import Foundation

/// One finished (or abandoned) job in the WX13 diagnostics ring.
///
/// **Coordinate-free by construction**: this type has no field a coordinate could ride in. What a
/// job knew about the rider's position lives only in the job checkpoint, which is deleted when the
/// job ends; what survives here is timing, outcome and provider evidence — enough for the epic's
/// connected-time targets and a truthful "last run" row, nothing a location history could be
/// reassembled from.
public struct WeatherJobHistoryEntry: Codable, Equatable, Sendable {
    public enum Outcome: String, Codable, Equatable, Sendable {
        /// The device answered `committed` — including the §11.6 duplicate/stale rows, which are
        /// success by contract.
        case committed
        case failed
        /// A newer device request replaced this job before it finished.
        case superseded
    }

    public var startedAt: Date
    public var finishedAt: Date
    public var requestID: UInt32
    public var outcome: Outcome
    public var failureReason: WeatherJobFailure?
    /// The furthest phase the job reached.
    public var phaseReached: WeatherJobPhase
    public var attempts: Int
    /// Size of the uploaded (or built) bundle.
    public var bundleByteCount: Int?
    /// Radio-held time of the two legs, for the ≤ 5 s median / ≤ 10 s p95 target.
    public var readConnectedMilliseconds: Int?
    public var uploadConnectedMilliseconds: Int?
    /// The manifest product that answered the corridor, when one did — a product id, never a place.
    public var precipitationProductID: String?
    public var noRainMapReason: String?

    public init(
        startedAt: Date, finishedAt: Date, requestID: UInt32, outcome: Outcome,
        failureReason: WeatherJobFailure? = nil, phaseReached: WeatherJobPhase, attempts: Int,
        bundleByteCount: Int? = nil, readConnectedMilliseconds: Int? = nil,
        uploadConnectedMilliseconds: Int? = nil, precipitationProductID: String? = nil,
        noRainMapReason: String? = nil
    ) {
        self.startedAt = startedAt
        self.finishedAt = finishedAt
        self.requestID = requestID
        self.outcome = outcome
        self.failureReason = failureReason
        self.phaseReached = phaseReached
        self.attempts = attempts
        self.bundleByteCount = bundleByteCount
        self.readConnectedMilliseconds = readConnectedMilliseconds
        self.uploadConnectedMilliseconds = uploadConnectedMilliseconds
        self.precipitationProductID = precipitationProductID
        self.noRainMapReason = noRainMapReason
    }
}

/// The small persisted ring WX13 reads. Append-only from the engine's side; the store owns the cap.
public protocol WeatherJobHistoryStore: Sendable {
    func append(_ entry: WeatherJobHistoryEntry)
    func entries() -> [WeatherJobHistoryEntry]
}

/// File-backed ring: newest last, capped, written atomically.
public final class FileWeatherJobHistoryStore: WeatherJobHistoryStore, @unchecked Sendable {
    public static let defaultCapacity = 20

    private let fileURL: URL
    private let capacity: Int
    private let queue = DispatchQueue(label: "com.openbikecomputer.weather.jobhistory")

    public init(fileURL: URL, capacity: Int = FileWeatherJobHistoryStore.defaultCapacity) {
        self.fileURL = fileURL
        self.capacity = max(1, capacity)
    }

    /// The standard location beside the job checkpoint: `Application Support/OBCWeather/history.json`
    /// — same directory, so it inherits the checkpoint's backup exclusion.
    public static func standard() -> FileWeatherJobHistoryStore {
        FileWeatherJobHistoryStore(
            fileURL: FileWeatherJobStore.standardDirectory()
                .appendingPathComponent("history.json"))
    }

    public func append(_ entry: WeatherJobHistoryEntry) {
        queue.sync {
            var current = read()
            current.append(entry)
            if current.count > capacity { current.removeFirst(current.count - capacity) }
            guard let data = try? encoder().encode(current) else { return }
            try? data.write(to: fileURL, options: .atomic)
        }
    }

    public func entries() -> [WeatherJobHistoryEntry] {
        queue.sync { read() }
    }

    private func read() -> [WeatherJobHistoryEntry] {
        guard let data = try? Data(contentsOf: fileURL) else { return [] }
        return (try? decoder().decode([WeatherJobHistoryEntry].self, from: data)) ?? []
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
