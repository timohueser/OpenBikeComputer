#if canImport(CoreBluetooth)
import Foundation
import OBCTransport

/// The BLE bring-up harness: scan for the OBC device, open the L2CAP CoC, and drive the *real* app
/// byte plane (`BLEChannel` + `L2CAPByteChannel` + the `TransferControl`/`StatusMessage`/`RouteList`
/// codecs) from the terminal — the oracle that isn't the iOS app, so failures localize.
///
/// Two layers:
/// - **bring-up planes** (A5–A7): the echo loopback (#273), the route object plane (#274 — upload → SD,
///   list/detail/delete, abort → discard → re-upload; uploads restart, not resume, spec §1 principle 4),
///   and the diagnostics blob (§7.5).
/// - **A9 soak + fault injection** (#277): golden-path soak, the drop/restart matrix, CRC/offset/malformed
///   corruption, the connect/disconnect storm, and concurrency probes — each reconciling the device's
///   diagnostics counters with the harness's own tally (see `Scenarios.swift`).
///
/// Run on a Mac with the device powered + advertising:
/// ```
/// swift run echo-harness echo --count 1000 --size 32768   # A5 DoD: 1000 × 32 KB
/// swift run echo-harness upload route.obcr                 # A6 golden path
/// swift run echo-harness list
/// swift run echo-harness detail 7 route.obcr              # download + byte-identity check
/// swift run echo-harness delete 7
/// swift run echo-harness abort-test route.obcr            # abort mid-upload → discard → re-upload
/// swift run echo-harness soak route.obcr --count 50       # A9 headline: 50 uploads, ledgers agree
/// swift run echo-harness drop-matrix route.obcr            # kill the link mid-transfer → restart + verify
/// swift run echo-harness corruption route.obcr             # CRC / offset / malformed → typed rejects
/// swift run echo-harness storm --iterations 50             # connect/disconnect churn
/// swift run echo-harness concurrency route.obcr            # busy gate + back-to-back reconnects
/// ```
@main
struct EchoHarness {
    static func main() async {
        let args = Array(CommandLine.arguments.dropFirst())
        let subcommand = args.first.flatMap { $0.hasPrefix("-") ? nil : $0 } ?? "echo"
        let positionals = args.filter { !$0.hasPrefix("-") }.dropFirst()  // after the subcommand
        let flags = args.filter { $0.hasPrefix("-") }

        if flags.contains("--help") || flags.contains("-h") {
            print(usage)
            return
        }

        do {
            switch subcommand {
            case "echo": try await runEcho(Options(args))
            case "upload": try await runUpload(path: try requirePath(positionals.first, "upload <file.obcr>"))
            case "list": try await runList()
            case "detail":
                guard let idArg = positionals.first, let id = UInt16(idArg) else {
                    throw CLIError.usage("detail <object-id> [reference.obcr] [--out file]")
                }
                try await runDetail(id: id, reference: positionals.dropFirst().first, out: flagValue("--out", flags: args))
            case "delete":
                guard let idArg = positionals.first, let id = UInt16(idArg) else {
                    throw CLIError.usage("delete <object-id>")
                }
                try await runDelete(id: id)
            case "abort-test": try await runAbortTest(path: try requirePath(positionals.first, "abort-test <file.obcr>"))
            // ── A9 soak rig (#277) ──
            case "soak":
                let files = fileArgs(after: subcommand, in: args, valueFlags: ["--count"])
                guard !files.isEmpty else { throw CLIError.usage("soak <file.obcr> [more.obcr ...] [--count N] [--no-cleanup]") }
                try await runSoak(paths: files, count: intOption("--count", default: 50, in: args), cleanup: !hasFlag("--no-cleanup", in: args))
            case "drop-matrix":
                let file = try requirePath(fileArgs(after: subcommand, in: args, valueFlags: ["--iterations"]).first, "drop-matrix <file.obcr> [--iterations K]")
                try await runDropMatrix(path: file, iterations: intOption("--iterations", default: 10, in: args))
            case "corruption":
                try await runCorruption(path: try requirePath(fileArgs(after: subcommand, in: args, valueFlags: []).first, "corruption <file.obcr>"))
            case "storm":
                try await runStorm(iterations: intOption("--iterations", default: 20, in: args))
            case "concurrency":
                try await runConcurrency(path: try requirePath(fileArgs(after: subcommand, in: args, valueFlags: []).first, "concurrency <file.obcr>"))
            case "diagnostics":
                try await runDiagnostics(verbose: hasFlag("--verbose", in: args))
            default:
                throw CLIError.usage("unknown subcommand '\(subcommand)'")
            }
        } catch {
            stderr("echo-harness: \(error)")
            exit(1)
        }
    }

