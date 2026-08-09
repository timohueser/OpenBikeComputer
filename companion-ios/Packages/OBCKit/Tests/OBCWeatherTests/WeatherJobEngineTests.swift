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
    private(set) var readCalls = 0
    private(set) var uploadedPayloads: [Data] = []

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
}

private final class ScriptedAssembler: WeatherAssembling, @unchecked Sendable {
    private let lock = NSLock()
    var results: [Result<BuiltWeatherBundle, Error>] = []
    private(set) var calls: [(requestID: UInt32, generation: UInt32)] = []

    func assemble(
        request: WeatherRequest, generation: UInt32, now: Date
    ) async throws -> BuiltWeatherBundle {
        let result: Result<BuiltWeatherBundle, Error>? = lock.withLock {
            calls.append((request.requestID, generation))
            return results.isEmpty ? nil : results.removeFirst()
        }
        guard let result else { throw WeatherProviderError.unavailable }
        return try result.get()
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
        hourly: hourly, precipitation: nil, noRainMapReason: .corridorNotCovered,
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
        #expect(rig.history.entries().last?.outcome == .failed)
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
            precipitationProductID: "dwd-rv", noRainMapReason: nil)
        let json = String(decoding: try JSONEncoder().encode(entry), as: UTF8.self).lowercased()
        for needle in ["lat", "lon", "coordinate", "position", "fix", "degree"] {
            #expect(!json.contains(needle), "history JSON must not carry '\(needle)'")
        }
    }
}
