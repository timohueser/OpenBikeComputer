import Foundation

/// The device's writable configuration — the semantic view of the OBC Control
/// `Config` characteristic / `config_blob`. Read with `readConfig()`, written
/// with `writeConfig(_:)`.
///
/// **Delta 1 — device name lives in `Config`.** Renaming the device (H3) is a
/// `writeConfig` with a changed `name`; there is no separate rename command. This
/// is a hard requirement on the wire contract — see `OBCProtocol.md` → *Delta 1*.
///
/// **B1 finalization:** carries the contract-mandated `name` plus the display
/// `units` the Settings screen (B8/G) edits. The rest of the config blob (display
/// prefs, sensor pairing, …) is B8's / firmware `S0`'s to grow — kept minimal here
/// so the config codec has a real, round-trippable shape without inventing fields.
public struct DeviceConfig: Equatable, Sendable {
    /// Unit system the device displays. Editable from Settings (G).
    public enum Units: UInt8, Equatable, Sendable, CaseIterable {
        case metric = 0
        case imperial = 1
    }

    /// User-facing device name. Writing this renames the device (H3).
    public var name: String
    /// Display unit system.
    public var units: Units

    public init(name: String, units: Units = .metric) {
        self.name = name
        self.units = units
    }

    /// Firmware S0 caps the device name at **48 UTF-8 bytes** (spec §7.3 /
    /// `OBCProtocol.md` → Delta 1). The `Config` codec truncates to this at
    /// encode and the rename UI limits to it, so an over-long name can never
    /// overflow the `u16` length field into a corrupt / undersized blob.
    public static let maxNameUTF8Bytes = 48
}

extension String {
    /// This string truncated to at most `maxUTF8Bytes` UTF-8 bytes on a
    /// **Character boundary** — never splitting a grapheme cluster (and so never
    /// a multi-byte UTF-8 sequence), which keeps the result valid UTF-8.
    public func truncatedToUTF8Bytes(_ maxUTF8Bytes: Int) -> String {
        guard utf8.count > maxUTF8Bytes else { return self }
        var result = ""
        var count = 0
        for character in self {
            let width = String(character).utf8.count
            if count + width > maxUTF8Bytes { break }
            result.append(character)
            count += width
        }
        return result
    }
}
