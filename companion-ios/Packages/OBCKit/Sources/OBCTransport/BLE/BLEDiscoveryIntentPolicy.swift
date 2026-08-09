import Foundation

/// Radio-free policy for multiplexing foreground OBC Control and one-shot Weather Request
/// discovery through one CoreBluetooth manager/peripheral/session. `BLETransport` executes these
/// decisions on its existing serial queue; tests drive this value directly with deterministic IDs.
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

    enum DiscoveryAction: Equatable, Sendable {
        case ignore
        case connect(owner: Ownership)
    }

    private(set) var foregroundRequested = false
    private(set) var weatherRequestPending = false
    private(set) var phase: Phase = .idle

    var scanServices: Set<Service> {
        if foregroundRequested { return [.control, .weatherRequest] }
        if weatherRequestPending { return [.weatherRequest] }
        return []
    }

    var hasIntent: Bool { foregroundRequested || weatherRequestPending }

    var connectionOwnership: Ownership? {
        switch phase {
        case .connecting(_, let owner), .connected(_, let owner): owner
        case .idle, .scanning: nil
        }
    }

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
    /// it. A simultaneous weather request remains pending and will scan after the drop.
    mutating func cancelForeground() -> Bool {
        foregroundRequested = false
        switch phase {
        case .connecting(_, .foreground), .connected(_, .foreground):
            return true
        case .scanning where !weatherRequestPending:
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

    mutating func discovered(peripheralID: UUID, knownPeripheralID: UUID?) -> DiscoveryAction {
        guard phase == .scanning else { return .ignore }
        if foregroundRequested {
            phase = .connecting(peripheralID: peripheralID, owner: .foreground)
            return .connect(owner: .foreground)
        }
        guard weatherRequestPending, knownPeripheralID == peripheralID else { return .ignore }
        phase = .connecting(peripheralID: peripheralID, owner: .weatherRequest)
        return .connect(owner: .weatherRequest)
    }

    mutating func didConnect(peripheralID: UUID) {
        guard case .connecting(peripheralID, let owner) = phase else { return }
        phase = .connected(peripheralID: peripheralID, owner: owner)
    }

    mutating func didDisconnect() {
        phase = hasIntent ? .scanning : .idle
    }

    /// Complete/cancel/timeout the one-shot. Only a connection originally created for the
    /// weather request may be disconnected by that completion.
    mutating func finishWeatherRequest() -> Bool {
        weatherRequestPending = false
        switch phase {
        case .connecting(_, .weatherRequest), .connected(_, .weatherRequest):
            return true
        case .scanning where !foregroundRequested:
            phase = .idle
        default:
            break
        }
        return false
    }

    mutating func radioBecameUnavailable() {
        foregroundRequested = false
        weatherRequestPending = false
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
}
