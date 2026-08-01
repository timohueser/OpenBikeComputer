import Foundation
import Testing
import OBCDomain
import OBCTransport
@testable import OBCUI

/// The S7 firmware-update view-model state machine, driven against a hand-built
/// transfer stub so every transition is deterministic: idle → staged →
/// transferring → awaiting-confirm → done, plus the failure branches (bad import,
/// dropped transfer, and each non-`accepted` `installFw` reply).
@MainActor
struct FirmwareUpdateModelTests {
    // MARK: Helpers

    /// A valid OBCU **v2** container tagged with `version` — both CRCs correct and the
    /// signature marker set, so `StagedFirmware.validate` accepts it and reports
    /// `version`. The trailer is a stand-in: the app never verifies signatures (the key
    /// lives in the firmware — `OBCU_Spec.md` §1.4), it only has to carry them.
    private func container(version: String, imageLen: Int = 96) -> Data {
        var image = Data()
        image.append(contentsOf: le32(0x2002_0000)) // plausible initial SP
        image.append(contentsOf: (4..<imageLen).map { UInt8($0 & 0xFF) })
        var header = Data(count: 64)
        header.replaceSubrange(0..<4, with: Array("OBCU".utf8))
        header[4] = 1 // header_version LE — still 1 in a v2 container (§1.2)
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

    /// Spin the run loop until `condition` holds or the timeout elapses (the model
    /// advances on `AsyncStream` / `await` hops, not synchronously).
    private func waitFor(_ condition: () -> Bool, within timeout: Duration = .seconds(2)) async {
        let deadline = ContinuousClock.now + timeout
        while ContinuousClock.now < deadline {
            if condition() { return }
            try? await Task.sleep(for: .milliseconds(5))
        }
    }

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

    @Test func runsIdleToStagedToTransferringToAwaitingToDone() async {
        let stub = StubTransport()
        stub.fwVersion = "0.4.2"
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead")
        model.start()
        await waitFor { model.connection == .connected }

        // idle → staged
        model.stage(container(version: "0.5.0"))
        #expect(model.phase == .staged)

        // staged → transferring
        model.send()
        #expect(model.phase == .transferring)

        // transferring → (commit) → installFw accepted → awaiting-confirm
        stub.installResult = .accepted
        stub.completeUpload()
        await waitFor { model.phase == .awaitingConfirm }
        #expect(model.phase == .awaitingConfirm)

        // awaiting-confirm: the device reboots (drop) then reconnects on the new
        // version → done.
        stub.push(.outOfRange)
        stub.fwVersion = "0.5.0"
        stub.push(.connected)
        await waitFor { model.phase == .done }
        #expect(model.phase == .done)
        #expect(model.runningVersion == "0.5.0")
    }

    @Test func reconnectingOnTheOldVersionStaysAwaiting() async {
        let stub = StubTransport()
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead")
        model.start()
        model.stage(container(version: "0.5.0"))
        model.send()
        stub.completeUpload()
        await waitFor { model.phase == .awaitingConfirm }

        // A reconnect that still reports the OLD version isn't "done".
        stub.push(.outOfRange)
        stub.fwVersion = "0.4.2"
        stub.push(.connected)
        await waitFor({ model.phase == .done }, within: .milliseconds(300))
        #expect(model.phase == .awaitingConfirm)
    }

    // MARK: Failure branches

    @Test func aDroppedTransferInterruptsThenResumes() async {
        let stub = StubTransport()
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead")
        model.start()
        model.stage(container(version: "0.5.0"))
        model.send()
        #expect(model.phase == .transferring)

        stub.push(.outOfRange) // link drops mid-transfer
        await waitFor { model.phase == .interrupted }
        #expect(model.phase == .interrupted)

        model.resume()
        #expect(model.phase == .transferring)
    }

    /// The tick and link-state watchers drain two independent streams, so under
    /// scheduler load a pre-drop progress tick can be *delivered* after the drop
    /// event. A stale tick must not read as "moving again" — it would resurrect
    /// `.transferring` for a transfer whose link is already gone, hiding the
    /// Resume affordance and wedging the sheet at a frozen percentage (the
    /// parked transfer emits nothing further). Same race as the route-upload
    /// sheet's `staleTickDeliveredAfterTheDropDoesNotReclaim`.
    @Test func staleTickDeliveredAfterTheDropDoesNotResurrectTransferring() async {
        let stub = StubTransport()
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead")
        model.start()
        await waitFor { model.connection == .connected }
        model.stage(container(version: "0.5.0"))
        model.send()
        #expect(model.phase == .transferring)

        // A live tick moves the bar (and proves the tick watcher is consuming).
        stub.tick(TransferProgress(bytesDone: 10, total: 160))
        await waitFor { model.progress.bytesDone == 10 }

        // The link drops — the transfer parks behind Resume.
        stub.push(.outOfRange)
        await waitFor { model.phase == .interrupted }

        // A tick that was in flight before the drop lands late. Sequencing it
        // after `.interrupted` reproduces deterministically what scheduler load
        // produces by starving the MainActor.
        stub.tick(TransferProgress(bytesDone: 20, total: 160))
        await waitFor { model.progress.bytesDone == 20 }
        #expect(model.phase == .interrupted, "a stale pre-drop tick must not resurrect .transferring")
    }

    @Test func aFailedTransferShowsAFailureSentence() async {
        let stub = StubTransport()
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead")
        model.start()
        model.stage(container(version: "0.5.0"))
        model.send()
        stub.failUpload(.transferRejected)
        await waitFor { model.phase == .failed }
        #expect(model.phase == .failed)
        #expect(model.failureMessage?.isEmpty == false)
    }

    @Test(arguments: [
        (FirmwareInstallResult.busy, "ride"),
        (.noStaged, "send it again"),
        (.rejected, "rejected"),
        (.unsupported, "Bluetooth"),
    ])
    func aNonAcceptedInstallReplyFailsWithMappedCopy(reply: FirmwareInstallResult, needle: String) async {
        let stub = StubTransport()
        stub.installResult = reply
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead")
        model.start()
        model.stage(container(version: "0.5.0"))
        model.send()
        stub.completeUpload()
        await waitFor { model.phase == .failed }
        #expect(model.phase == .failed)
        #expect(model.failureMessage?.contains(needle) == true)
    }

    // MARK: The #459/#754 ledger claim

    @Test func firmwareSendClaimsWhileTransferringAndReleasesOnCommit() async {
        let activity = TransferActivity()
        let stub = StubTransport()
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead", activity: activity)
        model.start()
        model.stage(container(version: "0.5.0"))
        #expect(!activity.isActive, "staged but not sending — no claim yet")

        model.send()
        #expect(activity.isActive, "the claim opens with the transfer")

        // Commit → installFw accepted → `.awaitingConfirm`: the byte-moving
        // phase is over, so the claim releases (the on-glass confirm + reboot
        // isn't a transfer the drain/idle-timer should wait on).
        stub.completeUpload()
        await waitFor { model.phase == .awaitingConfirm }
        #expect(!activity.isActive)
    }

    @Test func firmwareInterruptReleasesTheClaim() async {
        let activity = TransferActivity()
        let stub = StubTransport()
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead", activity: activity)
        model.start()
        model.stage(container(version: "0.5.0"))
        model.send()
        #expect(activity.isActive)

        // A drop leaves it stalled-resumable — NOT in flight; the background
        // drain must not wait on a transfer whose link is already gone.
        stub.push(.outOfRange)
        await waitFor { model.phase == .interrupted }
        #expect(!activity.isActive)

        // Resume re-claims for the fresh attempt.
        model.resume()
        #expect(activity.isActive)
    }

    @Test func firmwareFailureReleasesTheClaim() async {
        let activity = TransferActivity()
        let stub = StubTransport()
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead", activity: activity)
        model.start()
        model.stage(container(version: "0.5.0"))
        model.send()
        #expect(activity.isActive)

        stub.failUpload(.transferRejected)
        await waitFor { model.phase == .failed }
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

        // The screen popped mid-send — the claim must not leak (a `@MainActor`
        // deinit can't reach the actor-isolated ledger, so `stop()` owns it),
        // and the phase settles back to `.staged` (still validated, ready to
        // re-send) — not a frozen `.transferring` on a dead handle.
        model.stop()
        #expect(!activity.isActive)
        #expect(model.phase == .staged)
    }

    // MARK: stop()/start() re-entrancy

    @Test func startAfterStopResubscribesTheLinkState() async {
        let stub = StubTransport()
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead")
        model.start()
        await waitFor { model.connection == .connected }

        // An onDisappear→onAppear cycle on a persisting model (a presentation
        // pushed over the screen, a scene re-attach): the pair must be
        // re-entrant, or the model comes back with a dead subscription and
        // `connection`/`canSend` freeze.
        model.stop()
        model.start()

        stub.push(.outOfRange)
        await waitFor { model.connection == .outOfRange }
        #expect(model.connection == .outOfRange)

        stub.push(.connected)
        await waitFor { model.connection == .connected }
        model.stage(container(version: "0.5.0"))
        #expect(model.canSend, "a restarted model is fully live again")
    }

    @Test func startIsIdempotentWhileRunning() async {
        let activity = TransferActivity()
        let stub = StubTransport()
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead", activity: activity)
        model.start()
        model.start()  // second call while live must be a no-op
        model.stage(container(version: "0.5.0"))
        model.send()
        #expect(activity.isActive)
        stub.completeUpload()
        await waitFor { model.phase == .awaitingConfirm }
        #expect(!activity.isActive, "no doubled subscription/claim from the repeat start")
    }

    @Test func canRetryAfterAFailedInstall() async {
        let stub = StubTransport()
        stub.installResult = .busy
        let model = FirmwareUpdateModel(transport: stub, deviceName: "Trailhead")
        model.start()
        model.stage(container(version: "0.5.0"))
        model.send()
        stub.completeUpload()
        await waitFor { model.phase == .failed }

        // The file is still staged — a retry re-enters transferring.
        stub.installResult = .accepted
        model.send()
        #expect(model.phase == .transferring)
        stub.completeUpload()
        await waitFor { model.phase == .awaitingConfirm }
        #expect(model.phase == .awaitingConfirm)
    }
}

/// A minimal `DeviceTransport` for the model tests: a controllable link-state
/// stream, a settable running version, and a firmware transfer whose completion /
/// failure and `installFw` reply the test drives. Everything else is an inert stub.
private final class StubTransport: DeviceTransport, @unchecked Sendable {
    /// Last-value multicast, like the real transports (`AsyncMulticast`): every
    /// `state` access is a fresh subscription that replays the latest value —
    /// what lets the model's re-entrant `stop()`/`start()` re-subscribe (a
    /// single shared `AsyncStream` dies with its first canceled consumer).
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

    // MARK: DeviceTransport — the bits the model uses

    var state: AsyncStream<ConnectionState> {
        AsyncStream { cont in
            cont.yield(lastState)  // replay the latest, then live updates
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

    // MARK: DeviceTransport — inert stubs

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
