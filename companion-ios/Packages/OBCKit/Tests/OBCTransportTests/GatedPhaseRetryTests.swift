import Foundation
import Testing
@testable import OBCTransport

/// #753: the fresh-pair gated-phase retry-once state machine behind
/// `BLETransport.authenticate()`, exercised with a scripted `attempt` (no radio)
/// so the three failure/retry paths are pinned without CoreBluetooth. Mirrors the
/// wiring in `authenticate()`: a `GatedPairingWindowError` is the only retryable
/// class ("pairing visibly completed, firmware momentarily refused"); every other
/// error is terminal and fails immediately, as before.
struct GatedPhaseRetryTests {
    /// A stand-in terminal failure (a real decline / link drop / CoC failure —
    /// what `failAuthenticate` throws as a plain `DeviceError` on the real path).
    private struct TerminalFailure: Error {}

    /// Records each attempt's scripted outcome, and whether the beat was slept.
    private final class ScriptedGatedPhase {
        private let outcomes: [Result<Void, Error>]
        private(set) var attempts = 0
        private(set) var slept = false

        init(_ outcomes: [Result<Void, Error>]) { self.outcomes = outcomes }

        func attempt() throws {
            defer { attempts += 1 }
            // Beyond the script is a test-authoring bug (more attempts than the
            // policy should ever make) — surface it loudly rather than pass.
            try outcomes[attempts].get()
        }

        func sleep(_: Duration) { slept = true }
    }

    // (a) First gated phase fails auth-class, the retry succeeds → resolves.
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

    // (b) Both attempts fail auth-class → throws once, no second retry.
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

    // (c) Plain decline (link drop, no completed-pairing evidence) → immediate
    //     fail, no beat, no retry.
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

    // The happy path: the first attempt succeeds outright → no beat, no retry.
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
