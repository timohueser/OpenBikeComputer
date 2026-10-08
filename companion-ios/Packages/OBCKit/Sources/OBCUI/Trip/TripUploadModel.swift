import Foundation
import Observation
import OBCDomain
import OBCTransport

/// Owns a trip upload from catalog reconciliation through day routes, trip details and cleanup.
/// Each successful transfer records its link before the next step starts.
@MainActor @Observable
public final class TripUploadModel: Identifiable {
    public nonisolated let id = UUID()

    public enum Phase: Equatable, Sendable {
        case uploading
        case interrupted
        case done
        case failed
    }

    /// Why the whole-trip upload failed: a precheck deficit before any bytes, or a device reject
    /// mid-queue. Drives the failure copy.
    public enum Failure: Equatable, Sendable {
        /// The precheck found the trip cannot fit, by this many route slots.
        case storagePrecheck(routeDeficit: Int)
        /// A device transfer failed for good.
        case device(DeviceError)
        /// The trip is stored, but an old day route could not be deleted.
        case cleanup(DeviceError)
    }

    public struct Timing: Sendable {
        public var doneAutoDismiss: Duration
        public init(doneAutoDismiss: Duration = .seconds(2.6)) {
            self.doneAutoDismiss = doneAutoDismiss
        }
    }

    private enum Step {
        case day(TripDayPlan)
        case trip
    }

    // MARK: Observable state

    public private(set) var phase: Phase
    public private(set) var progress = TransferProgress(bytesDone: 0, total: 1)
    public private(set) var failure: Failure?
    public private(set) var shouldDismiss = false
    /// The live link state for the current transfer.
    public private(set) var connection: ConnectionState = .connected
    /// The current queue step, zero-based, which drives the header.
    public private(set) var stepIndex = 0
    /// Days skipped because the device already held them.
    public private(set) var skippedCount = 0
    /// Objects committed so far, days and trip.
    public private(set) var committedCount = 0

    // MARK: Fixed facts

    /// What the device's TRIP RECEIVED card shows once the trip lands.
    public let card: DeviceTripCard
    public var tripName: String { card.name }
    public let deviceName: String
    /// Total queue steps: the header's denominator.
    public var stepCount: Int { steps.count }
    // MARK: Wiring

    private let transport: any DeviceLink & DeviceObjects
    private let main: MainScreenModel
    private let tripID: TripID
    private var scope: LibraryScope?
    private var days: [TripDayRoute]
    private var steps: [Step] = []
    private var precheck: TripUploadPrecheck
    private let timing: Timing
    @ObservationIgnored private let activity: TransferActivity?
    @ObservationIgnored private var activityToken: TransferActivity.Token?
    @ObservationIgnored private var currentHandle: TransferHandle?
    @ObservationIgnored private var progressWatcher: Task<Void, Never>?
    @ObservationIgnored private var linkWatcher: Task<Void, Never>?
    @ObservationIgnored private var driver: Task<Void, Never>?
    @ObservationIgnored private var started = false
    @ObservationIgnored private var linkUp = true

    init?(tripID: TripID, main: MainScreenModel, timing: Timing = Timing()) {
        guard let trip = main.trip(tripID), let plan = Self.plan(tripID, in: main) else { return nil }
        let days = main.tripDays(tripID)
        self.main = main
        self.tripID = tripID
        self.transport = main.transport
        self.scope = main.connectedScope
        self.days = days
        self.card = DeviceTripCard(name: trip.name, days: days.map { $0.summary(tripID: tripID) })
        self.deviceName = main.deviceName
        self.precheck = plan.precheck
        self.timing = timing
        self.activity = main.transferActivity
        self.phase = .uploading
        updatePlan(plan)
    }

    static func plan(_ id: TripID, in main: MainScreenModel) -> TripUploadPlan? {
        guard let trip = main.trip(id) else { return nil }
        let inputs = main.tripDays(id).map { day in
            TripUploadPlanner.DayInput(
                day: day.day,
                isUpToDate: main.provenDayCRC(trip, day: day.day) == day.crc32,
                committedObjectID: main.scopedDayCopy(trip, day: day.day)?.link.objectID)
        }
        let target = trip.deviceLink.flatMap { link -> DeviceObjectID? in
            guard let scope = main.connectedScope, link.matches(scope) else { return nil }
            return link.objectID
        }
        return TripUploadPlanner.plan(
            days: inputs, tripObjectID: target,
            deviceRouteCount: main.lastRouteCatalog?.count ?? 0,
            deviceTripCount: main.lastTripCatalog?.count ?? 0)
    }

    /// Routes must reconcile before trips because trip adoption reads the day links.
    func prepare() async {
        do {
            try await verifyConnection()
            let routes = try await transport.listRoutes()
            try await verifyConnection()
            main.lastRouteCatalog = routes
            main.reconcileOnDevice(with: routes)
            main.routes = main.plannedList()
            let trips = try await transport.listTrips()
            try await verifyConnection()
            main.lastTripCatalog = trips
            main.reconcileTripsOnDevice(with: trips)
            main.reloadTrips()
            guard let plan = Self.plan(tripID, in: main) else { throw DeviceError.readFailed }
            days = main.tripDays(tripID)
            updatePlan(plan)
        } catch {
            main.reloadTrips()
            fail(error)
        }
    }

