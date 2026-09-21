import Foundation

/// The proactive update surfaces: whether to say anything at all, and to whom, when the rider
/// has not opened the firmware screen.
///
/// Every surface that can interrupt the rider calls ``UpdateSurfacePolicy/decide(_:)`` and does
/// exactly what it says. Nothing here presents anything: the mediums are dumb adapters, which is
/// what makes them safe to leave untested. The refusals are the numbered steps in `decide`.

/// What the app remembers about the device it last talked to, so a wake with no link still knows
/// which device it would be talking about.
///
/// Deliberately not part of ``BondRecord``: that is written wholesale on every rename, so a
/// firmware revision parked there would be erased by an unrelated edit. The device name stays the
/// bond record's job, and the surfaces read it from there, so a rename is never stale in a notice.
public struct LastSeenDevice: Equatable, Sendable, Codable {
    /// The ledger key: it survives a rename, so "already asked" means one device, not one phone.
    public let serial: String
    /// The running version the comparison needs.
    public let firmwareVersion: String
    /// When it was read. Not consulted by the policy; it is here so a future surface can decide
    /// a record is too old to reason about, without a migration.
    public let seenAt: Date

    public init(serial: String, firmwareVersion: String, seenAt: Date) {
        self.serial = serial
        self.firmwareVersion = firmwareVersion
        self.seenAt = seenAt
    }

    /// The ledger key. A device that reports no serial gets a stable, if shared, bucket rather
    /// than an unbounded ledger keyed on the empty string.
    public var ledgerKey: String { serial.isEmpty ? "unknown" : serial }
}

/// Whether `candidate` is a genuinely newer question than `answered`.
///
/// The ledger is monotonic by version, not by arrival order: a channel rollback must not re-ask
/// about an older release, and a late dismissal must not overwrite a newer background answer.
private func isNewerAnswer(_ candidate: String, than answered: String?) -> Bool {
    guard let answered else { return true }
    guard let order = FirmwareVersion.compare(candidate, answered) else {
        return candidate != answered
    }
    return order > 0
}

/// Persistence for the surfaces: the rider's toggle, the answered ledger, and the last-seen
/// device. Phone-local, never on the wire.
public protocol UpdateSurfaceStore: Sendable {
    /// "Check for updates automatically". Default on: off means no launch check, no background
    /// check and no notification.
    func loadAutoCheckEnabled() -> Bool
    func saveAutoCheckEnabled(_ enabled: Bool)
    /// The newest version this device has already been asked about, if any.
    func loadAnsweredVersion(device key: String) -> String?
    /// Record that the rider answered, by tapping through or by dismissing.
    func saveAnsweredVersion(_ version: String, device key: String)
    func loadLastSeenDevice() -> LastSeenDevice?
    func saveLastSeenDevice(_ device: LastSeenDevice)
    /// Whether authorization for update notices has been asked for yet. Once is enough, and
    /// asking is a decision moment, so repeating it would blur when it happened.
    func loadDidAskNotificationPermission() -> Bool
    func saveDidAskNotificationPermission(_ asked: Bool)
}

/// The real store: `UserDefaults`. `@unchecked` because `UserDefaults` is documented
/// thread-safe but unannotated.
public struct UserDefaultsUpdateSurfaceStore: UpdateSurfaceStore, @unchecked Sendable {
    private static let autoCheckKey = "obc.firmwareAutoCheck"
    private static let ledgerKey = "obc.firmwareAnsweredVersions"
    private static let lastDeviceKey = "obc.firmwareLastSeenDevice"
    private static let askedKey = "obc.firmwareDidAskNotifications"
    /// `UserDefaults` makes each operation thread-safe, not this read-modify-write transaction.
    /// Every store instance shares the lock, because the foreground and background paths
    /// deliberately construct separate stores over the same defaults domain.
    private static let ledgerLock = NSLock()
    private let defaults: UserDefaults

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    /// Absent means on: `bool(forKey:)` alone would read a fresh install as off, the opposite of
    /// the documented default.
    public func loadAutoCheckEnabled() -> Bool {
        defaults.object(forKey: Self.autoCheckKey) as? Bool ?? true
    }

    public func saveAutoCheckEnabled(_ enabled: Bool) {
        defaults.set(enabled, forKey: Self.autoCheckKey)
    }

    public func loadAnsweredVersion(device key: String) -> String? {
        (defaults.dictionary(forKey: Self.ledgerKey) as? [String: String])?[key]
    }

    /// One entry per device, advanced only, so the ledger cannot grow without bound or regress
    /// when two surfaces finish out of order.
    public func saveAnsweredVersion(_ version: String, device key: String) {
        Self.ledgerLock.withLock {
            var ledger = (defaults.dictionary(forKey: Self.ledgerKey) as? [String: String]) ?? [:]
            guard isNewerAnswer(version, than: ledger[key]) else { return }
            ledger[key] = version
            defaults.set(ledger, forKey: Self.ledgerKey)
        }
    }