    // MARK: - A6 route object plane (#274)

    /// Upload an OBCR file to the device (S0 §4.2 op 1), assert it commits, and confirm the store
    /// digest moved (a route was added).
    static func runUpload(path: String) async throws {
        let bytes = try Data(contentsOf: URL(fileURLWithPath: path))
        let crc = CRC32.checksum(bytes)
        let central = EchoCentral()
        let link = try await central.connect()
        let before = try await central.readDigest()
        print("echo-harness: uploading \(bytes.count) B (crc \(hex(crc)))…")

        let start = TransferControl(
            op: .upload, type: .route, objectID: TransferControl.newObjectID,
            totalLen: UInt32(bytes.count), crc32: crc
        )
        central.writeControl(start.encode(), to: link.transferControl)
        let result = try await withTimeout(60) {
            try await link.channel.send(bytes)  // all bytes handed to the CoC
            return await central.nextTransferResult()
        }
        guard result.status == .committed else { throw HarnessError.unexpectedStatus(result.status) }

        let after = try await central.readDigest()
        guard after.revision != before.revision, after.routeCount == before.routeCount + 1 else {
            throw HarnessError.digestUnchanged
        }
        print(
            "echo-harness: committed as object id \(result.objectID.map(String.init) ?? "?") ✓ "
                + "(routes \(before.routeCount)→\(after.routeCount), revision \(before.revision)→\(after.revision))"
        )
    }

    /// Download + decode the `routeList` (S0 §7.4) and print the catalog.
    static func runList() async throws {
        let central = EchoCentral()
        let link = try await central.connect()
        let entries = try await downloadRouteList(link: link, central: central)
        print("echo-harness: routeList — \(entries.count) route(s)")
        for e in entries {
            print(
                "  #\(e.objectID)  \"\(e.name)\"  \(fmtKm(e.distanceMeters)) km  ↑\(e.ascentMeters) m  "
                    + "\(e.pointCount) pts  \(e.waypointCount) wpt  \(e.byteLen) B"
            )
        }
    }

    /// Download a stored route (S0 §7.1: the OBCR bytes verbatim), verify its CRC, and — given a
    /// reference file — assert byte-identity with what was uploaded.
    static func runDetail(id: UInt16, reference: String?, out: String?) async throws {
        let central = EchoCentral()
        let link = try await central.connect()
        let bytes = try await downloadObject(link: link, central: central, type: .route, objectID: id)
        print("echo-harness: downloaded route \(id): \(bytes.count) B (crc verified)")
        if let out {
            try bytes.write(to: URL(fileURLWithPath: out))
            print("echo-harness: wrote \(out)")
        }
        if let reference {
            let refBytes = try Data(contentsOf: URL(fileURLWithPath: reference))
            guard bytes == refBytes else { throw HarnessError.notByteIdentical }
            print("echo-harness: byte-identical to \(reference) ✓")
        }
    }

    /// Delete a stored route by object id (S0 §4.4 `deleteObject`); assert the command succeeds and
    /// the store signals the change.
    static func runDelete(id: UInt16) async throws {
        let central = EchoCentral()
        _ = try await central.connect()
        let before = try await central.readDigest()
        var command = Data([1, ObjectType.route.rawValue])  // cmd 1 = deleteObject · type 1 = route
        command.append(UInt8(id & 0xFF))
        command.append(UInt8(id >> 8))
        central.writeCommand(command)

        let (result, changed) = try await withTimeout(20) {
            let result = await central.nextCommandResult()
            let changed = await central.nextStoreChanged()
            return (result, changed)
        }
        guard result.status == .ok else { throw HarnessError.unexpectedCommandStatus(result.status) }
        let after = try await central.readDigest()
        print(
            "echo-harness: deleted id \(id) ✓ (command ok, storeChanged revision \(changed.revision), "
                + "routes \(before.routeCount)→\(after.routeCount))"
        )
    }