    private func updatePlan(_ plan: TripUploadPlan) {
        precheck = plan.precheck
        steps = plan.days.map(Step.day)
        if !plan.allDaysSkip || main.tripContentOnDeviceState(tripID) != .upToDate {
            steps.append(.trip)
        }
    }

    private func verifyConnection() async throws {
        await main.identityTask?.value
        try Task.checkCancellation()
        guard main.connection == .connected, let connected = main.connectedScope,
              scope == nil || connected == scope else { throw DeviceError.readFailed }
        scope = connected
    }

    // MARK: Derived lines

    public var fraction: Double { progress.fraction }

    public var percentLine: String { "\(Int((progress.fraction * 100).rounded()))%" }

    public var sizeLine: String {
        OBCFormat.transferSizeLine(bytesDone: progress.bytesDone, totalBytes: progress.total, hasWaypoints: false)
    }

    /// The current step's title, a day name or the trip object's. Nil in a terminal phase.
    public var currentStepTitle: String? {
        guard stepIndex < steps.count else { return nil }
        switch steps[stepIndex] {
        case .day(let plan): return days[plan.day].name
        case .trip: return "Trip details"
        }
    }

    /// The queued-mode line over the bar: "Day 2 · 3 of 4". It counts every step, skips and the
    /// trip object included.
    public var stepProgressLabel: String {
        let position = min(stepIndex + 1, stepCount)
        return "\(currentStepTitle ?? tripName) · \(position) of \(stepCount)"
    }

    // MARK: Failure copy

    public var failedTitle: String {
        switch failure {
        case .cleanup: "Trip sent"
        case .storagePrecheck, .device(.storageFull): "\(deviceName) is full"
        default: "Not sent"
        }
    }

    public var failedMessage: String {
        switch failure {
        case .cleanup:
            return "The trip reached \(deviceName), but an old day route could not be deleted. Send the trip again to finish cleanup."
        case .storagePrecheck(let deficit):
            let noun = deficit == 1 ? "route" : "routes"
            return "\(tripName) needs room for \(deficit) more \(noun). Delete routes on \(deviceName), then send again."
        case .device(.storageFull):
            return "There is no room for more routes. Delete routes on \(deviceName), then send again. The days sent so far stay on it."
        default:
            return "\(deviceName) did not answer. Make sure it is on and near your phone, then send again."
        }
    }

    // MARK: Lifecycle

    public func start() {
        guard !started, phase != .failed else { return }
        started = true

        // A link drop stalls the current step. Progress after reconnect resumes it.
        linkWatcher = Task { [weak self, transport] in
            for await state in transport.state {
                guard let self else { return }
                connection = state
                let dropped = state == .outOfRange || state == .disconnected
                linkUp = !dropped
                if dropped, phase == .uploading, currentHandle?.currentOutcome == nil {
                    phase = .interrupted
                    setActive(false)
                }
            }
        }

        beginQueue()
    }
    /// Precheck, then start the queue. The precheck runs before any bytes, so a trip that cannot
    /// fit fails up front rather than as a partial upload that fills the device at the last day.
    private func beginQueue() {
        guard precheck.fits else {
            phase = .failed
            failure = .storagePrecheck(routeDeficit: precheck.routeSlotDeficit)
            return
        }
        phase = .uploading
        setActive(true)
        driver = Task { [weak self] in await self?.runQueue() }
    }

    /// Restart the current step's transfer after a drop: uploads restart, they do not resume.
    public func resume() {
        guard phase == .interrupted else { return }
        currentHandle?.resume()
        phase = .uploading
        setActive(true)
    }

    /// Cancel the whole trip upload, aborting the in-flight transfer. Completed days stay
    /// committed on the device, and re-running is idempotent.
    public func cancel() {
        currentHandle?.cancel()
        driver?.cancel()
        setActive(false)
        shouldDismiss = true
    }

    /// Done / a failure's Close.
    public func dismiss() { shouldDismiss = true }

    /// The sheet left the screen: cancel an unresolved transfer and stop the watchers. A completed
    /// queue's dismissal passes through untouched.
    public func sheetDismissed() {
        if let currentHandle, currentHandle.currentOutcome == nil { currentHandle.cancel() }
        tearDown()
        setActive(false)
    }

    deinit {
        driver?.cancel()
        linkWatcher?.cancel()
        progressWatcher?.cancel()
    }

    // MARK: Queue driver

