#if canImport(CoreBluetooth)
@preconcurrency import CoreBluetooth
import Foundation
import OBCDomain
import OBCProtocolV4

/// The real `DeviceTransport`: it scans for the OBC service, connects, discovers the services,
/// reads the PSM and opens the L2CAP CoC, then maps the protocol onto GATT reads, writes and
/// notifies plus the `BLEChannel` byte layer.
///
/// Ids on this transport's data plane are device-namespace (`DeviceObjectID`). Library ids never
/// cross this boundary; a persisted `deviceObjectID` is the link to a device copy.
///
/// Protocol operation state lives in the one `TransferClient`. All mutable state here is confined
/// to the serial `queue`, which is the CoreBluetooth callback queue: async methods hop onto it and
/// register continuations that the delegate callbacks resolve. That confinement is why this can be
/// a plain `@unchecked Sendable` class instead of a fight with `Sendable` on CoreBluetooth's own
/// object graph.
///
/// CoreBluetooth delivers every delegate callback on that queue.
public final class BLETransport: NSObject, DeviceTransport, @unchecked Sendable {
    private let queue = DispatchQueue(label: "com.openbikecomputer.ble")
    private lazy var central = CBCentralManager(
        delegate: self,
        queue: queue
    )
    private let discoveryStore: any BLEDiscoveryStore
    private var discoveryPolicy = BLEDiscoveryIntentPolicy()

    private let stateMulticast = AsyncMulticast<ConnectionState>(.disconnected)
    /// `nil` until the first real BAS value — the seed must not replay as "0%".
    private let batteryMulticast = AsyncMulticast<Int?>(nil)

    private var peripheral: CBPeripheral?
    private var characteristics: [CBUUID: CBCharacteristic] = [:]
    /// The live CoC byte pipe. The `BLEChannel` wrapper is rebuilt around it on every open.
    private var byteChannel: L2CAPByteChannel?
    private var bleChannel: BLEChannel?
    private lazy var transferClient = TransferClient(link: self)
    private var openingChannel = false
    private var channelWaiters: [CheckedContinuation<BLEChannel, Error>] = []

    // Watchdogs for the connect and CoC-open phases, which can stall silently: an empty or
    // partial GATT DB never fires `didDiscoverCharacteristicsFor`, and a PSM read that never
    // yields `didOpen` leaves `openingChannel` latched with every transfer parked. Each phase
    // arms a one-shot on entry and disarms it on the resolving callback.
    private var discoveryWatchdog: DispatchWorkItem?
    private var channelWatchdog: DispatchWorkItem?
    private static let phaseTimeout: DispatchTimeInterval = .seconds(10)
    /// The channel watchdog's budget across the gated PSM read. On a fresh pair iOS holds that
    /// read pending under the system passkey sheet while the rider reads the code off the device
    /// and types it, which is human-paced. Once the read resolves the watchdog re-arms at
    /// `phaseTimeout` for the machine-only openL2CAPChannel tail.
    private static let pairingTimeout: DispatchTimeInterval = .seconds(90)
    /// Imperative command acknowledgements are tiny and immediate. Bound the wait so a dropped
    /// `status` notification cannot hold the command lane forever. A timeout invalidates that lane
    /// until reconnect because command results have no exchange id that could reject a late reply.
    private static let commandResultTimeout: DispatchTimeInterval = .seconds(3)
    // Outstanding operations, touched only on `queue`. Connecting is two phases: `discover()`
    // un-gated, then `authenticate()` gated, which raises the passkey sheet.
    private var discoverContinuation: CheckedContinuation<Void, Error>?
    private var authenticateContinuation: CheckedContinuation<Void, Error>?

    /// True only across the gated-phase retry beat, where `authenticateContinuation` is
    /// momentarily nil. A disconnect in this window is terminal: kicking the reconnect loop
    /// could re-raise the passkey sheet.
    private var awaitingGatedRetry = false
    /// The beat before the one gated-phase retry: long enough for the firmware's post-pairing
    /// window to drain, short enough to stay imperceptible.
    private static let gatedRetryBeat: Duration = .milliseconds(500)
    /// Services still awaiting their characteristics during `discover()`; discovery
    /// is done (the un-gated surface is ready) when it reaches zero.
    private var pendingServiceDiscovery = 0
    private var pendingReads: [CBUUID: [CheckedContinuation<Data, Error>]] = [:]
    private var pendingWrites: [CBUUID: [CheckedContinuation<Void, Error>]] = [:]

    private struct CommandSlotWaiter {
        let token: UUID
        let continuation: CheckedContinuation<UUID, Never>
    }
    private struct CommandResultWaiter {
        let token: UUID
        let command: UInt8
        let timeout: DispatchWorkItem
        let continuation: CheckedContinuation<CommandResult, Error>
    }
    private var commandSlotOwner: UUID?
    private var commandSlotWaiters: [CommandSlotWaiter] = []
    private var commandResultWaiter: CommandResultWaiter?
    private var commandResults = CommandResultCorrelation()
    private var statusNotificationWaiters: [CheckedContinuation<Void, Error>] = []
    private var statusNotificationTimeout: DispatchWorkItem?

    // Physical indication records for protocol v4. Correlation, operation lifetime and STATUS
    // reconciliation belong to TransferClient; the transport only preserves received records.
    private var pendingObjectControlRecords: [Data] = []
    private var objectControlWaiters: [CheckedContinuation<Data, Error>] = []
    /// A control receive was abandoned before it parked — the next one resolves as cancelled
    /// instead of waiting for an answer its request will never get.
    private var objectControlReceiveCancelled = false

    public override convenience init() {
        self.init(discoveryStore: UserDefaultsBLEDiscoveryStore())
    }

    init(discoveryStore: any BLEDiscoveryStore) {
        self.discoveryStore = discoveryStore
        super.init()

        _ = central  // force manager creation (and a state callback)
    }

    // MARK: DeviceTransport — lifecycle

    public var state: AsyncStream<ConnectionState> { stateMulticast.stream() }
    public var battery: AsyncStream<Int> {
        // Drop the not-yet-known seed: subscribers get the first real reading, never a 0%.
        let source = batteryMulticast.stream()
        return AsyncStream { continuation in
            let pump = Task {
                for await value in source {
                    if let value { continuation.yield(value) }
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in pump.cancel() }
        }
    }

    public var catalogChanges: AsyncStream<CatalogChange> {
        AsyncStream { $0.finish() }
    }

    public func connect() async throws {
        // The two phases back to back. A bonded reconnect raises no sheet, because iOS
        // re-encrypts from the stored keys; a fresh pair does, which is why the launch flow
        // calls the phases separately.
        try await discover()
        try await authenticate()
    }

    public func discover() async throws {
        // Phase 1: scan, connect, and discover the services and the un-gated characteristics
        // only. Resolves once every service's characteristics are in hand, so `deviceInfo()` can
        // read them, and never touches a gated characteristic.
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            queue.async { [self] in
                discoverContinuation = cont
                discoveryPolicy.requestForeground()
                startConnectIfReady()
            }
        }
    }

    public func authenticate() async throws {
        // Phase 2: the gated ops (subscribe the `status` notify, read the PSM, open the CoC)
        // that establish the encrypted, LESC-authenticated link and raise the system passkey
        // sheet. Resolves when the CoC opens.
        //
        // On a fresh pair the gated phase can fail once inside the firmware's post-pairing
        // window although both sides bonded, so retry it once on the now-bonded link. Only an
        // auth-class failure while connected is retried; a decline, a link drop or a CoC failure
        // is terminal.
        do {
            try await GatedPhaseRetry.runOnce(
                beat: Self.gatedRetryBeat,
                isRetryable: { $0 is GatedPairingWindowError },
                attempt: { [self] in try await runGatedPhaseOnce() }
            )
        } catch is GatedPairingWindowError {
            // The retry also hit the pairing window. The retryable resolve left the link and the
            // intent up for it, so tear them down here and surface the error.
            await teardownAfterFailedRetry()
            throw DeviceError.pairingFailed
        }
        // A terminal `DeviceError` already tore the intent down and is rethrown straight through.
    }

