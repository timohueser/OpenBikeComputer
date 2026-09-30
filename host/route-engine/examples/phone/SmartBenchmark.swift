import CryptoKit
import Foundation
import JavaScriptCore

private final class SearchNetworkBlock: URLProtocol, @unchecked Sendable {
    private static let lock = NSLock()
    nonisolated(unsafe) private static var count = 0
    static var attempts: Int { lock.lock(); defer { lock.unlock() }; return count }
    override class func canInit(with request: URLRequest) -> Bool { ["http", "https"].contains(request.url?.scheme ?? "") }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        Self.lock.lock(); Self.count += 1; Self.lock.unlock()
        client?.urlProtocol(self, didFailWithError: URLError(.notConnectedToInternet))
    }
    override func stopLoading() {}
}

func runSmartBenchmark(root: URL) async -> [String: Any] {
    do {
        guard URLProtocol.registerClass(SearchNetworkBlock.self) else { throw plannerSearchError("Cannot block Foundation network requests") }
        defer { URLProtocol.unregisterClass(SearchNetworkBlock.self) }
        let networkAttempts = SearchNetworkBlock.attempts
        let search = root.appendingPathComponent("search")
        guard let reference = try JSONSerialization.jsonObject(with: Data(contentsOf: search.appendingPathComponent("smart-reference.json"))) as? [String: Any],
              let expected = reference["samples"] as? [[String: Any]], let cases = reference["cases"] as? [[String: Any]],
              let requestContext = reference["context"] as? [String: Any], let region = reference["region"] as? String,
              let model = reference["model"] as? [String: String], let databaseHash = reference["databaseSha256"] as? String else {
            throw plannerSearchError("Missing smart reference")
        }
        let session = try PlannerSearchSession(database: search.appendingPathComponent("baden-wuerttemberg.sqlite"),
            databaseHash: databaseHash, model: root.appendingPathComponent("parser/model"), modelHashes: model,
            scripts: search, region: region, countryCode: "de", timeZone: "Europe/Berlin")
        let started = ProcessInfo.processInfo.systemUptime
        var samples: [[String: Any]] = [], mismatches: [Int] = [], exactDifferences: [Int] = []
        for (index, item) in cases.enumerated() {
            let reverse = item["kind"] as? String == "reverse"
            let body: [String: Any] = reverse ? ["coordinate": item["coordinate"]!]
                : requestContext.merging(item["input"] as! [String: Any]) { _, value in value }
            let input = try JSONSerialization.data(withJSONObject: body)
            let start = ProcessInfo.processInfo.systemUptime
            let response = try await session.request(reverse ? "reverse" : "query", body: input)
            let elapsed = (ProcessInfo.processInfo.systemUptime - start) * 1000
            let result = try canonicalReply(JSONSerialization.jsonObject(with: response))
            samples.append(["kind": item["kind"]!, "elapsedMs": elapsed, "result": result])
            guard expected.indices.contains(index), let target = expected[index]["result"] else {
                throw plannerSearchError("Smart reference corpus differs")
            }
            if !equivalent(result, target) { mismatches.append(index) }
            if !(result as AnyObject).isEqual(target) { exactDifferences.append(index) }
        }
        return ["scope": "Reusable native session; complete shared searchRuntime replies after process restart",
            "total_ms": (ProcessInfo.processInfo.systemUptime - started) * 1000,
            "model_verification_ms": session.modelVerificationMs, "parser_initialization_ms": session.parserInitializationMs,
            "javascript_initialization_ms": session.javascriptInitializationMs,
            "database_sha256": databaseHash, "model_sha256": model, "calendarZone": "Europe/Berlin",
            "network_policy": "No JavaScript network capabilities; Foundation HTTP(S) blocked; CPU model telemetry disabled",
            "network_attempts": SearchNetworkBlock.attempts - networkAttempts,
            "samples": samples, "mismatches": mismatches, "exact_value_differences": exactDifferences,
            "numeric_relative_tolerance": 64 * Double.ulpOfOne]
    } catch { return ["error": error.localizedDescription] }
}

private func canonicalReply(_ value: Any) throws -> Any {
    if let array = value as? [Any] { return try array.map(canonicalReply) }
    if let object = value as? [String: Any] {
        return try object.filter { !["elapsed", "parserMs"].contains($0.key) }.mapValues(canonicalReply)
    }
    return value
}
