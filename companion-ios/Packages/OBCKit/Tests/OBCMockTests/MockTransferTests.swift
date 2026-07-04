import XCTest
import OBCDomain
import OBCTransport
@testable import OBCMock

/// The simulated bulk-transfer path: throughput-paced progress, cancel teardown, and
/// drop-at-fraction → resume — the B1M acceptance for uploads (F) and ride sync (H10),
/// with no wire bytes. A high throughput keeps the paced sleeps sub-millisecond.
final class MockTransferTests: XCTestCase {
    private func fastControl() -> MockControl {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        control.throughputBytesPerSec = 50_000_000   // fast → tests don't wait on the pacing
        return control
    }

    func testUploadEmitsProgressToCompletion() async throws {
        let transport = MockTransport(control: fastControl())
        let handle = transport.uploadRoute(makeRouteBlob(bytes: 200_000))
        let progress = try await drain(handle)
        XCTAssertFalse(progress.isEmpty)
        XCTAssertEqual(progress.last?.bytesDone, 200_000)
        XCTAssertEqual(progress.last?.total, 200_000)
        // Monotonic non-decreasing.
        XCTAssertEqual(progress.map(\.bytesDone), progress.map(\.bytesDone).sorted())
        let outcome = await handle.outcome
        XCTAssertEqual(outcome, .completed)
    }

    func testUploadReportsADeviceAssignedObjectID() async throws {
        let transport = MockTransport(control: fastControl())
        let handle = transport.uploadRoute(makeRouteBlob(bytes: 50_000))
        _ = try await drain(handle)
        let outcome = await handle.outcome
        let assigned = await handle.assignedObjectID
        XCTAssertEqual(outcome, .completed)
        // A fresh upload (no target) gets a new device-assigned id.
        XCTAssertNotNil(assigned, "the device assigns a new object id on a fresh upload")
    }

    func testReuploadKeepsTheTargetObjectID() async throws {
        let transport = MockTransport(control: fastControl())
        let blob = RouteBlob(
            summary: RouteSummary(id: RouteID("r"), name: "Edited", distanceMeters: 1_000, elevationGainMeters: 10),
            payload: MockPayload.make(count: 40_000),
            targetObjectID: DeviceObjectID(7)   // replacing device object 7
        )
        let handle = transport.uploadRoute(blob)
        _ = try await drain(handle)
        let outcome = await handle.outcome
        let assigned = await handle.assignedObjectID
        XCTAssertEqual(outcome, .completed)
        XCTAssertEqual(assigned, DeviceObjectID(7), "a replace commits under the same id, not a new one")
    }

    func testDownloadRidesSizesFromFixtures() async throws {
        let control = fastControl()
        let transport = MockTransport(control: control)
        let rides = try await transport.listRides()
        let id = try XCTUnwrap(rides.first).id
        let download = transport.downloadRides([id])
        let progress = try await drain(download.handle)
        XCTAssertGreaterThan(progress.last?.total ?? 0, 0)
        XCTAssertEqual(progress.last?.bytesDone, progress.last?.total)
    }

    func testDownloadDeliversEachRidePayload() async throws {
        let control = fastControl()
        let transport = MockTransport(control: control)
        let ids = try await transport.listRides().map(\.id)
        XCTAssertGreaterThan(ids.count, 1)   // needs ≥2 fixture rides to prove ordering

        let download = transport.downloadRides(ids)
        let landed = try await withTimeout(5) { () -> [DownloadedRide] in
            var out: [DownloadedRide] = []
            for try await ride in download.rides { out.append(ride) }
            return out
        }
        // Every requested ride lands, in transfer order, carrying the real
        // codec-encoded object (`downloadByteCount` only paces the progress —
        // it is NOT the payload size).
        XCTAssertEqual(landed.map(\.id), ids)
        let entries = control.fixtures.rides
        for ride in landed {
            let entry = try XCTUnwrap(entries.first { $0.summary.id == ride.id })
            let decoded = try RideObjectCodec.decode(ride.payload, id: ride.id)
            XCTAssertEqual(decoded.summary.name, entry.summary.name)
            XCTAssertEqual(decoded.points.count, entry.points.count)
        }
    }

