import Foundation
import JavaScriptCore

/// Synchronous query runtime. The owning actor serializes all access.
final class PlannerSearchRuntime {
    private let context: JSContext
    private let database: PlannerSearchDatabase
    public let javascriptInitializationMs: Double

    convenience init(database: URL, scripts: URL, region: String, countryCode: String, timeZone: String,
         parse: @escaping (String) -> String) throws {
        try self.init(databases: [database], scripts: scripts, region: region, countryCode: countryCode, timeZone: timeZone, parse: parse)
    }

    init(databases: [URL], bounds: [URL: [Double]] = [:], coverage: [Double]? = nil, scripts: URL, region: String, countryCode: String, timeZone: String,
         parse: @escaping (String) -> String) throws {
        self.database = try PlannerSearchDatabase(files: databases, bounds: bounds)
        let start = ProcessInfo.processInfo.systemUptime
        guard let context = JSContext() else { throw plannerSearchError("JavaScript runtime failed") }
        self.context = context
        let all: @convention(block) (String, String, String) -> String = { [database = self.database] in database.all($0, $1, $2) }
        let batch: @convention(block) (String) -> String = { [database = self.database] in database.batch($0) }
        let parse: @convention(block) (String) -> String = parse
        let clock: @convention(block) () -> Double = { ProcessInfo.processInfo.systemUptime * 1000 }
        context.setObject(all, forKeyedSubscript: "plannerSQL" as NSString)
        context.setObject(batch, forKeyedSubscript: "plannerSQLBatch" as NSString)
        context.setObject(parse, forKeyedSubscript: "plannerParse" as NSString)
        context.setObject(clock, forKeyedSubscript: "plannerNow" as NSString)
        context.setObject(["region": region, "countryCode": countryCode, "timeZone": timeZone,
                           "bytes": try databases.reduce(0) { $0 + (try $1.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0) }], forKeyedSubscript: "plannerConfig" as NSString)
        if let coverage { context.objectForKeyedSubscript("plannerConfig")?.setObject(coverage, forKeyedSubscript: "bounds" as NSString) }
        context.evaluateScript("globalThis.performance = {now:plannerNow};")
        for file in ["calendar.js", "native.js"] {
            context.evaluateScript(try String(contentsOf: scripts.appendingPathComponent(file), encoding: .utf8))
            if let error = context.exception { throw plannerSearchError(error.toString()) }
        }
        context.evaluateScript("""
        globalThis.plannerRuntime = PlannerNative.nativeSearch({
          all:plannerSQL,batch:plannerSQLBatch,parse:plannerParse,region:plannerConfig.region,bytes:plannerConfig.bytes,bounds:plannerConfig.bounds,
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

func plannerSearchError(_ message: String) -> NSError {
    NSError(domain: "PlannerSearch", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
}
