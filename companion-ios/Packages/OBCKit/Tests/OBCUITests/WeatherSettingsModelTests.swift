import Foundation
import Testing
import OBCDomain
import OBCMock
import OBCTransport
import OBCWeather
@testable import OBCUI

// WX13 acceptance, host-side: the Weather screen's model driven through `MockTransport` and
// scripted weather seams. Everything the screen claims is asserted here — that the refresh control
// tells the truth about a device it cannot reach, that provenance comes from the manifest and
// nowhere else, that a stale service is never drawn as healthy, and that the ring's states are
// distinguishable in the words a rider reads.

// MARK: - Scripted seams

private final class FakeHistory: WeatherJobHistoryStore, @unchecked Sendable {
    private let lock = NSLock()
    private var stored: [WeatherJobHistoryEntry]

    init(_ entries: [WeatherJobHistoryEntry] = []) { stored = entries }

    func append(_ entry: WeatherJobHistoryEntry) { lock.withLock { stored.append(entry) } }
    func entries() -> [WeatherJobHistoryEntry] { lock.withLock { stored } }
}

/// A one-shot rendezvous so a test can hold a retry mid-flight and look at the spinner.
actor RetryGate {
    private var opened = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func wait() async {
        guard !opened else { return }
        await withCheckedContinuation { waiters.append($0) }
    }

    func open() {
        opened = true
        for waiter in waiters { waiter.resume() }
        waiters = []
    }
}

private final class FakeJobs: WeatherJobControlling, @unchecked Sendable {
    private let lock = NSLock()
    private var job: WeatherJobPending?
    private(set) var retryCount = 0
    /// What the retry does to the checkpoint — the engine clears it on success.
    var clearsOnRetry = true
    /// When set, `retryNow()` parks here until the test opens it — the engine's real contract is
    /// that this call returns on *completion*, so a test of the spinner needs a slow one.
    var gate: RetryGate?

    init(_ job: WeatherJobPending?) { self.job = job }

    func pendingJob() async -> WeatherJobPending? { lock.withLock { job } }

    func retryNow() async {
        lock.withLock { retryCount += 1 }
        if let gate = lock.withLock({ gate }) { await gate.wait() }
        lock.withLock { if clearsOnRetry { job = nil } }
    }
}

/// A transport that models the config plane only, and counts both directions — the seam that pins
/// "this screen reads the device and never writes to it". `MockControl` counts ops in aggregate;
/// this separates reads from writes, which is the whole assertion.
private final class ConfigLinkTransport: DeviceTransport, @unchecked Sendable {
    private let lock = NSLock()
    private var _config: DeviceConfig
    private var _readCalls = 0
    private var _writesStarted = 0

    init(config: DeviceConfig) { _config = config }

    var config: DeviceConfig { lock.withLock { _config } }
    var readCalls: Int { lock.withLock { _readCalls } }
    var writesStarted: Int { lock.withLock { _writesStarted } }

    func readConfig() async throws -> DeviceConfig {
        lock.withLock {
            _readCalls += 1
            return _config
        }
    }

    func writeConfig(_ config: DeviceConfig) async throws {
        lock.withLock {
            _writesStarted += 1
            _config = config
        }
    }

    func deviceInfo() async throws -> DeviceInfo {
        DeviceInfo(
            name: "Trailhead", firmwareVersion: "0.4.2",
            featureBits: OBCProtocol.featureWeather)
    }

    // Inert remainder — a connected link, nothing else modelled.
    var state: AsyncStream<ConnectionState> {
        AsyncStream { $0.yield(.connected); $0.finish() }
    }
    var battery: AsyncStream<Int> { AsyncStream { $0.finish() } }
    var storeChanges: AsyncStream<StoreChanged> { AsyncStream { $0.finish() } }
    func connect() async throws {}
    func disconnect() async {}
    func listRoutes() async throws -> [RouteCatalogEntry] { [] }
    func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail { throw DeviceError.readFailed }
    func uploadRoute(_ route: RouteBlob) -> TransferHandle {
        .immediatelyFinished(.failed(.notConnected))
    }
    func deleteRoute(_ id: DeviceObjectID) async throws {}
    func listRides() async throws -> RideCatalog { RideCatalog(rides: []) }
    func rideDetail(_ id: RideID) async throws -> RideDetail { throw DeviceError.readFailed }
    func downloadRides(_ ids: [RideID]) -> RideDownload { .finished() }
    func readDiagnostics() async throws -> Data { Data() }
}

