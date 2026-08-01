import Foundation
import Testing
@testable import OBCTransport

/// The proactive-update rules (#773 U5). This is the suite that matters: the launch sheet and the
/// background notification are both thin adapters over ``UpdateSurfacePolicy``, so what is pinned
/// here is *everything either of them can decide*.
///
/// Two rules earn the most tests, because getting them wrong is a product failure rather than a bug:
/// a device on an unparseable version is **never** interrupted (#773's locked refusal, the same rule
/// the S7 screen honours), and a version already put to the rider is never raised twice — while a
/// newer one is a new question.
struct UpdateSurfacePolicyTests {
    // MARK: Fixtures

    static let now = Date(timeIntervalSince1970: 1_800_000_000)

    static func release(_ version: String, bytes: Int = 874_496) -> FirmwareRelease {
        FirmwareRelease(
            version: version,
            bytes: bytes,
            sha256: String(repeating: "a", count: 64),
            url: URL(string: "https://updates.openbikecomputer.com/fw/UPDATE.BIN")!
        )
    }

    /// A cached answer `age` old.
    static func cached(_ version: String?, age: TimeInterval = 60) -> UpdateCheckRecord {
        UpdateCheckRecord(
            release: version.map { release($0) },
            checkedAt: now.addingTimeInterval(-age)
        )
    }

    static func context(
        autoCheck: Bool = true,
        running: String? = "1.3.0",
        cached: UpdateCheckRecord? = UpdateSurfacePolicyTests.cached("1.4.0"),
        answered: String? = nil
    ) -> UpdateSurfacePolicy.Context {
        UpdateSurfacePolicy.Context(
            autoCheckEnabled: autoCheck,
            runningVersion: running,
            cached: cached,
            answeredVersion: answered,
            now: now
        )
    }

    // MARK: The table

