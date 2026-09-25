import Foundation
import Network

/// Cesium modules and workers need an HTTP origin. Only bundled replay files are served.
final class ReplayResourceServer: @unchecked Sendable {
    static var bundledRoot: URL? { Bundle.module.url(forResource: "Replay", withExtension: nil) }

    private let root: URL
    private let queue = DispatchQueue(label: "obc.replay.resources")
    private var listener: NWListener?
    private var connections: [UUID: NWConnection] = [:]

    init(root: URL) { self.root = root.resolvingSymlinksInPath() }

    func start(ready: @escaping @Sendable (Result<URL, Error>) -> Void) throws {
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: .any)
        let listener = try NWListener(using: parameters)
        self.listener = listener
        listener.stateUpdateHandler = { [weak listener] state in
            switch state {
            case .ready:
                if let port = listener?.port,
                   let url = URL(string: "http://127.0.0.1:\(port.rawValue)/index.html") { ready(.success(url)) }
            case .failed(let error): ready(.failure(error))
            default: break
            }
        }
        listener.newConnectionHandler = { [weak self] connection in
            guard let self, self.connections.count < 32 else { connection.cancel(); return }
            let id = UUID()
            self.connections[id] = connection
            connection.start(queue: self.queue)
            self.queue.asyncAfter(deadline: .now() + 10) { [weak self] in self?.finish(id) }
            self.receive(connection, id: id, prefix: Data())
        }
        listener.start(queue: queue)
    }

    func stop() {
        listener?.cancel()
        queue.async { [self] in
            for connection in connections.values { connection.cancel() }
            connections.removeAll()
        }
    }

    private func finish(_ id: UUID) { connections.removeValue(forKey: id)?.cancel() }

    private func receive(_ connection: NWConnection, id: UUID, prefix: Data) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 8192) { [weak self] data, _, done, error in
            guard let self else { connection.cancel(); return }
            guard let data, error == nil else { self.finish(id); return }
            let request = prefix + data
            guard request.count <= 8192 else { self.finish(id); return }
            if request.range(of: Data("\r\n\r\n".utf8)) != nil {
                self.respond(connection, id: id, request: request)
            } else if done { self.finish(id) }
            else { self.receive(connection, id: id, prefix: request) }
        }
    }

    static func resource(path: String, root: URL) -> URL? {
        guard path.hasPrefix("/"), let decoded = path.removingPercentEncoding,
              !decoded.contains("\0"), !decoded.contains("\\") else { return nil }
        let root = root.resolvingSymlinksInPath()
        let relative = decoded == "/" ? "index.html" : String(decoded.dropFirst())
        let file = root.appendingPathComponent(relative).standardizedFileURL.resolvingSymlinksInPath()
        guard file.path.hasPrefix(root.path + "/") else { return nil }
        return file
    }

    private func respond(_ connection: NWConnection, id: UUID, request: Data) {
        let words = String(decoding: request, as: UTF8.self).split(separator: " ")
        guard words.count >= 2, words[0] == "GET",
              let path = String(words[1]).split(separator: "?").first,
              let file = Self.resource(path: String(path), root: root)
        else { finish(id); return }
        let body = try? Data(contentsOf: file, options: .mappedIfSafe)
        let payload = body ?? Data("Not found".utf8)
        let types = ["html": "text/html", "js": "text/javascript", "mjs": "text/javascript",
                     "css": "text/css", "json": "application/json", "png": "image/png", "jpg": "image/jpeg",
                     "gif": "image/gif", "svg": "image/svg+xml", "wasm": "application/wasm", "xml": "application/xml"]
        let mime = types[file.pathExtension] ?? "application/octet-stream"
        let header = "HTTP/1.1 \(body == nil ? "404 Not Found" : "200 OK")\r\nContent-Type: \(mime)\r\nContent-Length: \(payload.count)\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n"
        connection.send(content: Data(header.utf8) + payload, completion: .contentProcessed { [weak self] _ in
            self?.finish(id)
        })
    }

    deinit { listener?.cancel() }
}
