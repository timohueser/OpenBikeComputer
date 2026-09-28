import Foundation
import OBCDomain

public enum DeviceRenaming {
    public static func normalized(_ name: String) -> String {
        name.trimmingCharacters(in: .whitespacesAndNewlines)
            .truncatedToUTF8Bytes(DeviceConfig.maxNameUTF8Bytes)
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Rename through the existing config command, preserving every other setting.
    public static func save(_ name: String, to transport: any DeviceConfiguration) async throws {
        let name = normalized(name)
        guard !name.isEmpty else { throw DeviceError.writeFailed }
        var config = try await transport.readConfig()
        config.name = name
        try await transport.writeConfig(config)
    }
}
