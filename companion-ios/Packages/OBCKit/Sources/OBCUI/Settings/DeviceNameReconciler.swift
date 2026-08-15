import Foundation
import OBCTransport

/// Self-heal for the H3 device rename (#361). The rename is optimistic — the
/// phone shows the new name at once and fires the config write after — so a
/// failed or interrupted write leaves the phone and device disagreeing. The
/// bond record's `deviceName` already *is* the desired name (`SettingsModel.
/// rename` saves it before the write), so this pass pushes it back into the
/// device config until the two converge.
///
/// The policy device-bound writes follow (see companion CLAUDE.md →
/// Conventions):
/// - **Once per established connection, never a hot retry loop.** If the
///   device rejects the write, the next connect tries again.
/// - Reconciles against the **config blob's** name (authoritative), never
///   `deviceInfo().name` — that's the advertised peripheral name, which can
///   lag the config.
/// - A failed `readConfig` or `writeConfig` is a **silent skip** — the
///   following connect retries; the one-time rename toast already told the
///   user.
/// - No bond record (post-`forget()`) → no-op.
/// - **Last-writer-wins**: a device renamed from *another* phone between our
///   rename and this reconnect gets our bond name pushed back over it.
///   Acceptable for now — revisit if multi-phone setups become real.
public struct DeviceNameReconciler: Sendable {
    private let transport: any DeviceConfiguration
    private let bondStore: any BondStore

    public init(transport: any DeviceConfiguration, bondStore: any BondStore) {
        self.transport = transport
        self.bondStore = bondStore
    }

    /// One reconcile pass — call once per established connection (the main
    /// screen's post-connect path does). Read-modify-write so the other
    /// config fields (units, …) survive, and a no-op when the names already
    /// match (the common case: the rename's own write landed).
    public func reconcile() async {
        guard let bond = bondStore.load() else { return }
        guard var config = try? await transport.readConfig() else { return }
        guard config.name != bond.deviceName else { return }
        config.name = bond.deviceName
        try? await transport.writeConfig(config)
    }
}