    @Test("A newer published build nobody has been asked about is surfaced")
    func availableAndUnsurfacedSurfaces() {
        #expect(
            UpdateSurfacePolicy.decide(Self.context())
                == .surface(Self.release("1.4.0"))
        )
    }

    @Test("The same version is raised once — acting on it or dismissing it both answer it")
    func alreadyAnsweredIsSilent() {
        #expect(UpdateSurfacePolicy.decide(Self.context(answered: "1.4.0")) == .nothing)
    }

    @Test("A newer version than the one already answered is a new question")
    func newerVersionAsksAgain() {
        let context = Self.context(cached: Self.cached("1.5.0"), answered: "1.4.0")
        #expect(UpdateSurfacePolicy.decide(context) == .surface(Self.release("1.5.0")))
    }

    @Test("A channel rollback never re-asks about an older answered release")
    func olderVersionStaysSilent() {
        let context = Self.context(cached: Self.cached("1.4.0"), answered: "1.5.0")
        #expect(UpdateSurfacePolicy.decide(context) == .nothing)
        // Different spellings of the same semantic version are not new questions either.
        #expect(UpdateSurfacePolicy.decide(Self.context(answered: "v1.4.0")) == .nothing)
    }

    /// #773's locked refusal, and the reason it is a table rather than one case: an unparseable
    /// running version must lose to *nothing* — not to a published release, not to a stale cache,
    /// not to an empty ledger. A probe-flashed dev build is never interrupted and never polled for.
    @Test(
        "A running version that isn't a release version is never surfaced",
        arguments: ["abc1234", "main", "0.4", "v", "1.2.3.4", "twelve"]
    )
    func unparseableRunningVersionNeverSurfaces(running: String) {
        #expect(UpdateSurfacePolicy.decide(Self.context(running: running)) == .nothing)
        // …and not even a network request is spent finding out.
        #expect(UpdateSurfacePolicy.decide(Self.context(running: running, cached: nil)) == .nothing)
    }

    @Test("A device that has never reported a version is silent, not surfaced")
    func unknownDeviceIsSilent() {
        #expect(UpdateSurfacePolicy.decide(Self.context(running: nil, cached: nil)) == .nothing)
        #expect(UpdateSurfacePolicy.decide(Self.context(running: nil)) == .nothing)
        #expect(UpdateSurfacePolicy.decide(Self.context(running: "")) == .nothing)
    }

    @Test("Up to date says nothing")
    func currentIsSilent() {
        #expect(UpdateSurfacePolicy.decide(Self.context(running: "1.4.0")) == .nothing)
        // Build metadata is not a version difference (the U4 dialect) — still silent.
        #expect(UpdateSurfacePolicy.decide(Self.context(running: "1.4.0+abc1234")) == .nothing)
    }

    @Test("A device ahead of the published build is never offered a downgrade")
    func aheadIsSilent() {
        #expect(UpdateSurfacePolicy.decide(Self.context(running: "1.5.0")) == .nothing)
    }

    @Test("Nothing published says nothing")
    func noReleaseIsSilent() {
        #expect(UpdateSurfacePolicy.decide(Self.context(cached: Self.cached(nil))) == .nothing)
    }

    @Test("The toggle gates the surface AND the network")
    func toggleOffIsSilent() {
        #expect(UpdateSurfacePolicy.decide(Self.context(autoCheck: false)) == .nothing)
        // Off with no cache asks for no check either — the rider is not quietly polled.
        #expect(UpdateSurfacePolicy.decide(Self.context(autoCheck: false, cached: nil)) == .nothing)
    }

    @Test("No cached answer asks for a check first")
    func absentCacheChecks() {
        #expect(UpdateSurfacePolicy.decide(Self.context(cached: nil)) == .check)
    }

    @Test("A stale cached answer asks for a check first, even when it says an update is available")
    func staleCacheChecks() {
        let stale = Self.cached("1.4.0", age: UpdateChecker.freshness + 1)
        #expect(UpdateSurfacePolicy.decide(Self.context(cached: stale)) == .check)
    }

    @Test("A cache just inside the freshness window is decided on, not re-fetched")
    func freshCacheDecides() {
        let fresh = Self.cached("1.4.0", age: UpdateChecker.freshness - 1)
        #expect(UpdateSurfacePolicy.decide(Self.context(cached: fresh)) == .surface(Self.release("1.4.0")))
    }

    /// A clock that moved backwards (time zone, manual set) must read as stale rather than fresh
    /// forever — the same rule ``UpdateChecker/isFresh(_:now:)`` keeps.
    @Test("A cache from the future is stale, not fresh forever")
    func futureCacheIsStale() {
        let future = Self.cached("1.4.0", age: -3600)
        #expect(UpdateSurfacePolicy.decide(Self.context(cached: future)) == .check)
    }

    @Test("Silence never depends on which refusal applied")
    func refusalsAreIndistinguishable() {
        // Nothing in the enum lets a surface tell "dev build" from "up to date" — deliberately, so
        // no adapter can grow a special case for one of them.
        let refusals: [UpdateSurfacePolicy.Context] = [
            Self.context(autoCheck: false),
            Self.context(running: "abc1234"),
            Self.context(running: "1.4.0"),
            Self.context(running: "1.5.0"),
            Self.context(cached: Self.cached(nil)),
            Self.context(answered: "1.4.0"),
        ]
        for context in refusals {
            #expect(UpdateSurfacePolicy.decide(context) == .nothing)
        }
    }
}

/// The runner — the one code path both surfaces share. What matters here is that it performs the
/// check the policy asks for, decides again on the answer, and stays silent when the network fails.
struct UpdateSurfaceRunnerTests {
    private static let manifestURL = UpdateChecker.manifestURL
    private static let device = LastSeenDevice(
        serial: "OBC-0001", firmwareVersion: "1.3.0", seenAt: Date(timeIntervalSince1970: 1_799_000_000)
    )

