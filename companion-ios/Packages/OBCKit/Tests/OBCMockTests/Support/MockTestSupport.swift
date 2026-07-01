import XCTest
import OBCDomain
import OBCTransport
@testable import OBCMock

// Shared async helpers for the mock tests — a timeout wrapper (streams never hang a
// test), stream probes, and a payload-sized `RouteBlob` builder. Free functions (not
// XCTestCase methods) so the `Task {}` probes don't capture a non-Sendable `self`.

private struct TimeoutError: Error {}

func withTimeout<T: Sendable>(_ seconds: Double = 3, _ operation: @Sendable @escaping () async throws -> T) async throws -> T {
    try await withThrowingTaskGroup(of: T.self) { group in
        group.addTask { try await operation() }
        group.addTask {
            try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
            throw TimeoutError()
        }
        let result = try await group.next()!
        group.cancelAll()
        return result
    }
}

/// Collect every progress tick until the transfer's stream finishes.
func drain(_ handle: TransferHandle, timeout: Double = 5) async throws -> [TransferProgress] {
    try await withTimeout(timeout) {
        var out: [TransferProgress] = []
        for await progress in handle.progress { out.append(progress) }
        return out
    }
}

/// True iff the connection stream yields `target` within `timeout`
/// (AsyncMulticast replays the latest value, so this catches an already-set state).
func awaitState(_ stream: AsyncStream<ConnectionState>, equals target: ConnectionState, timeout: Double = 2) async -> Bool {
    ((try? await withTimeout(timeout) {
        for await state in stream where state == target { return true }
        return false
    }) ?? false)
}

/// True iff the battery stream yields `target` within `timeout`.
func awaitBattery(_ stream: AsyncStream<Int>, equals target: Int, timeout: Double = 2) async -> Bool {
    ((try? await withTimeout(timeout) {
        for await value in stream where value == target { return true }
        return false
    }) ?? false)
}

/// A `RouteBlob` whose opaque payload is exactly `bytes` long — sizes an upload.
func makeRouteBlob(bytes: Int, id: String = "test-route") -> RouteBlob {
    RouteBlob(
        summary: RouteSummary(id: RouteID(id), name: "Test", distanceMeters: 1_000, elevationGainMeters: 10),
        payload: MockPayload.make(count: bytes)
    )
}