    /// Upload an OBCR file, **abort it mid-transfer** (op=3), and confirm the device discards the
    /// partial — then re-upload the whole object and confirm it commits and lands in the catalog.
    /// A6's "interrupted upload → discard → re-upload" acceptance (uploads restart, not resume —
    /// spec §1 principle 4). The abort is an explicit `transferControl` write, not a CoC-close, so
    /// it's a reliable GATT signal rather than relying on the device detecting a dropped channel.
    static func runAbortTest(path: String) async throws {
        let bytes = try Data(contentsOf: URL(fileURLWithPath: path))
        guard bytes.count >= 2 else { throw CLIError.usage("abort-test needs a route of at least 2 bytes") }
        let crc = CRC32.checksum(bytes)
        let central = EchoCentral()
        let link = try await central.connect()
        let before = try await central.readDigest()

        // 1. Announce the upload, stream only a prefix, then abort it (op=3).
        let start = TransferControl(
            op: .upload, type: .route, objectID: TransferControl.newObjectID,
            totalLen: UInt32(bytes.count), crc32: crc
        )
        central.writeControl(start.encode(), to: link.transferControl)
        try await link.channel.send(Data(bytes.prefix(bytes.count / 2)))  // prefix handed to the CoC
        let abort = TransferControl(op: .abort, type: .route, objectID: TransferControl.newObjectID)
        central.writeControl(abort.encode(), to: link.transferControl)
        let aborted = await central.nextTransferResult()
        guard aborted.status == .aborted else { throw HarnessError.unexpectedStatus(aborted.status) }
        let afterAbort = try await central.readDigest()
        guard afterAbort.routeCount == before.routeCount else { throw HarnessError.unexpectedStatus(.error) }
        print("echo-harness: aborted mid-upload → device discarded the partial (routes still \(afterAbort.routeCount)) ✓")

        // 2. Re-upload the whole object from the start — must commit.
        central.writeControl(start.encode(), to: link.transferControl)
        let committed = try await withTimeout(60) {
            try await link.channel.send(bytes)
            return await central.nextTransferResult()
        }
        guard committed.status == .committed, let newID = committed.objectID else {
            throw HarnessError.unexpectedStatus(committed.status)
        }
        print("echo-harness: re-uploaded from the start → committed as object id \(newID) ✓")

        // 3. Confirm the route is in the catalog.
        let entries = try await downloadRouteList(link: link, central: central)
        guard entries.contains(where: { $0.objectID == newID.raw }) else { throw HarnessError.routeNotListed }
        print("echo-harness: id \(newID) present in routeList ✓ (\(entries.count) route(s))")
    }

    /// The shared download flow (S0 §4.2 op 2): write the request, await the device's announce
    /// descriptor (total_len + crc32), stream the payload off the CoC verifying the whole-object CRC,
    /// then consume the closing `transferResult`.
    static func downloadObject(
        link: EchoLink, central: EchoCentral, type: ObjectType, objectID: UInt16
    ) async throws -> Data {
        let request = TransferControl(op: .download, type: type, objectID: objectID)
        central.writeControl(request.encode(), to: link.transferControl)
        let announce = await central.nextAnnounce()
        let bytes = try await withTimeout(60) {
            try await link.channel.receive(length: Int(announce.totalLen), expectedCRC: announce.crc32)
        }
        let result = await central.nextTransferResult()
        guard result.status == .committed else { throw HarnessError.unexpectedStatus(result.status) }
        return bytes
    }

    static func downloadRouteList(link: EchoLink, central: EchoCentral) async throws -> [RouteListEntry] {
        try RouteList.decode(try await downloadObject(link: link, central: central, type: .routeList, objectID: 0))
    }

