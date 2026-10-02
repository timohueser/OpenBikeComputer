import CryptoKit
import Darwin
import Foundation
@preconcurrency import Network
import SwiftUI
import WebKit

private func verifyMapFiles(_ root: URL) throws {
    struct File: Decodable { let bytes: UInt64; let sha256: String }
    struct Archive: Decodable { let sha256: String; let files: [String: File] }
    struct Config: Decodable { let archives: [String: Archive]; let renderer_sha256: String }
    let config = try JSONDecoder().decode(Config.self, from: Data(contentsOf: root.appendingPathComponent("map-benchmark/config.json")))
    func verify(_ name: String, _ files: [String: File]) throws {
        let directory = root.appendingPathComponent(name)
        for (path, expected) in files {
            let file = directory.appendingPathComponent(path).standardizedFileURL.resolvingSymlinksInPath()
            guard file.path.hasPrefix(root.resolvingSymlinksInPath().path + "/") else { throw CocoaError(.fileReadInvalidFileName) }
            let reader = try FileHandle(forReadingFrom: file)
            defer { try? reader.close() }
            var hash = SHA256(), bytes: UInt64 = 0
            while try autoreleasepool(invoking: { () throws -> Bool in
                guard let chunk = try reader.read(upToCount: 1 << 20), !chunk.isEmpty else { return false }
                hash.update(data: chunk)
                bytes += UInt64(chunk.count)
                return true
            }) {}
            let checksum = hash.finalize().map { String(format: "%02x", $0) }.joined()
            guard bytes == expected.bytes, checksum == expected.sha256 else {
                throw NSError(domain: "MapBenchmark", code: 2, userInfo: [NSLocalizedDescriptionKey: "Map checksum differs: \(name)/\(path)"])
            }
        }
    }
    for (name, archive) in config.archives {
        var files = archive.files
        let manifest = root.appendingPathComponent(name).appendingPathComponent("manifest.json")
        files["manifest.json"] = File(bytes: UInt64(try manifest.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0), sha256: archive.sha256)
        try verify(name, files)
    }
    let renderer = root.appendingPathComponent("map-benchmark/main.js")
    try verify("map-benchmark", ["main.js": File(bytes: UInt64(try renderer.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0), sha256: config.renderer_sha256)])
}

private final class MapFiles: @unchecked Sendable {
    private let root: URL
    private let queue = DispatchQueue(label: "planner.map.files")
    private let listener: NWListener
    private let lock = NSLock()
    private var requests = 0
    private var bytes = 0
    private var failures: [String] = []

    init(root: URL, ready: @escaping @Sendable (Result<URL, Error>) -> Void) throws {
        self.root = root.resolvingSymlinksInPath()
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: .any)
        listener = try NWListener(using: parameters)
        listener.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            if case .ready = state, let port = listener.port {
                ready(.success(URL(string: "http://127.0.0.1:\(port.rawValue)/map-benchmark/index.html")!))
            } else if case .failed(let error) = state { ready(.failure(error)) }
        }
        listener.newConnectionHandler = { [weak self] connection in
            guard let self else { return }
            connection.start(queue: queue)
            receive(connection, accumulated: Data())
        }
        listener.start(queue: queue)
    }

    deinit { listener.cancel() }

    func report() -> [String: Any] {
        lock.lock()
        defer { lock.unlock() }
        return ["requests": requests, "response_bytes": bytes, "errors": failures,
                "transport": "Read-only loopback HTTP, bounded Range reads, CSP blocks external network"]
    }

    private func receive(_ connection: NWConnection, accumulated: Data) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 16384) { [weak self] data, _, complete, error in
            guard let self else { connection.cancel(); return }
            let request = accumulated + (data ?? Data())
            if request.range(of: Data("\r\n\r\n".utf8)) != nil { respond(connection, request) }
            else if complete || error != nil || request.count > 16384 { connection.cancel() }
            else { receive(connection, accumulated: request) }
        }
    }

    private func respond(_ connection: NWConnection, _ request: Data) {
        var status = 200, body = Data(), fields: [String: String] = [:]
        var requested = ""
        do {
            let lines = String(decoding: request, as: UTF8.self).components(separatedBy: "\r\n")
            let parts = lines[0].split(separator: " ")
            guard parts.count == 3, parts[0] == "GET" else { throw failure("Only GET is supported") }
            requested = String(parts[1])
            let path = URLComponents(string: requested)?.path ?? ""
            guard ["/map-benchmark/", "/maps/", "/map-cutout/"].contains(where: path.hasPrefix) else {
                throw failure("Path is outside the map benchmark")
            }
            let file = root.appendingPathComponent(path).standardizedFileURL.resolvingSymlinksInPath()
            guard file.path.hasPrefix(root.path + "/") else { throw failure("Path escapes map root") }
            let size = try file.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
            let header = lines.dropFirst().first { $0.lowercased().hasPrefix("range:") }
            var start = 0, end = size - 1
            if let header {
                let range = header.split(separator: "=", maxSplits: 1).last!.split(separator: "-", omittingEmptySubsequences: false)
                guard range.count == 2, let offset = Int(range[0]), offset >= 0, offset < size else { throw failure("Invalid Range") }
                start = offset
                end = min(Int(range[1]) ?? end, end)
                status = 206
                fields["Content-Range"] = "bytes \(start)-\(end)/\(size)"
            }
            let length = max(0, end - start + 1)
            guard end >= start, length <= 16 * 1024 * 1024 else { throw failure("A request exceeds the bounded read limit") }
            let reader = try FileHandle(forReadingFrom: file)
            defer { try? reader.close() }
            try reader.seek(toOffset: UInt64(start))
            body = try reader.read(upToCount: length) ?? Data()
            guard body.count == length else { throw failure("Short file read") }
            fields["Content-Type"] = ["html": "text/html", "js": "text/javascript", "css": "text/css", "json": "application/json", "png": "image/png", "webp": "image/webp" ][file.pathExtension] ?? "application/octet-stream"
        } catch {
            status = 404
            body = Data(error.localizedDescription.utf8)
            lock.lock()
            failures.append("\(requested): \(error.localizedDescription)")
            lock.unlock()
        }
        fields.merge(["Content-Length": String(body.count), "Connection": "close", "Accept-Ranges": "bytes", "Cache-Control": "no-store"], uniquingKeysWith: { _, rhs in rhs })
        let headers = "HTTP/1.1 \(status) \(status == 206 ? "Partial Content" : status == 200 ? "OK" : "Not Found")\r\n" + fields.map { "\($0): \($1)\r\n" }.joined() + "\r\n"
        let response = Data(headers.utf8) + body
        lock.lock()
        requests += 1
        bytes += response.count
        lock.unlock()
        connection.send(content: response, completion: .contentProcessed { _ in connection.cancel() })
    }

    private func failure(_ message: String) -> NSError {
        NSError(domain: "MapBenchmark", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
    }
}

