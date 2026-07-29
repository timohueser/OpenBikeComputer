import Foundation

/// The proactive update surfaces (#773 U5): the decision of **whether to say anything at all**,
/// and to whom, when the rider hasn't opened the firmware screen.
///
/// U4 built the check as a pure service with no timers on purpose; this file is the other half —
/// the one rule everything that *can* interrupt the rider has to pass through. The launch sheet,
/// the background refresh's notification, and any later surface all call
/// ``UpdateSurfacePolicy/decide(_:)`` and do exactly what it says. Nothing here presents anything:
/// the mediums are dumb adapters (a SwiftUI sheet, one `UNNotificationRequest`), which is what
/// makes them safe to leave untested.
///
/// **The locked refusals**, in the order they're applied:
///
/// 1. Auto-check off → silence, and no network request either. The toggle gates the *check*, not
///    just the notice — a rider who turned it off is not quietly polled.
/// 2. A running version that isn't a release version → **never**, exactly as on the screen
///    (``FirmwareVersion/updateStatus(running:latest:)`` answers `.unknown` and #773 locks the
///    consequence). A probe-flashed dev build is not interrupted, and it isn't checked for either.
/// 3. Nothing known about the device yet → silence. There is nothing to compare against, so any
///    notice would be a guess.
/// 4. Everything else answers from the **cached** check when it's fresh (U4's 6 h), so becoming
///    active does not mean a network request; a stale or absent cache asks for one first.
/// 5. `current` / `ahead` / `noRelease` → silence. Only `available` is worth a rider's attention.
/// 6. A version this device has already been asked about → silence. Acting on the offer and
///    dismissing it both count as answered (the builder's ledger semantics, PR #1004); a **newer**
///    published version is a new question.

/// What the app remembers about the device it last talked to, so a wake with no link still knows
/// which device it would be talking about.
///
/// Deliberately **not** part of ``BondRecord``: that is bond state, written wholesale on every
/// rename (`bondStore.save(BondRecord(deviceName:))`), and a firmware revision parked there would
/// be erased by an unrelated edit. The device *name* is not duplicated here for the same reason —
/// it stays the bond record's job, and the surfaces read it from there so a rename is never stale
/// in a notification.
public struct LastSeenDevice: Equatable, Sendable, Codable {
    /// DIS 0x2A25. The ledger key: it survives a rename, and it's what makes "this device has
    /// already been asked" mean one device rather than one phone.
    public let serial: String
    /// DIS 0x2A26 as last read — the running version the comparison needs.
    public let firmwareVersion: String
    /// When it was read. Not consulted by the policy; it's here so a future surface can decide a
    /// record is too old to reason about without a migration.
    public let seenAt: Date

    public init(serial: String, firmwareVersion: String, seenAt: Date) {
        self.serial = serial
        self.firmwareVersion = firmwareVersion
        self.seenAt = seenAt
    }

    /// The ledger key. A device that reports no serial still gets a stable (if shared) bucket —
    /// better than an unbounded ledger keyed on the empty string.
    public var ledgerKey: String { serial.isEmpty ? "unknown" : serial }
}

/// Persistence for the surfaces: the rider's toggle, the answered ledger, and the last-seen device.
/// Beside ``UpdateCheckStore`` — phone-local, never on the wire.
public protocol UpdateSurfaceStore: Sendable {
    /// "Check for updates automatically". **Default on**, so a fresh install is told about
    /// updates; off means no launch check, no background check, no notification.
    func loadAutoCheckEnabled() -> Bool
    func saveAutoCheckEnabled(_ enabled: Bool)
    /// The newest version this device has already been asked about, if any.
    func loadAnsweredVersion(device key: String) -> String?
    /// Record that the rider answered — by tapping through *or* by dismissing.
    func saveAnsweredVersion(_ version: String, device key: String)
    func loadLastSeenDevice() -> LastSeenDevice?
    func saveLastSeenDevice(_ device: LastSeenDevice)
    /// Whether authorization for update notices has been asked for yet (once is enough; iOS
    /// answers a second request from its stored decision, but asking is a decision *moment* and
    /// repeating it would blur when it happened).
    func loadDidAskNotificationPermission() -> Bool
    func saveDidAskNotificationPermission(_ asked: Bool)
}

