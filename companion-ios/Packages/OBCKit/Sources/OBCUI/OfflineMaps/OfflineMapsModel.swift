import Foundation
import Network
import Observation
import OBCPlanner
import SwiftUI

@MainActor @Observable
public final class OfflineMapsModel {
    public private(set) var maps: [OfflineMap] = []
    public private(set) var regions: [OfflineRegion] = []
    public private(set) var availableBytes: Int64?
    public private(set) var quote: OfflineDownloadQuote?
    public private(set) var isBusy = false
    public private(set) var isDownloading = false
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
                quote = pending; status = "Download paused"
            }
        }
        catch { self.error = error.localizedDescription }
    }

    func loadRegions() async {
        guard regions.isEmpty else { return }
        do { regions = try await store.regions() }
        catch is CancellationError {} catch { self.error = error.localizedDescription }
    }

    func prepare(bounds: [Double], region: String?, name: String) {
        guard !isBusy else { return }
        quote = nil; error = nil; isBusy = true; status = "Preparing your map…"
        operation = Task { [weak self, store] in
            guard let self else { return }
            defer { isBusy = false; operation = nil }
            do {
                let quote = try await store.prepare(bounds: bounds, region: region, name: name) { _ in }
                try Task.checkCancellation()
                self.quote = quote
                availableBytes = try await store.availableBytes()
                status = "Review download"
            } catch is CancellationError { status = "" }
            catch { if !Task.isCancelled { self.error = error.localizedDescription }; status = "" }
        }
    }

    func download(allowMobileData: Bool) {
        guard let quote, !isBusy else { return }
        isBusy = true; isDownloading = true; error = nil; fraction = 0; status = "Starting download…"
        transferTotal = quote.transferBytes
        operation = Task { [weak self, store] in
            guard let self else { return }
            do {
                try await store.install(quote, allowMobileData: allowMobileData) { [weak self] received, total, message in
                    Task { @MainActor [weak self] in
                        guard let self, isDownloading else { return }
                        transferred = received; transferTotal = total
                        fraction = total > 0 ? min(1, Double(received) / Double(total)) : 0
                        status = message
                    }
                }
                isDownloading = false
                try Task.checkCancellation()
                self.quote = nil; status = "Ready offline"; revision += 1
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

    func stop() { operation?.cancel() }
    func clearSelection() { guard !isBusy else { return }; quote = nil; error = nil; status = "" }

    func discard() async {
        guard !isBusy else { return }
        do { try await store.discardPending(); clearSelection(); await refresh() }
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
