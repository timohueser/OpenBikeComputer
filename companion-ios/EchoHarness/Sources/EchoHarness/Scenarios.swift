#if canImport(CoreBluetooth)
import Foundation
import OBCTransport

/// The A9 soak + fault-injection scenarios (issue #277) — the reliability rig grown from the A5 echo
/// seed. Each scenario is scriptable, repeatable, and asserts on **both ledgers**: the harness's own
/// observations *and* the device's diagnostics counters (§7.5), which must agree. They reuse the real
/// app byte layer + codecs (`BLEChannel` / `TransferControl` / `RouteList` / `CRC32`) exactly as the
/// echo/route subcommands do; only the *fault injection* — dropping the link mid-transfer, flipping
/// bytes, storming reconnects — is harness-owned, because the app transport has no verb for "fail now".
///
/// The golden path (spec §1): transfers **restart, never resume** — a dropped upload is re-sent whole
/// and the device's partial is discarded; a dropped download is re-requested whole. Every scenario
/// proves that end to end and confirms the device neither rebooted (`boot_count` steady) nor wedged.
extension EchoHarness {
    /// One CoC SDU — the read/write granularity a partial transfer drop is cut on.
    static let chunkBytes = 244

    // MARK: - Golden-path soak (the 50-upload DoD gate)

    /// Upload `count` routes (cycling the given files for size variety, incl. waypoint-bearing ones),
    /// **verify-by-list-read after each**, and reconcile the ledgers: one connection, no reboot, no
    /// stray drops, the catalog tracking the uploads. `cleanup` deletes each route after verifying it so
    /// a long soak can't fill the card; `--no-cleanup` accumulates (bounded by the device's catalog cap).
    static func runSoak(paths: [String], count: Int, cleanup: Bool) async throws {
        let routes = try paths.map { (path: $0, bytes: try Data(contentsOf: URL(fileURLWithPath: $0))) }
        let central = EchoCentral()
        let link = try await central.connect()
        let base = try await readDiagnostics(link: link, central: central)
        let baseDigest = try await central.readDigest()
        print(
            "echo-harness: soak — \(count) uploads across \(routes.count) route file(s), "
                + (cleanup ? "cleanup on" : "accumulating"))
        print("echo-harness: baseline — \(base.summary)")

        let start = Date()
        var totalBytes = 0
        for i in 1...count {
            let route = routes[(i - 1) % routes.count]
            let id = try await uploadObject(link: link, central: central, bytes: route.bytes)
            try await assertListed(link: link, central: central, id: id)  // verify-by-list-read (A9)
            totalBytes += route.bytes.count
            if cleanup { try await deleteRoute(link: link, central: central, id: id) }
            if i % 10 == 0 || i == count {
                print("echo-harness: [\(i)/\(count)] ok — id \(id), \(route.bytes.count) B")
            }
        }
        let elapsed = Date().timeIntervalSince(start)

        // Reconcile both ledgers.
        let after = try await readDiagnostics(link: link, central: central)
        let afterDigest = try await central.readDigest()
        print("echo-harness: final — \(after.summary)")
        try expect(after.bootCount == base.bootCount, "device rebooted during the soak (boot #\(base.bootCount)→#\(after.bootCount))")
        try expect(after.disconnects == base.disconnects, "the link dropped during the soak (disconnects \(base.disconnects)→\(after.disconnects))")
        try expect(after.sdOK, "device reports SD not ok after the soak")
        if cleanup {
            try expect(afterDigest.routeCount == baseDigest.routeCount, "routeCount drifted \(baseDigest.routeCount)→\(afterDigest.routeCount) despite cleanup")
        } else {
            let expected = Int(baseDigest.routeCount) + count
            try expect(Int(afterDigest.routeCount) == expected, "routeCount \(baseDigest.routeCount)→\(afterDigest.routeCount), expected +\(count)")
        }
        let kbps = Double(totalBytes) / 1024 / max(elapsed, 0.001)
        print(
            "echo-harness: soak PASSED ✓ — \(count)/\(count) uploads, ledgers agree, no reboot ("
                + String(format: "%.1f s, ~%.1f kB/s", elapsed, kbps) + ")")
    }