    /// One gated-phase attempt: park the authenticate continuation and kick `beginAuthenticate()`.
    /// Throws `GatedPairingWindowError` on a retryable failure, a `DeviceError` on a terminal one.
    private func runGatedPhaseOnce() async throws {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            queue.async { [self] in
                // This attempt is now live; from here `authenticateContinuation`
                // (not `awaitingGatedRetry`) owns drop handling.
                awaitingGatedRetry = false
                authenticateContinuation = cont
                beginAuthenticate()
            }
        }
    }

    public func disconnect() async {
        await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
            queue.async { [self] in
                let cancelForegroundConnection = discoveryPolicy.cancelForeground()
                if cancelForegroundConnection, let peripheral { central.cancelPeripheralConnection(peripheral) }
                if central.isScanning, !discoveryPolicy.hasIntent { central.stopScan() }
                stateMulticast.send(.disconnected)
                if discoveryPolicy.hasIntent { startConnectIfReady() }
                cont.resume()
            }
        }
    }

    // `suspendLink()` uses the protocol default, `disconnect()`, which is already the full
    // suspend: it drops the foreground intent latch that every reconnect path checks, cancels
    // the pending connect iOS holds, and stops the scan. While that latch is down, nothing in
    // the delegate flow re-raises the link.

    public func resumeLink() async {
        // Re-arm the intent latch and let the existing delegate flow re-raise the link: the same
        // bonded silent-reconnect path a mid-ride drop takes, with no passkey sheet. Deliberately
        // not `connect()`, which parks fresh discover and authenticate continuations and would
        // clobber any still waiting from an interrupted launch attempt.
        await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
            queue.async { [self] in
                discoveryPolicy.requestForeground()
                startConnectIfReady()
                cont.resume()
            }
        }
    }

    // MARK: DeviceTransport — control plane

    public func deviceInfo() async throws -> DeviceInfo {
        async let fw = readString(GATT.firmwareRevision)
        async let hw = readString(GATT.hardwareRevision)
        async let serial = readString(GATT.serialNumber)
        // BLE v4 exposes only the two-byte wire version. Store identity comes from LIST.
        let versionData = try await read(GATT.protocolVersion)
        guard versionData.count == 2 else { throw DeviceError.readFailed }
        let b = versionData.startIndex
        let version = UInt16(versionData[b]) | (UInt16(versionData[b + 1]) << 8)
        let name = await currentPeripheralName() ?? "OBC"
        let serialValue = try await serial
        let storeID: String?
        if version == OBCProtocol.version {
            storeID = try await transferClient.storeID().description
        } else {
            storeID = nil
        }
        let info = DeviceInfo(
            name: name, firmwareVersion: try await fw, hardwareVersion: try await hw,
            serial: serialValue, protocolVersion: version, storeID: storeID
        )
        return info
    }

    public func readConfig() async throws -> DeviceConfig {
        try ConfigObjectCodec.decode(try await read(GATT.config))
    }

    public func writeConfig(_ config: DeviceConfig) async throws {
        try await write(ConfigObjectCodec.encode(config), to: GATT.config)
    }

    public func readDiagnostics() async throws -> Data {
        // Protocol v4 has no diagnostics kind, so this surface stays fail-closed.
        throw DeviceError.readFailed
    }

    public func deleteRoute(_ id: DeviceObjectID) async throws {
        try await remove(id)
    }

    public func setClock(_ sample: WallClockSample) async throws -> ClockSyncOutcome {
        switch try await exchangeCommand(
            SetClockCommand.encode(sample), command: SetClockCommand.commandByte
        ).status {
        case .ok: .stamped
        case .unknownCommand: .unsupported
        case .notFound, .busy, .error: throw DeviceError.writeFailed
        }
    }
    public func listRoutes() async throws -> [RouteCatalogEntry] {
        // This catalog is reconcile-only: identity and CRC are its proof, and it never feeds
        // route rows. LIST carries both. Downloading every object to fill the display fields
        // would be an N+1 transfer storm and would block a foreground PUT behind the operation gate.
        try await headEntries(kind: .route).map { entry in
            RouteCatalogEntry(
                id: DeviceObjectID(entry.objectID.rawValue), name: entry.displayName,
                distanceMeters: 0, elevationGainMeters: 0,
                pointCount: 0, crc32: entry.payloadCRC32)
        }
    }

    public func listRides() async throws -> RideCatalog {
        // LIST is the catalog and its StoreId scopes every ride id minted here.
        let catalog: (storeID: StoreID, entries: [CatalogEntry])
        do { catalog = try await transferClient.catalog(kind: .ride) }
        catch { throw deviceError(for: error) }
        let scope = LibraryScope(
            serial: try await readString(GATT.serialNumber), storeID: catalog.storeID.description)
        var rides: [RideSummary] = []
        for entry in catalog.entries where !entry.flags.contains(.retained)
            && !entry.flags.contains(.reserved) && !entry.flags.contains(.recording) {
            let id = RideID(
                deviceObjectID: DeviceObjectID(entry.objectID.rawValue), scope: scope)
            // The ride footer is not frozen yet, so the fielded decoder stays behind the GET path.
            let source = RideSource(storeID: catalog.storeID.description,
                                    objectID: entry.objectID.rawValue, revision: entry.revision.rawValue,
                                    payloadLength: entry.payloadLength, payloadCRC32: entry.payloadCRC32)
            let downloaded = try await downloadRide(id: id, source: source)
            var summary = try RideObjectCodec.decode(downloaded.payload, id: id).summary
            summary.source = downloaded.source
            rides.append(summary)
        }
        return RideCatalog(rides: rides, hiddenRideCount: 0)
    }

    public func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail {
        // The stored route blob, decoded app-side for the waypoints and the elevation profile.
        // Header totals are exact; the profile and max grade come from the stored geometry.
        let decoded = try RouteObjectCodec.decode(try await download(id))
        let geometry = RouteStats.compute(from: decoded.points)
        // A device-stored object has no library identity, so the summary rides under a
        // placeholder id that nothing keys on.
        let summary = RouteSummary(
            id: RouteID("device-\(id.raw)"),
            name: decoded.name,
            distanceMeters: Double(decoded.totalDistanceMeters),
            elevationGainMeters: Double(decoded.totalAscentMeters),
            estimatedDuration: geometry.estimatedDuration,
            pointCount: decoded.points.count,
            trackPreview: TrackPreview.normalizing(decoded.points.map(\.coordinate))
        )
        return RouteDetail(
            summary: summary,
            waypoints: decoded.waypoints,
            elevationProfile: geometry.elevationProfile,
            maxGradePercent: geometry.maxGradePercent
        )
    }

    public func rideDetail(_ id: RideID) async throws -> RideDetail {
        // The synced library copy answers this screen; nothing reads a ride detail from a device.
        throw DeviceError.readFailed
    }

    public func listTrips() async throws -> [TripCatalogEntry] {
        // Badge and reconcile input, like routes. Stage details are fetched only by a download.
        try await headEntries(kind: .trip).map { entry in
            TripCatalogEntry(
                id: DeviceObjectID(entry.objectID.rawValue), name: entry.displayName,
                distanceMeters: 0, elevationGainMeters: 0,
                stageCount: 0, crc32: entry.payloadCRC32)
        }
    }

    public func downloadTrip(_ id: DeviceObjectID) async throws -> TripObjectCodec.Decoded {
        // The stored trip blob, decoded app-side for its name and stage ids. Reconcile falls back
        // to it only when the trip catalog's CRC cannot confirm the fingerprint.
        try TripObjectCodec.decode(try await download(id))
    }

    public func deleteTrip(_ id: DeviceObjectID) async throws {
        try await remove(id)
    }

    private func headEntries(kind: ObjectKind) async throws -> [CatalogEntry] {
        do {
            return try await transferClient.list(kind: kind).filter {
                !$0.flags.contains(.retained) && !$0.flags.contains(.reserved)
            }
        } catch { throw deviceError(for: error) }
    }

    /// One object's head revision, in one request. STATUS reports the current head whatever
    /// revision the request names, and zero is the one value the request cannot carry. A catalog
    /// LIST is the wrong instrument: it pages about two entries per round trip to answer one.
    private func headRevision(of id: DeviceObjectID) async throws -> Revision {
        do {
            let result = try await transferClient.status(
                objectID: ObjectID(rawValue: id.raw), revision: Revision(rawValue: 1))
            guard result.state != .absent else { throw DeviceError.readFailed }
            return result.headRevision
        } catch { throw deviceError(for: error) }
    }

    public func confirmRideArchive(_ receipt: RideArchiveReceipt) async throws -> RideArchiveConfirmation {
        let source = receipt.source
        do {
            try Task.checkCancellation()
            let storeID = try await transferClient.storeID()
            try Task.checkCancellation()
            guard storeID.description == source.storeID else { return .sourceUnavailable }
            _ = try await transferClient.archiveRide(
                storeID: storeID, objectID: ObjectID(rawValue: source.objectID),
                revision: Revision(rawValue: source.revision),
                payloadLength: source.payloadLength, payloadCRC32: source.payloadCRC32)
            return .confirmed
        } catch is CancellationError {
            throw CancellationError()
        } catch TransferClientError.storeChanged {
            return .sourceUnavailable
        } catch WireError.remote(let error) {
            switch error.code {
            case .unsupported: return .unsupported
            case .invalidRequest, .notFound, .revisionConflict: return .sourceUnavailable
            case .readOnly, .noSpace: return .refused
            default: throw DeviceError.writeFailed
            }
        } catch {
            try Task.checkCancellation()
            throw deviceError(for: error)
        }
    }

    fileprivate func downloadRide(id: RideID, source: RideSource) async throws -> DownloadedRide {
        do {
            guard source.matches(id) else { throw DeviceError.readFailed }
            let storeID = try await transferClient.storeID()
            guard storeID.description == source.storeID else { throw DeviceError.readFailed }
            let downloaded = try await transferClient.get(
                objectID: ObjectID(rawValue: source.objectID),
                revision: Revision(rawValue: source.revision), expectedStoreID: storeID)
            guard downloaded.result.payloadLength == source.payloadLength,
                  downloaded.result.payloadCRC32 == source.payloadCRC32 else {
                throw DeviceError.readFailed
            }
            let verified = RideSource(
                storeID: storeID.description, objectID: source.objectID,
                revision: downloaded.result.revision.rawValue,
                payloadLength: downloaded.result.payloadLength,
                payloadCRC32: downloaded.result.payloadCRC32)
            return DownloadedRide(id: id, payload: downloaded.payload, source: verified)
        } catch { throw deviceError(for: error) }
    }

    /// GET with no revision returns the head, so reading one object by id needs no revision read.
    fileprivate func download(_ id: DeviceObjectID) async throws -> Data {
        do {
            return try await transferClient.get(objectID: ObjectID(rawValue: id.raw)).payload
        } catch { throw deviceError(for: error) }
    }

    private func remove(_ id: DeviceObjectID) async throws {
        let revision = try await headRevision(of: id)
        do { _ = try await transferClient.remove(objectID: ObjectID(rawValue: id.raw), expectedRevision: revision) }
        catch { throw deviceError(for: error) }
    }

    private func deviceError(for error: Error) -> DeviceError {
        if let error = error as? DeviceError { return error }
        if error is TransferLinkLost { return .transferDropped }
        if let error = error as? TransferClientError {
            switch error {
            case .checksumMismatch: return .crcMismatch
            case .storeChanged, .outcomeNotCommitted: return .transferDropped
            default: return .transferRejected
            }
        }
        if let wire = error as? WireError, case .remote(let body) = wire {
            switch body.code {
            case .noSpace: return .storageFull
            case .checksumFailure: return .crcMismatch
            case .cancelled: return .transferDropped
            case .notFound: return .readFailed
            default: return .transferRejected
            }
        }
        return .transferRejected
    }

    // MARK: DeviceTransport — data plane

    /// The shared upload service for route, trip, and firmware objects. It borrows this
    /// transport's queue-confined connection/channel engine; it does not own another manager,
    /// queue, or channel.
    private enum UploadService {
        static func start(
            over transport: BLETransport, payload: Data, kind: ObjectKind,
            objectID: DeviceObjectID?, displayName: String, reportsAssignedID: Bool
        ) -> TransferHandle {
            guard !payload.isEmpty else {
                return .immediatelyFinished(.failed(.transferRejected))
            }
            let (stream, continuation) = AsyncStream<TransferProgress>.makeStream()
            let outcome = AsyncPromise<TransferOutcome>()
            let assignedID = AsyncPromise<DeviceObjectID?>()
            let runner = V4UploadRunner(
                transport: transport, payload: payload, kind: kind, objectID: objectID,
                displayName: displayName,
                progress: continuation, outcome: outcome, assignedID: assignedID
            )
            Task { await runner.start() }
            return TransferHandle(
                progress: stream, outcome: outcome,
                assignedObjectID: reportsAssignedID ? assignedID : nil,
                onCancel: { Task { await runner.cancel() } },
                onResume: {}
            )
        }
    }

    public func uploadRoute(_ route: RouteBlob) -> TransferHandle {
        // A fresh upload sends ObjectId zero and keeps the id the PUT result assigns.
        // Re-uploading an edited route names its stored id and exact revision.
        UploadService.start(
            over: self, payload: route.payload, kind: .route,
            objectID: route.targetObjectID, displayName: route.summary.name,
            reportsAssignedID: true
        )
    }

    public func uploadTrip(_ trip: TripBlob) -> TransferHandle {
        // The trip sibling of `uploadRoute`. A whole-trip push sends it last, after the stages.
        UploadService.start(
            over: self, payload: trip.payload, kind: .trip,
            objectID: trip.targetObjectID, displayName: trip.name,
            reportsAssignedID: true
        )
    }

    public func uploadFirmware(_ container: Data) -> TransferHandle {
        // The whole OBCU container is an ordinary update-kind create. Staging never installs;
        // `installFirmware` performs the separate ARM request.
        UploadService.start(
            over: self, payload: container, kind: .update,
            objectID: nil, displayName: "UPDATE.BIN", reportsAssignedID: false
        )
    }

    public func installFirmware() async throws -> FirmwareInstallResult {
        guard let package = try await headEntries(kind: .update).max(by: { $0.revision < $1.revision })
        else { return .noStaged }
        do {
            _ = try await transferClient.arm(
                packageObjectID: package.objectID, expectedRevision: package.revision)
            return .accepted
        } catch WireError.remote(let body) {
            switch body.code {
            case .notFound: return .noStaged
            case .busy: return .busy
            case .unsupported: return .unsupported
            case .rejected: return .rejected
            default: return .rejected
            }
        } catch {
            throw deviceError(for: error)
        }
    }

    fileprivate func performUpload(
        payload: Data, kind: ObjectKind, objectID: DeviceObjectID?, displayName: String,
        progress: @escaping @Sendable (TransferProgress) -> Void
    ) async throws -> DeviceObjectID {
        var target = objectID

        let expected: Revision?
        if let target {
            expected = try await headRevision(of: target)
        } else {
            expected = nil
        }
        do {
            let result = try await transferClient.put(
                payload, objectID: target.map { ObjectID(rawValue: $0.raw) },
                expectedRevision: expected, kind: kind,
                displayName: displayName
            ) { done, total in
                progress(TransferProgress(bytesDone: done, total: total))
            }
            return DeviceObjectID(result.objectID.rawValue)
        } catch { throw deviceError(for: error) }
    }

    public func forgetBond() async throws {
        // The opaque CoreBluetooth identifier is useful only while this bond is trusted. Clear it
        // even when the device command fails, so restoration cannot act on a forgotten device.
        defer {
            discoveryStore.clearKnownPeripheralID()

        }
        let result = try await exchangeCommand(
            ForgetBondCommand.encode(), command: ForgetBondCommand.commandByte
        )
        guard result.status == .ok else { throw DeviceError.writeFailed }
    }

    public func downloadRides(_ ids: [RideID]) -> RideDownload {
        // Device downloads require the exact source from the catalog.
        ids.isEmpty ? .finished() : .finished(.failed(.transferRejected))
    }

    public func downloadRides(from summaries: [RideSummary]) -> RideDownload {
        guard !summaries.isEmpty else { return .finished() }
        if stateMulticast.value == .disconnected {
            return .finished(.failed(.notConnected))
        }
        var requests: [(id: RideID, source: RideSource)] = []
        for summary in summaries {
            guard let source = summary.source, source.matches(summary.id) else {
                return .finished(.failed(.transferRejected))
            }
            requests.append((summary.id, source))
        }
        let (rideStream, rideContinuation) = AsyncThrowingStream<DownloadedRide, Error>.makeStream()
        let (progressStream, progressContinuation) = AsyncStream<TransferProgress>.makeStream()
        let outcome = AsyncPromise<TransferOutcome>()
        let runner = RideDownloadRunner(
            transport: self, requests: requests, rides: rideContinuation,
            progress: progressContinuation, outcome: outcome
        )
        Task { await runner.start() }
        let handle = TransferHandle(
            progress: progressStream, outcome: outcome,
            onCancel: { Task { await runner.cancel() } },
            onResume: {}
        )
        return RideDownload(handle: handle, rides: rideStream)
    }

    // MARK: Physical CoC access

    /// The live CoC channel, opening it if necessary.
    fileprivate func readyChannel() async throws -> BLEChannel {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<BLEChannel, Error>) in
            queue.async { [self] in
                if let bleChannel, byteChannel?.isOpen == true {
                    cont.resume(returning: bleChannel)
                    return
                }
                guard let peripheral, let psm = characteristics[GATT.psm] else {
                    cont.resume(throwing: DeviceError.notConnected)
                    return
                }
                channelWaiters.append(cont)
                if !openingChannel {
                    openingChannel = true
                    armChannelWatchdog()
                    byteChannel = nil
                    bleChannel = nil
                    peripheral.readValue(for: psm)  // → PSM update → openL2CAPChannel → didOpen
                }
            }
        }
    }

    // MARK: Connect flow (queue-confined)

    private func startConnectIfReady() {
        guard discoveryPolicy.hasIntent else { return }
        guard discoveryPolicy.phase == .scanning || discoveryPolicy.phase == .idle else { return }
        switch central.state {
        case .poweredOn:
            stateMulticast.send(.connecting)
            guard !central.isScanning else { return }
            // A known device is selected by its identifier in `discovered`.
            let scanServices: [CBUUID]? = discoveryStore.knownPeripheralID() == nil
                ? [GATT.obcControlService] : nil
            central.scanForPeripherals(withServices: scanServices)
        case .poweredOff:
            failRadioUnavailable(.bluetoothUnavailable(.poweredOff))
        case .unauthorized:
            failRadioUnavailable(.bluetoothUnavailable(.unauthorized))
        case .unsupported:
            failRadioUnavailable(.bluetoothUnavailable(.unsupported))
        default:
            break  // .resetting / .unknown → wait for the next state update
        }
    }

    private func failRadioUnavailable(_ error: DeviceError) {

        if discoverContinuation != nil {
            failDiscover(error)
        } else {
            _ = discoveryPolicy.cancelForeground()
            stateMulticast.send(.disconnected)
        }
        if central.isScanning { central.stopScan() }
    }

    /// Kick off phase 2: arm the gated notifies, then read the PSM to open the CoC. The first
    /// gated op is what raises the passkey sheet. Drives both an explicit `authenticate()` and
    /// the auto-resume after a background reconnect.
    private func beginAuthenticate() {
        guard let peripheral, let psm = characteristics[GATT.psm] else {
            failAuthenticate(.notConnected)
            return
        }
        // Protocol v4 answers every object request on the one indicated control characteristic.
        if let objectControl = characteristics[GATT.objectControl] {
            peripheral.setNotifyValue(true, for: objectControl)
        }
        // The gated retry can begin with the CoC already up: a CCCD write failed and resolved
        // attempt 1 as retryable while its PSM read stayed in flight on the serialized ATT
        // bearer, and that read then opened the channel during the retry beat. The phase's goal
        // state is reached, so resolve the parked authenticate instead of waiting for a `didOpen`
        // that already fired.
        if bleChannel != nil, byteChannel?.isOpen == true {
            finishConnect()
            return
        }
        if bleChannel == nil, !openingChannel {
            openingChannel = true
            // The gated PSM read raises the passkey sheet on a fresh pair, so with an
            // `authenticate()` parked the budget must cover the rider typing the code. A bonded
            // background reconnect re-encrypts silently and keeps the tight budget.
            armChannelWatchdog(after: authenticateContinuation != nil ? Self.pairingTimeout : Self.phaseTimeout)
            peripheral.readValue(for: psm)  // → PSM update → openL2CAPChannel → didOpen
        } else if openingChannel, channelWatchdog == nil {
            // A retryable resolve disarms the watchdog while attempt 1's PSM read keeps
            // `openingChannel` latched, because its response is still owed on the serialized ATT
            // bearer, so this entry cannot re-issue the read. Re-watch the in-flight open, or a
            // read that never resolves parks the retry forever.
            armChannelWatchdog(after: authenticateContinuation != nil ? Self.pairingTimeout : Self.phaseTimeout)
        }
    }

    /// Arm the GATT-discovery watchdog. If it fires, discovery never completed, so fail a parked
    /// `discover()` and drop the link; a bonded reconnect then retries clean.
    private func armDiscoveryWatchdog() {
        discoveryWatchdog?.cancel()
        let item = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.discoveryWatchdog = nil
            if self.discoverContinuation != nil {
                self.failDiscover(.deviceNotFound)
            }
            if let peripheral = self.peripheral { self.central.cancelPeripheralConnection(peripheral) }
        }
        discoveryWatchdog = item
        queue.asyncAfter(deadline: .now() + Self.phaseTimeout, execute: item)
    }

    private func disarmDiscoveryWatchdog() {
        discoveryWatchdog?.cancel()
        discoveryWatchdog = nil
    }

    /// Arm the CoC-open watchdog. If it fires, the open stalled, so clear the latch, fail the
    /// parked opens and unwind a pending authenticate; the next transfer re-opens from scratch.
    /// `timeout` is `phaseTimeout` except across a fresh pair's PSM read, where the passkey sheet
    /// makes the phase human-paced.
    private func armChannelWatchdog(after timeout: DispatchTimeInterval = BLETransport.phaseTimeout) {
        channelWatchdog?.cancel()
        let item = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.channelWatchdog = nil
            self.openingChannel = false
            let waiters = self.channelWaiters
            self.channelWaiters.removeAll()
            for cont in waiters { cont.resume(throwing: DeviceError.channelOpenFailed) }
            if self.authenticateContinuation != nil { self.failAuthenticate(.channelOpenFailed) }
        }
        channelWatchdog = item
        queue.asyncAfter(deadline: .now() + timeout, execute: item)
    }

    private func disarmChannelWatchdog() {
        channelWatchdog?.cancel()
        channelWatchdog = nil
    }

    /// Phase 1 failed (radio, scan, GATT discovery) — the link never came up.
    private func failDiscover(_ error: DeviceError) {
        disarmDiscoveryWatchdog()
        _ = discoveryPolicy.cancelForeground()
        stateMulticast.send(.disconnected)
        discoverContinuation?.resume(throwing: error)
        discoverContinuation = nil
    }

    private func failConnectionSetup() {
        if discoverContinuation != nil {
            failDiscover(.notConnected)
        }
        if let peripheral { central.cancelPeripheralConnection(peripheral) }
    }

    /// Phase 2 failed (declined passkey, refused encryption, CoC open). Tear the intent down so a
    /// background reconnect does not spin on a bond that will not take.
    private func failAuthenticate(_ error: DeviceError) {
        disarmChannelWatchdog()
        awaitingGatedRetry = false
        _ = discoveryPolicy.cancelForeground()
        stateMulticast.send(.disconnected)
        authenticateContinuation?.resume(throwing: error)
        authenticateContinuation = nil
    }

    /// Resolve a parked fresh-pair `authenticate()` as retryable, so it runs the gated phase once
    /// more on this same bonded link. Unlike `failAuthenticate` it leaves the intent and the live
    /// link alone and publishes no `.disconnected`, and it flags the beat with `awaitingGatedRetry`
    /// so a drop in the window is terminal.
    private func resolveAuthenticateRetryable() {
        disarmChannelWatchdog()
        awaitingGatedRetry = true
        authenticateContinuation?.resume(throwing: GatedPairingWindowError())
        authenticateContinuation = nil
    }

    /// The single retry also failed with the pairing-window error, so the link may still be up
    /// with the intent set. Drop both, so the reconnect loop cannot re-raise the passkey and a
    /// fresh "Try again" can re-discover a disconnected peripheral.
    private func teardownAfterFailedRetry() async {
        await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
            queue.async { [self] in
                awaitingGatedRetry = false
                _ = discoveryPolicy.cancelForeground()
                if let peripheral { central.cancelPeripheralConnection(peripheral) }
                if stateMulticast.value != .disconnected { stateMulticast.send(.disconnected) }
                cont.resume()
            }
        }
    }

    /// Whether an ATT or CB error means the encrypted, LESC-authenticated link the gated
    /// characteristics need was never established: the passkey was declined or wrong, or the bond
    /// was refused. Separates a real pairing failure from an ordinary read or open error.
    private static func isAuthError(_ error: Error?) -> Bool {
        if let att = error as? CBATTError {
            switch att.code {
            case .insufficientAuthentication, .insufficientEncryption, .insufficientAuthorization:
                return true
            default:
                return false
            }
        }
        if let cb = error as? CBError {
            switch cb.code {
            case .encryptionTimedOut, .peerRemovedPairingInformation:
                return true
            default:
                return false
            }
        }
        return false
    }

    /// The one shared retry proxy for a failed gated op, the `objectControl` CCCD write or the PSM
    /// read, so the two delegate branches cannot drift. Retryable only when the failure is
    /// auth-class, the peripheral is still connected, and a fresh-pair `authenticate()` is parked:
    /// the conservative "pairing visibly completed, firmware momentarily refused" evidence.
    /// Everything else stays terminal, and a nil error (the op succeeded) is never retryable.
    static func isRetryableGatedFailure(
        _ error: Error?, peripheralConnected: Bool, authenticatePending: Bool
    ) -> Bool {
        authenticatePending && peripheralConnected && isAuthError(error)
    }

    private func finishConnect() {

        // Announce only a real transition: a mid-session CoC reopen re-enters here although the
        // link never left `.connected`, and re-sending would re-fire edge-triggered observers.
        // The authenticate continuation still resolves either way.
        if stateMulticast.value != .connected { stateMulticast.send(.connected) }
        if let peripheral {
            // Reaching the authenticated CoC proves this opaque CoreBluetooth identifier belongs to
            // the trusted device; later reconnects can use it to select the same peripheral.
            discoveryStore.saveKnownPeripheralID(peripheral.identifier)
        }
        awaitingGatedRetry = false
        authenticateContinuation?.resume()
        authenticateContinuation = nil

    }

    /// The link is gone: every parked continuation must resolve, because a leaked
    /// `CheckedContinuation` hangs its caller forever. Buffered notifications are dropped.
    private func failAllPending() {
        let reads = pendingReads.values.flatMap { $0 }
        pendingReads.removeAll()
        let writes = pendingWrites.values.flatMap { $0 }
        pendingWrites.removeAll()
        let channels = channelWaiters
        channelWaiters.removeAll()
        let objectControls = objectControlWaiters
        objectControlWaiters.removeAll()
        let commandResult = commandResultWaiter
        commandResultWaiter = nil
        commandResult?.timeout.cancel()
        commandResults.clearPending()
        let statusNotifications = statusNotificationWaiters
        statusNotificationWaiters.removeAll()
        statusNotificationTimeout?.cancel()
        statusNotificationTimeout = nil
        pendingObjectControlRecords.removeAll()
        openingChannel = false
        disarmChannelWatchdog()
        for cont in reads { cont.resume(throwing: DeviceError.notConnected) }
        for cont in writes { cont.resume(throwing: DeviceError.notConnected) }
        for waiter in objectControls { waiter.resume(throwing: TransferLinkLost()) }
        commandResult?.continuation.resume(throwing: DeviceError.notConnected)
        for waiter in statusNotifications { waiter.resume(throwing: DeviceError.notConnected) }
        for cont in channels { cont.resume(throwing: DeviceError.notConnected) }
    }

    // MARK: Async ↔ delegate bridges (queue-confined)

    private func read(_ uuid: CBUUID) async throws -> Data {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Data, Error>) in
            queue.async { [self] in
                guard let peripheral, let characteristic = characteristics[uuid] else {
                    cont.resume(throwing: DeviceError.notConnected)
                    return
                }
                pendingReads[uuid, default: []].append(cont)
                peripheral.readValue(for: characteristic)
            }
        }
    }

    /// Run one authenticated BLE imperative command. The protocol-v4 object client has its own
    /// operation gate; this much smaller lane covers only the command and status pair.
    private func exchangeCommand(_ payload: Data, command: UInt8) async throws -> CommandResult {
        precondition(payload.first == command)
        let slot = await acquireCommandSlot()
        defer { releaseCommandSlot(slot) }
        try Task.checkCancellation()
        try await validateCommandLane()
        try await ensureStatusNotifications()
        try Task.checkCancellation()
        await clearPendingCommandResult(command)
        try await write(payload, to: GATT.command)
        let result = try await nextCommandResult(command: command)
        try Task.checkCancellation()
        return result
    }

    private func acquireCommandSlot() async -> UUID {
        let token = UUID()
        return await withCheckedContinuation { continuation in
            queue.async { [self] in
                if commandSlotOwner == nil {
                    commandSlotOwner = token
                    continuation.resume(returning: token)
                } else {
                    commandSlotWaiters.append(CommandSlotWaiter(token: token, continuation: continuation))
                }
            }
        }
    }

    private func releaseCommandSlot(_ token: UUID) {
        queue.async { [self] in
            guard commandSlotOwner == token else { return }
            if commandSlotWaiters.isEmpty {
                commandSlotOwner = nil
            } else {
                let next = commandSlotWaiters.removeFirst()
                commandSlotOwner = next.token
                next.continuation.resume(returning: next.token)
            }
        }
    }

    private func validateCommandLane() async throws {
        try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<Void, Error>) in
            queue.async { [self] in
                if commandResults.isAvailable {
                    continuation.resume()
                } else {
                    continuation.resume(throwing: DeviceError.writeFailed)
                }
            }
        }
    }

    /// Command results identify only their command byte. Any failure that can leave a reply in
    /// flight makes correlation ambiguous, so fail closed and reconnect before accepting another.
    private func invalidateCommandLane(_ error: DeviceError) {
        dispatchPrecondition(condition: .onQueue(queue))
        let waiter = commandResultWaiter
        commandResultWaiter = nil
        waiter?.timeout.cancel()
        commandResults.invalidate()
        if let peripheral { central.cancelPeripheralConnection(peripheral) }
        waiter?.continuation.resume(throwing: error)
    }

    /// The answer can beat CoreBluetooth's write callback, so clear the previous attempt before
    /// writing and buffer a matching early notification until the waiter is registered.
    private func clearPendingCommandResult(_ command: UInt8) async {
        await withCheckedContinuation { continuation in
            queue.async { [self] in
                commandResults.clearPending(command: command)
                continuation.resume()
            }
        }
    }

    private func nextCommandResult(command: UInt8) async throws -> CommandResult {
        let token = UUID()
        return try await withCheckedThrowingContinuation { continuation in
            queue.async { [self] in
                if let result = commandResults.take(command: command) {
                    continuation.resume(returning: result)
                    return
                }
                let timeout = DispatchWorkItem { [weak self] in
                    guard let self, let waiter = self.commandResultWaiter,
                          waiter.token == token else { return }
                    self.invalidateCommandLane(.writeFailed)
                }
                commandResultWaiter = CommandResultWaiter(
                    token: token, command: command, timeout: timeout,
                    continuation: continuation
                )
                queue.asyncAfter(deadline: .now() + Self.commandResultTimeout, execute: timeout)
            }
        }
    }

    /// Arm the command-result notification before writing. Lazy, so pairing does not gain a second
    /// gated CCCD: the first imperative command enables it after authentication is established.
    private func ensureStatusNotifications() async throws {
        try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<Void, Error>) in
            queue.async { [self] in
                guard let peripheral, let status = characteristics[GATT.status] else {
                    continuation.resume(throwing: DeviceError.notConnected)
                    return
                }
                guard !status.isNotifying else {
                    continuation.resume()
                    return
                }
                statusNotificationWaiters.append(continuation)
                guard statusNotificationWaiters.count == 1 else { return }
                let timeout = DispatchWorkItem { [weak self] in
                    guard let self, !self.statusNotificationWaiters.isEmpty else { return }
                    let waiters = self.statusNotificationWaiters
                    self.statusNotificationWaiters.removeAll()
                    self.statusNotificationTimeout = nil
                    for waiter in waiters { waiter.resume(throwing: DeviceError.writeFailed) }
                }
                statusNotificationTimeout = timeout
                queue.asyncAfter(deadline: .now() + Self.phaseTimeout, execute: timeout)
                peripheral.setNotifyValue(true, for: status)
            }
        }
    }

    private func readString(_ uuid: CBUUID) async throws -> String {
        String(decoding: try await read(uuid), as: UTF8.self)
    }

    fileprivate func write(_ data: Data, to uuid: CBUUID) async throws {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            queue.async { [self] in
                guard let peripheral, let characteristic = characteristics[uuid] else {
                    cont.resume(throwing: DeviceError.notConnected)
                    return
                }
                pendingWrites[uuid, default: []].append(cont)
                peripheral.writeValue(data, for: characteristic, type: .withResponse)
            }
        }
    }

    private func currentPeripheralName() async -> String? {
        await withCheckedContinuation { (cont: CheckedContinuation<String?, Never>) in
            queue.async { [self] in cont.resume(returning: peripheral?.name) }
        }
    }
}