@MainActor
private func makeConfigModel(
    refresh: WeatherRefresh = .every30
) -> (WeatherSettingsModel, ConfigLinkTransport) {
    var config = DeviceConfig(name: "Trailhead")
    config.weatherRefreshRaw = refresh.rawValue
    let transport = ConfigLinkTransport(config: config)
    let model = WeatherSettingsModel(
        transport: transport, historyStore: FakeHistory(), jobs: nil,
        statusProvider: FakeStatus(status: nil), now: { clock })
    return (model, transport)
}

private struct FakeStatus: WeatherServiceStatusProviding {
    var status: WeatherServiceStatus?

    func serviceStatus(now: Date) async throws -> WeatherServiceStatus {
        guard let status else { throw WeatherManifestError.malformed }
        return status
    }
}

// MARK: - Fixtures

private let clock = Date(timeIntervalSince1970: 1_800_000_000)

private func product(
    id: String, tier: UInt8 = 1, credit: String = "Source: Deutscher Wetterdienst (DWD)",
    staleness: TimeInterval = 900, cellMetres: UInt16 = 1_000
) -> WeatherServiceProductStatus {
    WeatherServiceProductStatus(
        id: id, tier: WeatherTier(rawValue: tier), nominalCellMetres: cellMetres,
        referenceTime: clock.addingTimeInterval(-300),
        generatedAt: clock.addingTimeInterval(-120),
        stalenessDeadline: clock.addingTimeInterval(staleness),
        attribution: WeatherAttribution(text: credit, url: "https://example.invalid/licence"),
        frameCount: 9, latestFrameValidAt: clock.addingTimeInterval(7_200))
}

private func historyEntry(
    outcome: WeatherJobHistoryEntry.Outcome, failure: WeatherJobFailure? = nil,
    phase: WeatherJobPhase = .uploading, minutesAgo: Double = 12, attempts: Int = 1,
    productID: String? = "dwd-rv"
) -> WeatherJobHistoryEntry {
    WeatherJobHistoryEntry(
        startedAt: clock.addingTimeInterval(-minutesAgo * 60 - 20),
        finishedAt: clock.addingTimeInterval(-minutesAgo * 60),
        requestID: 42, outcome: outcome, failureReason: failure, phaseReached: phase,
        attempts: attempts, bundleByteCount: 41_200, readConnectedMilliseconds: 1_800,
        uploadConnectedMilliseconds: outcome == .committed ? 2_600 : nil,
        precipitationProductID: productID)
}

@MainActor
private func makeModel(
    scenario: Scenario = .happyPath,
    supportsWeather: Bool = true,
    refresh: WeatherRefresh? = .every30,
    refreshRawOverride: UInt8? = nil,
    history: [WeatherJobHistoryEntry] = [],
    pending: WeatherJobPending? = nil,
    status: WeatherServiceStatus? = WeatherServiceStatus(
        generatedAt: clock.addingTimeInterval(-240), observedAt: clock,
        products: [product(id: "dwd-rv")], skippedProducts: 0),
    preferences: WeatherPreferencesStore = InMemoryWeatherPreferencesStore()
) -> (WeatherSettingsModel, MockControl, FakeJobs) {
    let control = MockControl(scenario: scenario)
    control.latency = .zero
    control.deviceInfo = DeviceInfo(
        name: "Trailhead", firmwareVersion: "0.4.2",
        featureBits: supportsWeather ? OBCProtocol.featureWeather : 0)
    var config = control.fixtures.config
    config.weatherRefreshRaw = refreshRawOverride ?? refresh?.rawValue
    control.fixtures.config = config
    let jobs = FakeJobs(pending)
    let model = WeatherSettingsModel(
        transport: MockTransport(control: control),
        historyStore: FakeHistory(history),
        jobs: jobs,
        statusProvider: FakeStatus(status: status),
        preferences: preferences,
        now: { clock })
    return (model, control, jobs)
}

