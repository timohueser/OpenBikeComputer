import Foundation

@_silgen_name("planner_router_open") private func routerOpen(_ root: UnsafePointer<CChar>, _ memoryBudgetBytes: Int, _ error: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>) -> OpaquePointer?
@_silgen_name("planner_router_request") private func routerRequest(_ handle: OpaquePointer, _ body: UnsafePointer<UInt8>, _ length: Int, _ status: UnsafeMutablePointer<UInt16>) -> UnsafeMutablePointer<CChar>?
@_silgen_name("planner_router_shape") private func routerShape(_ handle: OpaquePointer, _ body: UnsafePointer<UInt8>, _ length: Int, _ status: UnsafeMutablePointer<UInt16>) -> UnsafeMutablePointer<CChar>?
@_silgen_name("planner_router_close") private func routerClose(_ handle: OpaquePointer)
@_silgen_name("planner_response_free") private func responseFree(_ response: UnsafeMutablePointer<CChar>)

// Only the owning actor calls into the handle; its final owner closes it.
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

    func route(_ request: Data) throws -> (status: Int, body: Data) { try send(request, to: routerRequest) }

    func shape(_ request: Data) throws -> (status: Int, body: Data) { try send(request, to: routerShape) }

    private func send(_ request: Data, to call: (OpaquePointer, UnsafePointer<UInt8>, Int, UnsafeMutablePointer<UInt16>) -> UnsafeMutablePointer<CChar>?) throws -> (status: Int, body: Data) {
        try Task.checkCancellation()
        var status: UInt16 = 500
        let payload = request.isEmpty ? Data([0]) : request
        let response = payload.withUnsafeBytes { bytes in
            guard let base = bytes.bindMemory(to: UInt8.self).baseAddress else { return nil as UnsafeMutablePointer<CChar>? }
            return call(handle.pointer, base, request.count, &status)
        }
        return try consume(response, status: status)
    }

    private func consume(_ response: UnsafeMutablePointer<CChar>?, status: UInt16) throws -> (status: Int, body: Data) {
        guard let response else { throw NSError(domain: "PlannerRouter", code: 2, userInfo: [NSLocalizedDescriptionKey: "Routing provider returned no response"] ) }
        defer { responseFree(response) }
        return (Int(status), Data(bytes: response, count: strlen(response)))
    }
}
