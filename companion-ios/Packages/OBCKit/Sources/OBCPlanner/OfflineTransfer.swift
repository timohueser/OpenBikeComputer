import Foundation

/// Four network transfers share one bounded decoder and monotonic progress.
actor OfflineTransfer {
    private let root: URL
    private let source: URL
    private let total: Int64
    private let session: URLSession
    private let progress: @Sendable (Int64, Int64, String) -> Void
    private var received: [String: Int64] = [:]

    init(root: URL, source: URL, total: Int64, session: URLSession,
         progress: @escaping @Sendable (Int64, Int64, String) -> Void) {
        self.root = root; self.source = source; self.total = total; self.session = session; self.progress = progress
    }

    private func update(_ key: String, bytes: Int64, limit: Int64, message: String) {
        received[key] = max(received[key] ?? 0, min(bytes, limit))
        progress(received.values.reduce(0, +), total, message)
    }

    func install(_ entry: OfflineBundle.Entry) async throws {
        try Task.checkCancellation()
        let fm = FileManager.default
        let wire = OfflineFile(bytes: entry.transport.bytes, sha256: entry.transport.sha256)
        let target = root.appending(path: "objects/\(entry.sha256)")
        let download = root.appending(path: "downloads/\(wire.sha256)")
        let resume = download.appendingPathExtension("resume")
        if !fm.fileExists(atPath: download.path) {
            let delegate = OfflineDownloadProgress { [weak self] count, waiting in
                Task { await self?.update(entry.sha256, bytes: count, limit: wire.bytes,
                    message: waiting ? "Waiting for Wi-Fi…" : "Downloading map…") }
            }
            do {
                let temporary: URL, response: URLResponse
                if let data = try? Data(contentsOf: resume) {
                    (temporary, response) = try await session.download(resumeFrom: data, delegate: delegate)
                } else {
                    var request = URLRequest(url: source.appending(path: wire.sha256))
                    request.setValue("identity", forHTTPHeaderField: "Accept-Encoding")
                    (temporary, response) = try await session.download(for: request, delegate: delegate)
                }
                defer { try? fm.removeItem(at: temporary) }
                if let http = response as? HTTPURLResponse, [404, 410].contains(http.statusCode) {
                    throw OfflineMapFailure.unavailable("This download has expired. Cancel it and select the area again.")
                }
                guard let http = response as? HTTPURLResponse, [200, 206].contains(http.statusCode) else {
                    throw OfflineMapFailure.unavailable("The map download failed. Try again.")
                }
                try fm.moveItem(at: temporary, to: download)
                try? fm.removeItem(at: resume)
            } catch {
                if let data = (error as NSError).userInfo["NSURLSessionDownloadTaskResumeData"] as? Data {
                    try data.write(to: resume, options: .atomic)
                } else { try? fm.removeItem(at: resume) }
                throw error
            }
        }
        do { try OfflineMapStore.verify(download, wire) }
        catch is CancellationError { throw CancellationError() }
        catch { try? fm.removeItem(at: download); throw error }
        update(entry.sha256, bytes: wire.bytes, limit: wire.bytes, message: "Checking map files…")
        if fm.fileExists(atPath: target.path) { try fm.removeItem(at: target) }
        if entry.transport.encoding == "identity" { try fm.moveItem(at: download, to: target) }
        else {
            let decoded = target.appendingPathExtension("part")
            defer { try? fm.removeItem(at: decoded) }
            try OfflineMapStore.unpack(download, to: decoded, bytes: entry.bytes)
            try OfflineMapStore.verify(decoded, OfflineFile(bytes: entry.bytes, sha256: entry.sha256))
            try fm.moveItem(at: decoded, to: target)
            try fm.removeItem(at: download)
        }
    }
}

private final class OfflineDownloadProgress: NSObject, URLSessionDownloadDelegate, Sendable {
    let update: @Sendable (Int64, Bool) -> Void
    init(_ update: @escaping @Sendable (Int64, Bool) -> Void) { self.update = update }
    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {}
    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData bytesWritten: Int64,
                    totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) { update(totalBytesWritten, false) }
    func urlSession(_ session: URLSession, taskIsWaitingForConnectivity task: URLSessionTask) { update(0, true) }
}