// MARK: - The upload runner

/// One protocol-v4 upload task. TransferClient owns the live request and its STATUS/LIST
/// reconciliation, so this adapter has no restart/resume state machine.
private actor V4UploadRunner {
    private let transport: BLETransport
    private let payload: Data
    private let kind: ObjectKind
    private let objectID: DeviceObjectID?
    private let displayName: String
    private let progress: AsyncStream<TransferProgress>.Continuation
    private let outcome: AsyncPromise<TransferOutcome>
    private let assignedID: AsyncPromise<DeviceObjectID?>
    private var attempt: Task<Void, Never>?
    private var started = false

    init(
        transport: BLETransport, payload: Data, kind: ObjectKind,
        objectID: DeviceObjectID?, displayName: String,
        progress: AsyncStream<TransferProgress>.Continuation,
        outcome: AsyncPromise<TransferOutcome>, assignedID: AsyncPromise<DeviceObjectID?>
    ) {
        self.transport = transport
        self.payload = payload
        self.kind = kind
        self.objectID = objectID
        self.displayName = displayName
        self.progress = progress
        self.outcome = outcome
        self.assignedID = assignedID
    }

    func start() {
        guard !started else { return }
        started = true
        attempt = Task {
            await runAttempt()
            attempt = nil
        }
    }

    func cancel() async {
        attempt?.cancel()
        finish(.canceled)
    }

    private func runAttempt() async {
        do {
            let ticks = progress
            let id = try await transport.performUpload(
                payload: payload, kind: kind, objectID: objectID, displayName: displayName
            ) { ticks.yield($0) }
            assignedID.fulfill(id)
            finish(.completed)
        } catch is CancellationError {
            finish(.canceled)
        } catch let error as DeviceError {
            finish(.failed(error))
        } catch {
            finish(.failed(.transferRejected))
        }
    }

    private func finish(_ terminal: TransferOutcome) {
        guard outcome.current == nil else { return }
        progress.finish()
        outcome.fulfill(terminal)
        if terminal != .completed { assignedID.fulfill(nil) }
    }
}

