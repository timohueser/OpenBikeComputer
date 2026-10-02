import Foundation

struct PlannerAPI: Sendable {
    typealias Reply = (status: Int, body: Data)
    let search: PlannerSearchSession
    let route: @Sendable (Data) async throws -> Reply
    let region: @Sendable () async throws -> Reply
    let overlays: @Sendable (String) async throws -> Reply
    let sample: Data

    func respond(_ request: PlannerHTTPRequest) async -> PlannerHTTPResponse? {
        guard let url = URLComponents(string: request.target) else { return nil }
        let path = url.path
        guard path.hasPrefix("/api/planner-search/") || path.hasPrefix("/routing/") else { return nil }
        do {
            try Task.checkCancellation()
            if request.method == "GET" {
                switch path {
                case "/api/planner-search/status":
                    return response(body: try await search.request("status", body: Data("{}".utf8)))
                case "/api/planner-search/sample": return response(body: sample)
                case "/routing/v1/region":
                    let reply = try await region()
                    return response(status: reply.status, body: reply.body)
                case "/routing/v1/overlays":
                    let reply = try await overlays(url.percentEncodedQuery ?? "")
                    return response(status: reply.status, body: reply.body)
                default: return error(404, "Not found.")
                }
            }
            guard request.method == "POST" else { return error(405, "Method not allowed.") }
            switch path {
            case "/api/planner-search/query", "/api/planner-search/reverse":
                return response(body: try await search.request(path.hasSuffix("/query") ? "query" : "reverse", body: request.body))
            case "/routing/v1/route":
                let reply = try await route(request.body)
                return response(status: reply.status, body: reply.body)
            case "/api/planner-search/route":
                let prepared = try await search.request("route-request", body: request.body)
                let reply = try await route(prepared)
                guard (200..<300).contains(reply.status) else {
                    let value = try JSONSerialization.jsonObject(with: reply.body) as? [String: Any]
                    return error(reply.status, value?["message"] as? String ?? "The routing engine could not find a route.")
                }
                return response(body: reply.body)
            default: return error(404, "Not found.")
            }
        } catch {
            return self.error(error is CancellationError ? 499 : 400, error.localizedDescription)
        }
    }

    private func response(status: Int = 200, body: Data) -> PlannerHTTPResponse {
        PlannerHTTPResponse(status: status, headers: ["Content-Type": "application/json", "Cache-Control": "no-store"], body: body)
    }

    private func error(_ status: Int, _ message: String) -> PlannerHTTPResponse {
        response(status: status, body: (try? JSONSerialization.data(withJSONObject: ["error": message, "message": message])) ?? Data())
    }
}