/// The real store: `UserDefaults`, mirroring ``UserDefaultsUpdateCheckStore``. `@unchecked` for the
/// same reason it is there — `UserDefaults` is documented thread-safe but unannotated.
public struct UserDefaultsUpdateSurfaceStore: UpdateSurfaceStore, @unchecked Sendable {
    private static let autoCheckKey = "obc.firmwareAutoCheck"
    private static let ledgerKey = "obc.firmwareAnsweredVersions"
    private static let lastDeviceKey = "obc.firmwareLastSeenDevice"
    private static let askedKey = "obc.firmwareDidAskNotifications"
    private let defaults: UserDefaults

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    /// Absent means *on*: `bool(forKey:)` alone would read a fresh install as off, which is the
    /// opposite of the documented default.
    public func loadAutoCheckEnabled() -> Bool {
        defaults.object(forKey: Self.autoCheckKey) as? Bool ?? true
    }

    public func saveAutoCheckEnabled(_ enabled: Bool) {
        defaults.set(enabled, forKey: Self.autoCheckKey)
    }

    public func loadAnsweredVersion(device key: String) -> String? {
        (defaults.dictionary(forKey: Self.ledgerKey) as? [String: String])?[key]
    }

    /// One entry per device, overwritten — the ledger records the newest answered version, not a
    /// growing history, so it can't grow without bound on a phone that sees many updates.
    public func saveAnsweredVersion(_ version: String, device key: String) {
        var ledger = (defaults.dictionary(forKey: Self.ledgerKey) as? [String: String]) ?? [:]
        ledger[key] = version
        defaults.set(ledger, forKey: Self.ledgerKey)
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

/// In-memory — the default for previews/tests, so no run leaks its ledger into the next.
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
        lock.withLock { ledger[key] = version }
    }
    public func loadLastSeenDevice() -> LastSeenDevice? { lock.withLock { lastSeen } }
    public func saveLastSeenDevice(_ device: LastSeenDevice) { lock.withLock { lastSeen = device } }
    public func loadDidAskNotificationPermission() -> Bool { lock.withLock { asked } }
    public func saveDidAskNotificationPermission(_ asked: Bool) { lock.withLock { self.asked = asked } }
}

/// What a surface should do.
public enum UpdateSurfaceDecision: Equatable, Sendable {
    /// Say nothing. Every refusal lands here — they are deliberately indistinguishable to the
    /// caller, because there is no surface that treats "dev build" differently from "up to date".
    case nothing
    /// Nothing usable is cached: ask the network, then decide again on the answer.
    case check
    /// A newer published build this device hasn't been asked about.
    case surface(FirmwareRelease)
}

/// The decision core — pure, total, and the only place the rules live. Everything it needs is
/// passed in (including `now`), so the whole table is a table test.
public enum UpdateSurfacePolicy {
    /// Everything the decision depends on, gathered by the caller.
    public struct Context: Equatable, Sendable {
        /// The rider's "Check for updates automatically" setting.
        public var autoCheckEnabled: Bool
        /// DIS 0x2A26 for the device in question — live if the link is up, otherwise the persisted
        /// ``LastSeenDevice``. `nil` when the app has never seen a device.
        public var runningVersion: String?
        /// U4's cached answer, if any.
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

    /// A cached answer young enough to decide on. Same window and same clock-went-backwards rule as
    /// ``UpdateChecker/isFresh(_:now:)`` — one definition of fresh, read here without needing a
    /// checker instance.
    static func isFresh(_ record: UpdateCheckRecord, now: Date) -> Bool {
        let age = now.timeIntervalSince(record.checkedAt)
        return age >= 0 && age < UpdateChecker.freshness
    }

