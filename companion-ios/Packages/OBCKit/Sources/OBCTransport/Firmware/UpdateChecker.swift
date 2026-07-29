import CryptoKit
import Foundation

/// The update check (#773 U4): fetch the published manifest, cache the answer, and — when the
/// rider asks for it — download the container and prove it byte-for-byte before anything is
/// staged for the device.
///
/// **Privacy posture, which is a requirement and not a default.** Every request here is an
/// anonymous `GET`: no accounts, no cookies, no custom headers, no query string, and nothing about
/// the device — not its serial, not its running version — ever leaves the phone. The server learns
/// only that *someone* asked for a public file. That is #773's rule; a change that adds a header
/// or a parameter is a change to the product, not an implementation detail.
///
/// **What it is not.** Deciding *when* to check without the screen being open (a launch sheet, a
/// `BGAppRefreshTask`, a notification) is #773's U5 and deliberately absent: this type is a pure
/// service with no timers and no lifecycle, so U5 can drive it from anywhere.

/// The HTTP seam — one anonymous GET. Injected so a test never touches the network.
public protocol ManifestFetching: Sendable {
    /// Perform the GET. Throws only for transport-level failures (no route, TLS, cancellation);
    /// an HTTP error status comes back as a status code for the caller to judge.
    func get(_ url: URL) async throws -> (status: Int, body: Data)
}

/// The real fetcher. An **ephemeral** session so nothing (cookies, credentials, cache) is
/// persisted on the rider's phone by a check, and cookie handling is off outright.
public struct URLSessionManifestFetcher: ManifestFetching {
    private let session: URLSession

    public init(session: URLSession? = nil) {
        if let session {
            self.session = session
        } else {
            let config = URLSessionConfiguration.ephemeral
            config.httpCookieAcceptPolicy = .never
            config.httpShouldSetCookies = false
            config.urlCache = nil
            self.session = URLSession(configuration: config)
        }
    }

    public func get(_ url: URL) async throws -> (status: Int, body: Data) {
        var request = URLRequest(url: url)
        request.httpMethod = "GET"
        request.httpShouldHandleCookies = false
        request.cachePolicy = .reloadIgnoringLocalCacheData
        let (data, response) = try await session.data(for: request)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 200
        return (status, data)
    }
}

/// The last answer, cached so the screen has something to show the instant it opens.
/// `release == nil` records a real "nothing published yet" (a 404) — the check ran, it just found
/// nothing, and repeating it on every appear would be pointless.
public struct UpdateCheckRecord: Equatable, Sendable, Codable {
    public let release: FirmwareRelease?
    public let checkedAt: Date

    public init(release: FirmwareRelease?, checkedAt: Date) {
        self.release = release
        self.checkedAt = checkedAt
    }
}

/// Persistence seam for the check — the cached answer plus the pre-release opt-in. Beside
/// ``BondStore`` and ``RetentionDefaultsStore``: phone-local preferences, never on the wire.
public protocol UpdateCheckStore: Sendable {
    func loadCheck() -> UpdateCheckRecord?
    func saveCheck(_ record: UpdateCheckRecord)
    /// The dev opt-in: also consider the pre-release channel.
    func loadIncludePrereleases() -> Bool
    func saveIncludePrereleases(_ include: Bool)
}

/// The real store: two keys in `UserDefaults`, mirroring ``UserDefaultsRetentionDefaultsStore``.
/// `@unchecked`: `UserDefaults` is documented thread-safe but the SDK doesn't annotate it
/// `Sendable`.
public struct UserDefaultsUpdateCheckStore: UpdateCheckStore, @unchecked Sendable {
    private static let checkKey = "obc.firmwareUpdateCheck"
    private static let prereleaseKey = "obc.firmwareIncludePrereleases"
    private let defaults: UserDefaults

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    public func loadCheck() -> UpdateCheckRecord? {
        guard let data = defaults.data(forKey: Self.checkKey) else { return nil }
        // A record written by an older build that can no longer be read is simply no cache — the
        // next check refills it. Never a crash, never a migration.
        return try? JSONDecoder().decode(UpdateCheckRecord.self, from: data)
    }

    public func saveCheck(_ record: UpdateCheckRecord) {
        guard let data = try? JSONEncoder().encode(record) else { return }
        defaults.set(data, forKey: Self.checkKey)
    }

    public func loadIncludePrereleases() -> Bool { defaults.bool(forKey: Self.prereleaseKey) }

    public func saveIncludePrereleases(_ include: Bool) {
        defaults.set(include, forKey: Self.prereleaseKey)
    }
}

/// An in-memory store — the default for previews/tests, so no run leaks its cached answer into
/// the next (the same reason ``InMemoryRetentionDefaultsStore`` exists).
public final class InMemoryUpdateCheckStore: UpdateCheckStore, @unchecked Sendable {
    private let lock = NSLock()
    private var record: UpdateCheckRecord?
    private var includePrereleases: Bool

    public init(record: UpdateCheckRecord? = nil, includePrereleases: Bool = false) {
        self.record = record
        self.includePrereleases = includePrereleases
    }

    public func loadCheck() -> UpdateCheckRecord? { lock.withLock { record } }
    public func saveCheck(_ record: UpdateCheckRecord) { lock.withLock { self.record = record } }
    public func loadIncludePrereleases() -> Bool { lock.withLock { includePrereleases } }
    public func saveIncludePrereleases(_ include: Bool) { lock.withLock { includePrereleases = include } }
}

