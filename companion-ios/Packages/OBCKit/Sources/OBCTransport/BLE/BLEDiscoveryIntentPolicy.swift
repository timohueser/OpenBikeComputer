import Foundation

/// Radio-free policy for multiplexing foreground OBC Control, the standing Weather Request watch,
/// the one-shot context read and the one-shot bundle upload (WX9) through one CoreBluetooth
/// manager/peripheral/session. `BLETransport` executes these decisions on its existing serial
/// queue; tests drive this value directly with deterministic IDs.
struct BLEDiscoveryIntentPolicy: Equatable, Sendable {
    enum Service: Equatable, Hashable, Sendable {
        case control
        case weatherRequest
    }

    enum Ownership: Equatable, Sendable {
        case foreground
        case weatherRequest
    }

    enum Phase: Equatable, Sendable {
        case idle
        case scanning
        case connecting(peripheralID: UUID, owner: Ownership)
        case connected(peripheralID: UUID, owner: Ownership)
    }

    enum RequestAction: Equatable, Sendable {
        case scan
        case readOnExistingConnection
        case waitForCurrentConnection
    }

    enum UploadAction: Equatable, Sendable {
        /// No scan: connect straight to the retrieved known peripheral. The device advertises OBC
        /// Control again once the context read consumed the hint (§11.3), so a UUID-filtered scan
        /// would never see it — but a pending direct connect completes the moment it advertises
        /// anything at all.
        case connectDirect
        case uploadOnExistingConnection
        case waitForCurrentConnection
    }

    enum DiscoveryAction: Equatable, Sendable {
        case ignore
        case connect(owner: Ownership)
        /// The standing watch matched the known peripheral with no read waiter in flight: the
        /// transport arms an autonomous one-shot read (result published on the events stream)
        /// and connects. The policy has already raised `weatherRequestPending` for it.
        case connectForWeatherRead
    }

    private(set) var foregroundRequested = false
    private(set) var weatherRequestPending = false
    /// The one-shot upload intent (WX9). Direct-connect based — never contributes to
    /// `scanServices`.
    private(set) var weatherUploadPending = false
    /// The standing background watch: scan for the Weather Request UUID whenever nothing else
    /// needs the radio, so a device raising a request wakes the app (foreground or background).
    private(set) var weatherWatchArmed = false
    private(set) var phase: Phase = .idle

    var scanServices: Set<Service> {
        if foregroundRequested { return [.control, .weatherRequest] }
        if weatherRequestPending || weatherWatchArmed { return [.weatherRequest] }
        return []
    }

    var hasIntent: Bool {
        foregroundRequested || weatherRequestPending || weatherWatchArmed
    }

    var connectionOwnership: Ownership? {
        switch phase {
        case .connecting(_, let owner), .connected(_, let owner): owner
        case .idle, .scanning: nil
        }
    }

    /// Whether the scanning phase should persist with the current intents.
    private var wantsScan: Bool { hasIntent }

    mutating func requestForeground() -> RequestAction {
        foregroundRequested = true
        switch phase {
        case .idle:
            phase = .scanning
            return .scan
        case .scanning:
            return .scan
        case .connecting, .connected:
            return .waitForCurrentConnection
        }
    }

    /// Returns true when the foreground caller owns a live/in-flight connection and should cancel
    /// it. A simultaneous weather intent (read, upload or watch) remains pending and keeps the
    /// radio after the drop.
    mutating func cancelForeground() -> Bool {
        foregroundRequested = false
        switch phase {
        case .connecting(_, .foreground), .connected(_, .foreground):
            return true
        case .scanning where !wantsScan:
            phase = .idle
        default:
            break
        }
        return false
    }

    mutating func requestWeather(knownPeripheralID: UUID, connectedPeripheralID: UUID?) -> RequestAction {
        weatherRequestPending = true
        if connectedPeripheralID == knownPeripheralID, case .connected = phase {
            return .readOnExistingConnection
        }
        switch phase {
        case .idle:
            phase = .scanning
            return .scan
        case .scanning:
            return .scan
        case .connecting, .connected:
            return .waitForCurrentConnection
        }
    }

    /// The upload leg (WX9). From idle *or* a scanning wait it claims the phase for a direct
    /// connect — a scan cannot find a device that has stopped advertising the weather UUID, and
    /// CoreBluetooth holds the pending connect until the peripheral advertises anything.
    ///
    /// **Never over a foreground request.** A user who tapped Connect owns the radio: claiming
    /// `.connecting` for the background job would stop the foreground scan's only path to a
    /// `.connect(owner: .foreground)` and park the user's link behind an upload for up to the
    /// upload's 90 s budget. With a foreground intent raised the upload waits instead — and the
    /// connection the foreground raises is one it can ride (`beginWeatherUploadIfReady`).
    mutating func requestWeatherUpload(
        knownPeripheralID: UUID, connectedPeripheralID: UUID?
    ) -> UploadAction {
        weatherUploadPending = true
        if connectedPeripheralID == knownPeripheralID, case .connected = phase {
            return .uploadOnExistingConnection
        }
        switch phase {
        case .idle, .scanning:
            guard !foregroundRequested else { return .waitForCurrentConnection }
            phase = .connecting(peripheralID: knownPeripheralID, owner: .weatherRequest)
            return .connectDirect
        case .connecting, .connected:
            return .waitForCurrentConnection
        }
    }

