import Foundation
import Testing
import OBCWeatherWire
@testable import OBCWeather

// The WX9 durable-job state machine, driven entirely against scripted seams: no CoreBluetooth, no
// network, no clock. "Relaunch" in these tests is literal — a fresh engine instance over the same
// stores — which is exactly what an iOS process death plus CoreBluetooth restoration produces.

// MARK: - Scripted seams

private final class ClockBox: @unchecked Sendable {
    private let lock = NSLock()
    private var date: Date

    init(_ date: Date = Date(timeIntervalSince1970: 1_770_000_000)) { self.date = date }

    var now: Date {
        lock.lock()
        defer { lock.unlock() }
        return date
    }

    func advance(_ seconds: TimeInterval) {
        lock.lock()
        defer { lock.unlock() }
        date = date.addingTimeInterval(seconds)
    }
}

private final class ScriptedLink: WeatherDeviceLink, @unchecked Sendable {
    private let lock = NSLock()
    var readResults: [Result<WeatherContextReadReceipt, WeatherDeviceLinkError>] = []
    var uploadResults: [Result<WeatherBundleUploadReceipt, WeatherDeviceLinkError>] = []
    var unchangedResults: [Result<WeatherBundleUploadReceipt, WeatherDeviceLinkError>] = []
    private(set) var readCalls = 0
    private(set) var uploadedPayloads: [Data] = []
    private(set) var unchangedCalls: [(requestID: UInt32, retryAfterSeconds: UInt16)] = []

    func readRequestContext() async throws -> WeatherContextReadReceipt {
        let result: Result<WeatherContextReadReceipt, WeatherDeviceLinkError>? = lock.withLock {
            readCalls += 1
            return readResults.isEmpty ? nil : readResults.removeFirst()
        }
        guard let result else { throw WeatherDeviceLinkError.timedOut }
        return try result.get()
    }

    func uploadBundle(_ bytes: Data) async throws -> WeatherBundleUploadReceipt {
        let result: Result<WeatherBundleUploadReceipt, WeatherDeviceLinkError>? = lock.withLock {
            uploadedPayloads.append(bytes)
            return uploadResults.isEmpty ? nil : uploadResults.removeFirst()
        }
        guard let result else { throw WeatherDeviceLinkError.timedOut }
        return try result.get()
    }

    func acknowledgeUnchanged(
        requestID: UInt32, retryAfterSeconds: UInt16
    ) async throws -> WeatherBundleUploadReceipt {
        let result: Result<WeatherBundleUploadReceipt, WeatherDeviceLinkError>? = lock.withLock {
            unchangedCalls.append((requestID, retryAfterSeconds))
            return unchangedResults.isEmpty ? nil : unchangedResults.removeFirst()
        }
        guard let result else { throw WeatherDeviceLinkError.timedOut }
        return try result.get()
    }
}

/// A one-shot rendezvous: a fetch parks inside it until the test opens it. The only way to hold a
/// run *in flight* long enough to land a second trigger on top of it, which is what the trigger
/// queue exists for.
actor Gate {
    private var opened = false
    private var entered = false
    private var openWaiters: [CheckedContinuation<Void, Never>] = []
    private var enteredWaiters: [CheckedContinuation<Void, Never>] = []

    /// A latch a task can raise on completion, so a test can assert something has *not* finished.
    actor Flag {
        private(set) var isRaised = false
        func raise() { isRaised = true }
    }

    /// Called from inside the work: mark arrival, then wait for the test.
    func enter() async {
        entered = true
        for waiter in enteredWaiters { waiter.resume() }
        enteredWaiters = []
        guard !opened else { return }
        await withCheckedContinuation { openWaiters.append($0) }
    }

    func waitUntilEntered() async {
        guard !entered else { return }
        await withCheckedContinuation { enteredWaiters.append($0) }
    }

    func open() {
        opened = true
        for waiter in openWaiters { waiter.resume() }
        openWaiters = []
    }
}

private final class ScriptedAssembler: WeatherAssembling, @unchecked Sendable {
    private let lock = NSLock()
    var results: [Result<BuiltWeatherBundle, Error>] = []
    var preflightResults: [Result<WeatherAssemblyOutcome, Error>] = []
    /// When set, the *first* assemble parks here until the test opens it.
    var gate: Gate?
    private(set) var calls: [(requestID: UInt32, generation: UInt32)] = []
    private(set) var preflightCalls: [(requestID: UInt32, allowReuse: Bool)] = []

    func assemble(
        request: WeatherRequest, generation: UInt32, now: Date
    ) async throws -> BuiltWeatherBundle {
        let result: Result<BuiltWeatherBundle, Error>? = lock.withLock {
            calls.append((request.requestID, generation))
            return results.isEmpty ? nil : results.removeFirst()
        }
        if let gate {
            self.gate = nil
            await gate.enter()
        }
        guard let result else { throw WeatherProviderError.unavailable }
        return try result.get()
    }

    func assembleIfChanged(
        request: WeatherRequest, generation: UInt32, heldBundleGeneratedAt _: Date?,
        allowHeldBundleReuse: Bool, now: Date
    ) async throws -> WeatherAssemblyOutcome {
        let result: Result<WeatherAssemblyOutcome, Error>? = lock.withLock {
            preflightCalls.append((request.requestID, allowHeldBundleReuse))
            return preflightResults.isEmpty ? nil : preflightResults.removeFirst()
        }
        if let result { return try result.get() }
        return .bundle(try await assemble(request: request, generation: generation, now: now))
    }
}

private final class InMemoryJobStore: WeatherJobStore, @unchecked Sendable {
    private let lock = NSLock()
    private var record: WeatherJobRecord?
    private(set) var saveCount = 0

    func load() -> WeatherJobRecord? {
        lock.lock()
        defer { lock.unlock() }
        return record
    }

    func save(_ newRecord: WeatherJobRecord) {
        lock.lock()
        defer { lock.unlock() }
        record = newRecord
        saveCount += 1
    }

    func clear() {
        lock.lock()
        defer { lock.unlock() }
        record = nil
    }
}

