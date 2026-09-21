import Foundation
import Testing
import OBCDomain
import OBCTransport
@testable import OBCUI

/// The firmware-update view-model state machine, driven against a hand-built transfer stub so
/// every transition is deterministic.
@MainActor
struct FirmwareUpdateModelTests {
    // MARK: Helpers

    /// A valid OBCU v2 container tagged with `version`: both CRCs correct and the signature
    /// marker set. The trailer is a stand-in; the app carries signatures but never verifies them.
    private func container(version: String, imageLen: Int = 96) -> Data {
        var image = Data()
        image.append(contentsOf: le32(0x2002_0000)) // plausible initial SP
        image.append(contentsOf: (4..<imageLen).map { UInt8($0 & 0xFF) })
        var header = Data(count: 64)
        header.replaceSubrange(0..<4, with: Array("OBCU".utf8))
        header[4] = 1 // header_version, still 1 in a v2 container
        header.replaceSubrange(8..<12, with: le32(UInt32(image.count)))
        header.replaceSubrange(12..<16, with: le32(CRC32.checksum(image)))
        let v = Array(version.utf8.prefix(32))
        header.replaceSubrange(16..<16 + v.count, with: v)
        header.replaceSubrange(48..<50, with: le16(1)) // sig_scheme = Ed25519
        header.replaceSubrange(50..<52, with: le16(64)) // sig_len
        header.replaceSubrange(60..<64, with: le32(CRC32.checksum(header[0..<60])))
        return header + image + Data(repeating: 0x5A, count: 64)
    }

    private func le16(_ v: UInt16) -> [UInt8] {
        withUnsafeBytes(of: v.littleEndian, Array.init)
    }

    private func le32(_ v: UInt32) -> [UInt8] { withUnsafeBytes(of: v.littleEndian, Array.init) }

    // MARK: Import

    @Test func staysIdleAndSurfacesAnAlertForABadFile() {
        let model = FirmwareUpdateModel(transport: StubTransport(), deviceName: "Trailhead")
        model.stage(Data([0x01, 0x02, 0x03]))
        #expect(model.phase == .idle)
        #expect(model.staged == nil)
        #expect(model.importError != nil)
    }

    @Test func stagesAValidFile() {
        let model = FirmwareUpdateModel(transport: StubTransport(), deviceName: "Trailhead")
        model.stage(container(version: "1.2.0"))
        #expect(model.phase == .staged)
        #expect(model.staged?.version == "1.2.0")
        #expect(model.importError == nil)
    }

    // MARK: Happy path

    @Test func runsIdleToStagedToTransferringToAwaitingToDone() async throws {
        let stub = StubTransport()
        stub.fwVersion = "0.4.2"
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead")
        model.start()
        try await waitFor(interval: .milliseconds(5)) { model.connection == .connected }

        model.stage(container(version: "0.5.0"))
        #expect(model.phase == .staged)

        model.send()
        #expect(model.phase == .transferring)

        stub.installResult = .accepted
        stub.completeUpload()
        try await waitFor(interval: .milliseconds(5)) { model.phase == .awaitingConfirm }
        #expect(model.phase == .awaitingConfirm)

        // The device reboots (a drop), then reconnects on the new version.
        stub.push(.outOfRange)
        stub.fwVersion = "0.5.0"
        stub.push(.connected)
        try await waitFor(interval: .milliseconds(5)) { model.phase == .done }
        #expect(model.phase == .done)
        #expect(model.runningVersion == "0.5.0")
    }

