import Foundation
@preconcurrency import Network

struct PlannerHTTPRequest: Sendable {
    let method: String
    let target: String
    let headers: [String: String]
    let body: Data
}

struct PlannerHTTPResponse: Sendable {
    var status = 200
    var headers: [String: String] = [:]
    var body = Data()
}

/// One loopback origin owns local assets and the native planner API.
final class PlannerHTTPServer: @unchecked Sendable {
    typealias Handler = @Sendable (PlannerHTTPRequest) async -> PlannerHTTPResponse?
    private let mounts: [(String, URL)]
    private let handler: Handler
    private let queue = DispatchQueue(label: "planner.http")
    private var listener: NWListener?
    private var connections: [UUID: NWConnection] = [:]
    private var tasks: [UUID: Task<Void, Never>] = [:]
    private let port: UInt16
    private static let headerLimit = 16 * 1024
    private static let bodyLimit = 2 * 1024 * 1024
    private static let fileLimit = 16 * 1024 * 1024

    init(port: UInt16, mounts: [String: URL], handler: @escaping Handler) {
        precondition(mounts.keys.allSatisfy { $0.hasPrefix("/") && $0.hasSuffix("/") })
        self.port = port
        self.mounts = mounts.map { ($0.key, $0.value.resolvingSymlinksInPath()) }.sorted { $0.0.count > $1.0.count }
        self.handler = handler
    }