    public func loadLastSeenDevice() -> LastSeenDevice? {
        guard let data = defaults.data(forKey: Self.lastDeviceKey) else { return nil }
        return try? JSONDecoder().decode(LastSeenDevice.self, from: data)
    }

    public func saveLastSeenDevice(_ device: LastSeenDevice) {
        guard let data = try? JSONEncoder().encode(device) else { return }
        defaults.set(data, forKey: Self.lastDeviceKey)
    }

    public func loadDidAskNotificationPermission() -> Bool { defaults.bool(forKey: Self.askedKey) }

    public func saveDidAskNotificationPermission(_ asked: Bool) {
        defaults.set(asked, forKey: Self.askedKey)
    }
}

/// In-memory, the default for previews and tests, so no run leaks its ledger into the next.
public final class InMemoryUpdateSurfaceStore: UpdateSurfaceStore, @unchecked Sendable {
    private let lock = NSLock()
    private var autoCheck: Bool
    private var ledger: [String: String]
    private var lastSeen: LastSeenDevice?
    private var asked: Bool

    public init(
        autoCheckEnabled: Bool = true,
        answered: [String: String] = [:],
        lastSeen: LastSeenDevice? = nil,
        didAskNotificationPermission: Bool = false
    ) {
        self.autoCheck = autoCheckEnabled
        self.ledger = answered
        self.lastSeen = lastSeen
        self.asked = didAskNotificationPermission
    }

    public func loadAutoCheckEnabled() -> Bool { lock.withLock { autoCheck } }
    public func saveAutoCheckEnabled(_ enabled: Bool) { lock.withLock { autoCheck = enabled } }
    public func loadAnsweredVersion(device key: String) -> String? { lock.withLock { ledger[key] } }
    public func saveAnsweredVersion(_ version: String, device key: String) {
        lock.withLock {
            guard isNewerAnswer(version, than: ledger[key]) else { return }
            ledger[key] = version
        }
    }
    public func loadLastSeenDevice() -> LastSeenDevice? { lock.withLock { lastSeen } }
    public func saveLastSeenDevice(_ device: LastSeenDevice) { lock.withLock { lastSeen = device } }
    public func loadDidAskNotificationPermission() -> Bool { lock.withLock { asked } }
    public func saveDidAskNotificationPermission(_ asked: Bool) { lock.withLock { self.asked = asked } }
}

/// What a surface should do.
public enum UpdateSurfaceDecision: Equatable, Sendable {
    /// Say nothing. Every refusal lands here, deliberately indistinguishable to the caller,
    /// because no surface treats "dev build" differently from "up to date".
    case nothing
    /// Nothing usable is cached: ask the network, then decide again on the answer.
    case check
    /// A newer published build this device hasn't been asked about.
    case surface(FirmwareRelease)
}

/// The decision core: pure, total, and the only place the rules live. Everything it needs is
/// passed in, `now` included, so the whole table is a table test.
public enum UpdateSurfacePolicy {
    /// Everything the decision depends on, gathered by the caller.
    public struct Context: Equatable, Sendable {
        public var autoCheckEnabled: Bool
        /// The running version for the device in question: live if the link is up, otherwise
        /// the persisted ``LastSeenDevice``. Nil when the app has never seen a device.
        public var runningVersion: String?
        /// The cached answer, if any.
        public var cached: UpdateCheckRecord?
        /// The newest version this device has already been asked about.
        public var answeredVersion: String?
        public var now: Date

        public init(
            autoCheckEnabled: Bool,
            runningVersion: String?,
            cached: UpdateCheckRecord?,
            answeredVersion: String?,
            now: Date = Date()
        ) {
            self.autoCheckEnabled = autoCheckEnabled
            self.runningVersion = runningVersion
            self.cached = cached
            self.answeredVersion = answeredVersion
            self.now = now
        }
    }

    /// A cached answer young enough to decide on. The same window, and the same
    /// clock-went-backwards rule, as ``UpdateChecker/isFresh(_:now:)``: one definition of fresh,
    /// read here without a checker instance.
    static func isFresh(_ record: UpdateCheckRecord, now: Date) -> Bool {
        let age = now.timeIntervalSince(record.checkedAt)
        return age >= 0 && age < UpdateChecker.freshness
    }

