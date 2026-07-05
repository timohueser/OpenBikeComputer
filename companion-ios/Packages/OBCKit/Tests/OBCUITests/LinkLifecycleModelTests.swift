import Testing
import Foundation
import SwiftUI
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// #459 acceptance: the foreground-only link policy. A spy transport records
/// exactly which lifecycle seam each transition used — the suspend must go
/// through `suspendLink()` (drop **and pause the reconnect loop**) and the
/// foreground return through `resumeLink()` (the bonded silent-reconnect
/// path), never a bare `disconnect()`/`connect()`.
@MainActor
struct LinkLifecycleModelTests {
    private func make() -> (LinkLifecycleModel, SpyTransport, GraceSpy, TransferActivity) {
        let transport = SpyTransport()
        let grace = GraceSpy()
        let activity = TransferActivity()
        let model = LinkLifecycleModel(
            transport: transport, activity: activity, backgroundTasks: grace)
        return (model, transport, grace, activity)
    }

    /// Poll until `condition` holds (the model moves on free-running tasks).
    private func eventually(
        _ what: String,
        timeout: Duration = .seconds(30),
        _ condition: @MainActor () -> Bool
    ) async {
        let deadline = ContinuousClock.now.advanced(by: timeout)
        while !condition() {
            if ContinuousClock.now > deadline {
                Issue.record("timed out waiting for \(what)")
                return
            }
            try? await Task.sleep(for: .milliseconds(10))
        }
    }

    /// A beat for negative assertions ("nothing further happens").
    private func settle() async {
        try? await Task.sleep(for: .milliseconds(120))
    }

    private func startConnected(
        _ model: LinkLifecycleModel, _ transport: SpyTransport
    ) async {
        transport.setState(.connected)
        model.start()
        await eventually("link mirror") { model.connection == .connected }
    }

    // MARK: DoD — background mid-transfer drains, then disconnects

    @Test func backgroundMidTransferDrainsThenSuspends() async {
        let (model, transport, grace, activity) = make()
        await startConnected(model, transport)

        let token = activity.begin()
        model.scenePhaseChanged(to: .inactive)
        model.scenePhaseChanged(to: .background)
        #expect(model.phase == .draining)
        #expect(grace.begun.count == 1, "the drain runs under a system grace window")

        await settle()
        #expect(transport.count("suspendLink") == 0, "an in-flight transfer is never dropped")

        activity.end(token)
        await eventually("suspend after the drain") { transport.count("suspendLink") == 1 }
        #expect(model.phase == .suspended)
        await eventually("grace window returned") { grace.ended == grace.begun }
        #expect(transport.count("disconnect") == 0, "the suspend uses the pausing seam, not a bare disconnect")
    }

    // MARK: DoD — background while idle disconnects promptly

    @Test func backgroundIdleSuspendsPromptly() async {
        let (model, transport, grace, _) = make()
        await startConnected(model, transport)

        model.scenePhaseChanged(to: .inactive)
        model.scenePhaseChanged(to: .background)
        await eventually("prompt suspend") { transport.count("suspendLink") == 1 }
        #expect(model.phase == .suspended)
        await eventually("grace window returned") { grace.ended == grace.begun }
    }

    // MARK: DoD — foreground reconnects via the bonded silent-reconnect path

    @Test func foregroundResumesViaBondedSilentReconnect() async {
        let (model, transport, _, _) = make()
        await startConnected(model, transport)
        model.scenePhaseChanged(to: .background)
        await eventually("suspended") { model.phase == .suspended }

        model.scenePhaseChanged(to: .active)
        await eventually("resume") { transport.count("resumeLink") == 1 }
        #expect(model.phase == .foreground)
        #expect(transport.count("connect") == 0, "never the pairing-capable connect path")
    }

