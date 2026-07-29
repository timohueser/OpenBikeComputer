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
/// (the S4 rule: actions that need the link dim). A failed write surfaces once
/// (`renameWriteFailed` → the view's toast) and self-heals on the next connect
/// via `DeviceNameReconciler` (#361) — reconciliation over error dialogs.
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
    /// The last rename's config write failed — the phone shows the new name
    /// but the device never got it. The view surfaces this once (toast) and
    /// clears it; `DeviceNameReconciler` pushes the bond name on the next
    /// connect (#361), so there is no retry here. Settable: the toast's
    /// auto-dismiss writes it back through the binding.
    public var renameWriteFailed = false

    /// The default retention a **new** upload seeds (epic #638 S7) — an app-local
    /// preference, mirrored from ``RetentionDefaultsStore`` on init and written
    /// back through ``setDefaultRetention(_:)``. Read-only from the view; the
    /// Auto-delete row picks a level via the setter so the persist can't be
    /// skipped. Changing it seeds only future uploads — never a retro write.
    public private(set) var defaultRetention: Retention

    /// "Check for updates automatically" (#773 U5) — **on by default**. It gates both proactive
    /// surfaces: the launch sheet and the background refresh, and with it the network request
    /// itself. Off means the app never asks the update server anything on its own; the firmware
    /// screen's own check (which the rider opened deliberately) is untouched.
    public private(set) var autoCheckUpdates: Bool

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

    /// "v0.4.2" — the firmware group's version row. It used to read "· latest", which was true only
    /// while the phone had no update channel to compare against; since #773 U4 it has one, and the
    /// firmware screen is where that comparison is stated. A row that claims "latest" on its own
    /// authority would now be a guess, and sometimes a wrong one.
    public var firmwareLine: String { firmwareDisplay ?? "—" }

    /// H3 is a config write — link-bound, dimmed when unreachable (S4 rule).
    public var canRename: Bool { isConnected }

    // MARK: Wiring

    private let transport: any DeviceTransport
    private let bondStore: any BondStore
    /// The app-local default-retention preference (epic #638) — the persist seam
    /// behind the Auto-delete row, shared with `MainScreenModel` so a change here
    /// seeds the next upload's picker.
    private let retentionDefaults: any RetentionDefaultsStore
    /// The proactive-update preference store (#773 U5) — the same seam the launch sheet and the
    /// background refresh read, so flipping the toggle here silences both at once.
    private let updateSurface: any UpdateSurfaceStore
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
        retentionDefaults: any RetentionDefaultsStore = InMemoryRetentionDefaultsStore(),
        updateSurface: any UpdateSurfaceStore = InMemoryUpdateSurfaceStore(),
        onDeviceRenamed: @escaping (String) -> Void = { _ in },
        onForget: @escaping () -> Void = {}
    ) {
        self.transport = transport
        self.bondStore = bondStore
        self.retentionDefaults = retentionDefaults
        self.defaultRetention = retentionDefaults.loadDefaultRetention()
        self.updateSurface = updateSurface
        self.autoCheckUpdates = updateSurface.loadAutoCheckEnabled()
        self.onDeviceRenamed = onDeviceRenamed
        self.onForget = onForget
    }

    // MARK: Default retention (epic #638 S7)

    /// Pick the default retention for new uploads: update the observable state and
    /// persist it. A no-op when unchanged. Seeds only future uploads — existing
    /// routes keep whatever level they already carry (no retro writes).
    public func setDefaultRetention(_ retention: Retention) {
        guard retention != defaultRetention else { return }
        defaultRetention = retention
        retentionDefaults.saveDefaultRetention(retention)
    }

    // MARK: Automatic update checks (#773 U5)

    /// Flip "Check for updates automatically" and persist it. Takes effect at the next foreground /
    /// background wake — nothing is cancelled retroactively because nothing is in flight; the
    /// policy reads this store every time it decides.
    public func setAutoCheckUpdates(_ enabled: Bool) {
        guard enabled != autoCheckUpdates else { return }
        autoCheckUpdates = enabled
        updateSurface.saveAutoCheckEnabled(enabled)
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
        Task { [weak self, transport] in
            // Name rides in the Config blob (Delta 1): read-modify-write so the
            // other fields (units, …) survive the rename.
            do {
                var config = try await transport.readConfig()
                config.name = trimmed
                try await transport.writeConfig(config)
            } catch {
                // Either leg failing means the device never got the name.
                // Flag it once for the view's toast; the reconcile pass
                // self-heals on the next connect (#361) — the bond record
                // above already carries the desired name.
                self?.renameWriteFailed = true
            }
        }
        return true
    }

    // MARK: Forget (H2)

    /// The confirm dialog's message. When connected, the app dissolves the
    /// device's side of the bond too (#756), so pairing again just works — no
    /// mention of a device step. When offline it can't reach the device, so the
    /// device keeps its bond and the rider must Forget phone on it before
    /// re-pairing — the guidance the not-connected case keeps.
    public var forgetMessage: String {
        isConnected
            ? "You'll pair again to use it. Your routes and rides stay on this phone."
            : "You'll pair again to use it. The device keeps its pairing until you use Forget phone on it. Your routes and rides stay on this phone."
    }

    /// Clear the bond and drop the link. iOS keeps the underlying system BLE bond
    /// until the user removes it in Settings; the app just stops assuming it
    /// (see `BondStore`). The library store is deliberately untouched.
    ///
    /// **#756**: when connected, first ask the device to dissolve *its* side of
    /// the bond (`forgetBond`) so a one-sided app forget doesn't leave the pair
    /// wedged (the device's reject-when-bonded posture would otherwise refuse
    /// re-pairing until the rider ran Forget phone on it). Best-effort by design
    /// (the locked #756 decision, not a silent-`try?` violation): the happy path
    /// is the device acking then *dropping the link itself*, so "failure" here is
    /// indistinguishable from success-then-disconnect — we await the
    /// ack-or-timeout, then clear the local record + drop the link **whether or
    /// not** it succeeded. When offline we can't reach the device, so this is
    /// exactly the prior behaviour — clear and drop, immediately.
    ///
    /// The connected task captures `bondStore` + `onForget` directly — never
    /// `self`, weak or strong: once `forgetBond` is sent, the local clear must
    /// not depend on this model surviving the ack window (the Settings screen can
    /// pop and tear the model down mid-wait; RootView makes a fresh model per
    /// push). A dropped clear would leave the device bond-less while the phone
    /// still holds its record — a bonded next launch against a device in open
    /// pairing, the inverse of the wedge this exists to fix.
    public func forget() {
        guard isConnected else {
            bondStore.clear()
            onForget()
            Task { [transport] in await transport.disconnect() }
            return
        }
        Task { [transport, bondStore, onForget] in
            try? await transport.forgetBond()
            bondStore.clear()
            onForget()
            await transport.disconnect()
        }
    }
}