private final class InMemoryHistory: WeatherJobHistoryStore, @unchecked Sendable {
    private let lock = NSLock()
    private var stored: [WeatherJobHistoryEntry] = []

    func append(_ entry: WeatherJobHistoryEntry) {
        lock.lock()
        defer { lock.unlock() }
        stored.append(entry)
    }

    func entries() -> [WeatherJobHistoryEntry] {
        lock.lock()
        defer { lock.unlock() }
        return stored
    }
}

// MARK: - Fixture helpers

private func snapshot(
    requestID: UInt32 = 7, latitude: Int32? = 47_500_000, longitude: Int32? = 7_600_000,
    heldGeneration: UInt32? = nil, readAt: Date
) -> WeatherDeviceRequestSnapshot {
    WeatherDeviceRequestSnapshot(
        requestID: requestID,
        latitudeMicrodegrees: latitude,
        longitudeMicrodegrees: longitude,
        fixUnixSeconds: latitude != nil ? Int64(readAt.timeIntervalSince1970) - 5 : nil,
        heldBundleGeneration: heldGeneration,
        heldBundleGeneratedAtUnixSeconds: heldGeneration != nil ? 1_769_000_000 : nil,
        readAt: readAt)
}

private func readReceipt(_ snapshot: WeatherDeviceRequestSnapshot) -> WeatherContextReadReceipt {
    WeatherContextReadReceipt(
        snapshot: snapshot, connectedDuration: .seconds(2), reusedForegroundConnection: false)
}

private func uploadReceipt() -> WeatherBundleUploadReceipt {
    WeatherBundleUploadReceipt(
        connectLatency: .seconds(1), connectedDuration: .seconds(3),
        reusedForegroundConnection: false)
}

/// A structurally plausible built bundle without invoking the codec: the engine treats the bytes
/// as opaque and reads only generation + bounds from the decoded value.
private func builtBundle(
    generation: UInt32, requestID: UInt32 = 7, at now: Date,
    bytes: Data = Data([0x4F, 0x42, 0x43, 0x57, 0x01])
) -> BuiltWeatherBundle {
    let bounds = OBCWeatherBounds(
        southLatitudeMicrodegrees: 47_000_000, westLongitudeMicrodegrees: 7_000_000,
        northLatitudeMicrodegrees: 48_000_000, eastLongitudeMicrodegrees: 8_200_000,
        gridOriginLatitudeMicrodegrees: 47_000_000, gridOriginLongitudeMicrodegrees: 7_000_000)
    let hours = (0..<24).map { hour in
        HourlyCondition(
            validAt: now.addingTimeInterval(TimeInterval(hour) * 3_600),
            temperatureCelsius: 18, condition: .clear)
    }
    let hourly = HourlyForecast(hours: hours, attribution: .met, retrievedAt: now)
    let bundle = OBCWeatherBundle(
        generation: generation, requestID: requestID,
        generatedAtUnixSeconds: Int64(now.timeIntervalSince1970),
        validFromUnixSeconds: Int64(now.timeIntervalSince1970),
        validUntilUnixSeconds: Int64(now.timeIntervalSince1970) + 24 * 3_600,
        bounds: bounds, hourly: [], rainFrames: [])
    let state = WeatherState(
        hourly: hourly, precipitation: nil, noRainMapReason: .outOfDomain,
        attributions: [.met], diagnostics: WeatherDiagnostics())
    return BuiltWeatherBundle(bytes: bytes, bundle: bundle, state: state)
}

private struct Rig {
    let clock: ClockBox
    let link: ScriptedLink
    let assembler: ScriptedAssembler
    let store: InMemoryJobStore
    let history: InMemoryHistory
    let configuration: WeatherJobEngine.Configuration

    init(configuration: WeatherJobEngine.Configuration = .init()) {
        clock = ClockBox()
        link = ScriptedLink()
        assembler = ScriptedAssembler()
        store = InMemoryJobStore()
        history = InMemoryHistory()
        self.configuration = configuration
    }

    func engine() -> WeatherJobEngine {
        WeatherJobEngine(
            link: link, assembler: assembler, store: store, history: history,
            configuration: configuration, now: { [clock] in clock.now })
    }
}

// MARK: - The suite

@Suite("Weather job engine")
struct WeatherJobEngineTests {
    @Test func happyPathRunsBothLegsAndClearsTheCheckpoint() async {
        let rig = Rig()
        let read = snapshot(heldGeneration: 4, readAt: rig.clock.now)
        rig.link.readResults = [.success(readReceipt(read))]
        let built = builtBundle(generation: 5, at: rig.clock.now, bytes: Data([1, 2, 3, 4]))
        rig.assembler.results = [.success(built)]
        rig.link.uploadResults = [.success(uploadReceipt())]

        await rig.engine().kick(.deviceRaisedRequest)

        #expect(rig.link.readCalls == 1)
        // Generation is serially one past what the device holds — the engine owns monotonicity.
        #expect(rig.assembler.calls.map(\.generation) == [5])
        #expect(rig.assembler.calls.map(\.requestID) == [7])
        #expect(rig.link.uploadedPayloads == [Data([1, 2, 3, 4])])
        #expect(rig.store.load() == nil, "a committed job leaves no checkpoint behind")
        let entries = rig.history.entries()
        #expect(entries.count == 1)
        #expect(entries.first?.outcome == .committed)
        #expect(entries.first?.requestID == 7)
        #expect(entries.first?.readConnectedMilliseconds == 2_000)
        #expect(entries.first?.uploadConnectedMilliseconds == 3_000)
        #expect(entries.first?.bundleByteCount == 4)
    }

