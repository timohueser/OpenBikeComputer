import Foundation
import Network
import Observation
import OBCPlanner
import SwiftUI

@MainActor @Observable
public final class OfflineMapsModel {
    public private(set) var maps: [OfflineMap] = []
    public private(set) var coverage: OfflineCoverage?
    public private(set) var availableBytes: Int64?
    public private(set) var quote: OfflineDownloadQuote?
    public private(set) var isBusy = false
    public private(set) var isDownloading = false
    public private(set) var hasStartedDownload = false
    public private(set) var isStopping = false
    public private(set) var preparationStarted: Date?
    public private(set) var needsMobileConsent = true
    public private(set) var status = ""
    public private(set) var fraction = 0.0
    public private(set) var transferred: Int64 = 0
    public private(set) var transferTotal: Int64 = 0
    public var error: String?
    public var revision = 0
    @ObservationIgnored private let store: OfflineMapStore
    @ObservationIgnored private let monitor = NWPathMonitor()
    @ObservationIgnored private var operation: Task<Void, Never>?

    public init(store: OfflineMapStore) {
        self.store = store
        monitor.pathUpdateHandler = { [weak self] path in
            let metered = path.isExpensive || path.isConstrained || path.usesInterfaceType(.cellular)
            Task { @MainActor [weak self] in self?.needsMobileConsent = metered }
        }
        monitor.start(queue: DispatchQueue(label: "offline-maps.network"))
    }

    deinit { monitor.cancel(); operation?.cancel() }

    public func refresh() async {
        do {
            maps = try await store.maps(); availableBytes = try await store.availableBytes()
            if !isBusy, let pending = try await store.pending() {
                quote = pending; hasStartedDownload = true; status = "Download paused"
            }
        }
        catch { self.error = error.localizedDescription }
    }

    func loadCoverage() async {
        guard coverage == nil else { return }
        error = nil
        do { coverage = try await store.coverage() }
        catch is CancellationError {}
        catch { self.error = "Map coverage is unavailable. Check your connection and try again." }
    }

    func prepare(bounds: [Double], name: String) {
        guard !isBusy, !hasStartedDownload else { return }
        quote = nil; error = nil; isBusy = true; fraction = 0; preparationStarted = Date(); status = "Checking map coverage"
        operation = Task { [weak self, store] in
            guard let self else { return }
            defer { isBusy = false; isStopping = false; preparationStarted = nil; operation = nil }
            do {
                let quote = try await store.prepare(bounds: bounds, name: name) { [weak self] fraction, message in
                    Task { @MainActor [weak self] in
                        guard let self, isBusy, !isStopping else { return }
                        self.fraction = fraction; status = message
                    }
                }
                try Task.checkCancellation()
                self.quote = quote
                availableBytes = try await store.availableBytes()
                status = "Review download"
            } catch is CancellationError { status = "" }
            catch { self.error = error.localizedDescription; status = "" }
        }
    }

    func download(allowMobileData: Bool) {
        guard let quote, !isBusy else { return }
        let name = quote.map.name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return }
        let namedQuote = quote.named(name)
        self.quote = namedQuote
        hasStartedDownload = true
        isBusy = true; isDownloading = true; error = nil; fraction = 0; status = "Starting download…"
        transferTotal = namedQuote.transferBytes
        operation = Task { [weak self, store] in
            guard let self else { return }
            do {
                try await store.install(namedQuote, allowMobileData: allowMobileData) { [weak self] received, total, message in
                    Task { @MainActor [weak self] in
                        guard let self, isDownloading else { return }
                        transferred = received; transferTotal = total
                        fraction = total > 0 ? min(1, Double(received) / Double(total)) : 0
                        status = message
                    }
                }
                isDownloading = false
                try Task.checkCancellation()
                self.quote = nil; hasStartedDownload = false; status = "Ready offline"; revision += 1
                await refresh()
            } catch is CancellationError { status = "Download paused" }
            catch {
                status = Task.isCancelled ? "Download paused" : "Download interrupted"
                if !Task.isCancelled { self.error = error.localizedDescription }
            }
            isBusy = false; isDownloading = false; operation = nil
            await refresh()
        }
    }

    func stop() {
        guard !isStopping else { return }
        if !isDownloading { isStopping = true; status = "Cancelling…" }
        operation?.cancel()
    }
    func rename(_ name: String) {
        guard !isBusy, !hasStartedDownload else { return }
        quote = quote?.named(name)
    }

    func clearSelection() {
        guard !isBusy, !hasStartedDownload else { return }
        quote = nil; error = nil; status = ""
    }

    func discard() async {
        guard !isBusy else { return }
        do { try await store.discardPending(); hasStartedDownload = false; clearSelection(); await refresh() }
        catch { self.error = error.localizedDescription }
    }

    func remove(_ map: OfflineMap) async {
        do { try await store.remove(map); revision += 1; await refresh() }
        catch { self.error = error.localizedDescription }
    }

    static func bytes(_ count: Int64) -> String { ByteCountFormatter.string(fromByteCount: count, countStyle: .file) }
}

private struct OfflineMapsKey: EnvironmentKey { static let defaultValue: OfflineMapsModel? = nil }
extension EnvironmentValues {
    public var obcOfflineMaps: OfflineMapsModel? {
        get { self[OfflineMapsKey.self] }
        set { self[OfflineMapsKey.self] = newValue }
    }
}
