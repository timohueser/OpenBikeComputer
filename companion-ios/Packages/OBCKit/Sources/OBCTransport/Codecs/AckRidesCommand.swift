import Foundation
import OBCDomain

/// The `ackRides` command encoder (spec §4.4, cmd `2` — mirrored in `OBCProtocol.md`):
/// `cmd u8 · count u8 · count × object_id u16 LE`, the phone's **ride-possession ack**.
///
/// The device's per-ride "synced" flag (its Rides screen's delete-guard cue) is otherwise set only
/// when a ride download completes — and a ride the library already holds is never downloaded again,
/// so any divergence (rides synced before the device tracked the flag, a sidecar lost with a
/// reflashed card, an app reinstall) would be permanent. This command makes the library the ground
/// truth: send every synced ride id on connect and the device's record heals.
///
/// The command is **idempotent and order-free** (the device only ever *sets* flags), which is what
/// makes chunking trivially safe: a list longer than one GATT write is split into independent
/// writes, each answered by its own `commandResult`.
public enum AckRidesCommand {
    /// The `command` byte (spec §4.4).
    public static let commandByte: UInt8 = 2

    /// Ids per write — sized to the device's 64-byte `command` characteristic
    /// (`2 + 31 × 2`), comfortably under any negotiated ATT MTU.
    public static let maxIDsPerWrite = 31

    /// Encode `ids` as one command write per `maxIDsPerWrite` ids. An empty
    /// list encodes to no writes (nothing to ack — never a zero-count write).
    public static func chunks(_ ids: [DeviceObjectID]) -> [Data] {
        stride(from: 0, to: ids.count, by: maxIDsPerWrite).map { start in
            let slice = ids[start..<min(start + maxIDsPerWrite, ids.count)]
            var data = Data([commandByte, UInt8(slice.count)])
            for id in slice {
                data.append(UInt8(id.raw & 0xFF))
                data.append(UInt8(id.raw >> 8))
            }
            return data
        }
    }
}
