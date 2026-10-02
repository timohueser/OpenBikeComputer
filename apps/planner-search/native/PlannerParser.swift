import Foundation

@_silgen_name("planner_parser_create")
private func parserCreate(_ model: UnsafePointer<CChar>, _ bundle: UnsafePointer<CChar>, _ error: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>) -> UnsafeMutableRawPointer?
@_silgen_name("planner_parser_parse")
private func parserParse(_ handle: UnsafeMutableRawPointer, _ text: UnsafePointer<CChar>, _ length: Int) -> UnsafeMutablePointer<CChar>?
@_silgen_name("planner_parser_destroy")
private func parserDestroy(_ handle: UnsafeMutableRawPointer)
@_silgen_name("planner_parser_info")
private func parserInfo(_ handle: UnsafeMutableRawPointer) -> UnsafeMutablePointer<CChar>?

final class PlannerParser {
    private let handle: UnsafeMutableRawPointer
    let verificationMs: Double
    let initializationMs: Double

    init(model: URL, hashes: [String: String], pythonBundle: URL) throws {
        let required = Set(["model.int8.onnx", "tokenizer.json", "tokenizer_config.json", "labels.json"])
        guard Set(hashes.keys) == required else { throw plannerSearchError("Missing parser model identities") }
        var start = ProcessInfo.processInfo.systemUptime
        for (name, hash) in hashes {
            guard try plannerFileHash(model.appendingPathComponent(name)) == hash else {
                throw plannerSearchError("Parser model hash differs: \(name)")
            }
        }
        verificationMs = (ProcessInfo.processInfo.systemUptime - start) * 1000
        start = ProcessInfo.processInfo.systemUptime
        var error: UnsafeMutablePointer<CChar>?
        let pointer = model.path.withCString { directory in
            pythonBundle.path.withCString { parserCreate(directory, $0, &error) }
        }
        guard let pointer else {
            let message = error.map { String(cString: $0) } ?? "Parser initialization failed"
            free(error)
            throw plannerSearchError(message)
        }
        handle = pointer
        initializationMs = (ProcessInfo.processInfo.systemUptime - start) * 1000
    }

    deinit { parserDestroy(handle) }

    func parse(_ text: String) -> String {
        guard let result = text.withCString({ parserParse(handle, $0, text.utf8.count) }) else {
            return "{\"error\":\"Parser returned no response\"}"
        }
        defer { free(result) }
        return String(cString: result)
    }

    func info() throws -> [String: Any] {
        guard let result = parserInfo(handle) else { throw plannerSearchError("Parser returned no information") }
        defer { free(result) }
        return try JSONSerialization.jsonObject(with: Data(String(cString: result).utf8)) as? [String: Any] ?? [:]
    }
}
