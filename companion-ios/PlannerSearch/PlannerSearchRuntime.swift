import Foundation
import JavaScriptCore

/// Synchronous query runtime. The owning actor serializes all access.
final class PlannerSearchRuntime {
    private let context: JSContext
    private let database: PlannerSearchDatabase

    init(databases: [URL], scripts: URL) throws {
        self.database = try PlannerSearchDatabase(files: databases)
        guard let context = JSContext() else { throw plannerSearchError("JavaScript runtime failed") }
        self.context = context
        let groups: @convention(block) () -> String = { [database = self.database] in database.groups() }
        let run: @convention(block) (Int, String, String) -> String = { [database = self.database] in database.run($0, $1, $2) }
        let clock: @convention(block) () -> Double = { ProcessInfo.processInfo.systemUptime * 1000 }
        context.setObject(groups, forKeyedSubscript: "plannerGroups" as NSString)
        context.setObject(run, forKeyedSubscript: "plannerRun" as NSString)
        context.setObject(clock, forKeyedSubscript: "plannerNow" as NSString)
        context.evaluateScript("globalThis.performance = {now:plannerNow};")
        context.evaluateScript(try String(contentsOf: scripts.appendingPathComponent("native.js"), encoding: .utf8))
        if let error = context.exception { throw plannerSearchError(error.toString()) }
        context.evaluateScript("""
        globalThis.plannerRuntime = PlannerNative.nativeSearch({groups:plannerGroups,run:plannerRun});
        globalThis.plannerDispatch = async body => JSON.stringify(await plannerRuntime.query(JSON.parse(body)));
        """)
        if let error = context.exception { throw plannerSearchError(error.toString()) }
    }

    /// Answers a web query body with the server's JSON response.
    public func query(_ body: Data) throws -> Data {
        try Task.checkCancellation()
        guard let text = String(data: body, encoding: .utf8) else { throw plannerSearchError("Request is not UTF-8") }
        context.exception = nil
        context.setObject(text, forKeyedSubscript: "plannerBody" as NSString)
        context.evaluateScript("""
        globalThis.plannerReply = undefined;
        globalThis.plannerError = undefined;
        plannerDispatch(plannerBody).then(
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