struct MapBenchmarkView: UIViewRepresentable {
    let root: URL
    let completion: @MainActor (String) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(root: root, completion: completion) }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        configuration.userContentController.add(context.coordinator, name: "mapBenchmark")
        let view = WKWebView(frame: .zero, configuration: configuration)
        view.isInspectable = true
        context.coordinator.start(view)
        return view
    }

    func updateUIView(_ uiView: WKWebView, context: Context) {}

    @MainActor final class Coordinator: NSObject, WKScriptMessageHandler {
        private let root: URL
        private let completion: @MainActor (String) -> Void
        private var server: MapFiles?
        private weak var webView: WKWebView?
        private var meter = Meter()
        private var verificationMemory: [String: Any] = [:]
        private let thermalStart = ProcessInfo.processInfo.thermalState.rawValue

        init(root: URL, completion: @escaping @MainActor (String) -> Void) {
            self.root = root
            self.completion = completion
        }

        func start(_ webView: WKWebView) {
            self.webView = webView
            Task {
                do {
                    let root = self.root
                    try await Task.detached(priority: .userInitiated) { try verifyMapFiles(root) }.value
                    verificationMemory = meter.finish()
                    meter = Meter()
                    startServer()
                } catch { completion("Map verification failed: \(error.localizedDescription)") }
            }
        }

        private func startServer() {
            do {
                server = try MapFiles(root: root) { [weak self] result in
                    Task { @MainActor in
                        guard let self else { return }
                        switch result {
                        case .success(let url): self.webView?.load(URLRequest(url: url))
                        case .failure(let error): self.completion("Map server failed: \(error.localizedDescription)")
                        }
                    }
                }
            } catch { completion("Map server failed: \(error.localizedDescription)") }
        }

        func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
            guard let envelope = message.body as? [String: Any], envelope["kind"] as? String == "complete",
                  var report = envelope["data"] as? [String: Any] else { return }
            report["transport"] = server?.report()
            report["all_map_files_verified"] = true
            var memory = meter.finish()
            var usage = rusage()
            if getrusage(RUSAGE_SELF, &usage) == 0 { memory["process_peak_rss_bytes"] = usage.ru_maxrss }
            memory["scope"] = "Host app only; excludes WebContent and GPU processes"
            report["native_memory"] = memory
            report["verification_memory"] = verificationMemory
            report["os"] = ProcessInfo.processInfo.operatingSystemVersionString
            report["low_power_mode"] = ProcessInfo.processInfo.isLowPowerModeEnabled
            var system = utsname()
            uname(&system)
            let capacity = MemoryLayout.size(ofValue: system.machine)
            report["machine"] = withUnsafePointer(to: &system.machine) {
                $0.withMemoryRebound(to: CChar.self, capacity: capacity) { String(cString: $0) }
            }
            report["thermal_start"] = thermalStart
            report["thermal_end"] = ProcessInfo.processInfo.thermalState.rawValue
            do {
                try JSONSerialization.data(withJSONObject: report, options: [.sortedKeys]).write(to: root.appendingPathComponent("map-result.json"), options: .atomic)
                completion("Map benchmark complete. Report is in Documents.")
            } catch { completion("Cannot write map report: \(error.localizedDescription)") }
        }
    }
}
