import Foundation

struct WaitTimedOut: Error, CustomStringConvertible {
    let what: String
    let timeout: Duration
    let file: String
    let line: Int

    var description: String {
        "\(file):\(line): timed out after \(timeout) waiting for \(what)"
    }
}

/// Throws on timeout so the test stops before it asserts against an unsettled model.
@MainActor
func waitFor(
    _ what: String = "condition",
    timeout: Duration = .seconds(30),
    interval: Duration = .milliseconds(10),
    file: String = #fileID,
    line: Int = #line,
    _ condition: () -> Bool
) async throws {
    let deadline = ContinuousClock.now + timeout
    while !condition() {
        if ContinuousClock.now >= deadline {
            throw WaitTimedOut(what: what, timeout: timeout, file: file, line: line)
        }
        try await Task.sleep(for: interval)
    }
}

/// Observes absence for the entire window, including its final state.
@MainActor
func neverHolds(_ condition: () -> Bool, for duration: Duration) async -> Bool {
    let deadline = ContinuousClock.now + duration
    while ContinuousClock.now < deadline {
        if condition() { return false }
        try? await Task.sleep(for: .milliseconds(5))
    }
    return !condition()
}