    mutating func discovered(peripheralID: UUID, knownPeripheralID: UUID?) -> DiscoveryAction {
        guard phase == .scanning else { return .ignore }
        if foregroundRequested {
            phase = .connecting(peripheralID: peripheralID, owner: .foreground)
            return .connect(owner: .foreground)
        }
        guard knownPeripheralID == peripheralID else { return .ignore }
        if weatherRequestPending {
            phase = .connecting(peripheralID: peripheralID, owner: .weatherRequest)
            return .connect(owner: .weatherRequest)
        }
        if weatherWatchArmed {
            // The watch saw the known device raise a request with nobody asking yet — raise the
            // read intent here so one advertisement report cannot double-connect, and tell the
            // transport to arm its autonomous read bookkeeping.
            weatherRequestPending = true
            phase = .connecting(peripheralID: peripheralID, owner: .weatherRequest)
            return .connectForWeatherRead
        }
        return .ignore
    }

    mutating func didConnect(peripheralID: UUID) {
        guard case .connecting(peripheralID, let owner) = phase else { return }
        phase = .connected(peripheralID: peripheralID, owner: owner)
    }

    mutating func didDisconnect() {
        phase = wantsScan ? .scanning : .idle
    }

    /// Complete/cancel/timeout the one-shot read. Only a connection originally created for the
    /// weather lane may be disconnected by that completion — and not while the upload leg is
    /// still using it.
    mutating func finishWeatherRequest() -> Bool {
        weatherRequestPending = false
        switch phase {
        case .connecting(_, .weatherRequest), .connected(_, .weatherRequest):
            return !weatherUploadPending
        case .scanning where !wantsScan:
            phase = .idle
        default:
            break
        }
        return false
    }

    /// Complete/cancel/timeout the one-shot upload — the same disconnect rule, guarded against a
    /// read still sharing the connection.
    mutating func finishWeatherUpload() -> Bool {
        weatherUploadPending = false
        switch phase {
        case .connecting(_, .weatherRequest), .connected(_, .weatherRequest):
            return !weatherRequestPending
        case .scanning where !wantsScan:
            phase = .idle
        default:
            break
        }
        return false
    }

    /// Arm/disarm the standing watch. Arming while idle raises the scanning phase so a discovery
    /// is not ignored; disarming lowers it only when nothing else wants the radio.
    mutating func setWeatherWatch(_ armed: Bool) {
        weatherWatchArmed = armed
        if armed, phase == .idle { phase = .scanning }
        if !armed, phase == .scanning, !wantsScan { phase = .idle }
    }

    /// The transport started (or refreshed) a scan — make sure a discovery is not dropped because
    /// the phase never left `.idle` (the standing-watch scan has no `request*` call to raise it).
    mutating func noteScanning() {
        if phase == .idle, wantsScan { phase = .scanning }
    }

    mutating func radioBecameUnavailable() {
        foregroundRequested = false
        weatherRequestPending = false
        weatherUploadPending = false
        // The watch survives a radio toggle: it is a standing preference, not an in-flight op.
        phase = .idle
    }

    /// CoreBluetooth state restoration is accepted only for an unambiguous weather-only scan,
    /// and only when the restored set contains the peripheral UUID persisted after a successful
    /// authenticated foreground session. Arbitrary restored advertisers are ignored.
    mutating func restoreWeatherRequest(
        scannedServices: Set<Service>, restoredPeripheralIDs: Set<UUID>, knownPeripheralID: UUID
    ) -> UUID? {
        guard scannedServices == [.weatherRequest] else { return nil }
        guard restoredPeripheralIDs.contains(knownPeripheralID) || restoredPeripheralIDs.isEmpty else { return nil }
        weatherRequestPending = true
        if restoredPeripheralIDs.contains(knownPeripheralID) {
            phase = .connecting(peripheralID: knownPeripheralID, owner: .weatherRequest)
            return knownPeripheralID
        }
        phase = .scanning
        return nil
    }

    /// Restoration of the upload leg: a pending direct connect (no scan) relaunched the app. Only
    /// the known authenticated peripheral is ever adopted; anything else is ignored.
    mutating func restoreWeatherUpload(
        restoredPeripheralIDs: Set<UUID>, knownPeripheralID: UUID
    ) -> UUID? {
        guard restoredPeripheralIDs.contains(knownPeripheralID) else { return nil }
        weatherUploadPending = true
        phase = .connecting(peripheralID: knownPeripheralID, owner: .weatherRequest)
        return knownPeripheralID
    }
}
