import Foundation
import Testing
@testable import OBCTransport

/// The fresh-pair gated-phase retry-once state machine behind `BLETransport.authenticate()`,
/// exercised with a scripted `attempt` so the failure and retry paths are pinned without
/// CoreBluetooth. A `GatedPairingWindowError` is the only retryable class; anything else is terminal.
struct GatedPhaseRetryTests {
    /// A stand-in terminal failure: a decline, a link drop or a CoC failure on the real path.
    private struct TerminalFailure: Error {}

    /// Records each attempt's scripted outcome, and whether the beat was slept.
    private final class ScriptedGatedPhase {
        private let outcomes: [Result<Void, Error>]
        private(set) var attempts = 0
        private(set) var slept = false

        init(_ outcomes: [Result<Void, Error>]) { self.outcomes = outcomes }

        func attempt() throws {
            defer { attempts += 1 }
            // Running past the script means more attempts than the policy allows: trap loudly.
            try outcomes[attempts].get()
        }

        func sleep(_: Duration) { slept = true }
    }

    @Test func retryableFailureThenSuccessResolves() async throws {
        let script = ScriptedGatedPhase([.failure(GatedPairingWindowError()), .success(())])
        try await GatedPhaseRetry.runOnce(
            beat: .milliseconds(500),
            sleep: script.sleep,
            isRetryable: { $0 is GatedPairingWindowError },
            attempt: script.attempt
        )
        #expect(script.attempts == 2)  // one initial + exactly one retry
        #expect(script.slept)          // the beat ran before the retry
    }

    @Test func retryableFailureTwiceThrowsOnceNoDoubleRetry() async {
        let script = ScriptedGatedPhase([
            .failure(GatedPairingWindowError()),
            .failure(GatedPairingWindowError()),
        ])
        await #expect(throws: GatedPairingWindowError.self) {
            try await GatedPhaseRetry.runOnce(
                beat: .milliseconds(500),
                sleep: script.sleep,
                isRetryable: { $0 is GatedPairingWindowError },
                attempt: script.attempt
            )
        }
        #expect(script.attempts == 2)  // initial + one retry only, never a third
        #expect(script.slept)
    }

    @Test func terminalFailureFailsImmediately() async {
        let script = ScriptedGatedPhase([.failure(TerminalFailure())])
        await #expect(throws: TerminalFailure.self) {
            try await GatedPhaseRetry.runOnce(
                beat: .milliseconds(500),
                sleep: script.sleep,
                isRetryable: { $0 is GatedPairingWindowError },
                attempt: script.attempt
            )
        }
        #expect(script.attempts == 1)  // no retry
        #expect(!script.slept)         // no beat before failing
    }

    @Test func firstAttemptSuccessNeverRetries() async throws {
        let script = ScriptedGatedPhase([.success(())])
        try await GatedPhaseRetry.runOnce(
            beat: .milliseconds(500),
            sleep: script.sleep,
            isRetryable: { $0 is GatedPairingWindowError },
            attempt: script.attempt
        )
        #expect(script.attempts == 1)
        #expect(!script.slept)
    }
}