/// Why a downloaded container was thrown away. Nothing that fails here is ever handed to the
/// device: the manifest's own numbers are the contract, and a download that doesn't match them is
/// a corrupt or wrong file, full stop.
public enum FirmwareDownloadError: Error, Equatable, Sendable {
    case httpStatus(Int)
    case sizeMismatch(expected: Int, got: Int)
    case digestMismatch
}

/// The check itself. A value type with no mutable state of its own — everything durable lives in
/// the injected ``UpdateCheckStore`` — so it is trivially `Sendable` and safe to hold from the
/// `@MainActor` view model.
public struct UpdateChecker: Sendable {
    /// The stable channel. A constant, overridable so a test never touches the network.
    public static let manifestURL = URL(string: "https://updates.openbikecomputer.com/fw/manifest.json")!
    /// The pre-release channel, consulted only behind the dev opt-in.
    public static let prereleaseManifestURL =
        URL(string: "https://updates.openbikecomputer.com/fw/prerelease/manifest.json")!
    /// How long a cached answer counts as fresh. Six hours: the screen answers instantly from the
    /// cache and a check that just ran isn't repeated on every appear, while a rider who opens the
    /// screen the next day gets a real one.
    public static let freshness: TimeInterval = 6 * 60 * 60

    private let manifestURL: URL
    private let prereleaseURL: URL
    private let fetcher: any ManifestFetching
    private let store: any UpdateCheckStore

    public init(
        manifestURL: URL = UpdateChecker.manifestURL,
        prereleaseURL: URL = UpdateChecker.prereleaseManifestURL,
        fetcher: any ManifestFetching = URLSessionManifestFetcher(),
        store: any UpdateCheckStore = UserDefaultsUpdateCheckStore()
    ) {
        self.manifestURL = manifestURL
        self.prereleaseURL = prereleaseURL
        self.fetcher = fetcher
        self.store = store
    }

    // MARK: Cache + the dev opt-in

    /// The last answer, if there is one.
    public func cachedCheck() -> UpdateCheckRecord? { store.loadCheck() }

    /// A cached answer young enough that opening the screen needn't re-ask.
    public func isFresh(_ record: UpdateCheckRecord, now: Date = Date()) -> Bool {
        let age = now.timeIntervalSince(record.checkedAt)
        // A clock that moved backwards (time zone, manual set) reads as stale, not as fresh
        // forever.
        return age >= 0 && age < Self.freshness
    }

    /// Also consider the pre-release channel. A dev switch, off by default.
    public var includePrereleases: Bool { store.loadIncludePrereleases() }

    public func setIncludePrereleases(_ include: Bool) { store.saveIncludePrereleases(include) }

    // MARK: The check

    /// Fetch the manifest(s) and cache the answer.
    ///
    /// A **404 means "nothing published"** — the ordinary state until #773's U3 ships — and is
    /// recorded as a cached `nil` release, not thrown. A malformed manifest **is** thrown, because
    /// that one means something is wrong at the publishing end and hiding it would hide it forever.
    ///
    /// With the pre-release opt-in on, both channels are fetched and the **newer of the two** is
    /// the answer; a 404 on either is simply that channel having nothing.
    @discardableResult
    public func check(now: Date = Date()) async throws -> UpdateCheckRecord {
        var newest = try await fetch(manifestURL)
        if includePrereleases, let pre = try await fetch(prereleaseURL) {
            newest = newer(newest, pre)
        }
        let record = UpdateCheckRecord(release: newest, checkedAt: now)
        store.saveCheck(record)
        return record
    }

    /// One channel: `nil` for a 404 (nothing published there), a parsed release for a 2xx, and a
    /// throw for anything else.
    private func fetch(_ url: URL) async throws -> FirmwareRelease? {
        let (status, body) = try await fetcher.get(url)
        if status == 404 { return nil }
        guard (200..<300).contains(status) else { throw FirmwareManifestError.httpStatus(status) }
        return try parseFirmwareManifest(body)
    }

    /// Pick the newer of two candidate releases. Both parsed clean (the manifest parser rejects a
    /// version it can't read), so the comparison can only be `nil` in a case that cannot arise —
    /// and if it somehow did, the stable channel wins.
    private func newer(_ stable: FirmwareRelease?, _ pre: FirmwareRelease?) -> FirmwareRelease? {
        guard let stable else { return pre }
        guard let pre else { return stable }
        guard let order = FirmwareVersion.compare(pre.version, stable.version) else { return stable }
        return order > 0 ? pre : stable
    }

    // MARK: The download

    /// Download a release's container and verify it against the manifest's own numbers — the byte
    /// count first, then the SHA-256. Only a container that matches both is returned; a failure
    /// throws and **nothing is sent to the device**, which is the entire point of doing this on the
    /// phone rather than discovering it after a multi-minute BLE transfer.
    public func download(_ release: FirmwareRelease) async throws -> Data {
        let (status, body) = try await fetcher.get(release.url)
        guard (200..<300).contains(status) else { throw FirmwareDownloadError.httpStatus(status) }
        guard body.count == release.bytes else {
            throw FirmwareDownloadError.sizeMismatch(expected: release.bytes, got: body.count)
        }
        guard Self.sha256Hex(body) == release.sha256 else { throw FirmwareDownloadError.digestMismatch }
        return body
    }

    /// Lowercase hex SHA-256, the digest dialect the manifest speaks.
    public static func sha256Hex(_ data: Data) -> String {
        SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }
}