/// Drives a ride-sync batch through the same protocol-v4 client. A broken GET restarts itself on
/// the restored link; the batch keeps no resume cursor or reconciliation cache.
private actor RideDownloadRunner {
    private let transport: BLETransport
    private let requests: [(id: RideID, source: RideSource)]
    private let rides: AsyncThrowingStream<DownloadedRide, Error>.Continuation
    private let progress: AsyncStream<TransferProgress>.Continuation
    private let outcome: AsyncPromise<TransferOutcome>
    private var attempt: Task<Void, Never>?
    private var started = false
    private var finished = false

    init(
        transport: BLETransport, requests: [(id: RideID, source: RideSource)],
        rides: AsyncThrowingStream<DownloadedRide, Error>.Continuation,
        progress: AsyncStream<TransferProgress>.Continuation,
        outcome: AsyncPromise<TransferOutcome>
    ) {
        self.transport = transport
        self.requests = requests
        self.rides = rides
        self.progress = progress
        self.outcome = outcome
    }

    func start() {
        guard !started else { return }
        started = true
        attempt = Task {
            await runAttempt()
            attempt = nil
        }
    }

    func cancel() async {
        attempt?.cancel()
        finish(.canceled)
    }

    private func runAttempt() async {
        guard !finished else { return }
        do {
            for (index, request) in requests.enumerated() {
                try Task.checkCancellation()
                let downloaded = try await transport.downloadRide(id: request.id, source: request.source)
                rides.yield(downloaded)
                progress.yield(TransferProgress(bytesDone: index + 1, total: requests.count))
            }
            finish(.completed)
        } catch is CancellationError {
            finish(.canceled)
        } catch DeviceError.crcMismatch {
            // A corrupt ride object is a hard, non-retryable failure: the bytes on the card are bad.
            if !finished {
                finished = true
                progress.finish()
                rides.finish(throwing: DeviceError.crcMismatch)
                outcome.fulfill(.failed(.crcMismatch))
            }
        } catch {
            finish(.failed((error as? DeviceError) ?? .transferDropped))
        }
    }

    private func finish(_ terminal: TransferOutcome) {
        guard !finished else { return }
        finished = true
        progress.finish()
        rides.finish()
        outcome.fulfill(terminal)
    }
}