    @Test func reconnectingOnTheOldVersionStaysAwaiting() async throws {
        let stub = StubTransport()
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead")
        model.start()
        model.stage(container(version: "0.5.0"))
        model.send()
        stub.completeUpload()
        try await waitFor(interval: .milliseconds(5)) { model.phase == .awaitingConfirm }

        // The negative case: `.done` must stay away for the whole window, so the window
        // elapsing is the pass, not a timeout.
        stub.push(.outOfRange)
        stub.fwVersion = "0.4.2"
        stub.push(.connected)
        #expect(
            await neverHolds({ model.phase == .done }, for: .milliseconds(300)),
            "a reconnect still reporting the old version must never read as done"
        )
        #expect(model.phase == .awaitingConfirm)
    }

    // MARK: Failure branches

    @Test func aDroppedTransferInterruptsThenResumes() async throws {
        let stub = StubTransport()
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead")
        model.start()
        model.stage(container(version: "0.5.0"))
        model.send()
        #expect(model.phase == .transferring)

        stub.push(.outOfRange)
        try await waitFor(interval: .milliseconds(5)) { model.phase == .interrupted }
        #expect(model.phase == .interrupted)

        model.resume()
        #expect(model.phase == .transferring)
    }

    /// The tick and link-state watchers drain two independent streams, so a pre-drop progress
    /// tick can arrive after the drop event. A stale tick must not resurrect `.transferring`:
    /// that hides the Resume affordance and wedges the sheet at a frozen percentage.
    @Test func staleTickDeliveredAfterTheDropDoesNotResurrectTransferring() async throws {
        let stub = StubTransport()
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead")
        model.start()
        try await waitFor(interval: .milliseconds(5)) { model.connection == .connected }
        model.stage(container(version: "0.5.0"))
        model.send()
        #expect(model.phase == .transferring)

        // A live tick moves the bar and proves the tick watcher is consuming.
        stub.tick(TransferProgress(bytesDone: 10, total: 160))
        try await waitFor(interval: .milliseconds(5)) { model.progress.bytesDone == 10 }

        stub.push(.outOfRange)
        try await waitFor(interval: .milliseconds(5)) { model.phase == .interrupted }

        // A tick in flight before the drop lands late. Sequencing it after `.interrupted` makes
        // the race deterministic.
        stub.tick(TransferProgress(bytesDone: 20, total: 160))
        try await waitFor(interval: .milliseconds(5)) { model.progress.bytesDone == 20 }
        #expect(model.phase == .interrupted, "a stale pre-drop tick must not resurrect .transferring")
    }

    @Test func aFailedTransferShowsAFailureSentence() async throws {
        let stub = StubTransport()
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead")
        model.start()
        model.stage(container(version: "0.5.0"))
        model.send()
        stub.failUpload(.transferRejected)
        try await waitFor(interval: .milliseconds(5)) { model.phase == .failed }
        #expect(model.phase == .failed)
        #expect(model.failureMessage?.isEmpty == false)
    }

    @Test(arguments: [
        (FirmwareInstallResult.busy, "ride"),
        (.noStaged, "send it again"),
        (.rejected, "rejected"),
        (.unsupported, "Bluetooth"),
    ])
    func aNonAcceptedInstallReplyFailsWithMappedCopy(reply: FirmwareInstallResult, needle: String) async throws {
        let stub = StubTransport()
        stub.installResult = reply
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead")
        model.start()
        model.stage(container(version: "0.5.0"))
        model.send()
        stub.completeUpload()
        try await waitFor(interval: .milliseconds(5)) { model.phase == .failed }
        #expect(model.phase == .failed)
        #expect(model.failureMessage?.contains(needle) == true)
    }

    // MARK: The ledger claim

    @Test func firmwareSendClaimsWhileTransferringAndReleasesOnCommit() async throws {
        let activity = TransferActivity()
        let stub = StubTransport()
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead", activity: activity)
        model.start()
        model.stage(container(version: "0.5.0"))
        #expect(!activity.isActive, "staged but not sending — no claim yet")

        model.send()
        #expect(activity.isActive, "the claim opens with the transfer")

        // The byte-moving phase is over at `.awaitingConfirm`, so the claim releases: the
        // on-glass confirm and the reboot are not transfers the drain timer waits on.
        stub.completeUpload()
        try await waitFor(interval: .milliseconds(5)) { model.phase == .awaitingConfirm }
        #expect(!activity.isActive)
    }

    @Test func firmwareInterruptReleasesTheClaim() async throws {
        let activity = TransferActivity()
        let stub = StubTransport()
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead", activity: activity)
        model.start()
        model.stage(container(version: "0.5.0"))
        model.send()
        #expect(activity.isActive)

        // A drop leaves the transfer stalled-resumable, not in flight: the drain must not wait
        // on a transfer whose link is already gone.
        stub.push(.outOfRange)
        try await waitFor(interval: .milliseconds(5)) { model.phase == .interrupted }
        #expect(!activity.isActive)

        model.resume()
        #expect(activity.isActive)
    }

    @Test func firmwareFailureReleasesTheClaim() async throws {
        let activity = TransferActivity()
        let stub = StubTransport()
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead", activity: activity)
        model.start()
        model.stage(container(version: "0.5.0"))
        model.send()
        #expect(activity.isActive)

        stub.failUpload(.transferRejected)
        try await waitFor(interval: .milliseconds(5)) { model.phase == .failed }
        #expect(!activity.isActive)
    }

    @Test func firmwareStopReleasesTheClaim() async {
        let activity = TransferActivity()
        let stub = StubTransport()
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead", activity: activity)
        model.start()
        model.stage(container(version: "0.5.0"))
        model.send()
        #expect(activity.isActive)

        // A `@MainActor` deinit cannot reach the actor-isolated ledger, so `stop()` must release
        // the claim. The phase settles back to `.staged`: still validated, ready to re-send.
        model.stop()
        #expect(!activity.isActive)
        #expect(model.phase == .staged)
    }

    // MARK: stop()/start() re-entrancy

    @Test func startAfterStopResubscribesTheLinkState() async throws {
        let stub = StubTransport()
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead")
        model.start()
        try await waitFor(interval: .milliseconds(5)) { model.connection == .connected }

        // An onDisappear/onAppear cycle on a persisting model: the pair must be re-entrant, or
        // the model comes back with a dead subscription and `connection`/`canSend` freeze.
        model.stop()
        model.start()

        stub.push(.outOfRange)
        try await waitFor(interval: .milliseconds(5)) { model.connection == .outOfRange }
        #expect(model.connection == .outOfRange)

        stub.push(.connected)
        try await waitFor(interval: .milliseconds(5)) { model.connection == .connected }
        model.stage(container(version: "0.5.0"))
        #expect(model.canSend, "a restarted model is fully live again")
    }

    @Test func startIsIdempotentWhileRunning() async throws {
        let activity = TransferActivity()
        let stub = StubTransport()
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead", activity: activity)
        model.start()
        model.start()
        model.stage(container(version: "0.5.0"))
        model.send()
        #expect(activity.isActive)
        stub.completeUpload()
        try await waitFor(interval: .milliseconds(5)) { model.phase == .awaitingConfirm }
        #expect(!activity.isActive, "no doubled subscription/claim from the repeat start")
    }

    @Test func canRetryAfterAFailedInstall() async throws {
        let stub = StubTransport()
        stub.installResult = .busy
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead")
        model.start()
        model.stage(container(version: "0.5.0"))
        model.send()
        stub.completeUpload()
        try await waitFor(interval: .milliseconds(5)) { model.phase == .failed }

        stub.installResult = .accepted
        model.send()
        #expect(model.phase == .transferring)
        stub.completeUpload()
        try await waitFor(interval: .milliseconds(5)) { model.phase == .awaitingConfirm }
        #expect(model.phase == .awaitingConfirm)
    }
}

