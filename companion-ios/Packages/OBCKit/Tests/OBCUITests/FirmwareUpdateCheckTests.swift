import Foundation
import Testing
import OBCDomain
import OBCTransport
@testable import OBCUI

/// The S7 screen's half of the published-release check (#773 U4): what the model does with the
/// answer, and what "Download & Install" actually does.
///
/// The dialect itself is pinned in `FirmwareReleaseTests` (a port of the builder's matrix); what
/// is proved here is the wiring — that the cached answer is on screen before the network is
/// touched, that a device on an unparseable version is never offered anything, and that a
/// downloaded container reaches the device only through the same `stage(_:)` gate a picked file
/// goes through.
@MainActor
struct FirmwareUpdateCheckTests {
    // MARK: Helpers

    private static let containerURL = URL(string: "https://updates.openbikecomputer.com/fw/UPDATE.BIN")!

    /// A valid OBCU container tagged with `version` — both CRCs correct, so `StagedFirmware`
    /// accepts it (the same builder the S7 state-machine tests use).
    private func container(version: String, imageLen: Int = 96) -> Data {
        var image = Data()
        image.append(contentsOf: le32(0x2002_0000))
        image.append(contentsOf: (4..<imageLen).map { UInt8($0 & 0xFF) })
        var header = Data(count: 64)
        header.replaceSubrange(0..<4, with: Array("OBCU".utf8))
        header[4] = 1
        header.replaceSubrange(8..<12, with: le32(UInt32(image.count)))
        header.replaceSubrange(12..<16, with: le32(CRC32.checksum(image)))
        let v = Array(version.utf8.prefix(32))
        header.replaceSubrange(16..<16 + v.count, with: v)
        header.replaceSubrange(60..<64, with: le32(CRC32.checksum(header[0..<60])))
        return header + image
    }

    private func le32(_ v: UInt32) -> [UInt8] { withUnsafeBytes(of: v.littleEndian, Array.init) }

    /// The manifest body describing `payload` as `version`.
    private func manifest(version: String, payload: Data, notes: String? = nil) -> Data {
        let notesField = notes.map { ",\"notes\":\"\($0)\"" } ?? ""
        return Data(
            """
            {"version":"\(version)","bytes":\(payload.count),
             "sha256":"\(UpdateChecker.sha256Hex(payload))",
             "url":"\(Self.containerURL.absoluteString)"\(notesField)}
            """.utf8
        )
    }

    private func waitFor(_ condition: () -> Bool, within timeout: Duration = .seconds(2)) async {
        let deadline = ContinuousClock.now + timeout
        while ContinuousClock.now < deadline {
            if condition() { return }
            try? await Task.sleep(for: .milliseconds(5))
        }
    }

    /// A model wired to a stubbed network + an in-memory cache.
    private func makeModel(
        running: String = "1.3.0",
        published: (version: String, payload: Data, notes: String?)? = nil,
        cached: UpdateCheckRecord? = nil,
        prereleases: Bool = false
    ) -> (FirmwareUpdateModel, StubTransport, StubFetcher, InMemoryUpdateCheckStore) {
        let transport = StubTransport()
        transport.fwVersion = running
        let fetcher = StubFetcher()
        if let published {
            fetcher.stub(
                UpdateChecker.manifestURL,
                body: manifest(version: published.version, payload: published.payload, notes: published.notes)
            )
            fetcher.stub(Self.containerURL, body: published.payload)
        }
        let store = InMemoryUpdateCheckStore(record: cached, includePrereleases: prereleases)
        let model = FirmwareUpdateModel(
            transport: transport,
            deviceName: "Trailhead",
            updateChecker: UpdateChecker(fetcher: fetcher, store: store)
        )
        return (model, transport, fetcher, store)
    }

    // MARK: The check on appear

    @Test func opensOnTheCachedAnswerAndDoesNotReAskWhileItIsFresh() async {
        let cached = UpdateCheckRecord(
            release: FirmwareRelease(
                version: "1.4.0", bytes: 10, sha256: String(repeating: "a", count: 64),
                url: Self.containerURL
            ),
            checkedAt: Date()
        )
        let (model, _, fetcher, _) = makeModel(cached: cached)

        model.start()
        // Synchronously, before any await: the cache is the point.
        #expect(model.latestRelease?.version == "1.4.0")
        #expect(model.lastCheckedAt == cached.checkedAt)

        await waitFor { model.runningVersion != nil }
        #expect(model.updateStatus == .available)
        #expect(fetcher.requested.isEmpty, "a fresh cached answer must not re-ask the network")
    }