// MARK: - Protocol-v4 physical link

extension BLETransport: TransferLink {
    public nonisolated var maximumStreamPayload: Int {
        BLEChannel.defaultChunkSize - FlatStoreV4.streamHeaderLength
    }

    public func sendControlRecord(_ record: Data) async throws {
        do {
            _ = try ControlFrame(decoding: record, direction: .request)
            queue.async { [self] in objectControlReceiveCancelled = false }  // a request re-arms the lane
            try await write(record, to: GATT.objectControl)
        } catch is WireError {
            throw DeviceError.writeFailed
        } catch {
            throw TransferLinkLost()
        }
    }

    public func receiveControlRecord() async throws -> Data {
        try await withCheckedThrowingContinuation { continuation in
            queue.async { [self] in
                if objectControlReceiveCancelled {
                    objectControlReceiveCancelled = false
                    continuation.resume(throwing: CancellationError())
                } else if !pendingObjectControlRecords.isEmpty {
                    continuation.resume(returning: pendingObjectControlRecords.removeFirst())
                } else if peripheral?.state == .connected {
                    objectControlWaiters.append(continuation)
                } else {
                    continuation.resume(throwing: TransferLinkLost())
                }
            }
        }
    }

    /// Release the control receive its transfer has walked away from. A parked receive takes the
    /// cancellation here and now; only a cancel that found nobody is remembered for the receive
    /// about to park, because remembering both would leave an unclaimed cancellation for the next
    /// receive, and that next one is the reconciliation LIST. The next control write re-arms it.
    public func cancelControlReceive() async {
        let waiters = queue.sync { () -> [CheckedContinuation<Data, Error>] in
            let parked = objectControlWaiters
            objectControlWaiters.removeAll()
            objectControlReceiveCancelled = parked.isEmpty
            return parked
        }
        for waiter in waiters { waiter.resume(throwing: CancellationError()) }
    }

