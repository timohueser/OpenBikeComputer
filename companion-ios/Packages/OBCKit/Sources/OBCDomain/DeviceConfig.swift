import Foundation

/// The device's writable configuration: the semantic view of the OBC Control `Config`
/// characteristic. Read with `readConfig()`, written with `writeConfig(_:)`.
///
/// The device name lives in `Config`: a rename is a `writeConfig` with a changed `name`, and
/// there is no separate rename command (`OBCProtocol.md`).
public struct DeviceConfig: Equatable, Sendable {
    public enum Units: UInt8, Equatable, Sendable, CaseIterable {
        case metric = 0
        case imperial = 1
    }

    public var name: String
    public var units: Units

    public init(name: String, units: Units = .metric) {
        self.name = name
        self.units = units
    }

    /// The firmware caps the device name at 48 UTF-8 bytes. The `Config` codec truncates to
    /// this at encode and the rename UI limits to it, so an over-long name can never overflow
    /// the `u16` length field into a corrupt blob.
    public static let maxNameUTF8Bytes = 48
}

extension String {
    /// This string truncated to at most `maxUTF8Bytes` UTF-8 bytes on a Character boundary. It
    /// never splits a grapheme cluster, so the result stays valid UTF-8.
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