    func testEmptyDownloadFinishesImmediately() async throws {
        let transport = MockTransport(control: fastControl())
        let download = transport.downloadRides([])   // H9 up to date → nothing to pull
        let progress = try await drain(download.handle)
        XCTAssertTrue(progress.isEmpty)
        let landed = try await withTimeout(2) { () -> [DownloadedRide] in
            var out: [DownloadedRide] = []
            for try await ride in download.rides { out.append(ride) }
            return out
        }
        XCTAssertTrue(landed.isEmpty)
    }

    func testDownloadDropKeepsPartialRidesThenResumes() async throws {
        let control = fastControl()
        control.dropTransfer(atFraction: 0.5)        // H10 sync interrupted
        let transport = MockTransport(control: control)
        let ids = try await transport.listRides().map(\.id)
        let download = transport.downloadRides(ids)

        let sawDrop = await awaitState(transport.state, equals: .outOfRange)
        XCTAssertTrue(sawDrop)

        // Resume: the batch completes and *every* ride still lands exactly once.
        download.handle.resume()
        let landed = try await withTimeout(5) { () -> [DownloadedRide] in
            var out: [DownloadedRide] = []
            for try await ride in download.rides { out.append(ride) }
            return out
        }
        XCTAssertEqual(landed.map(\.id), ids)
    }

    func testUploadWhileDisconnectedFinishesImmediately() async throws {
        let control = fastControl()
        control.connection = .disconnected         // H4 on import: no device
        let transport = MockTransport(control: control)
        let handle = transport.uploadRoute(makeRouteBlob(bytes: 100_000))
        let progress = try await drain(handle)
        XCTAssertTrue(progress.isEmpty)
        let outcome = await handle.outcome
        XCTAssertEqual(outcome, .failed(.notConnected))   // explicit, not inferred
    }

    func testCancelStopsShortAndFinishes() async throws {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        control.throughputBytesPerSec = 300_000     // slow enough to cancel mid-flight
        let transport = MockTransport(control: control)
        let handle = transport.uploadRoute(makeRouteBlob(bytes: 400_000))

        // Cancel after the first tick; the stream must still finish (no hang).
        let progress = try await withTimeout(5) { () -> [TransferProgress] in
            var out: [TransferProgress] = []
            for await tick in handle.progress {
                out.append(tick)
                if out.count == 1 { handle.cancel() }
            }
            return out
        }
        XCTAssertLessThan(progress.last?.bytesDone ?? .max, 400_000)   // stopped short
        let outcome = await handle.outcome
        XCTAssertEqual(outcome, .canceled)
    }

    func testDropAtFractionStallsThenResumesToCompletion() async throws {
        let control = fastControl()
        control.dropTransfer(atFraction: 0.5)       // H10 / F interrupted
        let transport = MockTransport(control: control)
        let handle = transport.uploadRoute(makeRouteBlob(bytes: 400_000))

        // The drop pushes the link out of range (the observable H10 signal).
        let sawDrop = await awaitState(transport.state, equals: .outOfRange)
        XCTAssertTrue(sawDrop)

        // While dropped the outcome is still open — the transfer is resumable.
        XCTAssertNil(handle.currentOutcome)

        // Resume: link restored, transfer runs to completion, byte-exact.
        handle.resume()
        let progress = try await drain(handle)
        XCTAssertEqual(progress.last?.bytesDone, 400_000)
        let outcome = await handle.outcome
        XCTAssertEqual(outcome, .completed)
        let restored = await awaitState(transport.state, equals: .connected)
        XCTAssertTrue(restored)
    }

    func testArmedFailureDropsTransferImmediately() async throws {
        let control = fastControl()
        control.failNextOp(.writeFailed)            // "upload fail"
        let transport = MockTransport(control: control)
        let handle = transport.uploadRoute(makeRouteBlob(bytes: 200_000))
        let droppedImmediately = await awaitState(transport.state, equals: .outOfRange)
        XCTAssertTrue(droppedImmediately)
        // Nothing committed before the immediate drop.
        let progress = try await withTimeout(2) { () -> [TransferProgress] in
            var out: [TransferProgress] = []
            let watchdog = Task { try? await Task.sleep(for: .milliseconds(200)); handle.cancel() }
            for await tick in handle.progress { out.append(tick) }
            watchdog.cancel()
            return out
        }
        XCTAssertTrue(progress.isEmpty)
    }
}
