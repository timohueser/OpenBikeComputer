import Foundation

@_silgen_name("planner_router_open") private func routerOpen(_ root: UnsafePointer<CChar>, _ memoryBudgetBytes: Int, _ error: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>) -> OpaquePointer?
@_silgen_name("planner_router_call") private func routerCall(_ handle: OpaquePointer, _ call: UnsafePointer<CChar>, _ body: UnsafePointer<UInt8>, _ length: Int, _ status: UnsafeMutablePointer<UInt16>) -> UnsafeMutablePointer<CChar>?
@_silgen_name("planner_router_cancel") private func routerCancel(_ handle: OpaquePointer)
@_silgen_name("planner_router_close") private func routerClose(_ handle: OpaquePointer)
@_silgen_name("planner_response_free") private func responseFree(_ response: UnsafeMutablePointer<CChar>)

// Only the owning actor calls into the handle; a cancellation handler may cancel it from any
// thread. Its final owner closes it.
private final class RouterHandle: @unchecked Sendable {
    let pointer: OpaquePointer
    init(_ pointer: OpaquePointer) { self.pointer = pointer }
    deinit { routerClose(pointer) }
}

actor RouteProvider {
    private let handle: RouterHandle

    init(directory: URL) throws {
        var error: UnsafeMutablePointer<CChar>?
        // A zero budget selects the route server's default.
        guard let opened = directory.path.withCString({ routerOpen($0, 0, &error) }) else {
            defer { if let error { responseFree(error) } }
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
                    bytes.bindMemory(to: UInt8.self).baseAddress.flatMap { routerCall(handle.pointer, call, $0, request.count, &status) }
                }
            }
            guard let response else { throw NSError(domain: "PlannerRouter", code: 2, userInfo: [NSLocalizedDescriptionKey: "Routing provider returned no response"]) }
            defer { responseFree(response) }
            return (Int(status), Data(bytes: response, count: strlen(response)))
        } onCancel: {
            routerCancel(handle.pointer)
        }
    }
}
