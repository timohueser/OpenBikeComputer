import Foundation
import JavaScriptCore

/// Retain one session per installed search region. Calls use the web JSON contract.
public actor PlannerSearchSession {
    private let context: JSContext
    private let parser: PlannerParser
    private let database: PlannerSearchDatabase
    public let modelVerificationMs: Double
    public let parserInitializationMs: Double
    public let javascriptInitializationMs: Double

    public init(database: URL, databaseHash: String, model: URL, modelHashes: [String: String],
                scripts: URL, region: String, countryCode: String, timeZone: String,
                pythonBundle: URL = Bundle.main.bundleURL) throws {
        guard try plannerFileHash(database) == databaseHash else { throw plannerSearchError("Search database hash differs") }
        self.database = try PlannerSearchDatabase(database)
        parser = try PlannerParser(model: model, hashes: modelHashes, pythonBundle: pythonBundle)
        modelVerificationMs = parser.verificationMs
        parserInitializationMs = parser.initializationMs
        let start = ProcessInfo.processInfo.systemUptime
        guard let context = JSContext() else { throw plannerSearchError("JavaScript runtime failed") }
        self.context = context
        let all: @convention(block) (String, String) -> String = { [database = self.database] in database.all($0, $1) }
        let parse: @convention(block) (String) -> String = { [parser] in parser.parse($0) }
        let clock: @convention(block) () -> Double = { ProcessInfo.processInfo.systemUptime * 1000 }
        context.setObject(all, forKeyedSubscript: "plannerSQL" as NSString)
        context.setObject(parse, forKeyedSubscript: "plannerParse" as NSString)
        context.setObject(clock, forKeyedSubscript: "plannerNow" as NSString)
        context.setObject(["region": region, "countryCode": countryCode, "timeZone": timeZone,
                           "bytes": try database.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0], forKeyedSubscript: "plannerConfig" as NSString)
        context.evaluateScript("globalThis.performance = {now:plannerNow};")
        for file in ["calendar.js", "native.js"] {
            context.evaluateScript(try String(contentsOf: scripts.appendingPathComponent(file), encoding: .utf8))
            if let error = context.exception { throw plannerSearchError(error.toString()) }
        }
        context.evaluateScript("""
        globalThis.plannerRuntime = PlannerNative.nativeSearch({
          all:plannerSQL,parse:plannerParse,region:plannerConfig.region,bytes:plannerConfig.bytes,
          hours:PlannerCalendar.openingHours(plannerConfig)});
        globalThis.plannerDispatch = async (method,body) => {
          return JSON.stringify(await plannerRuntime.request(method,JSON.parse(body)));
        };
        """)
        if let error = context.exception { throw plannerSearchError(error.toString()) }
        javascriptInitializationMs = (ProcessInfo.processInfo.systemUptime - start) * 1000
    }

    public func request(_ method: String, body: Data) throws -> Data {
        try Task.checkCancellation()
        guard let text = String(data: body, encoding: .utf8) else { throw plannerSearchError("Request is not UTF-8") }
        context.exception = nil
        context.setObject(method, forKeyedSubscript: "plannerMethod" as NSString)
        context.setObject(text, forKeyedSubscript: "plannerBody" as NSString)
        context.evaluateScript("""
        globalThis.plannerReply = undefined;
        globalThis.plannerError = undefined;
        plannerDispatch(plannerMethod,plannerBody).then(
          value => globalThis.plannerReply = value,
          error => globalThis.plannerError = String(error));
        """)
        if let error = context.exception { throw plannerSearchError(error.toString()) }
        if let error = context.objectForKeyedSubscript("plannerError"), !error.isUndefined { throw plannerSearchError(error.toString()) }
        guard let reply = context.objectForKeyedSubscript("plannerReply"), !reply.isUndefined,
              let text = reply.toString() else { throw plannerSearchError("Local search did not settle synchronously") }
        return Data(text.utf8)
    }
}