    /// The reconnect's `.disconnected → .connected` edge drives the existing
    /// `MainScreenModel` reload — the mechanism that trues up anything that
    /// changed on the device while the app was backgrounded (`storeChanged`
    /// is not surfaced by the transport even in the foreground; freshness
    /// comes from this reload and from explicit Sync).
    @Test func foregroundReconnectTriggersMainScreenReload() async {
        let (model, transport, _, _) = make()
        let main = MainScreenModel(transport: transport)
        main.start()
        await startConnected(model, transport)

        model.scenePhaseChanged(to: .background)
        await eventually("suspended") { model.phase == .suspended }
        let baseline = transport.count("listRoutes")

        model.scenePhaseChanged(to: .active)
        await eventually("reload on the reconnect edge") { transport.count("listRoutes") > baseline }
    }

    // MARK: DoD — inactive flickers never churn the link

    @Test func inactiveFlickerNeverChurnsTheLink() async {
        let (model, transport, grace, _) = make()
        await startConnected(model, transport)

        model.scenePhaseChanged(to: .inactive)
        model.scenePhaseChanged(to: .active)
        await settle()
        #expect(transport.count("suspendLink") == 0)
        #expect(transport.count("disconnect") == 0)
        #expect(model.phase == .foreground)
        #expect(grace.begun.isEmpty, "a flicker must not even open a grace window")
    }

    // MARK: DoD — the reconnect stays paused while backgrounded

    @Test func reconnectStaysPausedWhileBackgrounded() async {
        let (model, transport, _, _) = make()
        await startConnected(model, transport)
        model.scenePhaseChanged(to: .background)
        await eventually("suspended") { model.phase == .suspended }

        // However long the app sits in the background, nothing re-raises the
        // link: the suspend went through `suspendLink()` (whose contract is
        // drop + pause the transport's own reconnect loop) and the model never
        // resumes without a foreground transition.
        await settle()
        #expect(transport.count("suspendLink") == 1)
        #expect(transport.count("disconnect") == 0)
        #expect(transport.count("resumeLink") == 0)
        #expect(transport.count("connect") == 0)

        model.scenePhaseChanged(to: .active)
        await eventually("resume on foreground only") { transport.count("resumeLink") == 1 }
    }

    // MARK: A quick return mid-drain keeps the link up

    @Test func foregroundDuringDrainKeepsTheLink() async {
        let (model, transport, grace, activity) = make()
        await startConnected(model, transport)

        let token = activity.begin()
        model.scenePhaseChanged(to: .background)
        #expect(model.phase == .draining)

        model.scenePhaseChanged(to: .active)
        #expect(model.phase == .foreground)
        await eventually("grace window returned") { grace.ended == grace.begun }

        // The transfer finishing later must not fire the canceled suspend.
        activity.end(token)
        await settle()
        #expect(transport.count("suspendLink") == 0, "the link never dropped")
        #expect(transport.count("resumeLink") == 0, "nothing to resume")
    }

    // MARK: Grace expiry forces the disconnect

    @Test func graceExpiryForcesTheSuspend() async {
        let (model, transport, grace, activity) = make()
        await startConnected(model, transport)

        let token = activity.begin()
        model.scenePhaseChanged(to: .background)
        #expect(model.phase == .draining)

        grace.fireExpiry()
        await eventually("forced suspend") { transport.count("suspendLink") == 1 }
        #expect(model.phase == .suspended)
        #expect(grace.ended == grace.begun, "the expired window is given back at once")

        // The stalled transfer resumes its story after the foreground
        // reconnect (upload sheet / H10 banner); the link itself comes back.
        model.scenePhaseChanged(to: .active)
        await eventually("resume after a forced suspend") { transport.count("resumeLink") == 1 }
        activity.end(token)
        await settle()
        #expect(transport.count("suspendLink") == 1, "the late drain must not re-suspend")
    }

    // MARK: A never-connected session must not start scanning