    public func sendStreamRecord(_ record: Data) async throws {
        do {
            let channel = try await readyChannel()
            try await channel.sendRecord(record)
        }
        catch is CancellationError { throw CancellationError() }
        catch { throw TransferLinkLost() }
    }

    public func receiveStreamRecord() async throws -> Data {
        do {
            let channel = try await readyChannel()
            return try await withTaskCancellationHandler {
                try await channel.receiveRecord()
            } onCancel: {
                channel.cancelReceive()
            }
        }
        catch is CancellationError { throw CancellationError() }
        catch { throw TransferLinkLost() }
    }

    public func cancelStreamReceive() async {
        queue.sync { bleChannel?.cancelReceive() }
    }

    public func restore() async throws {
        if stateMulticast.value != .connected {
            enum RestoreBeat: Sendable { case connected, timedOut }
            let states = state
            let beat = await withTaskGroup(of: RestoreBeat.self) { group in
                group.addTask {
                    for await value in states where value == .connected { return .connected }
                    return .timedOut
                }
                group.addTask {
                    try? await Task.sleep(for: .seconds(20))
                    return .timedOut
                }
                let first = await group.next() ?? .timedOut
                group.cancelAll()
                return first
            }
            guard case .connected = beat else { throw TransferLinkLost() }
        }
        do { _ = try await readyChannel() }
        catch { throw TransferLinkLost() }
    }
}

