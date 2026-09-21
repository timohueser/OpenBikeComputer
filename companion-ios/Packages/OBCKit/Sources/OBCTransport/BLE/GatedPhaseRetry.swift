import Foundation

/// The gated-phase failure `BLETransport.authenticate()` retries once: a gated op (the
/// indication setup or the PSM read) failed auth-class (ATT insufficient authentication or
/// encryption) while the peripheral was still connected.
///
/// On a fresh pair iOS replays the gated ops after the passkey, and the replay can land in the
/// window right after the firmware's `PairingComplete` where it still refuses auth-gated ATT
/// ops. The link is up and bonded, so an immediate retry succeeds with no passkey sheet. This
/// stays narrow because CoreBluetooth cannot show SMP: auth-class and still connected, or fail.
struct GatedPairingWindowError: Error {}

/// The retry-once policy behind `BLETransport.authenticate()`, factored out of the
/// CoreBluetooth flow so a scripted `attempt` can test it with no radio. It runs the gated
/// phase once. If that throws and `isRetryable` says the pairing visibly completed, it waits one
/// `beat` and runs the phase exactly once more. That second outcome is final; a non-retryable
/// failure throws at once, with no beat.
enum GatedPhaseRetry {
    static func runOnce(
        beat: Duration,
        sleep: (Duration) async -> Void = { try? await Task.sleep(for: $0) },
        isRetryable: (any Error) -> Bool,
        attempt: () async throws -> Void
    ) async throws {
        do {
            try await attempt()
        } catch {
            guard isRetryable(error) else { throw error }
            // Give the firmware's post-PairingComplete window a beat to drain, then retry the
            // gated phase once on the now bonded link. The second attempt is final.
            await sleep(beat)
            try await attempt()
        }
    }
}
