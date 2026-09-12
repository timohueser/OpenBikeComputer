import Foundation
import Testing

@MainActor @Suite struct WaitForTests {
    @Test func returnsWhenTheModelSettles() async throws {
        var ready = false
        let update = Task { @MainActor in ready = true }
        defer { update.cancel() }
        try await waitFor("ready", interval: .milliseconds(1)) { ready }
        #expect(ready)
    }

    @Test func timeoutStopsTheCallerWithItsContext() async throws {
        var continued = false
        do {
            try await waitFor("connected", timeout: .zero, file: "Caller.swift", line: 42) { false }
            continued = true
        } catch let error as WaitTimedOut {
            #expect(error.what == "connected")
            #expect(error.timeout == .zero)
            #expect(error.description.contains("Caller.swift:42"))
        }
        #expect(!continued)
    }

    @Test func negativeObservationChecksTheWholeWindow() async {
        let start = ContinuousClock.now
        #expect(await neverHolds({ false }, for: .milliseconds(5)))
        #expect(ContinuousClock.now - start >= .milliseconds(5))
        #expect(await neverHolds({ true }, for: .zero) == false)
        #expect(await neverHolds({ true }, for: .milliseconds(5)) == false)
    }
}