    // MARK: - Drop / restart matrix

    /// Kill the link at randomized points (incl. the first and last chunk) during **upload** *and*
    /// **download**, then prove whole-object restart: the device discards the upload partial (routeCount
    /// unchanged), the re-uploaded object is byte-identical, and a mid-download drop recovers by a whole
    /// re-request. Reconciles: the device saw exactly the drops we induced and never rebooted.
    static func runDropMatrix(path: String, iterations: Int) async throws {
        let bytes = try Data(contentsOf: URL(fileURLWithPath: path))
        try expect(bytes.count >= 8, "drop-matrix needs a route of at least 8 bytes")
        let crc = CRC32.checksum(bytes)
        let central = EchoCentral()
        var link = try await central.connect()
        let base = try await readDiagnostics(link: link, central: central)
        print("echo-harness: drop-matrix — \(iterations) iteration(s) on \(bytes.count) B, upload + download drops")
        print("echo-harness: baseline — \(base.summary)")
        var inducedDrops = 0

        for i in 1...iterations {
            // Spread the cut across [0, 1]; guarantee the first-chunk (0) and last-chunk (1) extremes.
            let fraction: Double = i == 1 ? 0.0 : (i == iterations ? 1.0 : Double.random(in: 0...1))

            // ── Upload drop → the device must discard the partial ──
            let beforeDigest = try await central.readDigest()
            let prefixLen = min(Int(Double(bytes.count) * fraction), bytes.count - 1)  // never the whole object
            let arm = TransferControl(op: .upload, type: .route, objectID: TransferControl.newObjectID, totalLen: UInt32(bytes.count), crc32: crc)
            central.writeControl(arm.encode(), to: link.transferControl)
            if prefixLen > 0 {
                let l = link
                _ = try? await withTimeout(30) { try await l.channel.send(bytes.prefix(prefixLen)) }
            }
            await central.disconnect()
            inducedDrops += 1
            link = try await central.connect()
            let afterDrop = try await central.readDigest()
            try expect(afterDrop.routeCount == beforeDigest.routeCount, "device kept an upload partial at \(pct(fraction)) (routeCount \(beforeDigest.routeCount)→\(afterDrop.routeCount))")

            // Whole-object restart → committed + byte-identical.
            let id = try await uploadObject(link: link, central: central, bytes: bytes)
            let round = try await downloadObject(link: link, central: central, type: .route, objectID: id)
            try expect(round == bytes, "restarted upload not byte-identical to the original")

            // ── Download drop → recover by a whole re-request ──
            let req = TransferControl(op: .download, type: .route, objectID: id)
            central.writeControl(req.encode(), to: link.transferControl)
            let announce = try await withTimeout(30) { await central.nextAnnounce() }
            do {  // read a prefix (CRC 0 never matches a partial → throws once the prefix is in), then drop
                let l = link
                let want = min(chunkBytes, Int(announce.totalLen))
                _ = try? await withTimeout(30) { try await l.channel.receive(length: want, expectedCRC: 0) }
            }
            await central.disconnect()
            inducedDrops += 1
            link = try await central.connect()
            let full = try await downloadObject(link: link, central: central, type: .route, objectID: id)
            try expect(full == bytes, "re-requested download not byte-identical")

            try await deleteRoute(link: link, central: central, id: id)  // keep the card bounded
            print("echo-harness: [\(i)/\(iterations)] drop@\(pct(fraction)) — upload restart + download re-request byte-identical ✓")
        }

        let after = try await readDiagnostics(link: link, central: central)
        print("echo-harness: final — \(after.summary)")
        try expect(after.disconnects >= base.disconnects + UInt32(inducedDrops), "device link_disconnects \(base.disconnects)→\(after.disconnects) < the \(inducedDrops) drops induced")
        try expect(after.bootCount == base.bootCount, "device rebooted during the drop matrix (boot #\(base.bootCount)→#\(after.bootCount))")
        print("echo-harness: drop-matrix PASSED ✓ — \(inducedDrops) induced drops, all recovered to byte-identical objects")
    }

