import Foundation
import Testing
import OBCDomain
import OBCTransport
@testable import OBCUI

@MainActor
struct UpdateSurfaceModelTests {
    private let release = FirmwareRelease(
        version: "1.4.0", bytes: 10, sha256: String(repeating: "a", count: 64),
        url: URL(string: "https://updates.openbikecomputer.com/fw/UPDATE.BIN")!
    )

    @Test func setupSuppressesOffersWithoutAnsweringThem() async throws {
        let store = InMemoryUpdateSurfaceStore()
        let notifier = SurfaceNotifier()
        let model = UpdateSurfaceModel(
            transport: SurfaceLink(), bondStore: SurfaceBondStore(),
            runner: UpdateSurfaceRunner(
                checker: UpdateChecker(store: InMemoryUpdateCheckStore(
                    record: UpdateCheckRecord(release: release, checkedAt: Date())
                )), store: store
            ), notifier: notifier
        )
        model.setEnabled(false)
        model.start()
        model.appBecameActive()
        #expect(await neverHolds({ model.pending != nil }, for: .milliseconds(50)))
        #expect(notifier.requests == 0)
        #expect(!store.loadDidAskNotificationPermission())

        model.setEnabled(true)
        model.appBecameActive()
        try await waitFor("offer after setup") { model.pending != nil }
        model.setEnabled(false)
        #expect(model.pending == nil)
        #expect(store.loadAnsweredVersion(device: "OBC-001") == nil)

        model.setEnabled(true)
        model.appBecameActive()
        try await waitFor("offer remains unanswered") { model.pending != nil }
        model.dismiss()
        #expect(store.loadAnsweredVersion(device: "OBC-001") == "1.4.0")
    }

    @Test func aCheckThatFinishesDuringSetupCannotOfferOrRequestPermission() async throws {
        let fetcher = HeldSurfaceFetcher()
        let store = InMemoryUpdateSurfaceStore()
        let cache = InMemoryUpdateCheckStore()
        let notifier = SurfaceNotifier()
        let model = UpdateSurfaceModel(
            transport: SurfaceLink(), bondStore: SurfaceBondStore(),
            runner: UpdateSurfaceRunner(
                checker: UpdateChecker(fetcher: fetcher, store: cache), store: store
            ), notifier: notifier
        )
        model.appBecameActive()
        try await waitFor("manifest request") { fetcher.started.current == true }
        model.setEnabled(false)
        fetcher.response.fulfill((200, try JSONEncoder().encode(release)))
        try await waitFor("late check completion") { cache.loadCheck() != nil }
        #expect(await neverHolds({ model.pending != nil }, for: .milliseconds(50)))
        #expect(notifier.requests == 0)
        #expect(!store.loadDidAskNotificationPermission())

        model.setEnabled(true)
        model.appBecameActive()
        try await waitFor("new check after setup") { model.pending != nil }
        try await waitFor("permission request") { notifier.requests == 1 }
        #expect(model.pending?.release == release)
    }
}

private struct SurfaceLink: DeviceLink {
    var state: AsyncStream<ConnectionState> {
        AsyncStream { $0.yield(.connected); $0.finish() }
    }
    func connect() async throws {}
    func disconnect() async {}
    func deviceInfo() async throws -> DeviceInfo {
        DeviceInfo(name: "Trailhead", firmwareVersion: "1.3.0", serial: "OBC-001")
    }
}

private struct SurfaceBondStore: BondStore {
    func load() -> BondRecord? { BondRecord(deviceName: "Trailhead") }
    func save(_ record: BondRecord) {}
    func clear() {}
}

private final class SurfaceNotifier: UpdateNotifying, @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0
    var requests: Int { lock.withLock { count } }
    func requestAuthorization() async { lock.withLock { count += 1 } }
    func notifyUpdateAvailable(version: String, deviceName: String) async -> Bool { false }
}

private struct HeldSurfaceFetcher: ManifestFetching {
    let started = AsyncPromise<Bool>()
    let response = AsyncPromise<(Int, Data)>()
    func get(_ url: URL) async throws -> (status: Int, body: Data) {
        started.fulfill(true)
        return await response.value
    }
}