@MainActor
private func waitFor(
    _ what: String, timeout: Duration = .seconds(5), _ condition: () -> Bool
) async {
    let deadline = ContinuousClock.now.advanced(by: timeout)
    while !condition() {
        if ContinuousClock.now > deadline {
            Issue.record("timed out waiting for \(what)")
            return
        }
        try? await Task.sleep(for: .milliseconds(5))
    }
}

// MARK: - The suite

@Suite("Weather settings model")
@MainActor
struct WeatherSettingsModelTests {
    // MARK: The refresh interval (device truth, reported — never written)

    @Test func theStoredIntervalIsReadFromTheDeviceAndReported() async {
        let (model, _, _) = makeModel(refresh: .every60)
        model.start()
        await waitFor("config") { model.hasReadConfig }
        #expect(model.refresh == .every60)
        #expect(model.canStateRefresh)
        #expect(model.refreshValue == "Every hour")
    }

    /// Every value on the wire, including `off` — which must never be reported as "the default",
    /// and must not be silently rendered as some nearby interval either.
    @Test(arguments: WeatherRefresh.allCases)
    func everyIntervalTheDeviceCanHoldIsReportedAsItself(_ stored: WeatherRefresh) async {
        let (model, _, _) = makeModel(refresh: stored)
        model.start()
        await waitFor("config") { model.hasReadConfig }
        #expect(model.refresh == stored)
        #expect(model.refreshValue == WeatherCopy.refreshLabel(stored))
    }

    /// An absent Config byte is the device's documented default (30 min) — not `off`, and not
    /// "unknown". The `??` that would have quietly disabled weather lives nowhere.
    @Test func anAbsentRefreshByteReadsAsTheDeviceDefault() async {
        let (model, _, _) = makeModel(refresh: nil)
        model.start()
        await waitFor("config") { model.hasReadConfig }
        #expect(model.refresh == .every30)
        #expect(!model.refreshIsUnknownToThisBuild)
        #expect(model.refreshValue == "Every 30 min")
    }

    /// A newer firmware naming a fifth interval: tolerated on read (§11.8), and said out loud
    /// rather than rendered as a plausible-looking 30 minutes. With no editor on this screen the
    /// wording is now simply where the value lives — there is nothing here to "replace it with".
    @Test func anIntervalThisBuildCannotNameIsStatedNotGuessed() async {
        let (model, _, _) = makeModel(refresh: nil, refreshRawOverride: 9)
        model.start()
        await waitFor("config") { model.hasReadConfig }
        #expect(model.refresh == nil)
        #expect(model.refreshIsUnknownToThisBuild)
        #expect(model.refreshValue == "Set on the device")
    }

    /// Not connected: the row does not claim a value. There is no phone-side mirror to fall back
    /// on, deliberately — a remembered interval is this screen guessing at a setting it does not
    /// own, and it would look exactly like a read.
    @Test func anUnreachableDeviceIsNotGuessedAt() async {
        let (model, _, _) = makeModel(scenario: .outOfRange)
        model.start()
        await waitFor("link") { model.connection != .connecting }
        #expect(!model.canStateRefresh)
        #expect(model.refreshValue == "Not connected")
    }

    /// **The whole write path is gone** (Timo's rule: device settings live on the device, and the
    /// OBC's own Weather screen is the interval's editor). This pins the absence rather than
    /// trusting it: nothing this screen does may ever call `writeConfig`, whatever the link state.
    /// A future "just one small setting" would have to delete this test to land, which is the
    /// point.
    @Test func theScreenNeverWritesToTheDevice() async {
        let (model, transport) = makeConfigModel(refresh: .every30)
        model.start()
        await waitFor("config") { model.hasReadConfig }

        // Everything a rider can do on this screen, plus a revisit.
        model.setWatchEnabled(false)
        model.setWatchEnabled(true)
        await model.retryNow()
        await model.appeared()
        await model.refreshAll()

        #expect(transport.writesStarted == 0, "the Weather screen has no write path at all")
        #expect(transport.readCalls > 0, "…and it is not vacuous: it does read the device")
    }