/// A controllable link and update stub: a link-state stream, a settable running version, and a
/// firmware transfer whose completion, failure and `installFw` reply the test drives.
private final class StubTransport: DeviceLink, DeviceUpdates, @unchecked Sendable {
    /// Every `state` access is a fresh subscription that replays the latest value, like the real
    /// transports. One shared `AsyncStream` would die with its first canceled consumer.
    private var stateConts: [AsyncStream<ConnectionState>.Continuation] = []
    private var lastState: ConnectionState = .connected
    var fwVersion = "0.4.2"
    var installResult: FirmwareInstallResult = .accepted
    var installError: DeviceError?

    private var uploadProgress: AsyncStream<TransferProgress>.Continuation?
    private var uploadOutcome = AsyncPromise<TransferOutcome>()

    func push(_ state: ConnectionState) {
        lastState = state
        stateConts.forEach { $0.yield(state) }
    }

    func tick(_ progress: TransferProgress) { uploadProgress?.yield(progress) }

    func completeUpload() {
        uploadProgress?.finish()
        uploadOutcome.fulfill(.completed)
    }

    func failUpload(_ error: DeviceError) {
        uploadProgress?.finish()
        uploadOutcome.fulfill(.failed(error))
    }

    // MARK: DeviceLink + DeviceUpdates

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

    func installFirmware() async throws -> FirmwareInstallResult {
        if let installError { throw installError }
        return installResult
    }

    func connect() async throws {}
    func disconnect() async {}
}
