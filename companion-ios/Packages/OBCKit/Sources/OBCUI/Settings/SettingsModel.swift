import Foundation
import Observation
import OBCDomain
import OBCTransport

/// Settings screen state: device identity, rename and forget.
///
/// A rename is a `writeConfig` with a changed name, because the device name lives in the config
/// blob and there is no separate rename command. The new name shows across the app at once and
/// rides to the device on the config write. Link-bound, so the row dims when unreachable. A failed
/// write surfaces once and self-heals on the next connect through `DeviceNameReconciler`.
///
/// Forget clears the app's bond record and drops the link. Everything on the phone stays: the
/// library store is untouched.
@MainActor @Observable
public final class SettingsModel {
    // MARK: Observable state

    public private(set) var deviceName = DeviceInfo.unnamed
    public private(set) var connection: ConnectionState = .connecting
    /// Battery percent, nil until the stream's first value.
    public private(set) var battery: Int?
    /// Raw firmware version, nil until the device info lands.
    public private(set) var firmwareVersion: String?
    /// The last rename's config write failed: the phone shows the new name but the device never
    /// got it. The view surfaces this once and clears it, and `DeviceNameReconciler` pushes the
    /// bond name on the next connect, so there is no retry here. Settable, because the toast's
    /// auto-dismiss writes it back through the binding.
    public var renameWriteFailed = false

    /// "Check for updates automatically", on by default. It gates both proactive surfaces, and
    /// with them the network request itself. Off means the app never asks the update server
    /// anything on its own; the firmware screen's own check is untouched.
    public private(set) var autoCheckUpdates: Bool

    // MARK: Derived copy

    /// The device row's status line.
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

    /// The device row's trailing version value; nil renders nothing.
    public var firmwareDisplay: String? {
        firmwareVersion.map { $0.hasPrefix("v") ? $0 : "v\($0)" }
    }

    /// The firmware group's version row. It states the version only: the firmware screen is where
    /// the comparison against the published build is made, so a row that claimed "latest" on its
    /// own authority would be a guess.
    public var firmwareLine: String { firmwareDisplay ?? "—" }

    /// A rename is a config write, so it dims when the link is unreachable.
    public var canRename: Bool { isConnected }

    // MARK: Wiring

    private let transport: any DeviceLink & DeviceBattery & DeviceConfiguration & DeviceBonding
    private let bondStore: any BondStore
    /// The proactive-update preference store: the same seam the launch sheet and the background
    /// refresh read, so flipping the toggle here silences both at once.
    private let updateSurface: any UpdateSurfaceStore
    /// Fires after a rename, so the composition root can refresh the main screen's top bar.
    /// Settings never reaches into another feature's model.
    private let onDeviceRenamed: (String) -> Void
    /// Fires after a forget, so the composition root can drop the launch flow back to the pairing
    /// prompt.
    private let onForget: () -> Void
    @ObservationIgnored private var started = false
    @ObservationIgnored private var streamTasks: [Task<Void, Never>] = []

    public init(
        transport: any DeviceLink & DeviceBattery & DeviceConfiguration & DeviceBonding,
        bondStore: any BondStore,
        updateSurface: any UpdateSurfaceStore = InMemoryUpdateSurfaceStore(),
        onDeviceRenamed: @escaping (String) -> Void = { _ in },
        onForget: @escaping () -> Void = {}
    ) {
        self.transport = transport
        self.bondStore = bondStore
        self.updateSurface = updateSurface
        self.autoCheckUpdates = updateSurface.loadAutoCheckEnabled()
        self.onDeviceRenamed = onDeviceRenamed
        self.onForget = onForget
    }
    // MARK: Automatic update checks

    /// Flip "Check for updates automatically" and persist it. The policy reads this store every
    /// time it decides, including after an in-flight check returns, so switching off cannot
    /// surface an answer that arrived a moment later.
    public func setAutoCheckUpdates(_ enabled: Bool) {
        guard enabled != autoCheckUpdates else { return }
        autoCheckUpdates = enabled
        updateSurface.saveAutoCheckEnabled(enabled)
    }

    /// Subscribe the live streams and read the identity. Call once. The stream loops capture self
    /// weakly, because the streams never finish and the host makes a fresh model per push, so a
    /// strong capture would strand every visited model for the session.
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

    // MARK: Rename

    /// Apply a device rename: trim, reject an empty name or an unreachable device, then update
    /// the app side at once and write the config to the device. Returns whether the name was
    /// accepted; the alert's Save is a no-op otherwise.
    public func rename(to newName: String) -> Bool {
        // Cap at the device's name limit, so the app-side name matches what the codec writes, and
        // trim again in case truncation left a trailing space.
        let trimmed = newName
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .truncatedToUTF8Bytes(DeviceConfig.maxNameUTF8Bytes)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, canRename else { return false }
        deviceName = trimmed
        bondStore.save(BondRecord(deviceName: trimmed))
        onDeviceRenamed(trimmed)
        Task { [weak self, transport] in
            // The name rides in the config blob, so read-modify-write and the other fields
            // survive the rename.
            do {
                var config = try await transport.readConfig()
                config.name = trimmed
                try await transport.writeConfig(config)
            } catch {
                // Either leg failing means the device never got the name. Flag it once for the
                // view's toast; the reconcile pass self-heals on the next connect, and the bond
                // record above already carries the desired name.
                self?.renameWriteFailed = true
            }
        }
        return true
    }

    // MARK: Forget

    /// The confirm dialog's message. When connected, the app dissolves the device's side of the
    /// bond too, so pairing again just works. When offline it cannot reach the device, so the
    /// device keeps its bond and the rider must forget the phone on it before re-pairing.
    public var forgetMessage: String {
        isConnected
            ? "You'll pair again to use it. Your routes and rides stay on this phone."
            : "You'll pair again to use it. The device keeps its pairing until you use Forget phone on it. Your routes and rides stay on this phone."
    }

    /// Clear the bond and drop the link. iOS keeps the underlying system BLE bond until the user
    /// removes it in Settings; the app just stops assuming it. The library store is untouched.
    ///
    /// When connected, first ask the device to dissolve its side of the bond, so a one-sided app
    /// forget does not leave the pair wedged. Best-effort by design: the happy path is the device
    /// acking and then dropping the link itself, so a failure here is indistinguishable from
    /// success followed by a disconnect. Await the ack or the timeout, then clear the local record
    /// and drop the link whether or not it succeeded.
    ///
    /// The connected task captures `bondStore` and `onForget` directly, never self: once the
    /// command is sent, the local clear must not depend on this model surviving the ack window,
    /// because the screen can pop and tear the model down mid-wait. A dropped clear would leave
    /// the device bond-less while the phone still holds its record, which is the inverse of the
    /// wedge this exists to fix.
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