    public static func decide(_ context: Context) -> UpdateSurfaceDecision {
        // 1. The toggle gates the network too, not just the notice.
        guard context.autoCheckEnabled else { return .nothing }
        // 2 + 3. A device that cannot be reasoned about is never interrupted, and never polled on
        // its behalf: an unparseable or missing running version can never become `.available`,
        // so a request would buy nothing.
        guard let running = context.runningVersion, !running.isEmpty,
              FirmwareVersion.parse(running) != nil
        else { return .nothing }
        // 4. Answer from the cache when it's fresh; otherwise ask once, and let the caller come
        // back through here with the answer.
        guard let cached = context.cached, isFresh(cached, now: context.now) else { return .check }
        // 5. Only `available` is worth saying unprompted.
        guard let release = cached.release,
              FirmwareVersion.updateStatus(running: running, latest: release.version) == .available
        else { return .nothing }
        // 6. Asked and answered, until a genuinely newer version publishes. Comparing the
        // versions rather than the strings also keeps a channel rollback silent.
        guard isNewerAnswer(release.version, than: context.answeredVersion) else { return .nothing }
        return .surface(release)
    }
}

/// The one code path both surfaces run: gather the context, decide, perform the check the policy
/// asked for, decide again. Returns the release to surface, or nil for silence.
///
/// Deliberately medium-agnostic: the launch sheet turns a non-nil answer into a sheet and the
/// background refresh turns the same answer into a notification, and neither re-derives a rule.
public struct UpdateSurfaceRunner: Sendable {
    private let checker: UpdateChecker
    private let store: any UpdateSurfaceStore

    public init(checker: UpdateChecker = UpdateChecker(), store: any UpdateSurfaceStore = UserDefaultsUpdateSurfaceStore()) {
        self.checker = checker
        self.store = store
    }

    /// Which device we would be talking about: the one passed in, or the last one seen. Nil
    /// when this phone has never read a device's version.
    public func device(_ live: LastSeenDevice? = nil) -> LastSeenDevice? {
        live ?? store.loadLastSeenDevice()
    }

    /// Remember a device we just read over the link, so a later wake with no link still knows what
    /// version to compare against.
    public func remember(_ device: LastSeenDevice) { store.saveLastSeenDevice(device) }

    /// Record that the rider answered for this device: tapping through and dismissing are the
    /// same answer.
    public func recordAnswered(version: String, device: LastSeenDevice?) {
        store.saveAnsweredVersion(version, device: device?.ledgerKey ?? "unknown")
    }

    public var autoCheckEnabled: Bool { store.loadAutoCheckEnabled() }

    /// Whether the one permission moment has already happened.
    public var didAskNotificationPermission: Bool { store.loadDidAskNotificationPermission() }

    public func markAskedNotificationPermission() { store.saveDidAskNotificationPermission(true) }

    /// Decide, checking the network only if the policy asks. A failed check is silence: there
    /// is nothing to act on, and a phone in a valley does not have an update problem.
    public func run(device live: LastSeenDevice? = nil, now: Date = Date()) async -> FirmwareRelease? {
        let target = device(live)
        func context(_ cached: UpdateCheckRecord?) -> UpdateSurfacePolicy.Context {
            UpdateSurfacePolicy.Context(
                autoCheckEnabled: store.loadAutoCheckEnabled(),
                runningVersion: target?.firmwareVersion,
                cached: cached,
                answeredVersion: target.flatMap { store.loadAnsweredVersion(device: $0.ledgerKey) },
                now: now
            )
        }

        switch UpdateSurfacePolicy.decide(context(checker.cachedCheck())) {
        case .nothing:
            return nil
        case .surface(let release):
            return release
        case .check:
            guard let record = try? await checker.check(now: now) else { return nil }
            // Re-decide on the fresh answer. It cannot ask for another check, because the record
            // it just wrote is fresh by construction, so this recursion is one level deep.
            if case .surface(let release) = UpdateSurfacePolicy.decide(context(record)) {
                return release
            }
            return nil
        }
    }
}

/// The notification seam. One method to ask, one to post, deliberately this small, because the
/// conformer that talks to `UNUserNotificationCenter` is then dumb enough that not testing it is
/// defensible. Denial is not an error here: it degrades to silence.
public protocol UpdateNotifying: Sendable {
    /// Ask for permission to post update notices. Called at one chosen moment, never at launch.
    func requestAuthorization() async
    /// Post the "update available" notice. Answers whether it was actually posted: false for a
    /// rider who declined. That distinction matters, because a notice nobody could receive must
    /// not mark the version answered and swallow the offer the launch sheet would have made.
    func notifyUpdateAvailable(version: String, deviceName: String) async -> Bool
}

/// Copy for the update notices, here rather than in the adapter so the wording is reviewable
/// beside the rules and testable without a notification center.
public enum UpdateNoticeCopy {
    /// The version is the news, so it goes in the title.
    public static func title(version: String) -> String {
        "Firmware \(versioned(version)) is available"
    }

    /// The body. Plain: what it's for, and what happens next. No urgency, no exclamation.
    public static func body(deviceName: String) -> String {
        "A new firmware version is published for \(deviceName). Open OBC to send it."
    }

    public static func versioned(_ version: String) -> String {
        version.hasPrefix("v") ? version : "v\(version)"
    }
}