    // MARK: - CRC / corruption / offset injection

    /// Feed the device malformed transfers and assert **typed rejects** with the store left clean after
    /// each: a CRC-corrupted echo (`crcMismatch`), a CRC-corrupted upload (`crcMismatch`, nothing
    /// committed), a non-zero upload offset (`error` — transfers restart, never resume), and a malformed
    /// descriptor (`error`). Confirms the device didn't commit garbage, didn't wedge, and didn't reboot.
    static func runCorruption(path: String) async throws {
        let bytes = try Data(contentsOf: URL(fileURLWithPath: path))
        try expect(bytes.count >= 2, "corruption needs a route of at least 2 bytes")
        let crc = CRC32.checksum(bytes)
        let central = EchoCentral()
        let link = try await central.connect()
        let base = try await readDiagnostics(link: link, central: central)
        let baseDigest = try await central.readDigest()
        print("echo-harness: corruption — 4 fault classes, device state must stay clean after each")

        // 1. Echo a payload whose bytes were flipped but whose announced CRC is intact → crcMismatch.
        let flipped: Data = {
            var f = bytes
            f[f.startIndex + f.count / 2] ^= 0xFF
            return f
        }()
        let echoArm = TransferControl(op: .upload, type: .echo, objectID: 0, totalLen: UInt32(bytes.count), crc32: crc)
        central.writeControl(echoArm.encode(), to: link.transferControl)
        let echoResult = try await withTimeout(30) { () -> TransferResult in
            let ch = link.channel
            async let up: Void = ch.send(flipped)
            async let down = try? ch.receive(length: bytes.count, expectedCRC: crc)  // may reject before EOF
            _ = try? await up
            _ = await down
            return await central.nextTransferResult()
        }
        try expect(echoResult.status == .crcMismatch, "corrupted echo returned \(echoResult.status), expected crcMismatch")
        print("echo-harness: corruption — flipped-byte echo → crcMismatch ✓")

        // 2. Upload a route with a flipped byte (announced CRC of the *clean* object) → crcMismatch, nothing committed.
        let uploadArm = TransferControl(op: .upload, type: .route, objectID: TransferControl.newObjectID, totalLen: UInt32(bytes.count), crc32: crc)
        central.writeControl(uploadArm.encode(), to: link.transferControl)
        let uploadResult = try await withTimeout(60) { () -> TransferResult in
            try await link.channel.send(flipped)
            return await central.nextTransferResult()
        }
        try expect(uploadResult.status == .crcMismatch, "corrupted upload returned \(uploadResult.status), expected crcMismatch")
        try await assertDigest(central, routeCount: baseDigest.routeCount, "a corrupted upload committed a route")
        print("echo-harness: corruption — flipped-byte upload → crcMismatch, nothing committed ✓")

        // 3. Non-zero upload offset → error (uploads start at 0; transfers restart, never resume).
        let offsetArm = TransferControl(op: .upload, type: .route, objectID: TransferControl.newObjectID, totalLen: UInt32(bytes.count), crc32: crc, offset: 4)
        central.writeControl(offsetArm.encode(), to: link.transferControl)
        let offsetResult = try await withTimeout(20) { await central.nextTransferResult() }
        try expect(offsetResult.status == .error, "non-zero upload offset returned \(offsetResult.status), expected error")
        print("echo-harness: corruption — non-zero upload offset → error ✓")

        // 4. Malformed descriptor (an 8-byte truncated transferControl) → error.
        central.writeControl(Data(repeating: 0xAB, count: 8), to: link.transferControl)
        let malformedResult = try await withTimeout(20) { await central.nextTransferResult() }
        try expect(malformedResult.status == .error, "malformed descriptor returned \(malformedResult.status), expected error")
        print("echo-harness: corruption — malformed descriptor → error ✓")

        // Device still clean + reachable + never rebooted.
        try await assertDigest(central, routeCount: baseDigest.routeCount, "the store changed after the corruption suite")
        let after = try await readDiagnostics(link: link, central: central)
        try expect(after.bootCount == base.bootCount, "device rebooted during the corruption suite (boot #\(base.bootCount)→#\(after.bootCount))")
        print("echo-harness: corruption PASSED ✓ — all four rejected cleanly; \(after.summary)")
    }

