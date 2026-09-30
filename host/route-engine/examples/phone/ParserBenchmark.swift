import Foundation

@_silgen_name("planner_python_run")
private func pythonRun(_ module: UnsafePointer<CChar>, _ root: UnsafePointer<CChar>, _ bundle: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>?

func runPythonBenchmark(module: String, root: URL) -> [String: Any] {
    let pointer = module.withCString { name in
        root.path.withCString { directory in
            Bundle.main.bundlePath.withCString { pythonRun(name, directory, $0) }
        }
    }
    guard let pointer else { return ["error": "Python runtime returned no report"] }
    defer { free(pointer) }
    do {
        return try JSONSerialization.jsonObject(with: Data(String(cString: pointer).utf8)) as? [String: Any]
            ?? ["error": "Python runtime returned invalid JSON"]
    } catch { return ["error": error.localizedDescription] }
}

func runParserBenchmark(root: URL) -> [String: Any] {
    do {
        guard let fixture = try JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent("parser-reference.json"))) as? [String: Any],
              let hashes = fixture["model"] as? [String: String],
              let cases = fixture["cases"] as? [[String: Any]], cases.count == 350 else {
            throw plannerSearchError("Missing parser reference")
        }
        let parser = try PlannerParser(model: root.appendingPathComponent("model"), hashes: hashes, pythonBundle: Bundle.main.bundleURL)
        var report = try parser.info()
        report["initialization_ms"] = parser.initializationMs
        report["model_verification_ms"] = parser.verificationMs
        report["model_sha256"] = hashes
        var samples: [[String: Any]] = [], mismatches: [[String: Any]] = []
        for pass in 0..<2 {
            for (index, row) in cases.enumerated() {
                try autoreleasepool {
                    let start = ProcessInfo.processInfo.systemUptime
                    let response = try JSONSerialization.jsonObject(with: Data(parser.parse(row["text"] as! String).utf8)) as! [String: Any]
                    if let error = response["error"] as? String { throw plannerSearchError(error) }
                    samples.append(["index": index, "pass": pass, "elapsed_ms": (ProcessInfo.processInfo.systemUptime - start) * 1000])
                    if !equivalent(response["request"]!, row["request"]!) {
                        mismatches.append(["index": index, "pass": pass, "actual": response["request"]!, "expected": row["request"]!])
                    }
                }
            }
        }
        report["samples"] = samples
        report["mismatches"] = mismatches
        report["scope"] = "Shared full smart parser; Rust tokenizers, native CPU ONNX Runtime, unchanged Python decoder with RapidFuzz Python backend"
        return report
    } catch { return ["error": error.localizedDescription] }
}
