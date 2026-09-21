import Foundation

/// The outcome of an `installFw` request. The device only accepts the request, then runs its
/// own on-glass check and confirm flow: the rider confirms and the device reboots to install.
/// The command never waits for the human and never installs on its own.
public enum FirmwareInstallResult: Equatable, Sendable {
    /// `ok` (0): the request is accepted and the device opens its confirm flow.
    case accepted
    /// `notFound` (2): no `UPDATE.BIN` on the card to install.
    case noStaged
    /// `busy` (3): a ride is recording, or an install request is already pending.
    case busy
    /// `error` (4): the staged image is already known-unusable.
    case rejected
    /// `unknownCommand` (1): the device can't be updated over Bluetooth.
    case unsupported
}
