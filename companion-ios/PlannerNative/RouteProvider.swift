import Foundation

// Only the owning actor calls into the handle; a cancellation handler may cancel it from any
// thread. Its final owner closes it.
private final class RouterHandle: @unchecked Sendable {
    let pointer: OpaquePointer
    init(_ pointer: OpaquePointer) { self.pointer = pointer }
    deinit { planner_router_close(pointer) }
}

actor RouteProvider {
    private let handle: RouterHandle

    init(directory: URL) throws {
        var error: UnsafeMutablePointer<CChar>?
        // The budget is the search limit of one search: zero selects the engine's default.
        guard let opened = directory.path.withCString({ planner_router_open($0, 0, &error) }) else {
            defer { if let error { planner_response_free(error) } }
            throw NSError(domain: "PlannerRouter", code: 1, userInfo: [NSLocalizedDescriptionKey: error.map { String(cString: $0) } ?? "Cannot open routing package"])
        }
        handle = RouterHandle(opened)
    }

    /// Answers the call `name` (`route` or `shape`) as `POST /v1/{name}` does. Cancelling the task stops it.
    func call(_ name: String, _ request: Data) async throws -> (status: Int, body: Data) {
        let handle = handle
        // The operation never suspends, so no other call runs on the handle while this handler is installed.
        return try await withTaskCancellationHandler {
            try Task.checkCancellation()
            var status: UInt16 = 500
            let payload = request.isEmpty ? Data([0]) : request
            let response = name.withCString { call in
                payload.withUnsafeBytes { bytes in
                    bytes.bindMemory(to: UInt8.self).baseAddress.flatMap { planner_router_call(handle.pointer, call, $0, request.count, &status) }
                }
            }
            guard let response else { throw NSError(domain: "PlannerRouter", code: 2, userInfo: [NSLocalizedDescriptionKey: "Routing provider returned no response"]) }
            defer { planner_response_free(response) }
            return (Int(status), Data(bytes: response, count: strlen(response)))
        } onCancel: {
            planner_router_cancel(handle.pointer)
        }
    }
}