    private func runQueue() async {
        var cleaningUp = false
        do {
            while stepIndex < steps.count {
                try await verifyConnection()
                let step = steps[stepIndex]
                if case .day(let plan) = step, plan.action == .skip {
                    skippedCount += 1
                    stepIndex += 1
                    continue
                }
                let (handle, crc) = try makeTransfer(step)
                currentHandle = handle
                phase = .uploading
                watchProgress(handle)
                let outcome = await handle.outcome
                progressWatcher?.cancel()
                progressWatcher = nil
                try Task.checkCancellation()
                switch outcome {
                case .completed:
                    guard let objectID = await handle.assignedObjectID else { throw DeviceError.writeFailed }
                    try await verifyConnection()
                    try recordCommit(step, objectID: objectID, crc: crc)
                    committedCount += 1
                    stepIndex += 1
                case .canceled:
                    setActive(false)
                    shouldDismiss = true
                    return
                case .failed(let error):
                    throw error
                }
                currentHandle = nil
            }
            cleaningUp = true
            try await deleteDroppedDays()
            phase = .done
            setActive(false)
            try await Task.sleep(for: timing.doneAutoDismiss)
            shouldDismiss = true
        } catch is CancellationError {
            setActive(false)
        } catch {
            fail(error, cleanup: cleaningUp)
        }
    }

    private func makeTransfer(_ step: Step) throws -> (TransferHandle, UInt32) {
        guard let trip = main.trip(tripID) else { throw DeviceError.readFailed }
        switch step {
        case .day(let plan):
            let currentDays = main.tripDays(tripID)
            guard currentDays.indices.contains(plan.day), !currentDays[plan.day].payload.isEmpty
            else { throw DeviceError.readFailed }
            let day = currentDays[plan.day]
            let target: DeviceObjectID? =
                if case .replace(let id) = plan.action { id } else { nil }
            let blob = RouteBlob(
                summary: day.summary(tripID: tripID), payload: day.payload, targetObjectID: target)
            return (transport.uploadRoute(blob), CRC32.checksum(blob.payload))
        case .trip:
            guard let object = main.currentTripObject(for: trip) else { throw DeviceError.readFailed }
            let target = trip.deviceLink.flatMap { link -> DeviceObjectID? in
                guard let scope, link.matches(scope) else { return nil }
                return link.objectID
            }
            let blob = TripBlob(
                name: trip.name, deviceStageIDs: object.days.map(\.routeID),
                payload: TripObjectCodec.encode(object), targetObjectID: target)
            return (transport.uploadTrip(blob), CRC32.checksum(blob.payload))
        }
    }

    private func recordCommit(_ step: Step, objectID: DeviceObjectID, crc: UInt32) throws {
        guard var trip = main.trip(tripID), let scope else { throw DeviceError.readFailed }
        let link = DeviceRouteLink(scope: scope, objectID: objectID)
        switch step {
        case .day(let plan):
            while trip.dayCopies.count <= plan.day { trip.dayCopies.append(nil) }
            trip.dayCopies[plan.day] = TripDayCopy(link: link, uploadedCRC32: crc)
            main.deviceRouteCRCs[objectID] = crc
        case .trip:
            trip.deviceLink = link
            trip.uploadedCRC32 = crc
            main.deviceTripCRCs[objectID] = crc
        }
        main.library.saveTrip(trip)
        main.reloadTrips()
    }

    /// Keep each dropped link until its delete succeeds, so a retry can finish cleanup.
    private func deleteDroppedDays() async throws {
        while true {
            try await verifyConnection()
            guard let trip = main.trip(tripID) else { throw DeviceError.readFailed }
            guard trip.dayCopies.count > trip.dayCount else { return }
            if let copy = trip.dayCopies.last ?? nil, let scope, copy.link.matches(scope) {
                try await transport.deleteRoute(copy.link.objectID)
                try await verifyConnection()
            }
            guard var current = main.trip(tripID), current.dayCount == trip.dayCount,
                  current.dayCopies.count == trip.dayCopies.count,
                  current.dayCopies.last == trip.dayCopies.last || current.dayCopies.last == .some(nil)
            else { throw DeviceError.readFailed }
            current.dayCopies.removeLast()
            main.library.saveTrip(current)
            main.reloadTrips()
        }
    }

    private func fail(_ error: Error, cleanup: Bool = false) {
        let deviceError = error as? DeviceError ?? .writeFailed
        failure = cleanup ? .cleanup(deviceError) : .device(deviceError)
        phase = .failed
        setActive(false)
    }

    private func watchProgress(_ handle: TransferHandle) {
        progressWatcher?.cancel()
        progressWatcher = Task { [weak self] in
            for await tick in handle.progress {
                guard let self else { return }
                progress = tick
                // A tick while interrupted, with the link back, means the restart is moving, so
                // flip to uploading and re-claim the ledger.
                if phase == .interrupted, linkUp {
                    phase = .uploading
                    setActive(true)
                }
            }
        }
    }

    private func tearDown() {
        driver?.cancel(); driver = nil
        linkWatcher?.cancel(); linkWatcher = nil
        progressWatcher?.cancel(); progressWatcher = nil
    }

    private func setActive(_ active: Bool) {
        if active {
            guard activityToken == nil else { return }
            activityToken = activity?.begin()
        } else if let token = activityToken {
            activityToken = nil
            activity?.end(token)
        }
    }
}