    /// Firmware without the weather feature bit: the screen says so once, and states no interval —
    /// reporting a schedule for a device that schedules nothing would be theatre.
    @Test func firmwareWithoutWeatherIsReportedAndClaimsNoInterval() async {
        let (model, _, _) = makeModel(supportsWeather: false)
        model.start()
        await waitFor("capability") { model.deviceSupportsWeather != nil }
        #expect(model.deviceSupportsWeather == false)
        #expect(!model.canStateRefresh)
        // The row goes away entirely rather than reporting the Config byte such a device still
        // happens to carry: a schedule for something that schedules nothing is theatre, and it
        // contradicted the banner directly above it (caught on glass).
        #expect(!model.showsRefreshRow)
        #expect(model.statusFooter.contains("doesn't include weather"))
    }

    /// …and it *is* shown for a device that has weather, connected or not — the row's absence has
    /// to mean "no such thing", never "not right now".
    @Test func theScheduleRowStaysForAWeatherCapableDeviceEvenOutOfRange() async {
        let (model, _, _) = makeModel(scenario: .outOfRange)
        model.start()
        await waitFor("link") { model.connection != .connecting }
        #expect(model.showsRefreshRow)
    }

    /// The status footer is where the rider learns the interval is changed on the OBC — the one
    /// thing the removed picker used to answer by existing.
    @Test func theStatusFooterSaysWhereTheIntervalIsChanged() async {
        let (model, _, _) = makeModel(refresh: .every30)
        model.start()
        await waitFor("config") { model.hasReadConfig }
        #expect(model.statusFooter.contains("on the OBC itself"))
    }

    // MARK: The standing watch (phone truth)

    @Test func theWatchSwitchPersistsAndReachesTheTransport() async {
        let preferences = InMemoryWeatherPreferencesStore(watchEnabled: true)
        let (model, control, _) = makeModel(preferences: preferences)
        model.start()
        #expect(model.watchEnabled)

        model.setWatchEnabled(false)
        #expect(!preferences.loadWeatherWatchEnabled())
        #expect(!control.weatherWatchArmed)

        // A fresh screen over the same store starts from the rider's choice, not the default.
        let (reopened, _, _) = makeModel(preferences: preferences)
        #expect(!reopened.watchEnabled)
    }

    // MARK: Status and retry

    @Test func withNoHistoryTheScreenSaysSoRatherThanImplyingSuccess() async {
        let (model, _, _) = makeModel()
        model.start()
        await waitFor("history") { model.service != .loading }
        #expect(model.lastDeliveryValue == "Never")
        #expect(model.statusLine == "No weather sent yet")
        #expect(!model.canRetry)
        // …and the live-status row stays away: with nothing owed and nothing failed it would only
        // repeat the row above it.
        #expect(!model.showsStatusRow)
    }

    @Test func aDeliveredJobShowsWhenItLanded() async {
        let (model, _, _) = makeModel(history: [historyEntry(outcome: .committed)])
        model.start()
        await waitFor("history") { model.lastDelivery != nil }
        #expect(model.lastDeliveryValue == "12 min ago")
        #expect(model.statusLine == "Delivered 12 min ago")
        #expect(!model.showsStatusRow, "a success needs one line, not two")
    }

    /// A failed last run is the one thing the delivery row cannot say, so it gets its own line —
    /// labelled as history ("Last try"), not as something still happening.
    @Test func aFailedLastRunIsStatedAsHistoryNotAsProgress() async {
        let (model, _, _) = makeModel(history: [
            historyEntry(outcome: .committed, minutesAgo: 90),
            historyEntry(outcome: .failed, failure: .uploadFailed, minutesAgo: 6, attempts: 6),
        ])
        model.start()
        await waitFor("history") { model.lastAttempt != nil }
        #expect(model.showsStatusRow)
        #expect(model.statusRowLabel == "Last try")
        #expect(model.statusLine == "Last try failed · The Bluetooth transfer dropped")
        #expect(model.lastDeliveryValue == "2 h ago", "the successful one is still stated")
    }

