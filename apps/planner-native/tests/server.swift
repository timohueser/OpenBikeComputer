import Foundation

@main struct ServerTests {
    static func main() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let payload = Data((0..<1000).map { UInt8($0 % 256) })
        try payload.write(to: root.appendingPathComponent("test.pmtiles"))
        let large = root.appendingPathComponent("large.pmtiles")
        try Data().write(to: large)
        let writer = try FileHandle(forWritingTo: large)
        try writer.truncate(atOffset: 32 * 1024 * 1024)
        try writer.close()
        try FileManager.default.createSymbolicLink(at: root.appendingPathComponent("escape"), withDestinationURL: root.deletingLastPathComponent())
        precondition(PlannerHTTPServer.resource(path: "/%2e%2e/secret", root: root) == nil)
        precondition(PlannerHTTPServer.resource(path: "/escape/secret", root: root) == nil)
        precondition(PlannerHTTPServer.range("bytes=-4", size: 10) == 6..<10)
        precondition(PlannerHTTPServer.range("bytes=3-99", size: 10) == 3..<10)
        for invalid in ["bytes=10-", "bytes=4-3", "bytes=0-1,3-4", "bytes=--1"] {
            precondition(PlannerHTTPServer.range(invalid, size: 10) == nil)
        }
        let head = Data("POST /api/echo HTTP/1.1\r\nContent-Length: 3\r\n\r\n".utf8)
        let incomplete = try PlannerHTTPServer.parse(head + Data("ab".utf8))
        precondition(incomplete == nil)
        let parsed = try PlannerHTTPServer.parse(head + Data("abc".utf8))!
        precondition(parsed.body == Data("abc".utf8) && parsed.method == "POST")
        for headers in ["Content-Length: -1", "Content-Length: 2097153", "Transfer-Encoding: chunked", "Content-Length: 0\r\nContent-Length: 0"] {
            do { _ = try PlannerHTTPServer.parse(Data("POST / HTTP/1.1\r\n\(headers)\r\n\r\n".utf8)); preconditionFailure("Invalid framing accepted") }
            catch {}
        }
        let server = PlannerHTTPServer(port: 0, mounts: ["/maps/": root]) { request in
            request.target == "/api/echo" ? PlannerHTTPResponse(headers: ["Content-Type": "application/json"], body: request.body) : nil
        }
        defer { server.stop() }
        let origin: URL = try await withCheckedThrowingContinuation { continuation in
            do { try server.start { continuation.resume(with: $0) } } catch { continuation.resume(throwing: error) }
        }
        func get(_ target: String, method: String = "GET", range: String? = nil, body: Data? = nil) async throws -> (Data, HTTPURLResponse) {
            var request = URLRequest(url: origin.appendingPathComponent(target))
            request.httpMethod = method; request.httpBody = body
            if let range { request.setValue(range, forHTTPHeaderField: "Range") }
            let (data, response) = try await URLSession.shared.data(for: request)
            return (data, response as! HTTPURLResponse)
        }
        let (slice, partial) = try await get("maps/test.pmtiles", range: "bytes=100-199")
        precondition(partial.statusCode == 206 && slice == payload[100..<200])
        precondition(partial.value(forHTTPHeaderField: "Content-Range") == "bytes 100-199/1000")
        let (empty, headResponse) = try await get("maps/test.pmtiles", method: "HEAD")
        precondition(empty.isEmpty && headResponse.value(forHTTPHeaderField: "Content-Length") == "1000")
        let (metadata, largeHead) = try await get("maps/large.pmtiles", method: "HEAD")
        precondition(largeHead.statusCode == 200 && metadata.isEmpty && largeHead.value(forHTTPHeaderField: "Content-Length") == "33554432")
        let (_, largeGet) = try await get("maps/large.pmtiles")
        precondition(largeGet.statusCode == 413)
        let (_, invalid) = try await get("maps/test.pmtiles", range: "bytes=1000-")
        precondition(invalid.statusCode == 416)
        let (echo, response) = try await get("api/echo", method: "POST", body: payload)
        precondition(response.statusCode == 200 && echo == payload)
        print("Planner HTTP framing, containment, Range, HEAD and API transport checks passed")
    }
}