    @Test func unchangedProvidersFinishWithACommandAndNoBundleUpload() async {
        let rig = Rig()
        rig.link.readResults = [.success(readReceipt(snapshot(heldGeneration: 4, readAt: rig.clock.now)))]
        rig.assembler.preflightResults = [.success(.unchanged(
            retryAfterSeconds: 90, precipitationGeneration: "20260812T1200Z"))]
        rig.link.unchangedResults = [.success(uploadReceipt())]

        await rig.engine().kick(.deviceRaisedRequest)

        #expect(rig.assembler.preflightCalls.map(\.allowReuse) == [true])
        #expect(rig.link.unchangedCalls.count == 1)
        #expect(rig.link.unchangedCalls.first?.requestID == 7)
        #expect(rig.link.unchangedCalls.first?.retryAfterSeconds == 90)
        #expect(rig.link.uploadedPayloads.isEmpty)
        #expect(rig.store.load() == nil)
        #expect(rig.history.entries().last?.outcome == .committed)
    }

    @Test func olderFirmwareFallsBackFromUnknownCommandToAFullUpload() async {
        let rig = Rig()
        rig.link.readResults = [.success(readReceipt(snapshot(heldGeneration: 4, readAt: rig.clock.now)))]
        rig.assembler.preflightResults = [.success(.unchanged(
            retryAfterSeconds: 90, precipitationGeneration: "20260812T1200Z"))]
        rig.link.unchangedResults = [.failure(.bundleRejected)]
        rig.assembler.results = [.success(builtBundle(
            generation: 5, at: rig.clock.now, bytes: Data([9, 8, 7])))]
        rig.link.uploadResults = [.success(uploadReceipt())]

        await rig.engine().kick(.deviceRaisedRequest)

        #expect(rig.link.unchangedCalls.count == 1)
        #expect(rig.assembler.calls.map(\.generation) == [5])
        #expect(rig.link.uploadedPayloads == [Data([9, 8, 7])])
        #expect(rig.store.load() == nil)
        #expect(rig.history.entries().last?.outcome == .committed)
    }

    @Test func contextIsCheckpointedBeforeTheFetchSoARelaunchNeverRereads() async {
        let rig = Rig()
        rig.link.readResults = [.success(readReceipt(snapshot(readAt: rig.clock.now)))]
        // The fetch fails — the read must already be on disk.
        rig.assembler.results = [.failure(WeatherProviderError.unavailable)]
        await rig.engine().kick(.deviceRaisedRequest)

        let persisted = rig.store.load()
        #expect(persisted?.phase == .fetching)
        #expect(persisted?.snapshot?.requestID == 7)
        #expect(persisted?.attempts == 1)
        #expect(persisted?.notBefore != nil)

        // "Relaunch": a fresh engine over the same stores, after the cooldown.
        rig.clock.advance(60)
        rig.assembler.results = [.success(builtBundle(generation: 1, at: rig.clock.now))]
        rig.link.uploadResults = [.success(uploadReceipt())]
        await rig.engine().kick(.resume)

        #expect(rig.link.readCalls == 1, "the persisted context is resumed, not re-read")
        #expect(rig.history.entries().last?.outcome == .committed)
        #expect(rig.store.load() == nil)
    }

    @Test func resumeHonoursTheRetryCooldownButADeviceDiscoveryOverridesIt() async {
        let rig = Rig()
        rig.link.readResults = [.success(readReceipt(snapshot(readAt: rig.clock.now)))]
        rig.assembler.results = [.failure(WeatherProviderError.unavailable)]
        await rig.engine().kick(.deviceRaisedRequest)
        #expect(rig.assembler.calls.count == 1)

        // Within the cooldown, a resume does nothing…
        await rig.engine().kick(.resume)
        #expect(rig.assembler.calls.count == 1)

        // …but the device re-advertising is its ladder speaking, and outranks our cooldown.
        rig.assembler.results = [.success(builtBundle(generation: 1, at: rig.clock.now))]
        rig.link.uploadResults = [.success(uploadReceipt())]
        await rig.engine().kick(.deviceRaisedRequest)
        #expect(rig.assembler.calls.count == 2)
        #expect(rig.history.entries().last?.outcome == .committed)
    }

