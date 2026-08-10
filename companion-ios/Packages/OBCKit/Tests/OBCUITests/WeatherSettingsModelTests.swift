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

private final class FakeJobs: WeatherJobControlling, @unchecked Sendable {
    private let lock = NSLock()
    private var job: WeatherJobPending?
    private(set) var retryCount = 0
    /// What the retry does to the checkpoint — the engine clears it on success.
    var clearsOnRetry = true

    init(_ job: WeatherJobPending?) { self.job = job }

    func pendingJob() async -> WeatherJobPending? { lock.withLock { job } }

    func retryNow() async {
        lock.withLock {
            retryCount += 1
            if clearsOnRetry { job = nil }
        }
    }
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
    // MARK: The refresh interval (device truth)

    @Test func theStoredIntervalIsReadFromTheDeviceAndEditable() async {
        let (model, _, _) = makeModel(refresh: .every60)
        model.start()
        await waitFor("config") { model.hasReadConfig }
        #expect(model.refresh == .every60)
        #expect(model.canEditRefresh)
        #expect(WeatherCopy.refreshValue(
            model.refresh, unknownToThisBuild: false, hasRead: true) == "Every hour")
    }

    /// Every value on the wire, including `off` — which must never be reported as "the default".
    @Test(arguments: WeatherRefresh.allCases)
    func everyIntervalRoundTripsThroughTheDevice(_ target: WeatherRefresh) async {
        let (model, control, _) = makeModel(refresh: target == .off ? .every30 : .off)
        model.start()
        await waitFor("config") { model.hasReadConfig }
        model.setRefresh(target)
        await waitFor("write") { control.fixtures.config.weatherRefreshRaw == target.rawValue }
        #expect(model.refresh == target)
        #expect(control.fixtures.config.knownWeatherRefresh == target)
    }

    /// An absent Config byte is the device's documented default (30 min) — not `off`, and not
    /// "unknown". The `??` that would have quietly disabled weather lives nowhere.
    @Test func anAbsentRefreshByteReadsAsTheDeviceDefault() async {
        let (model, _, _) = makeModel(refresh: nil)
        model.start()
        await waitFor("config") { model.hasReadConfig }
        #expect(model.refresh == .every30)
        #expect(!model.refreshIsUnknownToThisBuild)
    }

    /// A newer firmware naming a fifth interval: tolerated on read (§11.8), and said out loud
    /// rather than rendered as a plausible-looking 30 minutes.
    @Test func anIntervalThisBuildCannotNameIsStatedNotGuessed() async {
        let (model, _, _) = makeModel(refresh: nil, refreshRawOverride: 9)
        model.start()
        await waitFor("config") { model.hasReadConfig }
        #expect(model.refresh == nil)
        #expect(model.refreshIsUnknownToThisBuild)
        #expect(WeatherCopy.refreshValue(nil, unknownToThisBuild: true, hasRead: true)
            == "Set on the device")
    }

    /// Not connected: the control is dimmed and no write is attempted. The device owns the value,
    /// so the alternative — a phone-side edit that "applies later" — would be a promise the app
    /// cannot keep.
    @Test func anUnreachableDeviceCannotBeEdited() async {
        let (model, control, _) = makeModel(scenario: .outOfRange)
        model.start()
        await waitFor("link") { model.connection != .connecting }
        #expect(!model.canEditRefresh)
        let before = control.fixtures.config.weatherRefreshRaw
        model.setRefresh(.every15)
        #expect(control.fixtures.config.weatherRefreshRaw == before)
    }

    /// Firmware without the weather feature bit: the screen says so once, and the control stays
    /// shut — writing an interval to a device that schedules nothing is theatre.
    @Test func firmwareWithoutWeatherIsReportedAndNotWrittenTo() async {
        let (model, _, _) = makeModel(supportsWeather: false)
        model.start()
        await waitFor("capability") { model.deviceSupportsWeather != nil }
        #expect(model.deviceSupportsWeather == false)
        #expect(!model.canEditRefresh)
        #expect(model.statusFooter.contains("doesn't include weather"))
    }

    /// A failed write reverts the row and raises the toast — the screen never keeps showing a
    /// value the device refused.
    @Test func aFailedWriteRevertsAndSurfacesOnce() async {
        let (model, control, _) = makeModel(refresh: .every30)
        model.start()
        await waitFor("config") { model.hasReadConfig }
        // Two armed faults, not one: the model re-reads the device when the link stream reports
        // `.connected`, and that read would otherwise eat the single fault before the write leg
        // ever ran. Either leg failing means the device never took the interval, which is the
        // behaviour under test.
        control.failNextOp(.writeFailed)
        control.failNextOp(.writeFailed)
        model.setRefresh(.every120)
        await waitFor("failure surfaced") { model.refreshWriteFailed }
        #expect(model.refresh == .every30)
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
    }

    @Test func aDeliveredJobShowsWhenItLanded() async {
        let (model, _, _) = makeModel(history: [historyEntry(outcome: .committed)])
        model.start()
        await waitFor("history") { model.lastDelivery != nil }
        #expect(model.lastDeliveryValue == "12 min ago")
        #expect(model.statusLine == "Delivered 12 min ago")
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
        #expect(model.canRetry)

        await model.retryNow()
        #expect(jobs.retryCount == 1)
        #expect(model.pending == nil, "a finished job leaves no owed work behind")
        #expect(!model.canRetry)
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
}