    @Test func reAsksWhenTheCachedAnswerIsStale() async {
        let payload = container(version: "1.4.0")
        let stale = UpdateCheckRecord(
            release: nil,
            checkedAt: Date().addingTimeInterval(-UpdateChecker.freshness - 60)
        )
        let (model, _, fetcher, store) = makeModel(
            published: ("1.4.0", payload, nil), cached: stale
        )

        model.start()
        #expect(model.latestRelease == nil, "the stale answer is still shown until a better one lands")

        await waitFor { model.latestRelease != nil }
        #expect(model.latestRelease?.version == "1.4.0")
        #expect(fetcher.requested == [UpdateChecker.manifestURL])
        #expect(store.loadCheck()?.release?.version == "1.4.0", "the refreshed answer is cached")
    }

    @Test func aManualCheckReAsksEvenWithAFreshCache() async {
        let payload = container(version: "1.5.0")
        let cached = UpdateCheckRecord(
            release: FirmwareRelease(
                version: "1.4.0", bytes: 10, sha256: String(repeating: "a", count: 64),
                url: Self.containerURL
            ),
            checkedAt: Date()
        )
        let (model, _, fetcher, _) = makeModel(published: ("1.5.0", payload, nil), cached: cached)
        model.start()
        #expect(fetcher.requested.isEmpty)

        model.checkForUpdate(manual: true)
        await waitFor { model.latestRelease?.version == "1.5.0" }
        #expect(model.latestRelease?.version == "1.5.0")
        #expect(model.checkState == .idle)
    }

    /// A check nobody asked for stays quiet when it can't reach the network — an unreachable
    /// update server is not a problem the rider can act on. A check they *tapped* owes them a
    /// sentence.
    @Test func onlyAManualCheckReportsItsFailure() async {
        let (model, _, fetcher, _) = makeModel()
        fetcher.stub(UpdateChecker.manifestURL, status: 500)

        model.start()
        await waitFor({ model.checkState != .checking }, within: .seconds(1))
        #expect(model.checkState == .idle, "the automatic check fails silently")

        model.checkForUpdate(manual: true)
        await waitFor { model.checkState != .checking }
        guard case .failed(let message) = model.checkState else {
            Issue.record("a manual check must surface its failure")
            return
        }
        #expect(message.contains("500"))

        model.clearUpdateError()
        #expect(model.checkState == .idle)
    }

    // MARK: Status derivation on the screen

    @Test func offersNothingUntilTheRunningVersionIsKnown() async {
        let payload = container(version: "1.4.0")
        let (model, _, _, _) = makeModel(published: ("1.4.0", payload, nil))
        model.start()
        await waitFor { model.latestRelease != nil }

        // DIS may land after the manifest; until it does the screen must not claim this is a
        // development build — it simply has no answer yet.
        #expect(model.hasUpdateAnswer == (model.runningVersion != nil))
        await waitFor { model.runningVersion != nil }
        #expect(model.hasUpdateAnswer)
        #expect(!model.developmentBuild)
        #expect(model.updateStatus == .available)
        #expect(model.canDownloadUpdate)
    }

    /// #773's locked refusal, at the screen: a probe-flashed build reports a git hash, so no
    /// update is offered no matter what is published.
    @Test func neverOffersAnythingToADevelopmentBuild() async {
        let payload = container(version: "1.4.0")
        let (model, _, _, _) = makeModel(running: "abc1234", published: ("1.4.0", payload, nil))
        model.start()
        await waitFor { model.latestRelease != nil && model.runningVersion != nil }

        #expect(model.updateStatus == .unknown)
        #expect(model.developmentBuild)
        #expect(!model.canDownloadUpdate)

        // …and the offer stays refused even if the button is somehow reached.
        model.downloadUpdate()
        #expect(model.downloadState == .idle)
        #expect(model.phase == .idle, "the manual Files path is the only way in for a dev build")
    }

    @Test func saysAheadRatherThanOfferingADowngrade() async {
        let payload = container(version: "1.4.0")
        let (model, _, _, _) = makeModel(running: "1.5.0", published: ("1.4.0", payload, nil))
        model.start()
        await waitFor { model.latestRelease != nil && model.runningVersion != nil }

        #expect(model.updateStatus == .ahead)
        #expect(!model.canDownloadUpdate)
    }