    private func manifest(_ version: String) -> Data {
        Data(
            """
            {"version":"\(version)","bytes":874496,
             "sha256":"\(String(repeating: "a", count: 64))",
             "url":"https://updates.openbikecomputer.com/fw/UPDATE.BIN"}
            """.utf8
        )
    }

    private func makeRunner(
        body: Data? = nil,
        status: Int = 200,
        autoCheck: Bool = true,
        lastSeen: LastSeenDevice? = UpdateSurfaceRunnerTests.device,
        answered: [String: String] = [:],
        cached: UpdateCheckRecord? = nil
    ) -> (UpdateSurfaceRunner, InMemoryUpdateSurfaceStore, PolicyStubFetcher) {
        let fetcher = PolicyStubFetcher()
        if let body { fetcher.stub(Self.manifestURL, status: status, body: body) }
        let surface = InMemoryUpdateSurfaceStore(
            autoCheckEnabled: autoCheck, answered: answered, lastSeen: lastSeen
        )
        let runner = UpdateSurfaceRunner(
            checker: UpdateChecker(
                fetcher: fetcher, store: InMemoryUpdateCheckStore(record: cached)
            ),
            store: surface
        )
        return (runner, surface, fetcher)
    }

    @Test("A stale cache is checked, and a newer published build comes back")
    func checksThenSurfaces() async {
        let (runner, _, fetcher) = makeRunner(body: manifest("1.4.0"))
        let release = await runner.run()
        #expect(release?.version == "1.4.0")
        #expect(fetcher.requested == [Self.manifestURL])
    }

    @Test("The check is skipped entirely when the toggle is off")
    func toggleOffMakesNoRequest() async {
        let (runner, _, fetcher) = makeRunner(body: manifest("1.4.0"), autoCheck: false)
        #expect(await runner.run() == nil)
        #expect(fetcher.requested.isEmpty)
    }

    @Test("A dev build is never checked for, let alone surfaced")
    func devBuildMakesNoRequest() async {
        let (runner, _, fetcher) = makeRunner(
            body: manifest("1.4.0"),
            lastSeen: LastSeenDevice(serial: "OBC-0001", firmwareVersion: "abc1234", seenAt: Date())
        )
        #expect(await runner.run() == nil)
        #expect(fetcher.requested.isEmpty)
    }

    @Test("A phone that has never seen a device makes no request and says nothing")
    func noDeviceMakesNoRequest() async {
        let (runner, _, fetcher) = makeRunner(body: manifest("1.4.0"), lastSeen: nil)
        #expect(await runner.run() == nil)
        #expect(fetcher.requested.isEmpty)
    }

    @Test("A failed check is silence, not an error — a phone in a valley has no update problem")
    func failedCheckIsSilent() async {
        let (runner, _, _) = makeRunner(body: Data("nonsense".utf8))
        #expect(await runner.run() == nil)
    }

    @Test("An answered version stays answered across a fresh check")
    func answeredStaysSilent() async {
        let (runner, _, _) = makeRunner(
            body: manifest("1.4.0"), answered: ["OBC-0001": "1.4.0"]
        )
        #expect(await runner.run() == nil)
    }

    @Test("The ledger is keyed per device: another device's answer doesn't silence this one")
    func ledgerIsPerDevice() async {
        let (runner, _, _) = makeRunner(
            body: manifest("1.4.0"), answered: ["OBC-9999": "1.4.0"]
        )
        #expect(await runner.run()?.version == "1.4.0")
    }

    @Test("A live device read overrides — and refreshes — the remembered one")
    func liveDeviceWins() async {
        let (runner, surface, _) = makeRunner(body: manifest("1.4.0"), lastSeen: nil)
        let live = LastSeenDevice(serial: "OBC-0002", firmwareVersion: "1.3.0", seenAt: Date())
        runner.remember(live)
        #expect(await runner.run(device: live)?.version == "1.4.0")
        #expect(surface.loadLastSeenDevice() == live)
    }