    @Test func relaunchAtBundleReadyUploadsThePersistedBytesWithoutRefetching() async {
        let rig = Rig()
        rig.link.readResults = [.success(readReceipt(snapshot(readAt: rig.clock.now)))]
        rig.assembler.results = [.success(builtBundle(generation: 1, at: rig.clock.now, bytes: Data([9, 9])))]
        rig.link.uploadResults = [.failure(.connectionDropped)]
        await rig.engine().kick(.deviceRaisedRequest)

        let persisted = rig.store.load()
        #expect(persisted?.phase == .bundleReady)
        #expect(persisted?.bundleBytes == Data([9, 9]))

        rig.clock.advance(60)
        rig.link.uploadResults = [.success(uploadReceipt())]
        await rig.engine().kick(.resume)

        #expect(rig.assembler.calls.count == 1, "the persisted bundle is reused, not rebuilt")
        #expect(rig.link.uploadedPayloads == [Data([9, 9]), Data([9, 9])],
                "the retry re-sends the same bytes — a duplicate answers committed (§11.6)")
        #expect(rig.store.load() == nil)
    }

    @Test func aRejectedBundleForcesARebuildInsteadOfARetryOfTheSameBytes() async {
        let rig = Rig()
        rig.link.readResults = [.success(readReceipt(snapshot(readAt: rig.clock.now)))]
        rig.assembler.results = [
            .success(builtBundle(generation: 1, at: rig.clock.now, bytes: Data([1]))),
            .success(builtBundle(generation: 1, at: rig.clock.now, bytes: Data([2]))),
        ]
        rig.link.uploadResults = [.failure(.bundleRejected), .success(uploadReceipt())]
        await rig.engine().kick(.deviceRaisedRequest)

        #expect(rig.store.load()?.phase == .fetching, "rejected bytes are discarded, not retried")
        #expect(rig.store.load()?.bundleBytes == nil)

        rig.clock.advance(60)
        await rig.engine().kick(.resume)
        #expect(rig.assembler.calls.count == 2)
        #expect(rig.link.uploadedPayloads.last == Data([2]))
        #expect(rig.history.entries().last?.outcome == .committed)
    }

    @Test func theAttemptBudgetAbandonsTheJobToTheDeviceLadder() async {
        let rig = Rig(configuration: .init(maxAttempts: 2, retryCooldown: 1))
        rig.link.readResults = [
            .failure(.timedOut), .failure(.timedOut), .failure(.timedOut),
        ]
        await rig.engine().kick(.deviceRaisedRequest)
        rig.clock.advance(5)
        await rig.engine().kick(.resume)

        #expect(rig.store.load() == nil, "an exhausted job leaves no checkpoint")
        let entry = rig.history.entries().last
        #expect(entry?.outcome == .failed)
        #expect(entry?.failureReason == .contextReadFailed)
        #expect(entry?.attempts == 2)

        // And the abandoned job does not haunt the next: a later resume does nothing.
        await rig.engine().kick(.resume)
        #expect(rig.link.readCalls == 2)
    }

    @Test func aRequestWithoutAFixFailsHonestlyWithoutFetching() async {
        let rig = Rig()
        rig.link.readResults = [.success(readReceipt(
            snapshot(latitude: nil, longitude: nil, readAt: rig.clock.now)))]
        await rig.engine().kick(.deviceRaisedRequest)

        #expect(rig.assembler.calls.isEmpty)
        #expect(rig.store.load() == nil)
        let entry = rig.history.entries().last
        #expect(entry?.outcome == .failed)
        #expect(entry?.failureReason == .noPosition)
    }

    @Test func aNewerRequestSupersedesAStaleBundleAndRebuilds() async {
        let rig = Rig()
        rig.link.readResults = [.success(readReceipt(snapshot(requestID: 7, readAt: rig.clock.now)))]
        rig.assembler.results = [
            .success(builtBundle(generation: 1, at: rig.clock.now, bytes: Data([1]))),
            .success(builtBundle(generation: 1, requestID: 8, at: rig.clock.now, bytes: Data([2]))),
        ]
        rig.link.uploadResults = [.failure(.connectionDropped), .success(uploadReceipt())]
        await rig.engine().kick(.deviceRaisedRequest)
        #expect(rig.store.load()?.phase == .bundleReady)

        // A *new* request (new id) from a rider now outside the built window.
        let moved = snapshot(
            requestID: 8, latitude: 52_500_000, longitude: 13_400_000, readAt: rig.clock.now)
        await rig.engine().kick(.contextRead(moved, readConnectedMilliseconds: 1_500))

        let entries = rig.history.entries()
        #expect(entries.contains { $0.outcome == .superseded && $0.requestID == 7 })
        #expect(rig.assembler.calls.count == 2)
        #expect(rig.assembler.calls.last?.requestID == 8)
        #expect(rig.link.uploadedPayloads.last == Data([2]))
        #expect(entries.last?.outcome == .committed)
    }

    @Test func aLadderStepWithTheSameIdReusesAFreshCoveringBundle() async {
        let rig = Rig()
        rig.link.readResults = [.success(readReceipt(snapshot(requestID: 7, readAt: rig.clock.now)))]
        rig.assembler.results = [.success(builtBundle(generation: 1, at: rig.clock.now, bytes: Data([5])))]
        rig.link.uploadResults = [.failure(.connectionDropped), .success(uploadReceipt())]
        await rig.engine().kick(.deviceRaisedRequest)
        #expect(rig.store.load()?.phase == .bundleReady)

        // The ladder re-raises with the same id; the rider is still inside the window.
        rig.clock.advance(300)
        let step = snapshot(requestID: 7, readAt: rig.clock.now)
        await rig.engine().kick(.contextRead(step, readConnectedMilliseconds: nil))

        #expect(rig.assembler.calls.count == 1, "a covering, fresh bundle is not rebuilt")
        #expect(rig.link.uploadedPayloads == [Data([5]), Data([5])])
        #expect(rig.history.entries().last?.outcome == .committed)
    }

    @Test func aDeviceAlreadyHoldingOurGenerationForcesARebuild() async {
        let rig = Rig()
        rig.link.readResults = [.success(readReceipt(snapshot(requestID: 7, readAt: rig.clock.now)))]
        rig.assembler.results = [
            .success(builtBundle(generation: 1, at: rig.clock.now, bytes: Data([1]))),
            .success(builtBundle(generation: 2, at: rig.clock.now, bytes: Data([2]))),
        ]
        rig.link.uploadResults = [.failure(.connectionDropped), .success(uploadReceipt())]
        await rig.engine().kick(.deviceRaisedRequest)

        // Another attempt's upload landed meanwhile: the device now holds generation 1 — our
        // persisted generation-1 bundle would be stale on arrival.
        rig.clock.advance(60)
        let step = snapshot(requestID: 7, heldGeneration: 1, readAt: rig.clock.now)
        await rig.engine().kick(.contextRead(step, readConnectedMilliseconds: nil))

        #expect(rig.assembler.calls.count == 2)
        #expect(rig.assembler.calls.last?.generation == 2)
        #expect(rig.link.uploadedPayloads.last == Data([2]))
    }

    @Test func theEchoOfACommittedReadDoesNotRestartTheJob() async {
        let rig = Rig()
        let read = snapshot(requestID: 7, readAt: rig.clock.now)
        rig.link.readResults = [.success(readReceipt(read))]
        rig.assembler.results = [.success(builtBundle(generation: 1, at: rig.clock.now))]
        rig.link.uploadResults = [.success(uploadReceipt())]
        // One engine for both kicks: the echo arrives in the same process that committed (the
        // transport replays the event to its late subscriber); a relaunched process has a fresh
        // event stream, so the guard is deliberately in-memory.
        let engine = rig.engine()
        await engine.kick(.deviceRaisedRequest)
        #expect(rig.history.entries().count == 1)

        // The transport replays the completed read to late subscribers; the engine must not
        // treat its own answered request as a fresh one.
        await engine.kick(.contextRead(read, readConnectedMilliseconds: 2_000))
        #expect(rig.assembler.calls.count == 1)
        #expect(rig.link.uploadedPayloads.count == 1)
        #expect(rig.history.entries().count == 1)
    }

    @Test func aBundleThatSleptPastItsMaxAgeIsRebuiltNotUploaded() async {
        let rig = Rig()
        rig.link.readResults = [.success(readReceipt(snapshot(readAt: rig.clock.now)))]
        rig.assembler.results = [
            .success(builtBundle(generation: 1, at: rig.clock.now, bytes: Data([1]))),
            .success(builtBundle(generation: 1, at: rig.clock.now, bytes: Data([2]))),
        ]
        rig.link.uploadResults = [.failure(.connectionDropped), .success(uploadReceipt())]
        await rig.engine().kick(.deviceRaisedRequest)
        #expect(rig.store.load()?.phase == .bundleReady)

        // The phone slept for an hour; that bundle is yesterday's weather now.
        rig.clock.advance(3_600)
        await rig.engine().kick(.resume)
        #expect(rig.assembler.calls.count == 2)
        #expect(rig.link.uploadedPayloads.last == Data([2]))
    }

    @Test func aStaleCheckpointFromAPastRideIsDroppedNotFinished() async {
        let rig = Rig()
        rig.link.readResults = [.success(readReceipt(snapshot(readAt: rig.clock.now)))]
        rig.assembler.results = [.failure(WeatherProviderError.unavailable)]
        await rig.engine().kick(.deviceRaisedRequest)
        #expect(rig.store.load() != nil)

        // Three hours later (job lifetime is two): the checkpoint is history, literally.
        rig.clock.advance(3 * 3_600)
        await rig.engine().kick(.resume)
        #expect(rig.store.load() == nil)
        #expect(rig.assembler.calls.count == 1, "the dead job did not fetch again")
        // Its own outcome, not `.failed`: no leg failed here, a clock ran out (#1198 review).
        #expect(rig.history.entries().last?.outcome == .agedOut)
        // …and it says *why*: time ran out. It used to read `attemptsExhausted`, which was false
        // twice over — one attempt was spent, and nothing was exhausted (#1227 follow-up).
        #expect(rig.history.entries().last?.failureReason == .agedOut)
        #expect(rig.history.entries().last?.attempts == 1)
    }

    /// The crc-vs-drop split (#1227 follow-up). Both keep the bundle and re-upload it, so the
    /// engine's *behaviour* is identical — which is exactly why the ring is the only place the
    /// difference can survive, and why folding them lost it.
    @Test func aCorruptedTransferIsRecordedApartFromADroppedOne() async {
        let rig = Rig(configuration: .init(maxAttempts: 1))
        rig.link.readResults = [.success(readReceipt(snapshot(readAt: rig.clock.now)))]
        rig.assembler.results = [.success(builtBundle(generation: 1, at: rig.clock.now, bytes: Data([5])))]
        rig.link.uploadResults = [.failure(.transferCorrupted)]
        await rig.engine().kick(.deviceRaisedRequest)
        #expect(rig.history.entries().last?.failureReason == .transferCorrupted)

        let dropped = Rig(configuration: .init(maxAttempts: 1))
        dropped.link.readResults = [.success(readReceipt(snapshot(readAt: dropped.clock.now)))]
        dropped.assembler.results = [
            .success(builtBundle(generation: 1, at: dropped.clock.now, bytes: Data([5]))),
        ]
        dropped.link.uploadResults = [.failure(.connectionDropped)]
        await dropped.engine().kick(.deviceRaisedRequest)
        #expect(dropped.history.entries().last?.failureReason == .uploadFailed)
    }

    /// A corrupted transfer re-sends the *same* bytes: they were correct when they left.
    @Test func aCorruptedTransferResendsTheSameBytes() async {
        let rig = Rig()
        rig.link.readResults = [.success(readReceipt(snapshot(readAt: rig.clock.now)))]
        rig.assembler.results = [.success(builtBundle(generation: 1, at: rig.clock.now, bytes: Data([5])))]
        rig.link.uploadResults = [.failure(.transferCorrupted), .success(uploadReceipt())]
        await rig.engine().kick(.deviceRaisedRequest)
        rig.clock.advance(rig.configuration.retryCooldown + 1)
        await rig.engine().kick(.resume)
        #expect(rig.assembler.calls.count == 1, "corruption on the wire is not a producer bug")
        #expect(rig.link.uploadedPayloads == [Data([5]), Data([5])])
    }

    /// A ladder re-read landing on a bundle the app slept past records *aged out*, not
    /// *superseded*: nothing superseded it — the same request is still being answered.
    @Test func aBundleTheAppSleptPastIsRecordedAsAgedOutNotSuperseded() async {
        let rig = Rig()
        rig.link.readResults = [.success(readReceipt(snapshot(requestID: 7, readAt: rig.clock.now)))]
        rig.assembler.results = [
            .success(builtBundle(generation: 1, at: rig.clock.now, bytes: Data([1]))),
            .success(builtBundle(generation: 1, at: rig.clock.now, bytes: Data([2]))),
        ]
        rig.link.uploadResults = [.failure(.connectionDropped), .success(uploadReceipt())]
        await rig.engine().kick(.deviceRaisedRequest)
        #expect(rig.store.load()?.phase == .bundleReady)

        // An hour asleep (past bundleMaxAge, inside jobLifetime), then the device's ladder step —
        // the *same* request id, re-read by the transport's standing watch.
        rig.clock.advance(3_600)
        await rig.engine().kick(
            .contextRead(snapshot(requestID: 7, readAt: rig.clock.now),
                         readConnectedMilliseconds: 1_400))
        let rows = rig.history.entries()
        #expect(rows.first?.failureReason == .agedOut)
        // …and it is not painted as a failure either: the same job carried on and delivered two
        // lines below, so the row is information, not an alarm (#1198 review).
        #expect(rows.first?.outcome == .agedOut)
        #expect(rows.last?.outcome == .committed)
        #expect(rig.link.uploadedPayloads.last == Data([2]), "the rebuilt bundle, not the old one")
    }

    /// The **resume** half of the same horizon. `advance()`'s own `bundleReady` expiry check —
    /// the app simply waking up past `bundleMaxAge`, with no re-read to trigger `adopt` — used to
    /// bin the bundle in silence, so a paid-for corridor fetch vanished from the ring and only one
    /// of the engine's two expiry routes was visible (#1198 review). Same event, same row.
    @Test func aBundleThatExpiredWhileTheAppSleptIsRecordedOnTheResumePathToo() async {
        let rig = Rig()
        rig.link.readResults = [.success(readReceipt(snapshot(requestID: 7, readAt: rig.clock.now)))]
        rig.assembler.results = [
            .success(builtBundle(generation: 1, at: rig.clock.now, bytes: Data([1]))),
            .success(builtBundle(generation: 2, at: rig.clock.now, bytes: Data([2]))),
        ]
        rig.link.uploadResults = [.failure(.connectionDropped), .success(uploadReceipt())]
        await rig.engine().kick(.deviceRaisedRequest)
        #expect(rig.store.load()?.phase == .bundleReady)
        #expect(rig.history.entries().isEmpty, "a retryable drop is not a finished exchange")

        // An hour asleep — past bundleMaxAge, well inside jobLifetime — then a plain foreground
        // resume. No context read: the checkpoint alone drives this.
        rig.clock.advance(3_600)
        await rig.engine().kick(.resume)

        let rows = rig.history.entries()
        #expect(rows.count == 2)
        #expect(rows.first?.outcome == .agedOut)
        #expect(rows.first?.failureReason == .agedOut)
        #expect(rows.first?.phaseReached == .bundleReady)
        #expect(rows.last?.outcome == .committed)
        #expect(rig.link.readCalls == 1, "the resume finished the job without a second read leg")
        #expect(rig.assembler.calls.count == 2, "the expired bundle was rebuilt, not sent")
        #expect(rig.link.uploadedPayloads.last == Data([2]))
    }

    /// *Retry now* waives the cooldown a `.resume` honours — and still refuses to invent work.
    @Test func aRiderRetryWaivesTheCooldownButNeverManufacturesARequest() async {
        let rig = Rig()
        rig.link.readResults = [.success(readReceipt(snapshot(readAt: rig.clock.now)))]
        rig.assembler.results = [
            .success(builtBundle(generation: 1, at: rig.clock.now, bytes: Data([3]))),
        ]
        rig.link.uploadResults = [.failure(.connectionDropped), .success(uploadReceipt())]
        await rig.engine().kick(.deviceRaisedRequest)
        #expect(rig.store.load()?.notBefore != nil)

        // No clock advance at all: the cooldown is still in force, and `.resume` would sit it out.
        await rig.engine().kick(.userRetry)
        #expect(rig.history.entries().last?.outcome == .committed)
        #expect(rig.store.load() == nil)

        // Nothing owed now — a tap must not start a read leg the device never asked for.
        await rig.engine().kick(.userRetry)
        #expect(rig.link.readCalls == 1)
    }

    /// The other half of "only what is owed": when the checkpoint *is* parked at the read leg, a
    /// tap runs it. The device raised the request — its advertisement is what created the
    /// checkpoint — so re-reading finishes an exchange rather than manufacturing one, and this is
    /// the failure a rider is most likely to be staring at when they reach for Retry now. The
    /// `.userRetry` doc used to claim no read leg is ever started; it is, deliberately (#1198
    /// review).
    @Test func aRiderRetryDoesRunTheReadLegTheDeviceAlreadyAskedFor() async {
        let rig = Rig()
        rig.link.readResults = [
            .failure(.timedOut),
            .success(readReceipt(snapshot(readAt: rig.clock.now))),
        ]
        rig.assembler.results = [
            .success(builtBundle(generation: 1, at: rig.clock.now, bytes: Data([9]))),
        ]
        rig.link.uploadResults = [.success(uploadReceipt())]
        await rig.engine().kick(.deviceRaisedRequest)
        #expect(rig.store.load()?.phase == .readingContext, "parked at the read leg")
        #expect(rig.link.readCalls == 1)

        // No clock advance: a `.resume` here would sit out the cooldown and do nothing at all.
        await rig.engine().kick(.userRetry)
        #expect(rig.link.readCalls == 2, "the tap ran the owed read")
        #expect(rig.history.entries().last?.outcome == .committed)
    }

    /// A rider's tap does not spend the attempt budget. That budget bounds what the phone does on
    /// its own per request; if taps counted, the rider's own third press is what would abandon
    /// their job to the device's ladder (#1198 review).
    @Test func aRiderRetryDoesNotSpendAnAttempt() async {
        let rig = Rig(configuration: .init(maxAttempts: 3, retryCooldown: 30))
        rig.link.readResults = [
            .failure(.timedOut), .failure(.timedOut), .failure(.timedOut), .failure(.timedOut),
        ]
        await rig.engine().kick(.deviceRaisedRequest)
        #expect(rig.store.load()?.attempts == 1)

        // Three taps, none of them autonomous work.
        for _ in 0..<3 { await rig.engine().kick(.userRetry) }
        #expect(rig.link.readCalls == 4, "every tap still ran the owed leg")
        #expect(rig.store.load()?.attempts == 1, "the budget is untouched by taps")
        #expect(rig.store.load() != nil, "and the job is still there to finish")
        #expect(rig.history.entries().isEmpty, "nothing was abandoned")
    }

    /// …while an autonomous `.resume` still does spend one, or the budget would be decorative.
    @Test func aResumeStillSpendsAnAttempt() async {
        let rig = Rig(configuration: .init(maxAttempts: 3, retryCooldown: 30))
        rig.link.readResults = [.failure(.timedOut), .failure(.timedOut)]
        await rig.engine().kick(.deviceRaisedRequest)
        rig.clock.advance(31)
        await rig.engine().kick(.resume)
        #expect(rig.store.load()?.attempts == 2)
    }

    /// A tap that lands while a run is in flight is queued — and must survive the scene-phase
    /// `.resume` that so often lands on top of it (open the app, tap Retry now: the foreground kick
    /// and the press race). The merge table preferred `.resume`, which honours the very cooldown
    /// the tap exists to waive, so the press was swallowed (#1198 review).
    @Test func theTriggerMergeTableKeepsAQueuedRiderRetryOverAResume() async {
        let rig = Rig()
        let engine = rig.engine()
        let readAt = rig.clock.now
        #expect(await engine.merged(.userRetry, with: .resume) == .userRetry)
        // The rows that already held, re-pinned beside it: anything carrying more than a tap still
        // wins, and a tap never demotes them.
        #expect(await engine.merged(.deviceRaisedRequest, with: .userRetry) == .deviceRaisedRequest)
        #expect(await engine.merged(.userRetry, with: .deviceRaisedRequest) == .deviceRaisedRequest)
        #expect(await engine.merged(
            .contextRead(snapshot(readAt: readAt), readConnectedMilliseconds: 10),
            with: .userRetry)
            == .contextRead(snapshot(readAt: readAt), readConnectedMilliseconds: 10))
        #expect(await engine.merged(nil, with: .userRetry) == .userRetry)
    }

    /// `retryNow()` returns when the work is **finished**, not when it is scheduled. The screen
    /// binds its spinner to this call, so a tap that merely queues behind a run in flight used to
    /// stop the spinner instantly while the job carried on invisibly (#1198 review).
    @Test func retryNowReturnsOnCompletionEvenWhenItOnlyQueues() async {
        let rig = Rig()
        let gate = Gate()
        rig.link.readResults = [.success(readReceipt(snapshot(readAt: rig.clock.now)))]
        rig.assembler.gate = gate
        rig.assembler.results = [
            .success(builtBundle(generation: 1, at: rig.clock.now, bytes: Data([7]))),
        ]
        rig.link.uploadResults = [.success(uploadReceipt())]
        let engine = rig.engine()

        // A run parked inside the assembler — exactly the "fetch in flight" the trigger queue
        // exists for.
        let firstRun = Task { await engine.kick(.deviceRaisedRequest) }
        await gate.waitUntilEntered()

        let finished = Gate.Flag()
        let retry = Task {
            await engine.retryNow()
            await finished.raise()
        }
        // The tap can only have queued: the engine is busy. Its call must still be waiting.
        for _ in 0..<200 { await Task.yield() }
        #expect(await finished.isRaised == false,
                "retryNow returned while the job it asked for was still running")

        await gate.open()
        await retry.value
        await firstRun.value
        #expect(await finished.isRaised)
        #expect(rig.history.entries().last?.outcome == .committed,
                "retryNow returned only once the exchange was actually done")
    }

    /// The screen's view of the checkpoint carries no position, by construction.
    @Test func thePendingProjectionIsCoordinateFree() async {
        let rig = Rig()
        rig.link.readResults = [.success(readReceipt(snapshot(requestID: 11, readAt: rig.clock.now)))]
        rig.assembler.results = [.failure(WeatherProviderError.unavailable)]
        let engine = rig.engine()
        await engine.kick(.deviceRaisedRequest)

        #expect(rig.store.load()?.snapshot?.latitudeMicrodegrees != nil, "the checkpoint holds it")
        let pending = await engine.pendingJob()
        #expect(pending?.requestID == 11)
        #expect(pending?.phase == .fetching)
        #expect(pending?.attempts == 1)
        #expect(pending?.retryNotBefore != nil)
        // The projection is a fixed set of scalars, and this pins the set: adding a field to the
        // checkpoint must not quietly widen what a screen can hold.
        #expect(Mirror(reflecting: pending!).children.compactMap(\.label).sorted() == [
            "attempts", "bundleByteCount", "deferrals", "phase", "requestID", "retryNotBefore",
            "startedAt", "updatedAt",
        ])
    }

    /// A brand-new request id arriving mid-*fetch* abandons that fetch just as surely as one
    /// arriving at `bundleReady` — the ring must show the work ending, not vanishing.
    @Test func aNewRequestDuringTheFetchPhaseGetsItsSupersedeRow() async {
        let rig = Rig()
        rig.link.readResults = [.success(readReceipt(snapshot(requestID: 7, readAt: rig.clock.now)))]
        // The first job never gets past `.fetching`: its build fails.
        rig.assembler.results = [
            .failure(WeatherProviderError.unavailable),
            .success(builtBundle(generation: 1, requestID: 8, at: rig.clock.now, bytes: Data([9]))),
        ]
        rig.link.uploadResults = [.success(uploadReceipt())]
        await rig.engine().kick(.deviceRaisedRequest)
        #expect(rig.store.load()?.phase == .fetching)
        #expect(rig.history.entries().isEmpty)

        await rig.engine().kick(
            .contextRead(snapshot(requestID: 8, readAt: rig.clock.now), readConnectedMilliseconds: nil))

        let entries = rig.history.entries()
        #expect(entries.first?.outcome == .superseded)
        #expect(entries.first?.requestID == 7)
        #expect(entries.first?.phaseReached == .fetching)
        #expect(entries.last?.outcome == .committed)
        #expect(entries.last?.requestID == 8)
    }

    /// The device's ladder re-reads the *same* request every 5/10/20 minutes. If each re-read reset
    /// the job's birthday, `jobLifetime` could never elapse and a job could ride out a whole day.
    @Test func aLadderReReadKeepsTheJobsOriginalBirthday() async {
        // Attempts deliberately out of the way: what is under test is the *lifetime*, and with the
        // default budget the job would exhaust its attempts before its birthday could matter.
        let rig = Rig(configuration: .init(maxAttempts: 100))
        rig.link.readResults = [.success(readReceipt(snapshot(requestID: 7, readAt: rig.clock.now)))]
        rig.assembler.results = Array(
            repeating: .failure(WeatherProviderError.unavailable), count: 8)
        let born = rig.clock.now
        await rig.engine().kick(.deviceRaisedRequest)
        #expect(rig.store.load()?.startedAt == born)

        // 100 minutes of ladder steps on the same request id.
        for _ in 0..<5 {
            rig.clock.advance(20 * 60)
            await rig.engine().kick(
                .contextRead(snapshot(requestID: 7, readAt: rig.clock.now),
                             readConnectedMilliseconds: nil))
        }
        #expect(rig.store.load()?.startedAt == born, "a re-read of the same request is not a new job")

        // Past the two-hour lifetime the job is dropped — only reachable because the birthday
        // survived the re-reads.
        rig.clock.advance(30 * 60)
        await rig.engine().kick(.resume)
        #expect(rig.store.load() == nil)
        #expect(rig.history.entries().last?.outcome == .agedOut)
    }

    /// `storageFull` / `notFound` / `busy` say nothing about the bytes. Binning a good bundle and
    /// re-fetching the whole corridor for them (six times over) was the old behaviour.
    @Test func aDeviceThatCannotTakeTheBundleNowKeepsTheBytesAndDoesNotBurnAnAttempt() async {
        let rig = Rig()
        rig.link.readResults = [.success(readReceipt(snapshot(readAt: rig.clock.now)))]
        rig.assembler.results = [.success(builtBundle(generation: 1, at: rig.clock.now, bytes: Data([7])))]
        rig.link.uploadResults = [.failure(.deviceBusy), .success(uploadReceipt())]
        await rig.engine().kick(.deviceRaisedRequest)

        let deferred = rig.store.load()
        #expect(deferred?.phase == .bundleReady, "the built bytes stay owed, not re-fetched")
        #expect(deferred?.bundleBytes == Data([7]))
        #expect(deferred?.attempts == 0, "the device asking us to wait is not our attempt")
        #expect(deferred?.deferrals == 1)
        #expect(deferred?.notBefore != nil)

        rig.clock.advance(rig.configuration.retryCooldown + 1)
        await rig.engine().kick(.resume)
        #expect(rig.assembler.calls.count == 1, "no corridor re-fetch for a 'not now'")
        #expect(rig.link.uploadedPayloads == [Data([7]), Data([7])])
        #expect(rig.history.entries().last?.outcome == .committed)
    }

    /// …but a device that is *permanently* unable to take it cannot loop for the job's lifetime:
    /// past the attempt budget a deferral degrades into an ordinary attempt.
    @Test func endlessDeferralsStillEndTheJob() async {
        let rig = Rig(configuration: .init(maxAttempts: 2, retryCooldown: 10))
        rig.link.readResults = [.success(readReceipt(snapshot(readAt: rig.clock.now)))]
        rig.assembler.results = [.success(builtBundle(generation: 1, at: rig.clock.now, bytes: Data([7])))]
        rig.link.uploadResults = Array(repeating: .failure(.deviceBusy), count: 8)
        await rig.engine().kick(.deviceRaisedRequest)
        for _ in 0..<5 {
            rig.clock.advance(11)
            await rig.engine().kick(.resume)
        }
        #expect(rig.store.load() == nil, "the job is abandoned to the device's ladder in the end")
        #expect(rig.history.entries().last?.outcome == .failed)
        #expect(rig.history.entries().last?.failureReason == .deviceUnavailable)
    }

    /// §11.4's idle attribute (validity 0, reason 0, no nonce) is what a device with nothing due
    /// answers a read with. Running a job for it manufactures a `noPosition` row with request id 0
    /// at ladder rate — the diagnostics ring would fill with failures nobody caused.
    @Test func anIdleContextIsNotARequestAndWritesNoHistoryRow() async {
        let rig = Rig()
        let idle = WeatherDeviceRequestSnapshot(requestID: 0, readAt: rig.clock.now)
        #expect(!idle.carriesRequest)
        rig.link.readResults = [.success(readReceipt(idle))]
        await rig.engine().kick(.deviceRaisedRequest)
        #expect(rig.history.entries().isEmpty)
        #expect(rig.store.load() == nil)
        #expect(rig.assembler.calls.isEmpty)

        // A genuinely fixless *request* still carries its nonce, and still fails honestly.
        #expect(snapshot(requestID: 4, latitude: nil, longitude: nil, readAt: rig.clock.now)
            .carriesRequest)
    }

    @Test func serialGenerationArithmeticMatchesTheDeviceRule() {
        #expect(serialIsNewer(1, than: 0))
        #expect(!serialIsNewer(0, than: 1))
        #expect(!serialIsNewer(5, than: 5))
        // Across the wrap: 0 is newer than 0xFFFFFFFF, exactly like the device's slot selector.
        #expect(serialIsNewer(0, than: UInt32.max))
        #expect(!serialIsNewer(UInt32.max, than: 0))
    }

    @Test func nextGenerationIsSeriallyOnePastTheHeldOneAcrossTheWrap() {
        let held = snapshot(heldGeneration: UInt32.max, readAt: Date())
        #expect(held.nextGeneration == 0)
        #expect(serialIsNewer(held.nextGeneration, than: UInt32.max))
        let none = snapshot(heldGeneration: nil, readAt: Date())
        #expect(none.nextGeneration == 1)
    }

    @Test func theHistoryRingCarriesNoCoordinateInAnyField() throws {
        // Structural, not behavioural: encode a fully populated entry and assert the JSON has no
        // key a coordinate could hide behind. The snapshot (which does carry the rider position)
        // lives in the job checkpoint and dies with it — never in this type.
        let entry = WeatherJobHistoryEntry(
            startedAt: Date(), finishedAt: Date(), requestID: 9, outcome: .committed,
            failureReason: nil, phaseReached: .uploading, attempts: 1, bundleByteCount: 46_000,
            readConnectedMilliseconds: 1_800, uploadConnectedMilliseconds: 3_200,
            precipitationGeneration: "dwd-rv", noRainMapReason: nil)
        let json = String(decoding: try JSONEncoder().encode(entry), as: UTF8.self).lowercased()
        for needle in ["lat", "lon", "coordinate", "position", "fix", "degree"] {
            #expect(!json.contains(needle), "history JSON must not carry '\(needle)'")
        }
    }
}
