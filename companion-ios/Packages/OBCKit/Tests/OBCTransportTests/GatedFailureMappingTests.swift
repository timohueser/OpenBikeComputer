#if canImport(CoreBluetooth)
import CoreBluetooth
import Foundation
import Testing
@testable import OBCTransport

/// The delegate-level mapping `BLETransport.isRetryableGatedFailure`, shared by the gated CCCD
/// write and PSM read failure branches so they cannot drift. It uses real CoreBluetooth error
/// values and no radio; the seam guard confines CoreBluetooth to the BLE sources, and a test may
/// exercise that seam.
struct GatedFailureMappingTests {
    /// An auth-class ATT error on a gated op, with the peripheral still connected and a
    /// fresh-pair authenticate parked, is the proxy for "pairing visibly completed".
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

    /// A disconnect carries no completed-pairing evidence, and a decline commonly tears the link
    /// down, so a gated failure on a disconnected peripheral is never retried.
    @Test func disconnectedPeripheralIsNeverRetryable() {
        #expect(!BLETransport.isRetryableGatedFailure(
            CBATTError(.insufficientAuthentication), peripheralConnected: false, authenticatePending: true
        ))
    }

    /// A background re-arm has no `authenticate()` parked, so it never takes the fresh-pair retry.
    @Test func backgroundReArmWithoutAuthenticatePendingIsNeverRetryable() {
        #expect(!BLETransport.isRetryableGatedFailure(
            CBATTError(.insufficientAuthentication), peripheralConnected: true, authenticatePending: false
        ))
    }

    /// Non-auth failures, and a `nil` error (the op succeeded), say nothing about a completed pairing.
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