    @Test("Recording an answer writes the ledger under that device's serial")
    func recordAnswered() {
        let (runner, surface, _) = makeRunner()
        runner.recordAnswered(version: "1.4.0", device: Self.device)
        runner.recordAnswered(version: "1.5.0", device: Self.device)
        runner.recordAnswered(version: "1.4.0", device: Self.device)
        #expect(surface.loadAnsweredVersion(device: "OBC-0001") == "1.5.0")
    }

    /// A fresh cache is the common case (the app was foregrounded twice in an hour) — and it must
    /// cost nothing, or "on launch" becomes "a request every launch".
    @Test("A fresh cache is answered from memory, with no request at all")
    func freshCacheMakesNoRequest() async {
        let (runner, _, fetcher) = makeRunner(
            body: manifest("1.4.0"),
            cached: UpdateCheckRecord(
                release: UpdateSurfacePolicyTests.release("1.4.0"), checkedAt: Date()
            )
        )
        #expect(await runner.run()?.version == "1.4.0")
        #expect(fetcher.requested.isEmpty)
    }

    /// The default is documented as ON — a fresh install is told about updates.
    @Test("Automatic checks default to on")
    func defaultsToOn() {
        let defaults = UserDefaults(suiteName: "obc.tests.updateSurface.\(UUID().uuidString)")!
        let store = UserDefaultsUpdateSurfaceStore(defaults: defaults)
        #expect(store.loadAutoCheckEnabled())
        store.saveAutoCheckEnabled(false)
        #expect(!store.loadAutoCheckEnabled())
    }

    @Test("The persisted ledger keeps one monotonic entry per device")
    func ledgerPersistsPerDevice() {
        let defaults = UserDefaults(suiteName: "obc.tests.updateSurface.\(UUID().uuidString)")!
        let store = UserDefaultsUpdateSurfaceStore(defaults: defaults)
        store.saveAnsweredVersion("1.4.0", device: "A")
        store.saveAnsweredVersion("1.4.0", device: "B")
        store.saveAnsweredVersion("1.5.0", device: "A")
        store.saveAnsweredVersion("1.4.0", device: "A")
        #expect(store.loadAnsweredVersion(device: "A") == "1.5.0")
        #expect(store.loadAnsweredVersion(device: "B") == "1.4.0")
        #expect(store.loadAnsweredVersion(device: "C") == nil)
    }

    @Test("A device that reports no serial still gets a stable ledger bucket")
    func seriallessDeviceHasAKey() {
        let device = LastSeenDevice(serial: "", firmwareVersion: "1.3.0", seenAt: Date())
        #expect(device.ledgerKey == "unknown")
    }

    @Test("The notice copy names the version in the title and the device in the body")
    func noticeCopy() {
        #expect(UpdateNoticeCopy.title(version: "1.4.0") == "Firmware v1.4.0 is available")
        #expect(UpdateNoticeCopy.title(version: "v1.4.0") == "Firmware v1.4.0 is available")
        #expect(UpdateNoticeCopy.body(deviceName: "Trailhead").contains("Trailhead"))
    }
}

/// A stubbed HTTP seam for the runner tests — records what was asked for, so "made no request" is
/// an assertion rather than a hope.
final class PolicyStubFetcher: ManifestFetching, @unchecked Sendable {
    private let lock = NSLock()
    private var responses: [URL: (status: Int, body: Data)] = [:]
    private var seen: [URL] = []

    var requested: [URL] { lock.withLock { seen } }

    func stub(_ url: URL, status: Int = 200, body: Data) {
        lock.withLock { responses[url] = (status, body) }
    }

    func get(_ url: URL) async throws -> (status: Int, body: Data) {
        lock.withLock { seen.append(url) }
        guard let response = lock.withLock({ responses[url] }) else { return (404, Data()) }
        return response
    }
}