    // MARK: - A5 echo loopback (#273)

    static func runEcho(_ opts: Options) async throws {
        let central = EchoCentral()
        let link = try await central.connect()
        print(
            "echo-harness: link up — \(opts.count) × \(opts.size) B echoes"
                + (opts.corrupt ? " (CRC-corruption injection)" : "")
        )

        var failures = 0
        let overallStart = Date()
        for i in 1...opts.count {
            do {
                try await runOneEcho(index: i, of: opts.count, link: link, central: central, opts: opts)
            } catch {
                failures += 1
                print("echo-harness: [\(i)/\(opts.count)] FAILED — \(error)")
            }
        }
        let elapsed = Date().timeIntervalSince(overallStart)
        let aggregateKBps = Double(opts.count * opts.size) / 1024 / max(elapsed, 0.001)
        print(
            "echo-harness: done — \(opts.count - failures)/\(opts.count) ok in "
                + String(format: "%.1f", elapsed) + " s (~" + String(format: "%.1f", aggregateKBps) + " kB/s aggregate)"
        )
        if failures != 0 { exit(1) }
    }

    /// One echo round-trip. Upload + download run concurrently (each `BLEChannel` call launches its
    /// own task) so the CoC's bidirectional credit flow never deadlocks; the device's `transferResult`
    /// is awaited alongside.
    static func runOneEcho(index: Int, of total: Int, link: EchoLink, central: EchoCentral, opts: Options) async throws {
        let payload = Data((0..<opts.size).map { _ in UInt8.random(in: 0...255) })
        let announcedCRC = CRC32.checksum(payload)
        // The bytes actually sent: one flipped byte when injecting a CRC fault (announced CRC unchanged).
        var sent = payload
        if opts.corrupt { sent[sent.startIndex + sent.count / 2] ^= 0xFF }

        let start = TransferControl(
            op: .upload, type: .echo, objectID: 0, totalLen: UInt32(payload.count), crc32: announcedCRC
        )
        // Arm the device (control plane), then run the duplex echo (data plane). A per-object timeout
        // turns a stalled CoC into a reported failure instead of a silent hang.
        central.writeControl(start.encode(), to: link.transferControl)
        let deviceResult = Task { await central.nextTransferResult() }
        // Upload + download run concurrently so the CoC's bidirectional credit
        // flow never deadlocks.
        let channel = link.channel
        let uploadTask = Task { try await channel.send(sent) }
        let downloadTask = Task { try await channel.receive(length: payload.count, expectedCRC: announcedCRC) }

        let clockStart = Date()
        let (echoed, result) = try await withTimeout(20) {
            _ = try? await uploadTask.value // all bytes handed to the CoC
            let echoed = try? await downloadTask.value // bytes streamed back (nil on the expected corrupt reject)
            let result = await deviceResult.value
            return (echoed, result)
        }
        let ms = Date().timeIntervalSince(clockStart) * 1000

        if opts.corrupt {
            guard result.status == .crcMismatch else { throw HarnessError.unexpectedStatus(result.status) }
            print("echo-harness: [\(index)/\(total)] corruption rejected — device → crcMismatch ✓")
        } else {
            guard result.status == .committed else { throw HarnessError.unexpectedStatus(result.status) }
            guard echoed == payload else { throw HarnessError.notByteIdentical }
            let kbps = Double(opts.size) / 1024 / max(ms / 1000, 0.001)
            print(
                "echo-harness: [\(index)/\(total)] byte-identical ✓ device → committed ("
                    + String(format: "%.0f", ms) + " ms, ~" + String(format: "%.0f", kbps) + " kB/s)"
            )
        }
    }

    // MARK: - helpers

    static func requirePath(_ path: String?, _ usage: String) throws -> String {
        guard let path else { throw CLIError.usage(usage) }
        return path
    }

    static func flagValue(_ name: String, flags: [String]) -> String? {
        guard let i = flags.firstIndex(of: name), i + 1 < flags.count else { return nil }
        return flags[i + 1]
    }

