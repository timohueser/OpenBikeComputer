import Foundation

@_silgen_name("planner_overlays_open")
private func overlaysOpen(_ root: UnsafePointer<CChar>, _ error: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>) -> UnsafeMutableRawPointer?
@_silgen_name("planner_overlays_query")
private func overlaysQuery(_ handle: UnsafeRawPointer, _ params: UnsafePointer<UInt8>, _ length: Int,
                           _ status: UnsafeMutablePointer<UInt16>) -> UnsafeMutablePointer<CChar>?
@_silgen_name("planner_overlays_close")
private func overlaysClose(_ handle: UnsafeMutableRawPointer)
@_silgen_name("planner_response_free")
private func overlaysResponseFree(_ response: UnsafeMutablePointer<CChar>)

// Only the owning actor calls into the handle; its final owner closes it.
private final class OverlayHandle: @unchecked Sendable {
    let pointer: UnsafeMutableRawPointer
    init(_ pointer: UnsafeMutableRawPointer) { self.pointer = pointer }
    deinit { overlaysClose(pointer) }
}

public actor OverlayProvider {
    private let handle: OverlayHandle

    public init(directory: URL) throws {
        var error: UnsafeMutablePointer<CChar>?
        guard let handle = directory.path.withCString({ overlaysOpen($0, &error) }) else {
            defer { if let error { overlaysResponseFree(error) } }
            throw NSError(domain: "PlannerOverlays", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: error.map { String(cString: $0) } ?? "Overlay data could not open"])
        }
        self.handle = OverlayHandle(handle)
    }

    public func request(_ query: String) throws -> (status: Int, body: Data) {
        try Task.checkCancellation()
        let components = URLComponents(string: "?\(query)")
        let params = Dictionary((components?.queryItems ?? []).map { ($0.name, $0.value ?? "") }, uniquingKeysWith: { _, last in last })
        let encoded = try JSONSerialization.data(withJSONObject: params)
        var status: UInt16 = 500
        return try encoded.withUnsafeBytes { bytes in
            guard let response = overlaysQuery(handle.pointer, bytes.bindMemory(to: UInt8.self).baseAddress!, encoded.count, &status) else {
                throw NSError(domain: "PlannerOverlays", code: 2, userInfo: [NSLocalizedDescriptionKey: "Overlay query failed"])
            }
            defer { overlaysResponseFree(response) }
            return (Int(status), Data(bytes: response, count: strlen(response)))
        }
    }
}