    @Test func saysNothingLoudWhenNothingIsPublished() async {
        let (model, _, fetcher, _) = makeModel()
        fetcher.stub(UpdateChecker.manifestURL, status: 404)
        model.start()
        await waitFor { model.lastCheckedAt != nil }

        #expect(model.updateStatus == .noRelease)
        #expect(model.latestRelease == nil)
        #expect(model.checkState == .idle)
    }

    @Test func quietlyConfirmsAnUpToDateDevice() async {
        let payload = container(version: "1.4.0")
        let (model, _, _, _) = makeModel(running: "1.4.0+deadbee", published: ("1.4.0", payload, nil))
        model.start()
        await waitFor { model.latestRelease != nil && model.runningVersion != nil }

        #expect(model.updateStatus == .current)
        #expect(!model.canDownloadUpdate)
    }

    // MARK: Download & Install

    @Test func downloadsVerifiesStagesAndSends() async {
        let payload = container(version: "1.4.0")
        let (model, _, _, _) = makeModel(
            published: ("1.4.0", payload, "https://example.com/notes")
        )
        model.start()
        await waitFor { model.canDownloadUpdate && model.connection == .connected }
        #expect(model.releaseNotesURL?.absoluteString == "https://example.com/notes")

        model.downloadUpdate()
        #expect(model.downloadState == .downloading)

        // The verified container goes through the *same* staging gate a picked file does, and
        // then straight out to the device — where the on-glass confirm is still the only thing
        // that installs anything.
        await waitFor { model.phase == .transferring }
        #expect(model.staged?.version == "1.4.0")
        #expect(model.progress.total == payload.count)
        #expect(model.downloadState == .idle)
        #expect(model.importError == nil)
    }

    @Test func staysStagedWhenTheLinkIsDown() async {
        let payload = container(version: "1.4.0")
        let (model, transport, _, _) = makeModel(published: ("1.4.0", payload, nil))
        model.start()
        await waitFor { model.canDownloadUpdate }
        transport.push(.outOfRange)
        await waitFor { model.connection == .outOfRange }

        model.downloadUpdate()
        await waitFor { model.phase == .staged }
        #expect(model.phase == .staged, "the file waits, validated, for the link to come back")
        #expect(!model.canSend)
    }

    /// A download that doesn't match the manifest is thrown away on the phone — nothing is
    /// staged, so nothing can be sent.
    @Test func refusesADownloadThatDoesNotMatchTheManifest() async {
        let payload = container(version: "1.4.0")
        let (model, _, fetcher, _) = makeModel(published: ("1.4.0", payload, nil))
        model.start()
        await waitFor { model.canDownloadUpdate }

        // The server hands back something else entirely (a redirect page, a truncated object).
        fetcher.stub(Self.containerURL, body: Data(repeating: 0x7F, count: payload.count))

        model.downloadUpdate()
        await waitFor { model.downloadState != .downloading }
        guard case .failed(let message) = model.downloadState else {
            Issue.record("a mismatched download must surface a failure")
            return
        }
        #expect(!message.isEmpty)
        #expect(model.staged == nil, "nothing was staged")
        #expect(model.phase == .idle, "and nothing was sent")

        model.clearUpdateError()
        #expect(model.downloadState == .idle)
    }

    /// A container that downloads intact but isn't an OBCU image dies in the *same* validator a
    /// picked file dies in — the download path has no privileged way past `stage(_:)`.
    @Test func aVerifiedDownloadThatIsNotAnUpdateStillFailsInTheStager() async {
        let payload = Data(repeating: 0x42, count: 200)
        let (model, _, _, _) = makeModel(published: ("1.4.0", payload, nil))
        model.start()
        await waitFor { model.canDownloadUpdate }

        model.downloadUpdate()
        await waitFor { model.importError != nil }
        #expect(model.importError != nil)
        #expect(model.staged == nil)
        #expect(model.phase == .idle)
    }

    // MARK: The dev switch

    @Test func thePreReleaseSwitchReAsksOnTheOtherChannel() async {
        let stable = container(version: "1.4.0")
        let (model, _, fetcher, store) = makeModel(published: ("1.4.0", stable, nil))
        let rc = container(version: "1.5.0-rc1")
        fetcher.stub(
            UpdateChecker.prereleaseManifestURL,
            body: manifest(version: "1.5.0-rc1", payload: rc)
        )
        model.start()
        await waitFor { model.latestRelease?.version == "1.4.0" }
        #expect(!model.includePrereleases)

        model.setIncludePrereleases(true)
        await waitFor { model.latestRelease?.version == "1.5.0-rc1" }
        #expect(model.includePrereleases)
        #expect(store.loadIncludePrereleases())
    }

