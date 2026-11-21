import Foundation
import Network

/// Serves the bundled TrailView page on 127.0.0.1 and passes every other request to the measuring kit, so the page has one
/// origin, as in Safari. A loopback origin is a secure context, which WebGPU needs.
final class Loopback: NSObject, URLSessionDelegate, @unchecked Sendable {
    static let shared = Loopback()
    static let origin = "http://127.0.0.1:8771"

    var kit: URL?
    private let root = Bundle.main.url(forResource: "www", withExtension: nil)!.resolvingSymlinksInPath()
    private let queue = DispatchQueue(label: "loopback")
    private var listener: NWListener?

    lazy var session: URLSession = {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.urlCache = nil
        configuration.timeoutIntervalForRequest = 120
        return URLSession(configuration: configuration, delegate: self, delegateQueue: nil)
    }()

    func start() {
        let parameters = NWParameters.tcp
        parameters.allowLocalEndpointReuse = true
        parameters.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: 8771)
        guard let listener = try? NWListener(using: parameters) else { return }
        self.listener = listener
        listener.newConnectionHandler = { [weak self] connection in
            guard let self else { return }
            connection.start(queue: self.queue)
            self.read(connection, Data())
        }
        listener.start(queue: queue)
    }

    // The kit's certificate is self-signed.
    func urlSession(_ session: URLSession, didReceive challenge: URLAuthenticationChallenge,
                    completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
        if let trust = challenge.protectionSpace.serverTrust { completionHandler(.useCredential, URLCredential(trust: trust)) }
        else { completionHandler(.performDefaultHandling, nil) }
    }

    private func read(_ connection: NWConnection, _ buffer: Data) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 1 << 20) { [weak self] data, _, done, error in
            guard let self else { return }
            var buffer = buffer
            if let data { buffer.append(data) }
            if let end = buffer.range(of: Data("\r\n\r\n".utf8)) {
                let lines = String(decoding: buffer[..<end.lowerBound], as: UTF8.self).components(separatedBy: "\r\n")
                var headers: [String: String] = [:]
                for line in lines.dropFirst() {
                    guard let colon = line.firstIndex(of: ":") else { continue }
                    headers[line[..<colon].lowercased()] = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
                }
                let length = Int(headers["content-length"] ?? "0") ?? 0
                if buffer.count - end.upperBound >= length {
                    return self.handle(connection, lines[0], headers, Data(buffer[end.upperBound..<(end.upperBound + length)]))
                }
            }
            if done || error != nil { return connection.cancel() }
            self.read(connection, buffer)
        }
    }

    private func handle(_ connection: NWConnection, _ requestLine: String, _ headers: [String: String], _ body: Data) {
        let words = requestLine.split(separator: " ")
        guard words.count >= 2 else { return connection.cancel() }
        let method = String(words[0]), target = String(words[1])
        if method == "GET", let file = file(String(target.split(separator: "?").first ?? "/")),
           let data = try? Data(contentsOf: file, options: .mappedIfSafe) {
            return send(connection, 200, ["Content-Type": mime(file.pathExtension)], data)
        }
        guard let kit, let url = URL(string: target, relativeTo: kit)?.absoluteURL else {
            return send(connection, 502, ["Content-Type": "text/plain"], Data("no kit address".utf8))
        }
        var request = URLRequest(url: url)
        request.httpMethod = method
        for name in ["range", "content-type", "accept"] { if let value = headers[name] { request.setValue(value, forHTTPHeaderField: name) } }
        if !body.isEmpty { request.httpBody = body }
        let kitOrigin = "\(kit.scheme ?? "https")://\(kit.host ?? ""):\(kit.port ?? 443)"
        session.dataTask(with: request) { [weak self] data, response, error in
            guard let self else { return }
            guard let http = response as? HTTPURLResponse else {
                return self.send(connection, 502, ["Content-Type": "text/plain"], Data(String(describing: error).utf8))
            }
            var out: [String: String] = [:]
            for name in ["Content-Type", "Content-Range", "Accept-Ranges", "ETag", "Last-Modified"] {
                if let value = http.value(forHTTPHeaderField: name) { out[name] = value }
            }
            var data = data ?? Data()
            // The catalogue and the release name the kit's origin; the page must stay on this one.
            if (out["Content-Type"] ?? "").contains("json"), let text = String(data: data, encoding: .utf8) {
                data = Data(text.replacingOccurrences(of: kitOrigin, with: Self.origin).utf8)
            }
            self.send(connection, http.statusCode, out, data)
        }.resume()
    }

    private func file(_ path: String) -> URL? {
        guard path.hasPrefix("/"), path.count > 1, let decoded = path.removingPercentEncoding, !decoded.contains("\0") else { return nil }
        let file = root.appendingPathComponent(String(decoded.dropFirst())).standardizedFileURL
        var directory: ObjCBool = false
        guard file.path.hasPrefix(root.path + "/"), FileManager.default.fileExists(atPath: file.path, isDirectory: &directory),
              !directory.boolValue else { return nil }
        return file
    }

    private func mime(_ ext: String) -> String {
        ["html": "text/html", "js": "text/javascript", "mjs": "text/javascript", "json": "application/json", "wasm": "application/wasm",
         "svg": "image/svg+xml", "webp": "image/webp", "png": "image/png", "css": "text/css", "txt": "text/plain"][ext]
            ?? "application/octet-stream"
    }

    private func send(_ connection: NWConnection, _ status: Int, _ headers: [String: String], _ body: Data) {
        var all = headers
        all["Content-Length"] = String(body.count)
        all["Cache-Control"] = "no-store"
        all["Connection"] = "close"
        let head = "HTTP/1.1 \(status) \(status < 400 ? "OK" : "Error")\r\n" + all.map { "\($0.key): \($0.value)\r\n" }.joined() + "\r\n"
        connection.send(content: Data(head.utf8) + body, completion: .contentProcessed { _ in connection.cancel() })
    }
}