    public static func decide(_ context: Context) -> UpdateSurfaceDecision {
        // 1. The toggle gates the network too, not just the notice.
        guard context.autoCheckEnabled else { return .nothing }
        // 2 + 3. A device that can't be reasoned about is never interrupted — and never polled on
        // its behalf either. `updateStatus` gives `.unknown` for both an unparseable running
        // version and a missing one; neither can ever become `.available`, so there is no point
        // spending a request to find out.
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
        // 6. Asked and answered — until a newer version publishes.
        guard context.answeredVersion != release.version else { return .nothing }
        return .surface(release)
    }
}

/// The one code path both surfaces run: gather the context, decide, perform the check the policy
/// asked for, decide again. Returns the release to surface, or `nil` for silence.
///
/// Deliberately medium-agnostic. The launch sheet turns a non-`nil` answer into a sheet and the
/// background refresh turns the same answer into one local notification; neither of them re-derives
/// a rule, which is what keeps the rules in one place.
public struct UpdateSurfaceRunner: Sendable {
    private let checker: UpdateChecker
    private let store: any UpdateSurfaceStore

    public init(checker: UpdateChecker = UpdateChecker(), store: any UpdateSurfaceStore = UserDefaultsUpdateSurfaceStore()) {
        self.checker = checker
        self.store = store
    }

    /// Which device we'd be talking about: the one passed in (a live DIS read) or the last one
    /// seen. `nil` when this phone has never read a device's version.
    public func device(_ live: LastSeenDevice? = nil) -> LastSeenDevice? {
        live ?? store.loadLastSeenDevice()
    }

    /// Remember a device we just read over the link, so a later wake with no link still knows what
    /// version to compare against.
    public func remember(_ device: LastSeenDevice) { store.saveLastSeenDevice(device) }

    /// Record that the rider answered for this device — tapping through and dismissing are the
    /// same answer.
    public func recordAnswered(version: String, device: LastSeenDevice?) {
        store.saveAnsweredVersion(version, device: device?.ledgerKey ?? "unknown")
    }

    public var autoCheckEnabled: Bool { store.loadAutoCheckEnabled() }

    /// Whether the one permission moment has already happened.
    public var didAskNotificationPermission: Bool { store.loadDidAskNotificationPermission() }

    public func markAskedNotificationPermission() { store.saveDidAskNotificationPermission(true) }

    /// Decide, checking the network only if the policy asks. A failed check is silence: there is
    /// nothing to act on, and a phone in a valley does not have an update problem.
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
            // Re-decide on the fresh answer. It cannot ask for another check (the record it just
            // wrote is fresh by construction), so this recursion is one level deep by definition.
            if case .surface(let release) = UpdateSurfacePolicy.decide(context(record)) {
                return release
            }
            return nil
        }
    }
}

/// The notification seam. One method to ask, one to post — deliberately this small, because the
/// conformer that talks to `UNUserNotificationCenter` is then dumb enough that not testing it is
/// defensible. Denial is not an error here: it degrades to silence, and the launch sheet keeps
/// working.
public protocol UpdateNotifying: Sendable {
    /// Ask for permission to post update notices. Called at the one moment #773 U5 picked (see
    /// the launch sheet), never at launch.
    func requestAuthorization() async
    /// Post the "update available" notice. Answers **whether it was actually posted** — `false` for
    /// a rider who declined, and that distinction matters: a notice nobody could receive must not
    /// mark the version answered, or a denied permission would silently swallow the offer the launch
    /// sheet would otherwise have made.
    func notifyUpdateAvailable(version: String, deviceName: String) async -> Bool
}

/// Copy for the update notices, here rather than in the adapter so the wording is reviewable
/// beside the rules and testable without a notification center.
public enum UpdateNoticeCopy {
    /// The notification title — the version is the news, so it goes in the title.
    public static func title(version: String) -> String {
        "Firmware \(versioned(version)) is available"
    }

    /// The body. Plain: what it's for, and what happens next. No urgency, no exclamation.
    public static func body(deviceName: String) -> String {
        "A new firmware version is published for \(deviceName). Open OBC to send it."
    }

    /// "v1.4.0" — prefix a bare version, pass through one that has it (the S7 screen's rule).
    public static func versioned(_ version: String) -> String {
        version.hasPrefix("v") ? version : "v\(version)"
    }
}