// MARK: - CBCentralManagerDelegate

extension BLETransport: CBCentralManagerDelegate {
    public func centralManagerDidUpdateState(_ central: CBCentralManager) {
        startConnectIfReady()

    }

    public func centralManager(_ central: CBCentralManager, didDiscover peripheral: CBPeripheral,
                               advertisementData: [String: Any], rssi RSSI: NSNumber) {
        let action = discoveryPolicy.discovered(
            peripheralID: peripheral.identifier,
            knownPeripheralID: discoveryStore.knownPeripheralID()
        )
        switch action {
        case .ignore:
            return
        case .connect:
            break
        }
        central.stopScan()
        self.peripheral = peripheral
        peripheral.delegate = self

        central.connect(peripheral)
    }

    public func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        commandResults.reconnect()
        discoveryPolicy.didConnect(peripheralID: peripheral.identifier)
        armDiscoveryWatchdog()
        let services = [GATT.deviceInformation, GATT.battery, GATT.obcControlService]
        peripheral.discoverServices(services)
    }

    public func centralManager(_ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral, error: Error?) {
        discoveryPolicy.didDisconnect()
        if discoverContinuation != nil {
            failDiscover(.notConnected)
        } else if discoveryPolicy.foregroundRequested {
            startConnectIfReady()
        }
    }

    public func centralManager(_ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral, error: Error?) {

        discoveryPolicy.didDisconnect()
        characteristics.removeAll()
        disarmDiscoveryWatchdog()  // the channel watchdog is disarmed by failAllPending below
        // Close the dead CoC; do not just drop the reference. An `L2CAPByteChannel` owns a
        // run-loop thread and a stall timer that stop only on `close()`, so nil-ing the refs
        // orphans a thread that wakes every 0.25 s, one per disconnect. Drop the refs inline and
        // fire the async close, which also resolves the channel's own parked waiters.
        let deadChannel = byteChannel
        byteChannel = nil
        bleChannel = nil
        if let deadChannel { Task { await deadChannel.close() } }
        failAllPending()
        // A disconnect that lands while a connect phase is pending is that phase's failure.
        // `failAllPending` leaves the two phase continuations alone, and a declined or wrong
        // passkey commonly tears the link down instead of erroring the gated PSM read, so without
        // this a fresh pair hangs forever: there is no timeout on `authenticate()`. Both helpers
        // also drop the intent, which stops a silent passkey sheet after a decline.
        if discoverContinuation != nil {
            failDiscover(.notConnected)
            return
        }
        if authenticateContinuation != nil {
            failAuthenticate(.pairingFailed)
            return
        }
        // A drop during the gated-retry beat, where the continuation is momentarily nil, is
        // terminal: the second attempt fails `.notConnected`. Drop the intent so the reconnect
        // loop below cannot re-raise the passkey behind it.
        if awaitingGatedRetry {
            awaitingGatedRetry = false
            _ = discoveryPolicy.cancelForeground()
            stateMulticast.send(.disconnected)
            return
        }
        stateMulticast.send(discoveryPolicy.foregroundRequested ? .outOfRange : .disconnected)
        if discoveryPolicy.hasIntent {
            startConnectIfReady()
        }
    }
}

// MARK: - CBPeripheralDelegate