    // MARK: - Connect / disconnect storm

    /// Cycle the link connect→disconnect `iterations` times (bonding active — the OS re-encrypts from the
    /// stored keys with no dialog), asserting each reconnect succeeds and the device's counters track the
    /// harness's exactly. The "always available" invariant (spec §6) under churn.
    static func runStorm(iterations: Int) async throws {
        let central = EchoCentral()
        var link = try await central.connect()
        let base = try await readDiagnostics(link: link, central: central)
        print("echo-harness: storm — \(iterations) connect/disconnect cycle(s)")
        print("echo-harness: baseline — \(base.summary)")

        for i in 1...iterations {
            await central.disconnect()
            link = try await central.connect()
            if i % 10 == 0 || i == iterations { print("echo-harness: [\(i)/\(iterations)] reconnected ✓") }
        }

        let after = try await readDiagnostics(link: link, central: central)
        print("echo-harness: final — \(after.summary)")
        try expect(after.connects == base.connects + UInt32(iterations), "device link_connects \(base.connects)→\(after.connects), expected +\(iterations)")
        try expect(after.disconnects == base.disconnects + UInt32(iterations), "device link_disconnects \(base.disconnects)→\(after.disconnects), expected +\(iterations)")
        try expect(after.bootCount == base.bootCount, "device rebooted during the storm (boot #\(base.bootCount)→#\(after.bootCount))")
        print("echo-harness: storm PASSED ✓ — \(iterations) reconnects, ledgers agree")
    }

    // MARK: - Concurrency probes

    /// Probe the plane boundaries: a second transfer opened while one is active → `busy`; a `command`
    /// issued while a transfer is parked is still answered (the control plane stays responsive under a
    /// held data plane); and five back-to-back reconnects with no settle time. (Disconnect-during-SD-write
    /// is exercised by `drop-matrix`.)
    static func runConcurrency(path: String) async throws {
        let bytes = try Data(contentsOf: URL(fileURLWithPath: path))
        let crc = CRC32.checksum(bytes)
        let central = EchoCentral()
        var link = try await central.connect()
        let base = try await readDiagnostics(link: link, central: central)
        print("echo-harness: concurrency — busy gate, command-during-transfer, back-to-back reconnects")

        // 1. Arm one upload (park it — no CoC bytes), then open a second → busy.
        let arm = TransferControl(op: .upload, type: .route, objectID: TransferControl.newObjectID, totalLen: UInt32(bytes.count), crc32: crc)
        central.writeControl(arm.encode(), to: link.transferControl)
        central.writeControl(arm.encode(), to: link.transferControl)  // second open while the first is active
        let busy = try await withTimeout(20) { await central.nextTransferResult() }
        try expect(busy.status == .busy, "second transfer during an active one returned \(busy.status), expected busy")
        print("echo-harness: concurrency — second transfer during active → busy ✓")

        // 2. A command while the first transfer is still parked → the control plane still answers.
        var del = Data([1, ObjectType.route.rawValue])
        del.append(0xFE)
        del.append(0xFF)  // delete id 0xFFFE (absent) → notFound, proving responsiveness
        central.writeCommand(del)
        let cmd = try await withTimeout(20) { await central.nextCommandResult() }
        try expect(cmd.status == .notFound, "command during an active transfer returned \(cmd.status), expected notFound")
        print("echo-harness: concurrency — command answered while a transfer is parked ✓")

        // Release the parked transfer.
        let abort = TransferControl(op: .abort, type: .route, objectID: TransferControl.newObjectID)
        central.writeControl(abort.encode(), to: link.transferControl)
        let aborted = try await withTimeout(20) { await central.nextTransferResult() }
        try expect(aborted.status == .aborted, "aborting the parked transfer returned \(aborted.status), expected aborted")

        // 3. Back-to-back reconnects, no settle time.
        for _ in 1...5 {
            await central.disconnect()
            link = try await central.connect()
        }
        print("echo-harness: concurrency — 5 back-to-back reconnects ✓")

        let after = try await readDiagnostics(link: link, central: central)
        try expect(after.bootCount == base.bootCount, "device rebooted during the concurrency probes (boot #\(base.bootCount)→#\(after.bootCount))")
        print("echo-harness: concurrency PASSED ✓ — \(after.summary)")
    }

