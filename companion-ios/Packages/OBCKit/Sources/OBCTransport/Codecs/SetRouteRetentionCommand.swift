import Foundation
import OBCDomain

/// The `setRouteRetention` command encoder (spec §4.4, cmd `6` — mirrored in
/// `OBCProtocol.md`): `cmd u8 = 6 · object_id u16 LE · retention u8`, the phone
/// setting a stored route's expiry policy **without re-uploading** it (epic #638).
///
/// Encode-only + `commandResult` correlation, the same shape as
/// [`AckRidesCommand`](AckRidesCommand.swift). The device writes the level into its
/// retention sidecar **without touching `last_used`** (changing retention never
/// resets the usage clock) and bumps the route store revision only on a real
/// change. Replies: `ok` (applied), `notFound` (unknown id), `unknownCommand` (a
/// device predating expiry → `unsupported`).
///
/// Pinned byte-for-byte against `protocol-vectors/command-set-route-retention.bin`
/// (`SetRouteRetentionCommandTests`), so the app and firmware can't drift from
/// spec §4.4.
public enum SetRouteRetentionCommand {
    /// The `command` byte (spec §4.4).
    public static let commandByte: UInt8 = 6

    /// Encode the 4-byte `setRouteRetention` write, all little-endian.
    public static func encode(objectID: DeviceObjectID, retention: Retention) -> Data {
        Data([
            commandByte,
            UInt8(objectID.raw & 0xFF),
            UInt8(objectID.raw >> 8),
            retention.rawValue,
        ])
    }
}