    /// An `--name N` integer option, or `default` if absent/unparseable.
    static func intOption(_ name: String, default def: Int, in args: [String]) -> Int {
        guard let i = args.firstIndex(of: name), i + 1 < args.count, let v = Int(args[i + 1]) else { return def }
        return max(1, v)
    }

    /// Whether a boolean `--flag` is present.
    static func hasFlag(_ name: String, in args: [String]) -> Bool { args.contains(name) }

    /// Positional (file) arguments after the subcommand, **excluding** the values consumed by
    /// value-taking flags (so `soak a.obcr b.obcr --count 50` yields `[a.obcr, b.obcr]`, not `…, 50`).
    static func fileArgs(after subcommand: String, in args: [String], valueFlags: Set<String>) -> [String] {
        var result: [String] = []
        var skipNext = false
        var droppedSubcommand = false
        for a in args {
            if skipNext {
                skipNext = false
                continue
            }
            if valueFlags.contains(a) {
                skipNext = true
                continue
            }
            if a.hasPrefix("-") { continue }
            if !droppedSubcommand {
                droppedSubcommand = true  // the subcommand token itself
                continue
            }
            result.append(a)
        }
        return result
    }

    static func hex(_ v: UInt32) -> String { "0x" + String(v, radix: 16, uppercase: true) }
    static func fmtKm(_ meters: UInt32) -> String { String(format: "%.1f", Double(meters) / 1000) }

    static func stderr(_ line: String) {
        FileHandle.standardError.write(Data((line + "\n").utf8))
    }

    static let usage = """
        echo-harness — drive + soak-test the firmware BLE data planes over the real app byte layer

        USAGE: swift run echo-harness <subcommand> [args]

        BRING-UP / SINGLE-SHOT (A5/A6/A7)
          echo [--count N] [--size BYTES] [--corrupt]   A5 loopback (#273); default subcommand
          upload <file.obcr>                            upload a route → SD, assert committed (A6)
          list                                          download + print the routeList (§7.4)
          detail <id> [reference.obcr] [--out file]     download a route; verify CRC + byte-identity
          delete <id>                                   deleteObject; assert command ok + storeChanged
          abort-test <file.obcr>                        abort mid-upload → discard → re-upload + verify
          diagnostics [--verbose]                       read + print the device diagnostics blob (§7.5)

        A9 SOAK + FAULT INJECTION (#277) — each asserts the device counters agree with the harness
          soak <file.obcr>... [--count N] [--no-cleanup]  N golden-path uploads, verify-by-list each
          drop-matrix <file.obcr> [--iterations K]        kill the link mid up/download → restart + verify
          corruption <file.obcr>                          CRC / offset / malformed → typed rejects, clean state
          storm [--iterations K]                          N connect/disconnect cycles; counters must track
          concurrency <file.obcr>                         busy gate, command-during-transfer, back-to-back reconnects

          --help                                          this message

        ECHO OPTIONS
          --count N       objects to echo (default 100; the A5 DoD run is 1000)
          --size BYTES    bytes per object (default 32768)
          --corrupt       flip one byte per object; expect the device to reject with crcMismatch

        SOAK OPTIONS
          --count N       soak: uploads to run (default 50 — the epic's headline DoD gate)
          --iterations K  drop-matrix / storm: cycles to run (default 10 / 20)
          --no-cleanup    soak: keep every uploaded route (default deletes each after verifying)
          --verbose       diagnostics: also print the full raw blob
        """
}

/// The harness's command-line options (echo subcommand).
struct Options {
    var count = 100
    var size = 32_768 // 32 KB — a route-object-scale payload
    var corrupt = false

    init(_ args: [String]) {
        var it = args.makeIterator()
        while let arg = it.next() {
            switch arg {
            case "--count": if let v = it.next().flatMap({ Int($0) }) { count = max(1, v) }
            case "--size": if let v = it.next().flatMap({ Int($0) }) { size = max(1, v) }
            case "--corrupt": corrupt = true
            default: break
            }
        }
    }
}

enum CLIError: Error, CustomStringConvertible {
    case usage(String)
    var description: String {
        switch self {
        case .usage(let message): return "usage: \(message)"
        }
    }
}
#endif
