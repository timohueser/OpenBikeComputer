import Foundation

/// The device's writable configuration — the semantic view of the OBC Control
/// `Config` characteristic / `config_blob`. Read with `readConfig()`, written
/// with `writeConfig(_:)`.
///
/// **Delta 1 — device name lives in `Config`.** Renaming the device (H3) is a
/// `writeConfig` with a changed `name`; there is no separate rename command. This
/// is a hard requirement on the wire contract — see `OBCProtocol.md` → *Delta 1*
/// and flag it to the firmware track.
///
/// **B-S0 skeleton** — only the contract-mandated `name` field is pinned here.
/// `B1` finalizes the rest of the config blob (units, display prefs, …).
public struct DeviceConfig: Equatable, Sendable {
    /// User-facing device name. Writing this renames the device (H3).
    public var name: String

    public init(name: String) {
        self.name = name
    }
}
