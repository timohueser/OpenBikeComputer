import XCTest
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// #361 — the H3 rename's self-heal. The rename is optimistic; when its config
/// write never lands, the bond record still carries the desired name and this
/// pass pushes it back on the next connect. Exercised against a call-counting
/// spy for the exact read/write discipline (once per pass, silent skips, no-op
/// on match / no bond).
final class DeviceNameReconcilerTests: XCTestCase {
    private func makeReconciler(
        config: DeviceConfig,
        bond: BondRecord?
    ) -> (DeviceNameReconciler, ConfigSpyTransport, RecordingBondStore) {
        let transport = ConfigSpyTransport(config: config)
        let bondStore = RecordingBondStore(bond)
        return (
            DeviceNameReconciler(transport: transport, bondStore: bondStore),
            transport, bondStore
        )
    }

    /// Happy path (the common case: the rename's own write landed) — one read
    /// to compare, **no** write.
    func testMatchingNamesReadOnceAndNeverWrite() async {
        let (reconciler, transport, _) = makeReconciler(
            config: DeviceConfig(name: "Trailhead"),
            bond: BondRecord(deviceName: "Trailhead")
        )
        await reconciler.reconcile()

        XCTAssertEqual(transport.readConfigCalls, 1)
        XCTAssertTrue(transport.writtenConfigs.isEmpty, "a matching name must not rewrite")
    }

    /// The heal: a diverged config gets the bond name pushed back — via
    /// read-modify-write, so the other config fields survive.
    func testDivergedNameWritesTheBondNamePreservingOtherFields() async {
        let (reconciler, transport, _) = makeReconciler(
            config: DeviceConfig(name: "Trailhead", units: .imperial),
            bond: BondRecord(deviceName: "Summit")
        )
        await reconciler.reconcile()

        XCTAssertEqual(
            transport.writtenConfigs,
            [DeviceConfig(name: "Summit", units: .imperial)],
            "exactly one write: the bond name over the read config"
        )
    }

    /// `forget()` cleared the bond → the pass must no-op (not even a read).
    func testNoBondRecordIsACompleteNoOp() async {
        let (reconciler, transport, _) = makeReconciler(
            config: DeviceConfig(name: "Trailhead"),
            bond: nil
        )
        await reconciler.reconcile()

        XCTAssertEqual(transport.readConfigCalls, 0)
        XCTAssertTrue(transport.writtenConfigs.isEmpty)
    }

    /// A failed `readConfig` is a silent skip — no write this pass; the
    /// following connect's pass converges.
    func testReadFailureSkipsSilentlyAndTheNextPassConverges() async {
        let (reconciler, transport, _) = makeReconciler(
            config: DeviceConfig(name: "Trailhead"),
            bond: BondRecord(deviceName: "Summit")
        )
        transport.failNextRead()
        await reconciler.reconcile()
        XCTAssertTrue(transport.writtenConfigs.isEmpty, "a failed read must not guess a write")

        await reconciler.reconcile()  // "the next connect"
        XCTAssertEqual(transport.config.name, "Summit")
    }

    /// A failed `writeConfig` is equally silent — never a hot retry within the
    /// pass; the following connect's pass converges.
    func testWriteFailureRetriesOnlyOnTheNextPass() async {
        let (reconciler, transport, _) = makeReconciler(
            config: DeviceConfig(name: "Trailhead"),
            bond: BondRecord(deviceName: "Summit")
        )
        transport.failNextWrite()
        await reconciler.reconcile()
        XCTAssertEqual(transport.config.name, "Trailhead", "the rejected write must not land")
        XCTAssertEqual(transport.writeConfigCalls, 1, "one attempt per pass — no hot retry")

        await reconciler.reconcile()  // "the next connect"
        XCTAssertEqual(transport.config.name, "Summit")
    }
}

// MARK: - Test doubles (shared with SettingsModelTests / MainScreenModelTests)

/// Call-counting `DeviceTransport` stand-in for the config plane: serves one
/// config, records successful writes, and can fail either leg once. Everything
/// else is inert (the reconciler must never touch it).
final class ConfigSpyTransport: DeviceTransport, @unchecked Sendable {
    private let lock = NSLock()
    private var _config: DeviceConfig
    private var _readConfigCalls = 0
    private var _writeConfigCalls = 0
    private var _writtenConfigs: [DeviceConfig] = []
    private var _failNextRead = false
    private var _failNextWrite = false

    init(config: DeviceConfig) {
        _config = config
    }

    var config: DeviceConfig { lock.withLock { _config } }
    var readConfigCalls: Int { lock.withLock { _readConfigCalls } }
    var writeConfigCalls: Int { lock.withLock { _writeConfigCalls } }
    /// Configs that **landed** (a failed write records the attempt count only).
    var writtenConfigs: [DeviceConfig] { lock.withLock { _writtenConfigs } }
    func failNextRead() { lock.withLock { _failNextRead = true } }
    func failNextWrite() { lock.withLock { _failNextWrite = true } }

    func readConfig() async throws -> DeviceConfig {
        try lock.withLock {
            _readConfigCalls += 1
            if _failNextRead {
                _failNextRead = false
                throw DeviceError.readFailed
            }
            return _config
        }
    }

    func writeConfig(_ config: DeviceConfig) async throws {
        try lock.withLock {
            _writeConfigCalls += 1
            if _failNextWrite {
                _failNextWrite = false
                throw DeviceError.writeFailed
            }
            _config = config
            _writtenConfigs.append(config)
        }
    }

    // Inert remainder — a connected link, nothing stored.
    var state: AsyncStream<ConnectionState> {
        AsyncStream { $0.yield(.connected); $0.finish() }
    }
    var battery: AsyncStream<Int> { AsyncStream { $0.finish() } }
    func connect() async throws {}
    func disconnect() async {}
    func deviceInfo() async throws -> DeviceInfo {
        DeviceInfo(name: config.name, firmwareVersion: "0.0.0")
    }
    func listRoutes() async throws -> [RouteCatalogEntry] { [] }
    func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail { throw DeviceError.readFailed }
    func uploadRoute(_ route: RouteBlob) -> TransferHandle {
        .immediatelyFinished(.failed(.notConnected))
    }
    func deleteRoute(_ id: DeviceObjectID) async throws {}
    func listRides() async throws -> [RideSummary] { [] }
    func rideDetail(_ id: RideID) async throws -> RideDetail { throw DeviceError.readFailed }
    func downloadRides(_ ids: [RideID]) -> RideDownload { .finished() }
    func readDiagnostics() async throws -> Data { Data() }
}

/// In-memory `BondStore` with real save/load semantics — unlike `MockBondStore`
/// it belongs to no `MockControl`, so tests can pair it with the spy transport.
final class RecordingBondStore: BondStore, @unchecked Sendable {
    private let lock = NSLock()
    private var _record: BondRecord?

    init(_ record: BondRecord? = nil) {
        _record = record
    }

    func load() -> BondRecord? { lock.withLock { _record } }
    func save(_ record: BondRecord) { lock.withLock { _record = record } }
    func clear() { lock.withLock { _record = nil } }
}
