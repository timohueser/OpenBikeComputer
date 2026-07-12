#if canImport(CoreBluetooth)
import CoreBluetooth
import Foundation
import Testing
@testable import OBCTransport

/// #753: the delegate-level mapping `BLETransport.isRetryableGatedFailure` —
/// shared by the gated CCCD-write (`didUpdateNotificationStateFor`) and PSM-read
/// (`didUpdateValueFor`) failure branches so they can't drift. Exercised with
/// real CoreBluetooth error values, no radio. (Importing CoreBluetooth here is
/// fine: the seam guard confines it to `Sources/OBCTransport/BLE` — tests may
/// exercise that seam.)
struct GatedFailureMappingTests {
    /// The reported symptom (#753): an auth-class ATT error on a gated op while
    /// the peripheral is still connected and a fresh-pair authenticate is
    /// parked — the "pairing visibly completed" proxy → retryable.
    @Test(arguments: [
        CBATTError.Code.insufficientAuthentication,
        .insufficientEncryption,
        .insufficientAuthorization,
    ])
    func authClassWhileConnectedWithAuthenticatePendingIsRetryable(code: CBATTError.Code) {
        #expect(BLETransport.isRetryableGatedFailure(
            CBATTError(code), peripheralConnected: true, authenticatePending: true
        ))
    }

    /// A pure disconnect carries no completed-pairing evidence — the proxy
    /// deliberately excludes it (a decline commonly tears the link down), so a
    /// gated failure on a no-longer-connected peripheral fails to D5 as today.
    @Test func disconnectedPeripheralIsNeverRetryable() {
        #expect(!BLETransport.isRetryableGatedFailure(
            CBATTError(.insufficientAuthentication), peripheralConnected: false, authenticatePending: true
        ))
    }

    /// A background re-arm (bonded reconnect, no `authenticate()` parked) keeps
    /// the pre-existing behavior — never the fresh-pair retry.
    @Test func backgroundReArmWithoutAuthenticatePendingIsNeverRetryable() {
        #expect(!BLETransport.isRetryableGatedFailure(
            CBATTError(.insufficientAuthentication), peripheralConnected: true, authenticatePending: false
        ))
    }

    /// Non-auth failures (and a `nil` error — the op succeeded) are terminal:
    /// they say nothing about a completed pairing.
    @Test func nonAuthErrorsAndSuccessesAreNeverRetryable() {
        #expect(!BLETransport.isRetryableGatedFailure(
            CBATTError(.readNotPermitted), peripheralConnected: true, authenticatePending: true
        ))
        #expect(!BLETransport.isRetryableGatedFailure(
            CBError(.connectionTimeout), peripheralConnected: true, authenticatePending: true
        ))
        #expect(!BLETransport.isRetryableGatedFailure(
            nil, peripheralConnected: true, authenticatePending: true
        ))
    }
}
#endif