    func start(ready: @escaping @Sendable (Result<URL, Error>) -> Void) throws {
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: NWEndpoint.Port(rawValue: port)!)
        let listener = try NWListener(using: parameters)
        self.listener = listener
        listener.stateUpdateHandler = { [weak listener] state in
            switch state {
            case .ready:
                if let port = listener?.port { ready(.success(URL(string: "http://127.0.0.1:\(port.rawValue)/")!)) }
            case .failed(let error): ready(.failure(error))
            default: break
            }
        }
        listener.newConnectionHandler = { [weak self] connection in
            guard let self, self.connections.count < 32, self.tasks.count < 32 else { connection.cancel(); return }
            let id = UUID()
            self.connections[id] = connection
            connection.stateUpdateHandler = { [weak self] state in
                switch state {
                case .failed, .cancelled: self?.finish(id)
                default: break
                }
            }
            connection.start(queue: self.queue)
            self.queue.asyncAfter(deadline: .now() + 60) { [weak self] in self?.finish(id) }
            self.receive(connection, id: id, prefix: Data())
        }
        listener.start(queue: queue)
    }

    func stop() {
        listener?.cancel()
        queue.async { [self] in
            for task in tasks.values { task.cancel() }
            for connection in connections.values { connection.cancel() }
            connections.removeAll()
        }
    }

    private func finish(_ id: UUID) {
        tasks[id]?.cancel()
        connections.removeValue(forKey: id)?.cancel()
    }

    private func receive(_ connection: NWConnection, id: UUID, prefix: Data) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 65536) { [weak self] data, _, done, error in
            guard let self else { connection.cancel(); return }
            let bytes = prefix + (data ?? Data())
            do {
                if let request = try Self.parse(bytes) {
                    self.tasks[id] = Task { [weak self] in
                        guard let self else { return }
                        let response = await handler(request) ?? fileResponse(request)
                        let cancelled = Task.isCancelled
                        queue.async { [weak self] in
                            guard let self else { return }
                            self.tasks.removeValue(forKey: id)
                            if cancelled { self.finish(id) }
                            else { self.send(response, request: request, connection: connection, id: id) }
                        }
                    }
                } else if done || error != nil { self.finish(id) }
                else { self.receive(connection, id: id, prefix: bytes) }
            } catch {
                self.send(PlannerHTTPResponse(status: 400, body: Data("Invalid request".utf8)), request: nil, connection: connection, id: id)
            }
        }
    }

    static func parse(_ bytes: Data) throws -> PlannerHTTPRequest? {
        guard bytes.count <= headerLimit + bodyLimit else { throw CocoaError(.fileReadTooLarge) }
        guard let separator = bytes.range(of: Data("\r\n\r\n".utf8)) else {
            if bytes.count > headerLimit { throw CocoaError(.fileReadTooLarge) }
            return nil
        }
        guard separator.lowerBound <= headerLimit else { throw CocoaError(.fileReadTooLarge) }
        let lines = String(decoding: bytes[..<separator.lowerBound], as: UTF8.self).components(separatedBy: "\r\n")
        let words = lines[0].split(separator: " ")
        guard words.count == 3, ["GET", "HEAD", "POST"].contains(String(words[0])),
              words[1].hasPrefix("/"), words[2] == "HTTP/1.1" else { throw CocoaError(.fileReadCorruptFile) }
        var headers: [String: String] = [:]
        for line in lines.dropFirst() {
            let pair = line.split(separator: ":", maxSplits: 1)
            guard pair.count == 2 else { throw CocoaError(.fileReadCorruptFile) }
            let key = pair[0].lowercased(), value = pair[1].trimmingCharacters(in: .whitespaces)
            guard headers[key] == nil else { throw CocoaError(.fileReadCorruptFile) }
            headers[key] = value
        }
        guard headers["transfer-encoding"] == nil,
              let length = Int(headers["content-length"] ?? "0"), (0...bodyLimit).contains(length) else { throw CocoaError(.fileReadTooLarge) }
        let end = separator.upperBound + length
        guard bytes.count >= end else { return nil }
        guard bytes.count == end else { throw CocoaError(.fileReadCorruptFile) }
        return PlannerHTTPRequest(method: String(words[0]), target: String(words[1]), headers: headers, body: bytes[separator.upperBound..<end])
    }

    static func resource(path: String, root: URL) -> URL? {
        guard path.hasPrefix("/"), let decoded = path.removingPercentEncoding,
              !decoded.contains("\0"), !decoded.contains("\\") else { return nil }
        let root = root.resolvingSymlinksInPath()
        let relative = decoded == "/" ? "index.html" : String(decoded.dropFirst())
        let candidate = root.appendingPathComponent(relative).standardizedFileURL
        let file = candidate.deletingLastPathComponent().resolvingSymlinksInPath()
            .appendingPathComponent(candidate.lastPathComponent).resolvingSymlinksInPath()
        return file.path.hasPrefix(root.path + "/") ? file : nil
    }

    func fileResponse(_ request: PlannerHTTPRequest) -> PlannerHTTPResponse {
        guard request.method == "GET" || request.method == "HEAD" else { return PlannerHTTPResponse(status: 405) }
        let path = request.target.components(separatedBy: "?")[0]
        guard let (prefix, root) = mounts.first(where: { path.hasPrefix($0.0) }),
              let file = Self.resource(path: "/" + path.dropFirst(prefix.count), root: root) else { return PlannerHTTPResponse(status: 404) }
        do {
            let attributes = try file.resourceValues(forKeys: [.fileSizeKey, .isRegularFileKey])
            guard attributes.isRegularFile == true, let size = attributes.fileSize else { return PlannerHTTPResponse(status: 404) }
            var start = 0, length = size, status = 200
            var headers = ["Accept-Ranges": "bytes", "Cache-Control": "no-store"]
            if let range = request.headers["range"] {
                guard let selected = Self.range(range, size: size) else { return PlannerHTTPResponse(status: 416, headers: ["Content-Range": "bytes */\(size)"]) }
                start = selected.lowerBound; length = selected.count; status = 206
                headers["Content-Range"] = "bytes \(start)-\(selected.upperBound - 1)/\(size)"
            }
            guard request.method == "HEAD" || length <= Self.fileLimit else { return PlannerHTTPResponse(status: 413) }
            headers["Content-Length"] = String(length)
            headers["Content-Type"] = ["html": "text/html", "js": "text/javascript", "mjs": "text/javascript", "css": "text/css", "json": "application/json", "png": "image/png", "webp": "image/webp", "svg": "image/svg+xml", "wasm": "application/wasm", "pbf": "application/x-protobuf"][file.pathExtension] ?? "application/octet-stream"
            if file.pathExtension == "html" {
                headers["Content-Security-Policy"] = "default-src 'self'; script-src 'self' blob:; worker-src 'self' blob:; style-src 'self' 'unsafe-inline'; img-src 'self' blob: data:; font-src 'self'; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"
            }
            var body = Data()
            if request.method != "HEAD" && length > 0 {
                let reader = try FileHandle(forReadingFrom: file)
                defer { try? reader.close() }
                try reader.seek(toOffset: UInt64(start))
                body = try reader.read(upToCount: length) ?? Data()
                guard body.count == length else { throw CocoaError(.fileReadCorruptFile) }
            }
            return PlannerHTTPResponse(status: status, headers: headers, body: body)
        } catch { return PlannerHTTPResponse(status: 404) }
    }

    static func range(_ value: String, size: Int) -> Range<Int>? {
        guard value.hasPrefix("bytes="), size > 0 else { return nil }
        let parts = value.dropFirst(6).split(separator: "-", omittingEmptySubsequences: false)
        guard parts.count == 2 else { return nil }
        if parts[0].isEmpty {
            guard let suffix = Int(parts[1]), suffix > 0 else { return nil }
            return max(0, size - suffix)..<size
        }
        guard let start = Int(parts[0]), start >= 0, start < size else { return nil }
        if parts[1].isEmpty { return start..<size }
        guard let end = Int(parts[1]), end >= start else { return nil }
        return start..<(min(end, size - 1) + 1)
    }

    private func send(_ response: PlannerHTTPResponse, request: PlannerHTTPRequest?, connection: NWConnection, id: UUID) {
        guard connections[id] != nil else { return }
        var fields = response.headers
        fields["Content-Length"] = fields["Content-Length"] ?? String(response.body.count)
        fields["Connection"] = "close"
        fields["X-Content-Type-Options"] = "nosniff"
        let reason = [200: "OK", 206: "Partial Content", 400: "Bad Request", 404: "Not Found", 405: "Method Not Allowed", 413: "Content Too Large", 416: "Range Not Satisfiable", 500: "Internal Server Error"][response.status] ?? "Response"
        let header = "HTTP/1.1 \(response.status) \(reason)\r\n" + fields.map { "\($0): \($1)\r\n" }.joined() + "\r\n"
        let body = request?.method == "HEAD" ? Data() : response.body
        connection.send(content: Data(header.utf8), isComplete: body.isEmpty, completion: .contentProcessed { [weak self] error in
            guard error == nil, !body.isEmpty else { self?.finish(id); return }
            connection.send(content: body, completion: .contentProcessed { [weak self] _ in self?.finish(id) })
        })
    }

    deinit { listener?.cancel() }
}
