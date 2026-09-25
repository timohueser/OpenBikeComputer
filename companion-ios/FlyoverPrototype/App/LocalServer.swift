import Foundation
import Network

/// A loopback origin lets bundled Cesium workers and ES modules use normal HTTP URLs.
final class LocalServer: @unchecked Sendable {
    private let root: URL
    private let queue = DispatchQueue(label: "flyover.assets")
    private var listener: NWListener?

    init(root: URL) { self.root = root.resolvingSymlinksInPath() }

    func start(ready: @escaping @Sendable (Result<URL, Error>) -> Void) throws {
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: .any)
        let listener = try NWListener(using: parameters)
        self.listener = listener
        listener.stateUpdateHandler = { [weak listener] state in
            switch state {
            case .ready:
                if let port = listener?.port {
                    ready(.success(URL(string: "http://127.0.0.1:\(port.rawValue)/index.html")!))
                }
            case .failed(let error): ready(.failure(error))
            default: break
            }
        }
        listener.newConnectionHandler = { [weak self] connection in
            guard let self else { connection.cancel(); return }
            connection.start(queue: self.queue)
            self.receive(connection, prefix: Data())
        }
        listener.start(queue: queue)
    }

    private func receive(_ connection: NWConnection, prefix: Data) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 8192) { [weak self] data, _, done, error in
            guard let self, let data, error == nil else { connection.cancel(); return }
            let request = prefix + data
            if request.range(of: Data("\r\n\r\n".utf8)) != nil {
                self.respond(connection, request: request)
            } else if done || request.count > 8192 {
                connection.cancel()
            } else {
                self.receive(connection, prefix: request)
            }
        }
    }

    private func respond(_ connection: NWConnection, request: Data) {
        let words = String(decoding: request, as: UTF8.self).split(separator: " ")
        guard words.count >= 2, words[0] == "GET",
              let path = String(words[1]).split(separator: "?").first?.removingPercentEncoding
        else { connection.cancel(); return }
        let file = root.appendingPathComponent(path == "/" ? "index.html" : path)
            .standardizedFileURL.resolvingSymlinksInPath()
        let allowed = file.path.hasPrefix(root.path + "/")
        let body = allowed ? try? Data(contentsOf: file) : nil
        let payload = body ?? Data("Not found".utf8)
        let types = ["html": "text/html", "js": "text/javascript", "mjs": "text/javascript",
                     "css": "text/css", "json": "application/json", "gpx": "application/xml",
                     "png": "image/png", "jpg": "image/jpeg", "gif": "image/gif",
                     "svg": "image/svg+xml", "wasm": "application/wasm"]
        let mime = types[file.pathExtension] ?? "application/octet-stream"
        let header = "HTTP/1.1 \(body == nil ? "404 Not Found" : "200 OK")\r\nContent-Type: \(mime)\r\nContent-Length: \(payload.count)\r\nConnection: close\r\n\r\n"
        connection.send(content: Data(header.utf8) + payload, completion: .contentProcessed { _ in
            connection.cancel()
        })
    }

    deinit { listener?.cancel() }
}