    // MARK: - Diagnostics read

    /// Print the device's diagnostics blob (§7.5) — the health line, plus the full raw text with
    /// `--verbose`.
    static func runDiagnostics(verbose: Bool) async throws {
        let central = EchoCentral()
        let link = try await central.connect()
        let diag = try await readDiagnostics(link: link, central: central)
        print("echo-harness: diagnostics — \(diag.summary)")
        if verbose { print("---\n\(diag.raw.trimmingCharacters(in: .whitespacesAndNewlines))\n---") }
    }

    // MARK: - Shared scenario helpers

    /// Upload one object and assert it commits; returns the device-assigned id. Drains the trailing
    /// `storeChanged` so the status buffer stays clean across a long soak.
    static func uploadObject(link: EchoLink, central: EchoCentral, bytes: Data) async throws -> UInt16 {
        let crc = CRC32.checksum(bytes)
        let arm = TransferControl(op: .upload, type: .route, objectID: TransferControl.newObjectID, totalLen: UInt32(bytes.count), crc32: crc)
        central.writeControl(arm.encode(), to: link.transferControl)
        let result = try await withTimeout(60) { () -> TransferResult in
            try await link.channel.send(bytes)
            return await central.nextTransferResult()
        }
        guard result.status == .committed, let id = result.objectID else { throw HarnessError.unexpectedStatus(result.status) }
        _ = try await withTimeout(20) { await central.nextStoreChanged() }  // drain the commit's storeChanged
        return id.raw
    }

    /// Download the routeList and assert `id` is present (verify-by-list-read).
    static func assertListed(link: EchoLink, central: EchoCentral, id: UInt16) async throws {
        let entries = try await downloadRouteList(link: link, central: central)
        try expect(entries.contains { $0.objectID == id }, "route \(id) not present in the routeList (\(entries.count) entries)")
    }

    /// Delete a route by id, draining both the `commandResult` and the trailing `storeChanged`.
    static func deleteRoute(link: EchoLink, central: EchoCentral, id: UInt16) async throws {
        var command = Data([1, ObjectType.route.rawValue])
        command.append(UInt8(id & 0xFF))
        command.append(UInt8(id >> 8))
        central.writeCommand(command)
        let result = try await withTimeout(20) { await central.nextCommandResult() }
        guard result.status == .ok else { throw HarnessError.unexpectedCommandStatus(result.status) }
        _ = try await withTimeout(20) { await central.nextStoreChanged() }
    }

    /// Assert the store digest still holds `routeCount` routes (nothing committed / nothing lost).
    static func assertDigest(_ central: EchoCentral, routeCount: UInt16, _ why: String) async throws {
        let digest = try await central.readDigest()
        try expect(digest.routeCount == routeCount, "\(why) (routeCount now \(digest.routeCount), expected \(routeCount))")
    }

    /// Throw a described assertion failure unless `condition` holds — the funnel for both harness-side
    /// checks and device-ledger disagreements.
    static func expect(_ condition: Bool, _ message: @autoclosure () -> String) throws {
        if !condition { throw HarnessError.assertion(message()) }
    }

    /// A drop fraction as a whole-percent label for logs.
    static func pct(_ fraction: Double) -> String { "\(Int((fraction * 100).rounded()))%" }
}
#endif
