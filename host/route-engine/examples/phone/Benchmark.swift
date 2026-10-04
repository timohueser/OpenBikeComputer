import Darwin
import SwiftUI

@_silgen_name("planner_benchmark")
func plannerBenchmark(_ root: UnsafePointer<CChar>, _ requests: UnsafePointer<CChar>, _ output: UnsafePointer<CChar>, _ retained: Int32) -> Int32
@_silgen_name("planner_overlay_benchmark")
func plannerOverlayBenchmark(_ root: UnsafePointer<CChar>, _ output: UnsafePointer<CChar>) -> Int32

@main
struct BenchmarkApp: App {
    @State private var status = "Preparing benchmark"

    var body: some Scene {
        WindowGroup {
            Text(status).padding().task {
                UIApplication.shared.isIdleTimerDisabled = true
                let result = await Task.detached(priority: .userInitiated) { run() }.value
                if !ProcessInfo.processInfo.arguments.contains("--hold") { UIApplication.shared.isIdleTimerDisabled = false }
                status = result
                print(result)
            }
        }
    }
}

private func memory() -> UInt64? {
    var info = task_vm_info_data_t()
    var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<integer_t>.size)
    let code = withUnsafeMutablePointer(to: &info) { pointer in
        pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
            task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
        }
    }
    return code == KERN_SUCCESS ? info.phys_footprint : nil
}

final class Meter: @unchecked Sendable {
    private let lock = NSLock()
    private var peak: UInt64 = 0
    private var samples = 0
    private var pressureWarnings = 0
    private var pressureCritical = 0
    let timer = DispatchSource.makeTimerSource(queue: .global(qos: .utility))
    let pressure = DispatchSource.makeMemoryPressureSource(eventMask: [.warning, .critical], queue: .global(qos: .utility))

    init() {
        timer.schedule(deadline: .now(), repeating: .milliseconds(20))
        timer.setEventHandler { [weak self] in
            guard let self, let bytes = memory() else { return }
            lock.lock()
            peak = max(peak, bytes)
            samples += 1
            lock.unlock()
        }
        timer.resume()
        pressure.setEventHandler { [weak self] in
            guard let self else { return }
            lock.lock()
            if pressure.data.contains(.warning) { pressureWarnings += 1 }
            if pressure.data.contains(.critical) { pressureCritical += 1 }
            lock.unlock()
        }
        pressure.resume()
    }

    func finish() -> [String: Any] {
        timer.cancel()
        pressure.cancel()
        lock.lock()
        defer { lock.unlock() }
        return ["sampled_peak_physical_footprint_bytes": peak, "memory_samples": samples, "memory_interval_ms": 20,
                "memory_pressure_warning_events": pressureWarnings, "memory_pressure_critical_events": pressureCritical]
    }
}

private func run() -> String {
    let root = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
    let monotonicStart = ProcessInfo.processInfo.systemUptime
    let meter = Meter()
    let thermalStart = ProcessInfo.processInfo.thermalState.rawValue
    let arguments = ProcessInfo.processInfo.arguments
    let package = arguments.firstIndex(of: "--package").flatMap { $0 + 1 < arguments.count ? arguments[$0 + 1] : nil } ?? "routing"
    guard !package.hasPrefix("/"), !package.split(separator: "/").contains("..") else { return "Invalid package path" }
    let search = arguments.contains("--search")
    let hours = arguments.contains("--hours")
    let overlays = arguments.contains("--overlays")
    let started = Date().ISO8601Format()
    let code: Int32
    if search || hours {
        let report = runSearchBenchmark(root: root.appendingPathComponent("search"), hoursOnly: hours)
        do {
            try JSONSerialization.data(withJSONObject: report, options: [.sortedKeys]).write(to: root.appendingPathComponent("result.json"), options: .atomic)
            code = report["error"] == nil && (report["mismatches"] as? [Any])?.isEmpty == true ? 0 : 1
        } catch { return "Could not write report: \(error.localizedDescription)" }
    } else if overlays {
        code = root.appendingPathComponent("overlays").path.withCString { directory in
            root.appendingPathComponent("result.json").path.withCString { plannerOverlayBenchmark(directory, $0) }
        }
    } else {
        code = root.appendingPathComponent(package).path.withCString { directory in
            root.appendingPathComponent("requests.json").path.withCString { requests in
                root.appendingPathComponent("result.json").path.withCString { output in
                    plannerBenchmark(directory, requests, output, arguments.contains("--retained") ? 1 : 0)
                }
            }
        }
    }
    var metadata = meter.finish()
    var system = utsname()
    uname(&system)
    let capacity = MemoryLayout.size(ofValue: system.machine)
    metadata["machine"] = withUnsafePointer(to: &system.machine) {
        $0.withMemoryRebound(to: CChar.self, capacity: capacity) { String(cString: $0) }
    }
    var usage = rusage()
    if getrusage(RUSAGE_SELF, &usage) == 0 { metadata["process_peak_rss_bytes"] = usage.ru_maxrss }
    metadata["os"] = ProcessInfo.processInfo.operatingSystemVersionString
    metadata["thermal_start"] = thermalStart
    metadata["thermal_end"] = ProcessInfo.processInfo.thermalState.rawValue
    metadata["exit_code"] = code
    metadata["build"] = "Release"
    metadata["started"] = started
    metadata["workload"] = search ? "search" : hours ? "hours" : overlays ? "overlays" : "routes"
    metadata["low_power_mode"] = ProcessInfo.processInfo.isLowPowerModeEnabled
    metadata["duration_ms"] = (ProcessInfo.processInfo.systemUptime - monotonicStart) * 1000
    metadata["file_cache"] = search || hours ? "Uncontrolled OS cache; one SQLite connection for the full corpus"
        : overlays ? "Uncontrolled OS cache; one SQLite connection for the complete query corpus"
        : arguments.contains("--retained") ? "Uncontrolled OS cache; retained router with distinct request coordinates"
        : "Uncontrolled OS cache; each cold sample creates a new router"
    do {
        try JSONSerialization.data(withJSONObject: metadata, options: [.sortedKeys]).write(to: root.appendingPathComponent("device.json"), options: .atomic)
        return code == 0 ? "Benchmark complete. Reports are in Documents." : "Benchmark failed (\(code)). Check the device log."
    } catch {
        return "Could not write device metadata: \(error.localizedDescription)"
    }
}
