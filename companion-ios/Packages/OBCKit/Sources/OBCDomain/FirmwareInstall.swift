import Foundation

/// The outcome of an install request. The device only accepts the request, then runs its own
/// on-glass check and confirm flow: the rider confirms and the device reboots to install. The
/// request never waits for the human and never installs on its own.
public enum FirmwareInstallResult: Equatable, Sendable {
    /// The request is accepted and the device opens its confirm flow.
    case accepted
    /// The device holds no staged update package, so there is nothing to install.
    case noStaged
    /// A ride is recording, or an install request is already pending.
    case busy
    /// The device refused this package.
    case rejected
    /// The device cannot be updated over this link.
    case unsupported
}
