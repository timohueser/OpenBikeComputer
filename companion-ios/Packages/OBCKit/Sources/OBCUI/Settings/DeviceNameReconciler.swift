import Foundation
import OBCTransport

/// Self-heal for the device rename. The rename is optimistic: the phone shows the new
/// name at once and fires the config write after, so a failed or interrupted write
/// leaves the phone and the device disagreeing. The bond record's `deviceName` is
/// already the desired name, so this pass pushes it into the device config until the
/// two converge.
///
/// It runs once per established connection, never as a hot retry loop, and reconciles
/// against the config blob's name, not `deviceInfo().name`, which is the advertised
/// peripheral name and can lag. A failed read or write is a silent skip; the next
/// connect retries. No bond record means no-op. Last writer wins: a device renamed
/// from another phone gets our bond name pushed back over it.
public struct DeviceNameReconciler: Sendable {
    private let transport: any DeviceConfiguration
    private let bondStore: any BondStore

    public init(transport: any DeviceConfiguration, bondStore: any BondStore) {
        self.transport = transport
        self.bondStore = bondStore
    }

    /// One reconcile pass; call it once per established connection. Read-modify-write
    /// so the other config fields survive, and a no-op when the names already match.
    public func reconcile() async {
        guard let bond = bondStore.load() else { return }
        guard var config = try? await transport.readConfig() else { return }
        guard config.name != bond.deviceName else { return }
        config.name = bond.deviceName
        try? await transport.writeConfig(config)
    }
}
