import Testing
import Foundation
import SwiftUI
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// The foreground-only link policy. A spy transport records which lifecycle seam each transition
/// used: the suspend must go through `suspendLink()` (drop the link and pause the reconnect loop)
/// and the foreground return through `resumeLink()`, never a bare `disconnect()` or `connect()`.
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

    private func startConnected(
        _ model: LinkLifecycleModel, _ transport: SpyTransport
    ) async throws {
        transport.setState(.connected)
        model.start()
        try await waitFor("link mirror") { model.connection == .connected }
    }

    // MARK: Background mid-transfer drains, then suspends

    @Test func backgroundMidTransferDrainsThenSuspends() async throws {
        let (model, transport, grace, activity) = make()
        try await startConnected(model, transport)

        let token = activity.begin()
        model.scenePhaseChanged(to: .inactive)
        model.scenePhaseChanged(to: .background)
        #expect(model.phase == .draining)
        #expect(grace.begun.count == 1, "the drain runs under a system grace window")

        let stayedConnected = await neverHolds({
            transport.count("suspendLink") > 0
        }, for: .milliseconds(120))
        #expect(stayedConnected, "an in-flight transfer is never dropped")
        #expect(transport.count("suspendLink") == 0, "an in-flight transfer is never dropped")

        activity.end(token)
        try await waitFor("suspend after the drain") { transport.count("suspendLink") == 1 }
        #expect(model.phase == .suspended)
        try await waitFor("grace window returned") { grace.ended == grace.begun }
        #expect(transport.count("disconnect") == 0, "the suspend uses the pausing seam, not a bare disconnect")
    }

    // MARK: Background while idle suspends promptly

    @Test func backgroundIdleSuspendsPromptly() async throws {
        let (model, transport, grace, _) = make()
        try await startConnected(model, transport)

        model.scenePhaseChanged(to: .inactive)
        model.scenePhaseChanged(to: .background)
        try await waitFor("prompt suspend") { transport.count("suspendLink") == 1 }
        #expect(model.phase == .suspended)
        try await waitFor("grace window returned") { grace.ended == grace.begun }
    }

    // MARK: Foreground reconnects via the bonded silent-reconnect path

    @Test func foregroundResumesViaBondedSilentReconnect() async throws {
        let (model, transport, _, _) = make()
        try await startConnected(model, transport)
        model.scenePhaseChanged(to: .background)
        try await waitFor("suspended") { model.phase == .suspended }

        model.scenePhaseChanged(to: .active)
        try await waitFor("resume") { transport.count("resumeLink") == 1 }
        #expect(model.phase == .foreground)
        #expect(transport.count("connect") == 0, "never the pairing-capable connect path")
    }

    /// The reconnect edge drives the `MainScreenModel` reload: that is what trues up whatever
    /// changed on the device while the app was backgrounded. Freshness comes from this reload and
    /// from an explicit Sync.
    @Test func foregroundReconnectTriggersMainScreenReload() async throws {
        let (model, transport, _, _) = make()
        let main = MainScreenModel(transport: transport)
        main.start()
        try await startConnected(model, transport)

        model.scenePhaseChanged(to: .background)
        try await waitFor("suspended") { model.phase == .suspended }
        let baseline = transport.count("listRoutes")

        model.scenePhaseChanged(to: .active)
        try await waitFor("reload on the reconnect edge") { transport.count("listRoutes") > baseline }
    }

    // MARK: Inactive flickers never churn the link

    @Test func inactiveFlickerNeverChurnsTheLink() async throws {
        let (model, transport, grace, _) = make()
        try await startConnected(model, transport)

        model.scenePhaseChanged(to: .inactive)
        model.scenePhaseChanged(to: .active)
        let stayedUp = await neverHolds({
            transport.count("suspendLink") > 0 || transport.count("disconnect") > 0
                || !grace.begun.isEmpty
        }, for: .milliseconds(120))
        #expect(stayedUp, "an inactive flicker must not churn the link")
        #expect(transport.count("suspendLink") == 0)
        #expect(transport.count("disconnect") == 0)
        #expect(model.phase == .foreground)
        #expect(grace.begun.isEmpty, "a flicker must not even open a grace window")
    }

    // MARK: The reconnect stays paused while backgrounded

    @Test func reconnectStaysPausedWhileBackgrounded() async throws {
        let (model, transport, _, _) = make()
        try await startConnected(model, transport)
        model.scenePhaseChanged(to: .background)
        try await waitFor("suspended") { model.phase == .suspended }

        // Nothing re-raises the link: `suspendLink()` also pauses the transport's own reconnect
        // loop, and the model never resumes without a foreground transition.
        let stayedPaused = await neverHolds({
            transport.count("resumeLink") > 0 || transport.count("connect") > 0
        }, for: .milliseconds(120))
        #expect(stayedPaused, "the backgrounded model must not raise the link")
        #expect(transport.count("suspendLink") == 1)
        #expect(transport.count("disconnect") == 0)
        #expect(transport.count("resumeLink") == 0)
        #expect(transport.count("connect") == 0)

        model.scenePhaseChanged(to: .active)
        try await waitFor("resume on foreground only") { transport.count("resumeLink") == 1 }
    }

    // MARK: A quick return mid-drain keeps the link up

    @Test func foregroundDuringDrainKeepsTheLink() async throws {
        let (model, transport, grace, activity) = make()
        try await startConnected(model, transport)

        let token = activity.begin()
        model.scenePhaseChanged(to: .background)
        #expect(model.phase == .draining)

        model.scenePhaseChanged(to: .active)
        #expect(model.phase == .foreground)
        try await waitFor("grace window returned") { grace.ended == grace.begun }

        // The transfer finishing later must not fire the canceled suspend.
        activity.end(token)
        let stayedConnected = await neverHolds({
            transport.count("suspendLink") > 0 || transport.count("resumeLink") > 0
        }, for: .milliseconds(120))
        #expect(stayedConnected, "the canceled drain must not act after foregrounding")
        #expect(transport.count("suspendLink") == 0, "the link never dropped")
        #expect(transport.count("resumeLink") == 0, "nothing to resume")
    }

    // MARK: Grace expiry forces the disconnect

    @Test func graceExpiryForcesTheSuspend() async throws {
        let (model, transport, grace, activity) = make()
        try await startConnected(model, transport)

        let token = activity.begin()
        model.scenePhaseChanged(to: .background)
        #expect(model.phase == .draining)

        grace.fireExpiry()
        try await waitFor("forced suspend") { transport.count("suspendLink") == 1 }
        #expect(model.phase == .suspended)
        #expect(grace.ended == grace.begun, "the expired window is given back at once")

        // The stalled transfer resumes its story after the foreground reconnect; the link comes back.
        model.scenePhaseChanged(to: .active)
        try await waitFor("resume after a forced suspend") { transport.count("resumeLink") == 1 }
        activity.end(token)
        let stayedOnce = await neverHolds({
            transport.count("suspendLink") > 1
        }, for: .milliseconds(120))
        #expect(stayedOnce, "the late drain must not suspend twice")
        #expect(transport.count("suspendLink") == 1, "the late drain must not re-suspend")
    }

    // MARK: A never-connected session must not start scanning

    @Test func neverConnectedSessionNeverResumes() async throws {
        let (model, transport, _, _) = make()
        model.start()  // state stays .disconnected — a pair-intro session

        model.scenePhaseChanged(to: .background)
        try await waitFor("suspended") { model.phase == .suspended }
        model.scenePhaseChanged(to: .active)
        let stayedIdle = await neverHolds({
            transport.count("resumeLink") > 0 || transport.count("connect") > 0
        }, for: .milliseconds(120))
        #expect(stayedIdle, "a never-connected session must not start a scan")
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
private final class SpyTransport: DeviceLink, DeviceBattery, DeviceObjects, DeviceClock,
    @unchecked Sendable {
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
    func listRoutes() async throws -> [RouteCatalogEntry] {
        record("listRoutes")
        return []
    }

    func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail { throw DeviceError.readFailed }
    func uploadRoute(_ route: RouteBlob) -> TransferHandle { .immediatelyFinished(.failed(.notConnected)) }
    func deleteRoute(_ id: DeviceObjectID) async throws { throw DeviceError.notConnected }
    func listRides() async throws -> RideCatalog { RideCatalog(rides: []) }
    func downloadRides(_ ids: [RideID]) -> RideDownload { .finished(.failed(.notConnected)) }
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
