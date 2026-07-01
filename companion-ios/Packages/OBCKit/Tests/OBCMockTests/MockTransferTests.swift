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
    }

    func testDownloadRidesSizesFromFixtures() async throws {
        let control = fastControl()
        let transport = MockTransport(control: control)
        let rides = try await transport.listRides()
        let id = try XCTUnwrap(rides.first).id
        let handle = transport.downloadRides([id])
        let progress = try await drain(handle)
        XCTAssertGreaterThan(progress.last?.total ?? 0, 0)
        XCTAssertEqual(progress.last?.bytesDone, progress.last?.total)
    }

    func testEmptyDownloadFinishesImmediately() async throws {
        let transport = MockTransport(control: fastControl())
        let handle = transport.downloadRides([])   // H9 up to date → nothing to pull
        let progress = try await drain(handle)
        XCTAssertTrue(progress.isEmpty)
    }

    func testUploadWhileDisconnectedFinishesImmediately() async throws {
        let control = fastControl()
        control.connection = .disconnected         // H4 on import: no device
        let transport = MockTransport(control: control)
        let progress = try await drain(transport.uploadRoute(makeRouteBlob(bytes: 100_000)))
        XCTAssertTrue(progress.isEmpty)
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
    }

    func testDropAtFractionStallsThenResumesToCompletion() async throws {
        let control = fastControl()
        control.dropTransfer(atFraction: 0.5)       // H10 / F interrupted
        let transport = MockTransport(control: control)
        let handle = transport.uploadRoute(makeRouteBlob(bytes: 400_000))

        // The drop pushes the link out of range (the observable H10 signal).
        let sawDrop = await awaitState(transport.state, equals: .outOfRange)
        XCTAssertTrue(sawDrop)

        // Resume: link restored, transfer runs to completion, byte-exact.
        handle.resume()
        let progress = try await drain(handle)
        XCTAssertEqual(progress.last?.bytesDone, 400_000)
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