    // MARK: Lifecycle

    @Test func aWiringWithoutACheckerIsTheOldFilesOnlyScreen() async {
        let model = FirmwareUpdateModel(transport: StubTransport(), deviceName: "Trailhead")
        model.start()
        await waitFor { model.runningVersion != nil }

        #expect(!model.supportsUpdateCheck)
        #expect(model.latestRelease == nil)
        #expect(model.lastCheckedAt == nil)
        #expect(!model.canDownloadUpdate)
        model.checkForUpdate(manual: true)
        #expect(model.checkState == .idle)
    }

    @Test func poppingTheScreenStopsAnInFlightCheck() async {
        let payload = container(version: "1.4.0")
        let (model, _, _, _) = makeModel(published: ("1.4.0", payload, nil))
        model.start()
        model.stop()
        #expect(model.checkState == .idle)
        #expect(model.downloadState == .idle)
    }
}

/// A `ManifestFetching` that answers from a table; anything unstubbed 404s.
private final class StubFetcher: ManifestFetching, @unchecked Sendable {
    private let lock = NSLock()
    private var responses: [URL: (Int, Data)] = [:]
    private var asked: [URL] = []

    var requested: [URL] { lock.withLock { asked } }

    func stub(_ url: URL, status: Int = 200, body: Data = Data()) {
        lock.withLock { responses[url] = (status, body) }
    }

    func get(_ url: URL) async throws -> (status: Int, body: Data) {
        lock.withLock {
            asked.append(url)
            return responses[url] ?? (404, Data())
        }
    }
}

/// The same minimal `DeviceTransport` the S7 state-machine tests use: a controllable link, a
/// settable running version, and an inert firmware transfer.
private final class StubTransport: DeviceTransport, @unchecked Sendable {
    private var stateConts: [AsyncStream<ConnectionState>.Continuation] = []
    private var lastState: ConnectionState = .connected
    var fwVersion = "1.3.0"
    var installResult: FirmwareInstallResult = .accepted

    private var uploadProgress: AsyncStream<TransferProgress>.Continuation?
    private var uploadOutcome = AsyncPromise<TransferOutcome>()

    func push(_ state: ConnectionState) {
        lastState = state
        stateConts.forEach { $0.yield(state) }
    }

    func completeUpload() {
        uploadProgress?.finish()
        uploadOutcome.fulfill(.completed)
    }

    var state: AsyncStream<ConnectionState> {
        AsyncStream { cont in
            cont.yield(lastState)
            stateConts.append(cont)
        }
    }

    func deviceInfo() async throws -> DeviceInfo {
        DeviceInfo(name: "Trailhead", firmwareVersion: fwVersion)
    }

    func uploadFirmware(_ container: Data) -> TransferHandle {
        let (stream, cont) = AsyncStream<TransferProgress>.makeStream()
        uploadProgress = cont
        uploadOutcome = AsyncPromise<TransferOutcome>()
        return TransferHandle(
            progress: stream,
            outcome: uploadOutcome,
            onCancel: { [uploadOutcome] in uploadOutcome.fulfill(.canceled) },
            onResume: {}
        )
    }

    func installFirmware() async throws -> FirmwareInstallResult { installResult }

    var battery: AsyncStream<Int> { AsyncStream { $0.finish() } }
    var storeChanges: AsyncStream<StoreChanged> { AsyncStream { $0.finish() } }
    func connect() async throws {}
    func disconnect() async {}
    func readConfig() async throws -> DeviceConfig { DeviceConfig(name: "Trailhead") }
    func writeConfig(_ config: DeviceConfig) async throws {}
    func listRoutes() async throws -> [RouteCatalogEntry] { [] }
    func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail { throw DeviceError.readFailed }
    func uploadRoute(_ route: RouteBlob) -> TransferHandle { .immediatelyFinished(.failed(.notConnected)) }
    func deleteRoute(_ id: DeviceObjectID) async throws {}
    func listRides() async throws -> RideCatalog { RideCatalog(rides: []) }
    func rideDetail(_ id: RideID) async throws -> RideDetail { throw DeviceError.readFailed }
    func downloadRides(_ ids: [RideID]) -> RideDownload { .finished() }
    func readDiagnostics() async throws -> Data { Data() }
}
