import CryptoKit
import Foundation
import JavaScriptCore
import SQLite3

func runSearchBenchmark(root: URL, hoursOnly: Bool = false) -> [String: Any] {
    do {
        let prefix = hoursOnly ? "hours" : "search"
        let databaseURL = root.appendingPathComponent("baden-wuerttemberg.sqlite")
        let reference = try JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent("\(prefix)-reference.json"))) as? [String: Any]
        let databaseHash = try plannerFileHash(databaseURL)
        guard databaseHash == reference?["databaseSha256"] as? String else {
            throw NSError(domain: "SearchBenchmark", code: 6, userInfo: [NSLocalizedDescriptionKey: "Reference database hash differs"])
        }
        let database = try PlannerSearchDatabase(databaseURL)
        guard let context = JSContext() else { throw NSError(domain: "SearchBenchmark", code: 3) }
        var exception: String?
        context.exceptionHandler = { _, error in exception = error?.toString() }
        let all: @convention(block) (String, String) -> String = { database.all($0, $1) }
        let digest: @convention(block) (String) -> String = { SHA256.hash(data: Data($0.utf8)).map { String(format: "%02x", $0) }.joined() }
        let clock: @convention(block) () -> Double = { ProcessInfo.processInfo.systemUptime * 1000 }
        context.setObject(all, forKeyedSubscript: "plannerSQL" as NSString)
        context.setObject(digest, forKeyedSubscript: "plannerDigest" as NSString)
        context.setObject(clock, forKeyedSubscript: "plannerNow" as NSString)
        context.evaluateScript("globalThis.performance = {now: plannerNow};")
        if hoursOnly {
            let calendar = try String(contentsOf: root.appendingPathComponent("calendar.js"), encoding: .utf8)
            context.evaluateScript(calendar)
            // Exercise a traveler calendar inside this realm without changing device settings.
            context.evaluateScript("globalThis.Date = PlannerCalendar.calendarDate('America/New_York');")
            context.evaluateScript(calendar)
        }
        context.evaluateScript(try String(contentsOf: root.appendingPathComponent("\(prefix)-benchmark.js"), encoding: .utf8))
        let started = ProcessInfo.processInfo.systemUptime
        let adapter = hoursOnly ? ", PlannerCalendar.openingHours({countryCode:'de',timeZone:'Europe/Berlin'})" : ""
        let value = context.evaluateScript("JSON.stringify(PlannerSearchBenchmark.run(plannerSQL, plannerDigest\(adapter)))")
        if let exception { return ["error": exception] }
        guard let text = value?.toString(), var report = try JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any],
              let samples = report["samples"] as? [[String: Any]] else { throw NSError(domain: "SearchBenchmark", code: 4) }
        report["total_ms"] = (ProcessInfo.processInfo.systemUptime - started) * 1000
        report["sqlite_version"] = String(cString: sqlite3_libversion())
        if hoursOnly { report["host_calendar_override"] = "America/New_York in an isolated JavaScript realm; device settings unchanged" }
        guard let expected = reference?["samples"] as? [[String: Any]], samples.count == expected.count else {
            throw NSError(domain: "SearchBenchmark", code: 5, userInfo: [NSLocalizedDescriptionKey: "Reference corpus differs"])
        }
        report["exact_fingerprint_differences"] = zip(samples, expected).enumerated().compactMap { index, pair in
            pair.0["sha256"] as? String == pair.1["sha256"] as? String ? nil : index
        }
        report["mismatches"] = zip(samples, expected).enumerated().compactMap { index, pair in
            guard let actual = pair.0["result"], let expected = pair.1["result"] else { return index }
            return equivalent(actual, expected) ? nil : index
        }
        report["numeric_relative_tolerance"] = 64 * Double.ulpOfOne
        report["reference_database_sha256"] = reference?["databaseSha256"]
        report["database_sha256"] = databaseHash
        return report
    } catch { return ["error": error.localizedDescription] }
}

func equivalent(_ lhs: Any, _ rhs: Any) -> Bool {
    if let a = lhs as? NSNumber, let b = rhs as? NSNumber {
        if CFGetTypeID(a) == CFBooleanGetTypeID() || CFGetTypeID(b) == CFBooleanGetTypeID() {
            return CFGetTypeID(a) == CFGetTypeID(b) && a == b
        }
        // V8 and JavaScriptCore can differ in the last bits of composed trig and distance operations.
        return abs(a.doubleValue - b.doubleValue) <= 64 * Double.ulpOfOne * max(1, abs(a.doubleValue), abs(b.doubleValue))
    }
    if let a = lhs as? String, let b = rhs as? String { return a == b }
    if lhs is NSNull, rhs is NSNull { return true }
    if let a = lhs as? [Any], let b = rhs as? [Any] {
        return a.count == b.count && zip(a, b).allSatisfy { equivalent($0.0, $0.1) }
    }
    if let a = lhs as? [String: Any], let b = rhs as? [String: Any] {
        return a.keys.sorted() == b.keys.sorted() && a.allSatisfy { key, value in equivalent(value, b[key]!) }
    }
    return false
}
