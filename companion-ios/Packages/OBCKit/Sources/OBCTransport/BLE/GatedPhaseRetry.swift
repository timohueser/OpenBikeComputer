import Foundation

/// The fresh-pair gated-phase failure that `BLETransport.authenticate()`'s
/// retry-once policy (#753) treats as *retryable*: the gated PSM read failed
/// **auth-class** (ATT insufficient-authentication / -encryption) while the
/// peripheral was **still connected**.
///
/// That is the conservative "pairing visibly completed but the firmware
/// momentarily refused" proxy: on a fresh pair iOS raises the passkey sheet on
/// the first gated op, and once the rider enters the code SMP pairing completes
/// and both sides bond — but iOS's *retry* of the gated op can land in the
/// window right after the firmware's `PairingComplete` where it still refuses
/// auth-gated ATT ops (the bond save runs under the GATT-serve lock, the same
/// unanswered-ATT class as the #744/#750 sensor-link drops). The link is up and
/// bonded, so an immediate retry succeeds with **no** sheet.
///
/// Every *terminal* gated failure — a real decline / reject-when-bonded (which
/// tears the link down), a CoC open failure, or a stall — throws a plain
/// `DeviceError` instead, which the policy never retries (it fails to D5 exactly
/// as today). We can't see SMP from CoreBluetooth, so this stays deliberately
/// narrow: auth-class **and** still-connected, or fail.
struct GatedPairingWindowError: Error {}

/// The retry-once state machine behind `BLETransport.authenticate()` (#753),
/// factored out of the CoreBluetooth flow so it is unit-testable with a scripted
/// `attempt` (no radio). Semantics: run the gated phase once; if it throws and
/// `isRetryable` says the pairing visibly completed but the firmware momentarily
/// refused, wait one short `beat` and run the gated phase **exactly once** more —
/// that second outcome is final (success resolves, any failure propagates). A
/// non-retryable failure throws immediately, with no beat and no retry.
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
            // A terminal failure (decline / link drop / CoC failure) fails now,
            // as before — no beat, no second attempt.
            guard isRetryable(error) else { throw error }
            // Pairing visibly completed; give the firmware's post-PairingComplete
            // window a beat to drain, then retry the gated phase once on the now
            // bonded link. Whatever the second attempt does is final: a second
            // failure lands on D5 exactly as today (one retry only).
            await sleep(beat)
            try await attempt()
        }
    }
}