    @Test func neverConnectedSessionNeverResumes() async {
        let (model, transport, _, _) = make()
        model.start()  // state stays .disconnected — a pair-intro session

        model.scenePhaseChanged(to: .background)
        await eventually("suspended") { model.phase == .suspended }
        model.scenePhaseChanged(to: .active)
        await settle()
        #expect(
            transport.count("resumeLink") == 0,
            "no link existed at suspend time — checking a text must not start a scan")
        #expect(transport.count("connect") == 0)
    }

    // MARK: The mock transport's default seam round-trips

    @Test func mockTransportSuspendResumeRoundTrip() async throws {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        let transport = MockTransport(control: control)
        try await transport.connect()
        #expect(control.connection == .connected)

        await transport.suspendLink()
        #expect(control.connection == .disconnected)

        await transport.resumeLink()
        #expect(control.connection == .connected, "the default resume replays the silent connect")
    }
}

// MARK: - Spies

/// Records which lifecycle seam each call used; state is a hand-driven
/// replay-latest stream, like the real transport's.
private final class SpyTransport: DeviceTransport, @unchecked Sendable {
    private let stateMulticast = AsyncMulticast<ConnectionState>(.disconnected)
    private let lock = NSLock()
    private var callLog: [String] = []

    func count(_ name: String) -> Int {
        lock.lock()
        defer { lock.unlock() }
        return callLog.filter { $0 == name }.count
    }

    private func record(_ name: String) {
        lock.lock()
        callLog.append(name)
        lock.unlock()
    }

    func setState(_ state: ConnectionState) { stateMulticast.send(state) }

    var state: AsyncStream<ConnectionState> { stateMulticast.stream() }
    var battery: AsyncStream<Int> { AsyncStream { $0.finish() } }

    func connect() async throws {
        record("connect")
        stateMulticast.send(.connected)
    }

    func disconnect() async {
        record("disconnect")
        stateMulticast.send(.disconnected)
    }

    func suspendLink() async {
        record("suspendLink")
        stateMulticast.send(.disconnected)
    }

    func resumeLink() async {
        record("resumeLink")
        stateMulticast.send(.connected)
    }

    func deviceInfo() async throws -> DeviceInfo { throw DeviceError.notConnected }
    func readConfig() async throws -> DeviceConfig { throw DeviceError.notConnected }
    func writeConfig(_ config: DeviceConfig) async throws { throw DeviceError.notConnected }

    func listRoutes() async throws -> [RouteCatalogEntry] {
        record("listRoutes")
        return []
    }

    func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail { throw DeviceError.readFailed }
    func uploadRoute(_ route: RouteBlob) -> TransferHandle { .immediatelyFinished(.failed(.notConnected)) }
    func deleteRoute(_ id: DeviceObjectID) async throws { throw DeviceError.notConnected }
    func listRides() async throws -> [RideSummary] { [] }
    func rideDetail(_ id: RideID) async throws -> RideDetail { throw DeviceError.readFailed }
    func downloadRides(_ ids: [RideID]) -> RideDownload { .finished(.failed(.notConnected)) }
    func readDiagnostics() async throws -> Data { throw DeviceError.notConnected }
}

/// A hand-fired `BackgroundTaskRunner`: records the begin/end pairing and lets
/// a test fire the system expiry.
@MainActor
private final class GraceSpy: BackgroundTaskRunner {
    private(set) var begun: [Int] = []
    private(set) var ended: [Int] = []
    private var expiryHandlers: [Int: @MainActor @Sendable () -> Void] = [:]
    private var nextID = 1

    nonisolated init() {}

    func begin(
        name: String,
        onExpiry: @escaping @MainActor @Sendable () -> Void
    ) -> BackgroundGraceToken? {
        let id = nextID
        nextID += 1
        begun.append(id)
        expiryHandlers[id] = onExpiry
        return BackgroundGraceToken(rawValue: id)
    }

    func end(_ token: BackgroundGraceToken) {
        ended.append(token.rawValue)
        expiryHandlers[token.rawValue] = nil
    }

    func fireExpiry() {
        for handler in expiryHandlers.values { handler() }
    }
}