extension BLETransport: CBPeripheralDelegate {
    public func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        guard error == nil else {
            failConnectionSetup()
            return
        }
        let services = peripheral.services ?? []
        guard !services.isEmpty else {
            failConnectionSetup()
            return
        }
        pendingServiceDiscovery = services.count
        for service in services {
            peripheral.discoverCharacteristics(nil, for: service)
        }
    }

    public func peripheral(_ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        guard error == nil else {
            failConnectionSetup()
            return
        }
        for characteristic in service.characteristics ?? [] {
            characteristics[characteristic.uuid] = characteristic
            // Only the un-gated BAS notify is armed here. The gated `objectControl` indication
            // and the PSM read wait for `authenticate()`, so first-time pairing raises no passkey
            // sheet yet. The device's connect-time battery notify fires before this subscription
            // lands, so read the level too and the UI has it at once.
            if characteristic.uuid == GATT.batteryLevel {
                peripheral.setNotifyValue(true, for: characteristic)
                peripheral.readValue(for: characteristic)
            }
        }
        pendingServiceDiscovery -= 1
        guard pendingServiceDiscovery <= 0 else { return }
        disarmDiscoveryWatchdog()

        // Every service's characteristics are in hand, so the un-gated surface is ready. A pending
        // `discover()` resolves here and its caller runs `authenticate()` next; an unsolicited
        // bonded reconnect has no waiter and goes straight to the gated phase.
        if discoveryPolicy.foregroundRequested {
            if let cont = discoverContinuation {
                discoverContinuation = nil
                cont.resume()
            } else {
                beginAuthenticate()
            }
        }
    }

    public func peripheral(_ peripheral: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic, error: Error?) {
        let uuid = characteristic.uuid

        if uuid == GATT.batteryLevel, let value = characteristic.value?.first {
            batteryMulticast.send(Int(value))
            return
        }
        // PSM read → open the L2CAP channel (initial connect and re-opens alike).
        if uuid == GATT.psm, bleChannel == nil {
            if error == nil, let data = characteristic.value, data.count >= 2 {
                let psm = UInt16(data[0]) | (UInt16(data[1]) << 8)
                // The read resolved, so any passkey entry is behind us: re-arm tight for the
                // machine-only openL2CAPChannel tail. Only while this open is the one being
                // watched; a stale resolve after the watchdog fired opens unwatched.
                if openingChannel { armChannelWatchdog(after: Self.phaseTimeout) }
                peripheral.openL2CAPChannel(CBL2CAPPSM(psm))
            } else {
                // The PSM characteristic is authenticated, and the read is the first gated op of
                // `authenticate()`, so a failure here is usually a declined or wrong passkey. Fail
                // the open waiters and the pending authenticate, or `confirmPairing()` hangs. An
                // auth-class error maps to `pairingFailed`, anything else to `channelOpenFailed`.
                // A decline that instead drops the link lands in `didDisconnectPeripheral`.
                openingChannel = false
                disarmChannelWatchdog()
                let waiters = channelWaiters
                channelWaiters.removeAll()
                // An auth-class failure on the gated PSM read while the peripheral is still
                // connected is the retryable pairing-window case, shared with the CCCD-write
                // branch. Resolve a parked fresh-pair `authenticate()` as retryable: it retries
                // the gated phase once on this bonded link instead of failing. Any channel
                // waiters fail as before; the retry re-opens the CoC.
                if Self.isRetryableGatedFailure(
                    error, peripheralConnected: peripheral.state == .connected,
                    authenticatePending: authenticateContinuation != nil
                ) {
                    for cont in waiters { cont.resume(throwing: DeviceError.channelOpenFailed) }
                    resolveAuthenticateRetryable()
                } else {
                    let failure: DeviceError = Self.isAuthError(error) ? .pairingFailed : .channelOpenFailed
                    for cont in waiters { cont.resume(throwing: failure) }
                    if authenticateContinuation != nil { failAuthenticate(failure) }
                }
            }
            return
        }
        if uuid == GATT.objectControl {
            guard error == nil, let record = characteristic.value else {
                let waiters = objectControlWaiters
                objectControlWaiters.removeAll()
                for waiter in waiters { waiter.resume(throwing: TransferLinkLost()) }
                return
            }
            if objectControlWaiters.isEmpty {
                pendingObjectControlRecords.append(record)
            } else {
                objectControlWaiters.removeFirst().resume(returning: record)
            }
            return
        }
        if uuid == GATT.status {
            guard commandResults.isAvailable else { return }
            if error != nil {
                invalidateCommandLane(.readFailed)
            } else if let data = characteristic.value,
                      case .commandResult(let result) = try? StatusMessage(decoding: data)
            {
                if let waiter = commandResultWaiter, waiter.command == result.command {
                    commandResultWaiter = nil
                    waiter.timeout.cancel()
                    waiter.continuation.resume(returning: result)
                } else {
                    commandResults.receive(result)
                }
            }
            return
        }

        resumeReads(uuid, error == nil ? .success(characteristic.value ?? Data()) : .failure(DeviceError.readFailed))
    }

    public func peripheral(
        _ peripheral: CBPeripheral, didUpdateNotificationStateFor characteristic: CBCharacteristic, error: Error?
    ) {
        if characteristic.uuid == GATT.status {
            statusNotificationTimeout?.cancel()
            statusNotificationTimeout = nil
            let waiters = statusNotificationWaiters
            statusNotificationWaiters.removeAll()
            if error == nil, characteristic.isNotifying {
                for waiter in waiters { waiter.resume() }

            } else {
                for waiter in waiters { waiter.resume(throwing: DeviceError.writeFailed) }

            }
            return
        }
        // On a fresh pair the first gated op is the `objectControl` CCCD write, not the PSM read,
        // so that write raises the passkey sheet and iOS's post-passkey replay hits it first. The
        // firmware's post-pairing refusal window therefore most likely clips the CCCD write, and
        // an unhandled failure there leaves the control indication dead while `authenticate()`
        // resolves. Map it like the PSM branch: the shared proxy resolves the parked authenticate
        // as retryable, and the retry's `beginAuthenticate` re-arms the gated ops. Anything else
        // is ignored: a background re-arm has no authenticate pending, and a real decline tears
        // the link down. `objectControl` is the sole gated CCCD this window can clip.
        guard characteristic.uuid == GATT.objectControl else { return }
        guard Self.isRetryableGatedFailure(
            error, peripheralConnected: peripheral.state == .connected,
            authenticatePending: authenticateContinuation != nil
        ) else { return }
        resolveAuthenticateRetryable()
    }

    public func peripheral(_ peripheral: CBPeripheral, didWriteValueFor characteristic: CBCharacteristic, error: Error?) {
        let result: Result<Void, Error> = error == nil ? .success(()) : .failure(DeviceError.writeFailed)
        let conts = pendingWrites.removeValue(forKey: characteristic.uuid) ?? []
        for cont in conts { cont.resume(with: result) }
    }

    public func peripheral(_ peripheral: CBPeripheral, didOpen channel: CBL2CAPChannel?, error: Error?) {
        openingChannel = false
        disarmChannelWatchdog()
        guard let channel, error == nil else {
            let waiters = channelWaiters
            channelWaiters.removeAll()
            for cont in waiters { cont.resume(throwing: DeviceError.channelOpenFailed) }
            if authenticateContinuation != nil { failAuthenticate(.channelOpenFailed) }
            return
        }
        let byte = L2CAPByteChannel(channel: channel)
        byteChannel = byte
        let ble = BLEChannel(channel: byte)
        bleChannel = ble
        let waiters = channelWaiters
        channelWaiters.removeAll()
        for cont in waiters { cont.resume(returning: ble) }
        // CoC up and services discovered, so the link is ready. `finishConnect` publishes
        // `.connected` either way and resolves `authenticate()` when one is pending; a background
        // reconnect has none but must still flip the state stream back.
        finishConnect()
    }

    private func resumeReads(_ uuid: CBUUID, _ result: Result<Data, Error>) {
        let conts = pendingReads.removeValue(forKey: uuid) ?? []
        for cont in conts { cont.resume(with: result) }
    }
}
#endif
