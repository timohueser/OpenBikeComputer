#if canImport(CoreBluetooth)
import Foundation
import OBCTransport

/// The device's parsed diagnostics blob (`obc-ble-interface-spec.md` §7.5) — object type 4, downloaded
/// over the CoC and rendered by the firmware as an opaque, human-readable UTF-8 text blob (not a wire
/// struct). This parser is deliberately **tolerant**: it keys off `key: value` lines and defaults any
/// missing/extra field, so a firmware that grows a new line doesn't break the harness (and an older
/// firmware missing `stack_*` just reports 0). It is the device-side ledger every A9 scenario reconciles
/// against — the counters the firmware keeps must agree with what the harness itself observed.
struct Diagnostics: Sendable {
    var fw = ""
    var hw = ""
    var serial = ""
    var protocolVersion = 0
    var bootCount: UInt32 = 0
    var uptimeS: UInt32 = 0
    /// Lifetime link counters — the soak's health line. The harness asserts these track its own tally.
    var connects: UInt32 = 0
    var disconnects: UInt32 = 0
    /// The HCI status code of the most recent disconnect (0 = none yet).
    var lastReason: UInt8 = 0
    var routes = 0
    var rides = 0
    var sdOK = false
    /// Stack high-water + total (bytes) — the "stack high-water + RAM numbers posted" DoD, read over the
    /// link with no RTT. `stackHW` is 0 until the status loop's first paint-scan.
    var stackHW = 0
    var stackTotal = 0
    /// The full blob, for `--verbose` dumps.
    var raw = ""

    init(parsing text: String) {
        raw = text
        var f: [String: String] = [:]
        for line in text.split(separator: "\n") {
            guard let colon = line.firstIndex(of: ":") else { continue }
            let key = line[..<colon].trimmingCharacters(in: .whitespaces)
            let value = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
            f[key] = value
        }
        fw = f["fw"] ?? fw
        hw = f["hw"] ?? hw
        serial = f["serial"] ?? serial
        protocolVersion = f["protocol"].flatMap { Int($0) } ?? protocolVersion
        bootCount = f["boot_count"].flatMap { UInt32($0) } ?? bootCount
        uptimeS = f["uptime_s"].flatMap { UInt32($0) } ?? uptimeS
        connects = f["link_connects"].flatMap { UInt32($0) } ?? connects
        disconnects = f["link_disconnects"].flatMap { UInt32($0) } ?? disconnects
        lastReason = f["link_last_reason"].flatMap(Self.parseHexByte) ?? lastReason
        routes = f["routes"].flatMap { Int($0) } ?? routes
        rides = f["rides"].flatMap { Int($0) } ?? rides
        sdOK = f["sd"] == "ok"
        stackHW = f["stack_hw"].flatMap { Int($0) } ?? stackHW
        stackTotal = f["stack_total"].flatMap { Int($0) } ?? stackTotal
    }

    /// `"0x3E"` / `"3E"` → the byte; nil on anything unparseable.
    private static func parseHexByte(_ s: String) -> UInt8? {
        let digits = s.hasPrefix("0x") || s.hasPrefix("0X") ? String(s.dropFirst(2)) : s
        return UInt8(digits, radix: 16)
    }

    /// The stack high-water as a percentage of the total, when both are known.
    var stackPercent: Int? {
        guard stackTotal > 0 else { return nil }
        return stackHW * 100 / stackTotal
    }

    /// The at-a-glance health line every scenario prints when it reconciles ledgers.
    var summary: String {
        let reason = "0x" + String(lastReason, radix: 16, uppercase: true)
        let stack = stackPercent.map { "\(stackHW)/\(stackTotal) B (\($0)%)" } ?? "\(stackHW)/\(stackTotal) B"
        return "connects \(connects) · disconnects \(disconnects) · lastReason \(reason) · "
            + "routes \(routes) · rides \(rides) · sd \(sdOK ? "ok" : "--") · stack \(stack) · "
            + "boot #\(bootCount) · up \(uptimeS)s"
    }
}

extension EchoHarness {
    /// Download + parse the device diagnostics blob (§7.5, object type 4) over the live link — the
    /// device-side ledger the scenarios assert against.
    static func readDiagnostics(link: EchoLink, central: EchoCentral) async throws -> Diagnostics {
        let bytes = try await downloadObject(link: link, central: central, type: .diagnostics, objectID: 0)
        guard let text = String(data: bytes, encoding: .utf8) else { throw HarnessError.badDiagnostics }
        return Diagnostics(parsing: text)
    }
}
#endif
