import Foundation

/// Retain one session per installed search region. Calls use the web JSON contract.
public actor PlannerSearchSession {
    private let runtime: PlannerSearchRuntime
    public let modelVerificationMs: Double
    public let parserInitializationMs: Double
    public let javascriptInitializationMs: Double

    public init(database: URL, databaseHash: String, model: URL, modelHashes: [String: String],
                scripts: URL, region: String, countryCode: String, timeZone: String,
                pythonBundle: URL = Bundle.main.bundleURL) throws {
        guard try plannerFileHash(database) == databaseHash else { throw plannerSearchError("Search database hash differs") }
        let parser = try PlannerParser(model: model, hashes: modelHashes, pythonBundle: pythonBundle)
        modelVerificationMs = parser.verificationMs
        parserInitializationMs = parser.initializationMs
        runtime = try PlannerSearchRuntime(database: database, scripts: scripts, region: region,
            countryCode: countryCode, timeZone: timeZone, parse: parser.parse)
        javascriptInitializationMs = runtime.javascriptInitializationMs
    }

    public func request(_ method: String, body: Data) throws -> Data {
        try runtime.request(method, body: body)
    }
}
