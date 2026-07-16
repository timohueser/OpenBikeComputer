import Foundation

/// The outcome of an `installFw` request (S7 — `obc-ble-interface-spec.md` §4.4
/// cmd 3): the device only ever *accepts* the request, then runs its on-glass
/// check → confirm flow; the rider confirms and the device reboots to install.
/// The command never waits for the human, never installs on its own.
///
/// These map one-to-one onto the `commandResult` status vocabulary (§4.3) — no
/// new status byte — with `unsupported` covering a device that predates BLE DFU
/// (it answers `unknownCommand`). The transport does the mapping; the UI turns
/// each case into one plain sentence (see the firmware-update view model).
public enum FirmwareInstallResult: Equatable, Sendable {
    /// `ok` (0): request accepted — the device opens its confirm flow. The rider
    /// confirms on the device, which then restarts to install.
    case accepted
    /// `notFound` (2): no `UPDATE.BIN` on the card to install (send it again).
    case noStaged
    /// `busy` (3): a ride is recording, or an install request is already pending.
    case busy
    /// `error` (4): the staged image is already known-unusable.
    case rejected
    /// `unknownCommand` (1): the device can't be updated over Bluetooth.
    case unsupported
}