    /// The acceptance question: "phone had the data but the upload failed" must read differently
    /// from "the fetch failed". Both the label and the explanation differ.
    @Test func anUploadFailureReadsDifferentlyFromAFetchFailure() async {
        let uploadFailure = historyEntry(
            outcome: .failed, failure: .uploadFailed, phase: .uploading)
        let fetchFailure = historyEntry(
            outcome: .failed, failure: .fetchFailed, phase: .fetching)
        #expect(WeatherCopy.failureLabel(.uploadFailed) != WeatherCopy.failureLabel(.fetchFailed))
        #expect(WeatherCopy.failureExplanation(uploadFailure)
            == "The phone had the weather ready; sending it to the OBC failed.")
        #expect(WeatherCopy.failureExplanation(fetchFailure)
            == "The forecast never reached the phone.")
        #expect(WeatherCopy.failureExplanation(historyEntry(outcome: .committed)) == nil)
        // A job that ran out of time is not a send that failed, whatever phase it died in — and it
        // is not even `.failed`: it carries its own calm outcome (#1198 review).
        #expect(WeatherCopy.failureExplanation(historyEntry(
            outcome: .agedOut, failure: .agedOut, phase: .bundleReady))
            == "It went out of date before it reached the OBC.")
    }

    /// Aged out is its own outcome with its own word, and it is not painted as a failure: the same
    /// job usually carries on and delivers moments later, so the row is information rather than an
    /// alarm. The status line proves it — a last run that aged out does not make the screen say
    /// something failed.
    @Test func anAgedOutRunIsItsOwnCalmOutcomeNotAFailure() async {
        #expect(WeatherCopy.outcomeLabel(.agedOut) == "Expired")
        #expect(Set([
            WeatherCopy.outcomeLabel(.committed), WeatherCopy.outcomeLabel(.failed),
            WeatherCopy.outcomeLabel(.superseded), WeatherCopy.outcomeLabel(.agedOut),
        ]).count == 4)

        let (model, _, _) = makeModel(history: [
            historyEntry(outcome: .committed, minutesAgo: 40),
            historyEntry(outcome: .agedOut, failure: .agedOut, phase: .bundleReady, minutesAgo: 3),
        ])
        model.start()
        await waitFor("history") { model.lastAttempt != nil }
        #expect(!model.showsStatusRow, "an expired run is not a failure to shout about")
        #expect(model.statusLine == "Delivered 40 min ago")
    }

    /// The no-rain-map reason is copy, not Swift's debug spelling. `String(describing:)` used to
    /// put `allCoveringProductsExpired(latestDeadline: …)` on a diagnostics row, complete with a
    /// UTC debug date (#1198 review).
    @Test func theNoRainMapReasonRendersInPlainWordsWithARealTime() {
        let deadline = clock.addingTimeInterval(-1_800)
        let expired = WeatherCopy.noRainMapReasonLabel(
            .allCoveringProductsExpired(latestDeadline: deadline))
        #expect(expired == "rain maps for this area expired at \(WeatherCopy.absolute(deadline))")
        #expect(!expired.contains("latestDeadline"))
        #expect(!expired.contains("("))

        let all: [NoRainMapReason] = [
            .corridorNotCovered, .allCoveringProductsExpired(latestDeadline: deadline),
            .serviceUnavailable, .framesUnavailable, .noFramesInWindow,
        ]
        let labels = all.map { WeatherCopy.noRainMapReasonLabel($0) }
        #expect(Set(labels).count == labels.count, "every case reads as itself")
        // No case name survives into the copy — the tell that a `String(describing:)` crept back.
        #expect(!labels.contains { $0.contains("corridorNotCovered") || $0.contains("Unavailable") })
    }

    /// The two source-level splits this issue landed, read at the screen: a drop and a corruption
    /// are different sentences, and so are superseded and aged out.
    @Test func theRingsNewVocabularyIsDistinguishableOnGlass() {
        let labels = [
            WeatherCopy.failureLabel(.uploadFailed),
            WeatherCopy.failureLabel(.transferCorrupted),
            WeatherCopy.failureLabel(.superseded),
            WeatherCopy.failureLabel(.agedOut),
        ]
        #expect(Set(labels).count == labels.count)
        #expect(WeatherCopy.outcomeLabel(.superseded) == "Replaced")
    }

    @Test func aPendingJobShowsItsPhaseAndOffersARetry() async {
        let pending = WeatherJobPending(
            phase: .bundleReady, requestID: 42, startedAt: clock.addingTimeInterval(-90),
            updatedAt: clock.addingTimeInterval(-30), attempts: 1, deferrals: 0,
            bundleByteCount: 40_000)
        let (model, _, jobs) = makeModel(pending: pending)
        model.start()
        await waitFor("pending") { model.pending != nil }
        #expect(model.statusLine == "Ready to send")
        #expect(model.showsStatusRow)
        #expect(model.statusRowLabel == "Now")
        #expect(model.canRetry)

        await model.retryNow()
        #expect(jobs.retryCount == 1)
        #expect(model.pending == nil, "a finished job leaves no owed work behind")
        #expect(!model.canRetry)
    }

    /// The spinner is bound to the whole retry, not to its scheduling. The engine's `retryNow()`
    /// returns on completion (a tap that only queues behind a run in flight still waits), and the
    /// model must not undo that by finishing early — nor let a second tap start a second one
    /// (#1198 review).
    @Test func theRetryRowStaysBusyUntilTheJobIsActuallyDone() async {
        let pending = WeatherJobPending(
            phase: .bundleReady, requestID: 42, startedAt: clock.addingTimeInterval(-90),
            updatedAt: clock.addingTimeInterval(-30), attempts: 1, deferrals: 0,
            bundleByteCount: 40_000)
        let (model, _, jobs) = makeModel(pending: pending)
        let gate = RetryGate()
        jobs.gate = gate
        model.start()
        await waitFor("pending") { model.pending != nil }

        let tap = Task { await model.retryNow() }
        await waitFor("in flight") { model.isRetrying }
        // A rider pressing again while it spins must not start a second run.
        await model.retryNow()
        #expect(jobs.retryCount == 1)
        #expect(model.isRetrying, "still going")

        await gate.open()
        await tap.value
        #expect(!model.isRetrying)
        #expect(model.pending == nil)
        #expect(jobs.retryCount == 1)
    }

    /// A cooldown is a rider staring at a screen where nothing happens; say what is going on.
    @Test func aCoolingDownJobSaysItIsWaiting() async {
        let pending = WeatherJobPending(
            phase: .uploading, requestID: 42, startedAt: clock.addingTimeInterval(-120),
            updatedAt: clock, attempts: 2, deferrals: 0,
            retryNotBefore: clock.addingTimeInterval(20))
        let (model, _, _) = makeModel(pending: pending)
        model.start()
        await waitFor("pending") { model.pending != nil }
        #expect(model.statusLine == "Waiting to retry in 20s")
    }

    // MARK: The service (manifest-sourced)

    @Test func attributionListsMETForHourlyAndTheManifestsCreditsForRain() async {
        let (model, _, _) = makeModel(status: WeatherServiceStatus(
            generatedAt: clock.addingTimeInterval(-240), observedAt: clock,
            products: [
                product(id: "dwd-rv", credit: "Source: Deutscher Wetterdienst (DWD)"),
                product(id: "mrms", tier: 1, credit: "Source: NOAA/NWS MRMS"),
            ],
            skippedProducts: 0))
        model.start()
        await waitFor("service") { model.service != .loading }
        let rows = model.attributions
        #expect(rows.first?.credit == .met)
        #expect(rows.first?.role == "Hourly forecast")
        #expect(rows.dropFirst().map(\.credit.text) == [
            "Source: Deutscher Wetterdienst (DWD)", "Source: NOAA/NWS MRMS",
        ])
        #expect(model.serviceValue == "Published 4 min ago")
    }

    /// A credit long enough to wrap several lines is still a credit: nothing truncates it, and the
    /// screen keeps rendering the rest of the list.
    @Test func aVeryLongAttributionIsCarriedWhole() async {
        let long = String(repeating: "Deutscher Wetterdienst, Offenbach am Main, ", count: 8)
        let (model, _, _) = makeModel(status: WeatherServiceStatus(
            generatedAt: clock, observedAt: clock,
            products: [product(id: "dwd-rv", credit: long)], skippedProducts: 0))
        model.start()
        await waitFor("service") { model.service != .loading }
        #expect(model.attributions.contains { $0.credit.text == long })
        #expect(model.products.count == 1)
    }

    @Test func aStaleProductSaysStaleSinceAndNeverReadsAsHealthy() async {
        let (model, _, _) = makeModel(status: WeatherServiceStatus(
            generatedAt: clock.addingTimeInterval(-3_600), observedAt: clock,
            products: [product(id: "dwd-rv", staleness: -600)], skippedProducts: 0))
        model.start()
        await waitFor("service") { model.service != .loading }
        #expect(model.staleProducts.map(\.id) == ["dwd-rv"])
        #expect(model.serviceFooter.contains("Service data stale since"))
        #expect(model.serviceFooter.contains("never shown as dry"))
        #expect(WeatherCopy.productFreshness(model.products[0], now: clock)
            .hasPrefix("Stale since"))
    }

    @Test func anUnavailableServiceKeepsTheHourlyPromiseHonest() async {
        let (model, _, _) = makeModel(status: nil)
        model.start()
        await waitFor("service") { model.service != .loading }
        #expect(model.service == .unavailable)
        #expect(model.serviceValue == "Unavailable")
        #expect(model.serviceFooter.contains("MET Norway"))
        #expect(model.products.isEmpty)
    }

    /// The tier vocabulary is a label, never a gate: a tier number this build predates renders as
    /// itself instead of vanishing from the list.
    @Test func anUnknownTierStillRenders() async {
        let (model, _, _) = makeModel(status: WeatherServiceStatus(
            generatedAt: clock, observedAt: clock,
            products: [product(id: "future-source", tier: 7)], skippedProducts: 0))
        model.start()
        await waitFor("service") { model.service != .loading }
        #expect(model.products.map(\.id) == ["future-source"])
        #expect(WeatherCopy.tierLabel(WeatherTier(rawValue: 7)) == "Tier 7")
    }

    // MARK: Privacy copy

    /// These sentences are claims about the system. If one stops being true, this test is where it
    /// gets caught rather than in a rider's hands.
    @Test func thePrivacyCopyStatesTheThreeLoadBearingFacts() {
        let copy = WeatherPrivacyCopy.standard
        let all = (copy.steps.map { $0.title + " " + $0.body } + copy.sent + copy.notSent
            + [copy.closing]).joined(separator: " ")
        #expect(all.contains("MET Norway"))
        #expect(copy.notSent.contains { $0.contains("location permission") })
        #expect(copy.notSent.contains { $0.contains("No account") })
        #expect(copy.sent.contains { $0.contains("corridor") })
        #expect(all.contains("excluded from backups"))
    }

    /// One thing, one name. The three surfaces used to call it "the weather service", "OBC's file
    /// storage" and "OBC's own service" — on pages that link to each other, in the exact place a
    /// reader is counting how many parties there are (#1198 review).
    @Test func theWeatherServiceHasOneNameOnEverySurface() async {
        let name = WeatherCopy.serviceName
        let privacy = WeatherPrivacyCopy.standard
        let privacyText = (privacy.steps.map(\.body) + privacy.sent + privacy.notSent)
            .joined(separator: " ")
        #expect(privacyText.contains(name))
        #expect(WeatherCopy.aboutFooter.contains(name))

        let (model, _, _) = makeModel(status: nil)
        model.start()
        await waitFor("service") { model.service != .loading }
        #expect(model.serviceFooter.contains(name))

        // The old spellings are gone, not merely joined by a fourth.
        let everySurface = privacyText + " " + WeatherCopy.aboutFooter + " " + model.serviceFooter
        for stale in ["OBC's file storage", "OBC's own file storage", "OBC's own service"] {
            #expect(!everySurface.contains(stale), "stale name still on glass: \(stale)")
        }
    }

    /// The main screen's footer makes the same claim the privacy page does, in the same words: the
    /// corridor names a region, and the position goes to MET alone. It used to say only "the
    /// weather service never receives your position", which is true but leaves the reader to
    /// discover on another page that *something* about where they are does leave (#1198 review).
    @Test func theMainScreenFooterCarriesTheCorridorNuance() {
        let footer = WeatherCopy.aboutFooter
        #expect(footer.contains("corridor"))
        #expect(footer.contains("region"))
        #expect(footer.contains("MET Norway"))
        #expect(footer.contains("never your position"))
        // The privacy page's own corridor sentence says the same thing at length.
        #expect(WeatherPrivacyCopy.standard.sent.contains {
            $0.contains("corridor") && $0.contains("region")
        })
    }
}
