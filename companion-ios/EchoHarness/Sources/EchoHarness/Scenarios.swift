#if canImport(CoreBluetooth)
import Foundation
import OBCDomain
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
        let baseRoutes = try await routeCount(link: link, central: central)
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
        let afterRoutes = try await routeCount(link: link, central: central)
        print("echo-harness: final — \(after.summary)")
        try expect(after.bootCount == base.bootCount, "device rebooted during the soak (boot #\(base.bootCount)→#\(after.bootCount))")
        try expect(after.disconnects == base.disconnects, "the link dropped during the soak (disconnects \(base.disconnects)→\(after.disconnects))")
        try expect(after.sdOK, "device reports SD not ok after the soak")
        if cleanup {
            try expect(afterRoutes == baseRoutes, "routeCount drifted \(baseRoutes)→\(afterRoutes) despite cleanup")
        } else {
            let expected = baseRoutes + count
            try expect(afterRoutes == expected, "routeCount \(baseRoutes)→\(afterRoutes), expected +\(count)")
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
            let beforeRoutes = try await routeCount(link: link, central: central)
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
            let afterDrop = try await routeCount(link: link, central: central)
            try expect(afterDrop == beforeRoutes, "device kept an upload partial at \(pct(fraction)) (routeCount \(beforeRoutes)→\(afterDrop))")

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
    /// committed), and a malformed descriptor (`error`). Confirms the device didn't commit garbage,
    /// didn't wedge, and didn't reboot. (v2 dropped the descriptor's `offset` field — a transfer restarts
    /// rather than resumes — so the old "non-zero upload offset" fault class no longer has a wire field to
    /// carry it; a short/garbage descriptor still exercises the malformed-descriptor reject below.)
    static func runCorruption(path: String) async throws {
        let bytes = try Data(contentsOf: URL(fileURLWithPath: path))
        try expect(bytes.count >= 2, "corruption needs a route of at least 2 bytes")
        let crc = CRC32.checksum(bytes)
        let central = EchoCentral()
        let link = try await central.connect()
        let base = try await readDiagnostics(link: link, central: central)
        let baseRoutes = try await routeCount(link: link, central: central)
        print("echo-harness: corruption — 3 fault classes, device state must stay clean after each")

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
        try await assertRouteCount(link: link, central: central, expected: baseRoutes, "a corrupted upload committed a route")
        print("echo-harness: corruption — flipped-byte upload → crcMismatch, nothing committed ✓")

        // 3. Malformed descriptor (an 8-byte write, under the 12-byte descriptor) → error.
        central.writeControl(Data(repeating: 0xAB, count: 8), to: link.transferControl)
        let malformedResult = try await withTimeout(20) { await central.nextTransferResult() }
        try expect(malformedResult.status == .error, "malformed descriptor returned \(malformedResult.status), expected error")
        print("echo-harness: corruption — malformed descriptor → error ✓")

        // Device still clean + reachable + never rebooted.
        try await assertRouteCount(link: link, central: central, expected: baseRoutes, "the store changed after the corruption suite")
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

    // MARK: - Trip object lifecycle (TR4, #653)

    /// The trip-object soak TR4 (#653) had to skip while the harness was still on v1 primitives — the
    /// whole type-9 trip lifecycle over the real byte layer. It exercises the invariants the trip work
    /// (epic #632) turns on: a trip **references** routes by device id and carries no route bytes; a
    /// replace-by-id upload is atomic and re-fingerprints the stored trip; deleting a member route leaves
    /// a **dangling** stage the device tolerates (it never rewrites a stored trip), so `stageCount` holds
    /// and the stored-bytes `crc32` is unchanged while the **live** `tripList` totals shrink to the
    /// still-resolvable stages; and a trip delete is non-cascading and signals `storeChanged(type = trip)`.
    ///
    /// The sequence: upload 2 routes + a trip referencing them → read the tripList back → replace the trip
    /// reordered (fingerprint moves, totals steady) → delete one member route (dangling tolerated, totals
    /// shrink, stored crc unchanged) → delete the trip (storeChanged type = trip, member survives as a
    /// top-level route). Reconciles the ledgers: no reboot, catalog back to baseline after cleanup.
    static func runTripSoak(paths: [String]) async throws {
        // Two routes give the trip two resolvable stages; a lone file is cycled twice (still two device
        // objects with distinct ids, so the reorder still moves the fingerprint and a member delete still
        // shrinks the totals).
        try expect(!paths.isEmpty, "trip-soak needs at least one .obcr route file")
        let files = paths.count >= 2 ? Array(paths.prefix(2)) : [paths[0], paths[0]]
        let routeBytes = try files.map { try Data(contentsOf: URL(fileURLWithPath: $0)) }
        let name = "Echo Soak Trip"

        let central = EchoCentral()
        let link = try await central.connect()
        let base = try await readDiagnostics(link: link, central: central)
        let baseRoutes = try await routeCount(link: link, central: central)
        let baseTrips = try await downloadTripList(link: link, central: central).count
        print("echo-harness: trip-soak — upload 2 routes + a trip, list, reorder, member-delete, trip-delete")
        print("echo-harness: baseline — \(base.summary)")

        // 1. Upload the two member routes; grab their catalog entries (for the resolvable-total check).
        let r1 = try await uploadObject(link: link, central: central, bytes: routeBytes[0])
        let r2 = try await uploadObject(link: link, central: central, bytes: routeBytes[1])
        let catalog = try await downloadRouteList(link: link, central: central)
        guard let e1 = catalog.first(where: { $0.objectID == r1 }),
            let e2 = catalog.first(where: { $0.objectID == r2 })
        else { throw HarnessError.assertion("uploaded member routes \(r1)/\(r2) not both in the routeList") }
        print("echo-harness: trip-soak — member routes uploaded (ids \(r1), \(r2)) ✓")

        // 2. Upload a fresh trip referencing [r1, r2] in ride order; the stored bytes are the codec's.
        let tripV1 = TripObjectCodec.encode(name: name, deviceStageIDs: [DeviceObjectID(r1), DeviceObjectID(r2)])
        let crcV1 = CRC32.checksum(tripV1)
        let tripID = try await uploadTrip(link: link, central: central, bytes: tripV1, objectID: TransferControl.newObjectID)
        print("echo-harness: trip-soak — trip committed as id \(tripID), storeChanged(trip) ✓")

        // 3. Read the tripList: both stages present, totals summed over both routes, stored-bytes crc ==
        //    the fingerprint we uploaded.
        var entry = try await tripEntry(link: link, central: central, id: tripID)
        try expect(entry.stageCount == 2, "fresh trip stageCount \(entry.stageCount), expected 2")
        try expect(entry.crc32 == crcV1, "tripList crc \(hex(entry.crc32)) != uploaded fingerprint \(hex(crcV1))")
        let totalBefore = entry.totalDistanceMeters
        print("echo-harness: trip-soak — tripList: 2 stages, \(fmtKm(entry.totalDistanceMeters)) km, crc \(hex(entry.crc32)) ✓")

        // 4. Replace-by-id, stages reordered → the fingerprint moves (byte_len + name unchanged); the
        //    totals do not (both routes still resolvable).
        let tripV2 = TripObjectCodec.encode(name: name, deviceStageIDs: [DeviceObjectID(r2), DeviceObjectID(r1)])
        let crcV2 = CRC32.checksum(tripV2)
        try expect(crcV2 != crcV1, "reordered trip has the same fingerprint \(hex(crcV2)) — the reorder was a no-op")
        let replacedID = try await uploadTrip(link: link, central: central, bytes: tripV2, objectID: tripID)
        try expect(replacedID == tripID, "replace-by-id reassigned the trip id \(tripID)→\(replacedID)")
        entry = try await tripEntry(link: link, central: central, id: tripID)
        try expect(entry.crc32 == crcV2, "post-reorder tripList crc \(hex(entry.crc32)), expected \(hex(crcV2))")
        try expect(entry.stageCount == 2, "post-reorder stageCount \(entry.stageCount), expected 2")
        try expect(entry.totalDistanceMeters == totalBefore, "reorder changed the totals (\(totalBefore)→\(entry.totalDistanceMeters) m) — only order changed")
        print("echo-harness: trip-soak — replace-by-id reorder → crc \(hex(crcV1))→\(hex(crcV2)), totals steady ✓")

        // 5. Delete one member route (r1). The device tolerates the now-dangling stage and NEVER rewrites
        //    the stored trip: stageCount holds at 2 and the stored-bytes crc is unchanged, but the live
        //    totals shrink to the one still-resolvable stage (r2).
        try await deleteRoute(link: link, central: central, id: r1)
        entry = try await tripEntry(link: link, central: central, id: tripID)
        try expect(entry.stageCount == 2, "dangling ref changed stageCount to \(entry.stageCount), expected 2 (device serves the trip verbatim)")
        try expect(entry.crc32 == crcV2, "member delete re-fingerprinted the stored trip (crc \(hex(entry.crc32)) != \(hex(crcV2)))")
        try expect(entry.totalDistanceMeters == e2.distanceMeters, "totals after member delete \(entry.totalDistanceMeters) m, expected route \(r2)'s \(e2.distanceMeters) m (the only resolvable stage)")
        try expect(entry.totalAscentMeters == e2.ascentMeters, "ascent after member delete \(entry.totalAscentMeters) m, expected route \(r2)'s \(e2.ascentMeters) m")
        print("echo-harness: trip-soak — member \(r1) (\(e1.distanceMeters) m) deleted → dangling tolerated, totals \(fmtKm(totalBefore))→\(fmtKm(entry.totalDistanceMeters)) km, crc unchanged ✓")

        // 6. Delete the trip → storeChanged(type = trip); the trip leaves the catalog and its surviving
        //    member route becomes a top-level route (still listed — a trip delete never cascades).
        let changed = try await deleteTrip(link: link, central: central, id: tripID)
        try expect(changed.type == .trip, "trip delete signalled storeChanged type \(changed.type), expected trip")
        let tripsAfter = try await downloadTripList(link: link, central: central)
        try expect(!tripsAfter.contains { $0.objectID == tripID }, "trip \(tripID) still listed after delete")
        try expect(tripsAfter.count == baseTrips, "tripList count \(tripsAfter.count) after delete, expected baseline \(baseTrips)")
        let routesAfterTripDelete = try await downloadRouteList(link: link, central: central)
        try expect(routesAfterTripDelete.contains { $0.objectID == r2 }, "surviving member route \(r2) not a top-level route after trip delete")
        print("echo-harness: trip-soak — trip \(tripID) deleted → storeChanged(trip), member \(r2) now top-level ✓")

        // Cleanup + reconcile the ledgers.
        try await deleteRoute(link: link, central: central, id: r2)  // orphaned survivor — keep the card bounded
        let after = try await readDiagnostics(link: link, central: central)
        let afterRoutes = try await routeCount(link: link, central: central)
        try expect(after.bootCount == base.bootCount, "device rebooted during trip-soak (boot #\(base.bootCount)→#\(after.bootCount))")
        try expect(afterRoutes == baseRoutes, "routeCount \(baseRoutes)→\(afterRoutes) after cleanup, expected baseline")
        print("echo-harness: trip-soak PASSED ✓ — full trip lifecycle, ledgers agree; \(after.summary)")
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

    /// Upload a trip object (type 9) and assert it commits, draining the trip `storeChanged`. Pass
    /// `objectID = TransferControl.newObjectID` for a fresh trip (the device assigns the id) or an
    /// existing trip id for a replace-by-id (atomic; the result echoes that id). Returns the committed id.
    static func uploadTrip(link: EchoLink, central: EchoCentral, bytes: Data, objectID: UInt16) async throws -> UInt16 {
        let crc = CRC32.checksum(bytes)
        let arm = TransferControl(op: .upload, type: .trip, objectID: objectID, totalLen: UInt32(bytes.count), crc32: crc)
        central.writeControl(arm.encode(), to: link.transferControl)
        let result = try await withTimeout(60) { () -> TransferResult in
            try await link.channel.send(bytes)
            return await central.nextTransferResult()
        }
        guard result.status == .committed, let id = result.objectID else { throw HarnessError.unexpectedStatus(result.status) }
        let changed = try await withTimeout(20) { await central.nextStoreChanged() }  // the trip store's commit signal
        try expect(changed.type == .trip, "trip commit signalled storeChanged type \(changed.type), expected trip")
        return id.raw
    }

    /// Delete a trip by id (deleteObject, trip type — non-cascading, §7.7), draining the `commandResult`;
    /// returns the trip `storeChanged` for the caller to assert on.
    static func deleteTrip(link: EchoLink, central: EchoCentral, id: UInt16) async throws -> StoreChanged {
        var command = Data([1, ObjectType.trip.rawValue])  // cmd 1 = deleteObject · type 9 = trip
        command.append(UInt8(id & 0xFF))
        command.append(UInt8(id >> 8))
        central.writeCommand(command)
        let result = try await withTimeout(20) { await central.nextCommandResult() }
        guard result.status == .ok else { throw HarnessError.unexpectedCommandStatus(result.status) }
        return try await withTimeout(20) { await central.nextStoreChanged() }
    }

    /// Download + decode the `tripList` (type 10, §7.4) — the trip sibling of `downloadRouteList`.
    static func downloadTripList(link: EchoLink, central: EchoCentral) async throws -> [TripListEntry] {
        try TripList.decode(try await downloadObject(link: link, central: central, type: .tripList, objectID: 0))
    }

    /// The `tripList` entry for `id`, or a described failure if the trip isn't listed.
    static func tripEntry(link: EchoLink, central: EchoCentral, id: UInt16) async throws -> TripListEntry {
        let entries = try await downloadTripList(link: link, central: central)
        guard let entry = entries.first(where: { $0.objectID == id }) else {
            throw HarnessError.assertion("trip \(id) not present in the tripList (\(entries.count) entries)")
        }
        return entry
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

    /// The device's current route count, read from the `routeList` catalog — the v2 replacement for the
    /// retired `objectStore` digest's `routeCount` (the digest characteristic is gone; the list object is
    /// the authoritative count the scenarios reconcile against).
    static func routeCount(link: EchoLink, central: EchoCentral) async throws -> Int {
        try await downloadRouteList(link: link, central: central).count
    }

    /// Assert the catalog still holds `expected` routes (nothing committed / nothing lost).
    static func assertRouteCount(link: EchoLink, central: EchoCentral, expected: Int, _ why: String) async throws {
        let now = try await routeCount(link: link, central: central)
        try expect(now == expected, "\(why) (routeCount now \(now), expected \(expected))")
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
