#if canImport(CoreBluetooth)
import Foundation
import OBCTransport

/// The A5 echo harness (issue #273): scan for the OBC device, open the L2CAP CoC, and echo-round-trip
/// N objects through the *real* app byte plane (`BLEChannel` + `L2CAPByteChannel`), asserting each
/// comes back byte-identical and the device commits it. `--corrupt` flips a byte per object and
/// asserts the device rejects it with the S0 `crcMismatch` status (§6). The A9 soak rig grows from
/// here (induced disconnects + offset-resume).
///
/// Run on a Mac with the device powered + advertising:
/// ```
/// swift run echo-harness --count 1000 --size 32768        # the DoD run: 1000 × 32 KB
/// swift run echo-harness --count 50 --corrupt             # CRC fault injection
/// ```
@main
struct EchoHarness {
    static func main() async {
        let opts = Options(CommandLine.arguments)
        if opts.showHelp {
            print(Options.usage)
            return
        }

        let central = EchoCentral()
        let link: EchoLink
        do {
            link = try await central.connect()
        } catch {
            stderr("echo-harness: connect failed: \(error)")
            exit(1)
        }
        print(
            "echo-harness: link up — \(opts.count) × \(opts.size) B echoes"
                + (opts.corrupt ? " (CRC-corruption injection)" : "")
        )

        var failures = 0
        let overallStart = Date()
        for i in 1...opts.count {
            do {
                try await runOne(index: i, of: opts.count, link: link, central: central, opts: opts)
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
        exit(failures == 0 ? 0 : 1)
    }

    /// One echo round-trip. Upload + download run concurrently (each `BLEChannel` call launches its
    /// own task) so the CoC's bidirectional credit flow never deadlocks; the device's `transferResult`
    /// is awaited alongside.
    static func runOne(index: Int, of total: Int, link: EchoLink, central: EchoCentral, opts: Options) async throws {
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
        let upload = link.channel.upload(sent)
        let (_, downloadTask) = link.channel.download(length: payload.count, expectedCRC: announcedCRC)

        let clockStart = Date()
        let (echoed, result) = try await withTimeout(20) {
            _ = await upload.outcome // all bytes handed to the CoC
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

    static func stderr(_ line: String) {
        FileHandle.standardError.write(Data((line + "\n").utf8))
    }
}

/// The harness's command-line options.
struct Options {
    var count = 100
    var size = 32_768 // 32 KB — a route-object-scale payload
    var corrupt = false
    var showHelp = false

    static let usage = """
        echo-harness — drive the firmware A5 L2CAP CoC echo loopback (issue #273)

        USAGE: swift run echo-harness [--count N] [--size BYTES] [--corrupt]

          --count N       number of objects to echo (default 100; the DoD run is 1000)
          --size BYTES    bytes per object (default 32768)
          --corrupt       flip one byte per object; expect the device to reject with crcMismatch
          --help          this message
        """

    init(_ args: [String]) {
        var it = args.dropFirst().makeIterator()
        while let arg = it.next() {
            switch arg {
            case "--count": if let v = it.next().flatMap({ Int($0) }) { count = max(1, v) }
            case "--size": if let v = it.next().flatMap({ Int($0) }) { size = max(1, v) }
            case "--corrupt": corrupt = true
            case "--help", "-h": showHelp = true
            default: break
            }
        }
    }
}
#endif
