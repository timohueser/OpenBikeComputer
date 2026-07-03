import Foundation
import Observation
import OBCDomain
import OBCTransport

/// State for the Settings screen (B8, design G) — the device identity cluster,
/// the H3 rename, and the H2 forget. Depends only on `DeviceTransport` +
/// `BondStore` (the golden rule); the coming-soon groups (OTA, services) are
/// static copy in the view.
///
/// **Rename (H3)** is a `writeConfig` with a changed `name` — device name lives
/// in the Config blob (B-S0 Delta 1), there is no separate rename command. The
/// new name shows across the app at once (top bar via the composition root's
/// callback, the bond record for the next launch greeting) and rides to the
/// device on the config write. Link-bound, so the row dims when unreachable
/// (the S4 rule: actions that need the link dim).
///
/// **Forget (H2)** clears the app's bond record and drops the link; the launch
/// flow returns to the D1 pairing prompt. Everything on the phone stays — the
/// library store is untouched (the design's reassurance copy is literal).
@MainActor @Observable
public final class SettingsModel {
    // MARK: Observable state

    public private(set) var deviceName = "Your OBC"
    public private(set) var connection: ConnectionState = .connecting
    /// Battery percent, `nil` until the stream's first value.
    public private(set) var battery: Int?
    /// Raw firmware version ("0.4.2"), `nil` until `deviceInfo` lands.
    public private(set) var firmwareVersion: String?

    // MARK: Derived (design G copy)

    /// The device row's status line: "Connected · 82%" in forest, or the
    /// degraded states in faint ink.
    public var statusLine: String {
        switch connection {
        case .connected:
            battery.map { "Connected · \($0)%" } ?? "Connected"
        case .connecting:
            "Connecting…"
        case .outOfRange:
            "Out of range"
        case .disconnected:
            "Not connected"
        }
    }

    public var isConnected: Bool { connection == .connected }

    /// "v0.4.2" — the device row's trailing value; `nil` renders nothing.
    public var firmwareDisplay: String? {
        firmwareVersion.map { $0.hasPrefix("v") ? $0 : "v\($0)" }
    }

    /// "v0.4.2 · latest" — the firmware group's version row. "Latest" is by
    /// definition until OTA lands: the phone has no update channel to compare
    /// against (the footer points at the desktop tool).
    public var firmwareLine: String {
        (firmwareDisplay ?? "—") + " · latest"
    }

    /// H3 is a config write — link-bound, dimmed when unreachable (S4 rule).
    public var canRename: Bool { isConnected }

    // MARK: Wiring

    private let transport: any DeviceTransport
    private let bondStore: any BondStore
    /// Fires after a rename so the composition root can refresh the main
    /// screen's top bar (Settings never reaches into another feature's model).
    private let onDeviceRenamed: (String) -> Void
    /// Fires after a forget so the composition root can drop the launch flow
    /// back to the D1 pairing prompt.
    private let onForget: () -> Void
    @ObservationIgnored private var started = false
    @ObservationIgnored private var streamTasks: [Task<Void, Never>] = []

    public init(
        transport: any DeviceTransport,
        bondStore: any BondStore,
        onDeviceRenamed: @escaping (String) -> Void = { _ in },
        onForget: @escaping () -> Void = {}
    ) {
        self.transport = transport
        self.bondStore = bondStore
        self.onDeviceRenamed = onDeviceRenamed
        self.onForget = onForget
    }

    /// Subscribe the live streams and read the identity (call once, from `.task`).
    /// The stream loops are `[weak self]` + per-iteration `guard let self` — the
    /// streams never finish, and RootView makes a fresh model per Settings push,
    /// so a strong capture would strand every visited model for the session.
    public func start() {
        guard !started else { return }
        started = true
        streamTasks.append(Task { [weak self, transport] in
            for await state in transport.state {
                guard let self else { return }
                connection = state
            }
        })
        streamTasks.append(Task { [weak self, transport] in
            for await percent in transport.battery {
                guard let self else { return }
                battery = percent
            }
        })
        Task { [weak self, transport] in
            guard let info = try? await transport.deviceInfo() else { return }
            guard let self else { return }
            deviceName = info.name
            firmwareVersion = info.firmwareVersion
        }
    }

    deinit {
        streamTasks.forEach { $0.cancel() }
    }

    // MARK: Rename (H3)

    /// Apply a device rename: trims, rejects empty/unreachable, then updates
    /// the app side at once (screen, top-bar callback, bond record) and writes
    /// the config to the device. Returns whether the name was accepted —
    /// the alert's Save is a no-op otherwise, matching the H12 route rename.
    public func rename(to newName: String) -> Bool {
        // Cap at the S0 name limit so the app-side name matches what the codec
        // actually writes to the device (§7.3); trim again in case truncation
        // left a trailing space.
        let trimmed = newName
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .truncatedToUTF8Bytes(DeviceConfig.maxNameUTF8Bytes)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, canRename else { return false }
        deviceName = trimmed
        bondStore.save(BondRecord(deviceName: trimmed))
        onDeviceRenamed(trimmed)
        Task { [transport] in
            // Name rides in the Config blob (Delta 1): read-modify-write so the
            // other fields (units, …) survive the rename.
            guard var config = try? await transport.readConfig() else { return }
            config.name = trimmed
            try? await transport.writeConfig(config)
        }
        return true
    }

    // MARK: Forget (H2)

    /// Clear the bond and drop the link. iOS keeps the underlying BLE bond
    /// until the user removes it in Settings; the app just stops assuming it
    /// (see `BondStore`). The library store is deliberately untouched.
    public func forget() {
        bondStore.clear()
        Task { [transport] in
            await transport.disconnect()
        }
        onForget()
    }
}
