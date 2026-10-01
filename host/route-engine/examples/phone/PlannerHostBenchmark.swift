import Darwin
import SwiftUI
import WebKit

@main
struct PlannerHostBenchmarkApp: App {
    @State private var host: OfflinePlannerHost?
    @State private var status = "Opening the installed planner"
    @State private var started = false

    var body: some Scene {
        WindowGroup {
            Group {
                if let host { OfflinePlannerView(host: host) }
                else { Text(status).padding() }
            }.task {
                guard !started else { return }
                started = true
                await run()
            }
        }
    }

    @MainActor private func run() async {
        let root = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let args = ProcessInfo.processInfo.arguments
        let phase = args.contains("--restore") ? "restore" : "create"
        let fullBW = args.contains("--full-bw")
        let budgetIndex = args.firstIndex(of: "--routing-memory-mib")
        let budgetMiB = budgetIndex.flatMap { $0 + 1 < args.count ? Int(args[$0 + 1]) : nil } ?? 768
        let index = args.firstIndex(of: "--installation")
        let installation = index.flatMap { $0 + 1 < args.count ? args[$0 + 1] : nil } ?? "offline-installed"
        UIApplication.shared.isIdleTimerDisabled = true
        if args.contains("--wait-for-trace") {
            status = "Waiting for the memory trace"
            let gate = root.appendingPathComponent("host-start")
            do {
                while !FileManager.default.fileExists(atPath: gate.path) { try await Task.sleep(for: .milliseconds(100)) }
                try FileManager.default.removeItem(at: gate)
            } catch { status = error.localizedDescription; return }
        }
        let start = ProcessInfo.processInfo.systemUptime
        var metadata: [String: Any] = ["phase": phase, "full_bw": fullBW,
            "started": Date().ISO8601Format(),
            "thermal_start": ProcessInfo.processInfo.thermalState.rawValue,
            "low_power_mode": ProcessInfo.processInfo.isLowPowerModeEnabled,
            "os": ProcessInfo.processInfo.operatingSystemVersionString,
            "isolation": "WKContentRuleList and CSP restrict all web resources to the fixed loopback origin; native providers use local files; device network settings are unchanged"]
        do {
            guard budgetMiB > 0, budgetMiB <= Int.max / (1024 * 1024) else { throw plannerSearchError("Invalid routing memory MiB") }
            metadata["routing_memory_budget_bytes"] = budgetMiB * 1024 * 1024
            let opened = try await OfflinePlannerHost.open(installation: root.appendingPathComponent(installation),
                assets: root.appendingPathComponent("planner-web"), searchScripts: root.appendingPathComponent("planner-search-runtime"),
                port: 48763, memoryBudgetBytes: budgetMiB * 1024 * 1024)
            metadata["cold_initialization_ms"] = (ProcessInfo.processInfo.systemUptime - start) * 1000
            let script = try String(contentsOf: Bundle.main.url(forResource: "planner-host-benchmark", withExtension: "js")!, encoding: .utf8)
            opened.webView.configuration.userContentController.addUserScript(WKUserScript(source: script, injectionTime: .atDocumentStart, forMainFrameOnly: true))
            host = opened
            opened.start()
            var ready = false
            for _ in 0..<600 {
                if let failure = opened.failure { throw plannerSearchError(failure) }
                if (try? await opened.webView.evaluateJavaScript("document.readyState === 'complete' && typeof runPlannerHostBenchmark === 'function'")) as? Bool == true {
                    ready = true
                    break
                }
                try await Task.sleep(for: .milliseconds(100))
            }
            guard ready else { throw plannerSearchError("Planner document did not load") }
            let previous: Any = phase == "restore" ? try JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent("host-draft.json"))) : NSNull()
            let result = try await opened.webView.callAsyncJavaScript("return await runPlannerHostBenchmark(phase, previous, fullBW);",
                arguments: ["phase": phase, "previous": previous, "fullBW": fullBW], in: nil, contentWorld: .page)
            guard let text = result as? String, let report = try JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any] else {
                throw plannerSearchError("Host returned no report")
            }
            try Data(text.utf8).write(to: root.appendingPathComponent("host-result.json"), options: .atomic)
            if phase == "create", let saved = report["saved"] {
                try JSONSerialization.data(withJSONObject: saved, options: [.sortedKeys]).write(to: root.appendingPathComponent("host-draft.json"), options: .atomic)
            }
            let image = try await opened.webView.takeSnapshot(configuration: nil)
            try image.pngData()?.write(to: root.appendingPathComponent("host-screen.png"), options: .atomic)
            status = "Persistent offline planner benchmark complete"
            metadata["exit_code"] = 0
        } catch {
            status = error.localizedDescription
            metadata["exit_code"] = 1
            try? JSONSerialization.data(withJSONObject: ["error": status, "details": String(describing: error as NSError)]).write(to: root.appendingPathComponent("host-result.json"), options: .atomic)
        }
        var usage = rusage()
        if getrusage(RUSAGE_SELF, &usage) == 0 { metadata["native_process_peak_rss_bytes"] = usage.ru_maxrss }
        metadata["thermal_end"] = ProcessInfo.processInfo.thermalState.rawValue
        metadata["duration_ms"] = (ProcessInfo.processInfo.systemUptime - start) * 1000
        metadata["memory_scope"] = "Native process only; measure WebContent, GPU, and Network processes with Instruments"
        try? JSONSerialization.data(withJSONObject: metadata, options: [.sortedKeys]).write(to: root.appendingPathComponent("host-device.json"), options: .atomic)
        print(status)
        if !args.contains("--hold") { UIApplication.shared.isIdleTimerDisabled = false }
    }
}
