import Foundation
import Testing
import OBCDomain
import OBCTransport
@testable import OBCUI

/// The update screen's half of the published-release check: what the model does with the answer,
/// and what "Download & Install" does. The version dialect itself is pinned in
/// `FirmwareReleaseTests`; what is proved here is the wiring.
@MainActor
struct FirmwareUpdateCheckTests {
    // MARK: Helpers

    private static let containerURL = URL(string: "https://updates.openbikecomputer.com/fw/UPDATE.BIN")!

    /// A phone-acceptable OBCU v2 container tagged with `version`: correct CRCs and a signature
    /// marker, so `StagedFirmware` accepts it. The phone treats the signature itself as opaque.
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
        header.replaceSubrange(48..<50, with: le16(OBCUHeader.sigSchemeEd25519))
        header.replaceSubrange(50..<52, with: le16(UInt16(OBCUHeader.sigLength)))
        header.replaceSubrange(60..<64, with: le32(CRC32.checksum(header[0..<60])))
        return header + image + Data(repeating: 0xA5, count: OBCUHeader.sigLength)
    }

    private func le16(_ v: UInt16) -> [UInt8] { withUnsafeBytes(of: v.littleEndian, Array.init) }
    private func le32(_ v: UInt32) -> [UInt8] { withUnsafeBytes(of: v.littleEndian, Array.init) }

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

    /// A model wired to a stubbed network and an in-memory cache.
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

    @Test func opensOnTheCachedAnswerAndDoesNotReAskWhileItIsFresh() async throws {
        let cached = UpdateCheckRecord(
            release: FirmwareRelease(
                version: "1.4.0", bytes: 10, sha256: String(repeating: "a", count: 64),
                url: Self.containerURL
            ),
            checkedAt: Date()
        )
        let (model, _, fetcher, _) = makeModel(cached: cached)

        model.start()
        // Synchronously, before any await: the cached answer is the point.
        #expect(model.latestRelease?.version == "1.4.0")
        #expect(model.lastCheckedAt == cached.checkedAt)

        try await waitFor(interval: .milliseconds(5)) { model.runningVersion != nil }
        #expect(model.updateStatus == .available)
        #expect(fetcher.requested.isEmpty, "a fresh cached answer must not re-ask the network")
    }

    @Test func reAsksWhenTheCachedAnswerIsStale() async throws {
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

        try await waitFor(interval: .milliseconds(5)) { model.latestRelease != nil }
        #expect(model.latestRelease?.version == "1.4.0")
        #expect(fetcher.requested == [UpdateChecker.manifestURL])
        #expect(store.loadCheck()?.release?.version == "1.4.0", "the refreshed answer is cached")
    }

    @Test func aManualCheckReAsksEvenWithAFreshCache() async throws {
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
        try await waitFor(interval: .milliseconds(5)) { model.latestRelease?.version == "1.5.0" }
        #expect(model.latestRelease?.version == "1.5.0")
        #expect(model.checkState == .idle)
    }

    /// An unreachable update server is not a problem the rider can act on, so an automatic check
    /// stays quiet. A check they tapped owes them a sentence.
    @Test func onlyAManualCheckReportsItsFailure() async throws {
        let (model, _, fetcher, _) = makeModel()
        fetcher.stub(UpdateChecker.manifestURL, status: 500)

        model.start()
        try await waitFor(interval: .milliseconds(5)) { model.checkState != .checking }
        #expect(model.checkState == .idle, "the automatic check fails silently")

        model.checkForUpdate(manual: true)
        try await waitFor(interval: .milliseconds(5)) { model.checkState != .checking }
        guard case .failed(let message) = model.checkState else {
            Issue.record("a manual check must surface its failure")
            return
        }
        #expect(message.contains("500"))

        model.clearUpdateError()
        #expect(model.checkState == .idle)
    }

    // MARK: Status derivation on the screen

    @Test func offersNothingUntilTheRunningVersionIsKnown() async throws {
        let payload = container(version: "1.4.0")
        let (model, _, _, _) = makeModel(published: ("1.4.0", payload, nil))
        model.start()
        try await waitFor(interval: .milliseconds(5)) { model.latestRelease != nil }

        // DIS may land after the manifest. Until it does the screen has no answer; it must not
        // call this a development build.
        #expect(model.hasUpdateAnswer == (model.runningVersion != nil))
        try await waitFor(interval: .milliseconds(5)) { model.runningVersion != nil }
        #expect(model.hasUpdateAnswer)
        #expect(!model.developmentBuild)
        #expect(model.updateStatus == .available)
        #expect(model.canDownloadUpdate)
    }

    /// A probe-flashed build reports a git hash, so nothing is offered whatever is published.
    @Test func neverOffersAnythingToADevelopmentBuild() async throws {
        let payload = container(version: "1.4.0")
        let (model, _, _, _) = makeModel(running: "abc1234", published: ("1.4.0", payload, nil))
        model.start()
        try await waitFor(interval: .milliseconds(5)) { model.latestRelease != nil && model.runningVersion != nil }

        #expect(model.updateStatus == .unknown)
        #expect(model.developmentBuild)
        #expect(!model.canDownloadUpdate)

        // The offer stays refused even if the button is somehow reached.
        model.downloadUpdate()
        #expect(model.downloadState == .idle)
        #expect(model.phase == .idle, "the manual Files path is the only way in for a dev build")
    }

    /// What makes a build undecidable is the version it reports, not whether a manifest exists.
    @Test func namesADevelopmentBuildEvenBeforeAnythingIsPublished() async throws {
        let (model, _, fetcher, _) = makeModel(running: "abc1234")
        fetcher.stub(UpdateChecker.manifestURL, status: 404)
        model.start()
        try await waitFor(interval: .milliseconds(5)) { model.lastCheckedAt != nil && model.runningVersion != nil }

        #expect(model.latestRelease == nil)
        #expect(model.updateStatus == .unknown)
        #expect(model.developmentBuild)
        #expect(!model.canDownloadUpdate)
    }

    @Test func aSilentDeviceIsNotADevelopmentBuild() {
        let (model, _, _, _) = makeModel()
        #expect(model.runningVersion == nil)
        #expect(!model.hasUpdateAnswer)
        #expect(!model.developmentBuild, "no answer yet is not the same as an unreadable answer")
    }

    @Test func saysAheadRatherThanOfferingADowngrade() async throws {
        let payload = container(version: "1.4.0")
        let (model, _, _, _) = makeModel(running: "1.5.0", published: ("1.4.0", payload, nil))
        model.start()
        try await waitFor(interval: .milliseconds(5)) { model.latestRelease != nil && model.runningVersion != nil }

        #expect(model.updateStatus == .ahead)
        #expect(!model.canDownloadUpdate)
    }

    @Test func saysNothingLoudWhenNothingIsPublished() async throws {
        let (model, _, fetcher, _) = makeModel()
        fetcher.stub(UpdateChecker.manifestURL, status: 404)
        model.start()
        try await waitFor(interval: .milliseconds(5)) { model.lastCheckedAt != nil }

        #expect(model.updateStatus == .noRelease)
        #expect(model.latestRelease == nil)
        #expect(model.checkState == .idle)
    }

    @Test func quietlyConfirmsAnUpToDateDevice() async throws {
        let payload = container(version: "1.4.0")
        let (model, _, _, _) = makeModel(running: "1.4.0+deadbee", published: ("1.4.0", payload, nil))
        model.start()
        try await waitFor(interval: .milliseconds(5)) { model.latestRelease != nil && model.runningVersion != nil }

        #expect(model.updateStatus == .current)
        #expect(!model.canDownloadUpdate)
    }

    // MARK: Download & Install

    @Test func downloadsVerifiesStagesAndSends() async throws {
        let payload = container(version: "1.4.0")
        let (model, _, _, _) = makeModel(
            published: ("1.4.0", payload, "https://example.com/notes")
        )
        model.start()
        try await waitFor(interval: .milliseconds(5)) { model.canDownloadUpdate && model.connection == .connected }
        #expect(model.releaseNotesURL?.absoluteString == "https://example.com/notes")

        model.downloadUpdate()
        #expect(model.downloadState == .downloading)

        // The verified container goes through the same staging gate a picked file does. The
        // on-glass confirm is still the only thing that installs anything.
        try await waitFor(interval: .milliseconds(5)) { model.phase == .transferring }
        #expect(model.staged?.version == "1.4.0")
        #expect(model.progress.total == payload.count)
        #expect(model.downloadState == .idle)
        #expect(model.importError == nil)
    }

    @Test func staysStagedWhenTheLinkIsDown() async throws {
        let payload = container(version: "1.4.0")
        let (model, transport, _, _) = makeModel(published: ("1.4.0", payload, nil))
        model.start()
        try await waitFor(interval: .milliseconds(5)) { model.canDownloadUpdate }
        transport.push(.outOfRange)
        try await waitFor(interval: .milliseconds(5)) { model.connection == .outOfRange }

        model.downloadUpdate()
        try await waitFor(interval: .milliseconds(5)) { model.phase == .staged }
        #expect(model.phase == .staged, "the file waits, validated, for the link to come back")
        #expect(!model.canSend)
    }

    @Test func refusesADownloadThatDoesNotMatchTheManifest() async throws {
        let payload = container(version: "1.4.0")
        let (model, _, fetcher, _) = makeModel(published: ("1.4.0", payload, nil))
        model.start()
        try await waitFor(interval: .milliseconds(5)) { model.canDownloadUpdate }

        // The server hands back something else entirely (a redirect page, a truncated object).
        fetcher.stub(Self.containerURL, body: Data(repeating: 0x7F, count: payload.count))

        model.downloadUpdate()
        try await waitFor(interval: .milliseconds(5)) { model.downloadState != .downloading }
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

    /// The download path has no privileged way past `stage(_:)`.
    @Test func aVerifiedDownloadThatIsNotAnUpdateStillFailsInTheStager() async throws {
        let payload = Data(repeating: 0x42, count: 200)
        let (model, _, _, _) = makeModel(published: ("1.4.0", payload, nil))
        model.start()
        try await waitFor(interval: .milliseconds(5)) { model.canDownloadUpdate }

        model.downloadUpdate()
        try await waitFor(interval: .milliseconds(5)) { model.importError != nil }
        #expect(model.importError != nil)
        #expect(model.staged == nil)
        #expect(model.phase == .idle)
    }

    // MARK: The dev switch

    @Test func thePreReleaseSwitchReAsksOnTheOtherChannel() async throws {
        let stable = container(version: "1.4.0")
        let (model, _, fetcher, store) = makeModel(published: ("1.4.0", stable, nil))
        let rc = container(version: "1.5.0-rc1")
        fetcher.stub(
            UpdateChecker.prereleaseManifestURL,
            body: manifest(version: "1.5.0-rc1", payload: rc)
        )
        model.start()
        try await waitFor(interval: .milliseconds(5)) { model.latestRelease?.version == "1.4.0" }
        #expect(!model.includePrereleases)

        model.setIncludePrereleases(true)
        try await waitFor(interval: .milliseconds(5)) { model.latestRelease?.version == "1.5.0-rc1" }
        #expect(model.includePrereleases)
        #expect(store.loadIncludePrereleases())
    }

    // MARK: Lifecycle

    @Test func aWiringWithoutACheckerIsTheOldFilesOnlyScreen() async throws {
        let model = FirmwareUpdateModel(transport: StubTransport(), deviceName: "Trailhead")
        model.start()
        try await waitFor(interval: .milliseconds(5)) { model.runningVersion != nil }

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

/// A minimal link and update stub: a controllable link, a settable running version, and an inert
/// firmware transfer.
private final class StubTransport: DeviceLink, DeviceUpdates, @unchecked Sendable {
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

    func connect() async throws {}
    func disconnect() async {}
}
