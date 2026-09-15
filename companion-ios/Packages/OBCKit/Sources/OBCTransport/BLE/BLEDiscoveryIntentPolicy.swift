import Foundation

struct BLEDiscoveryIntentPolicy: Equatable, Sendable {
    enum Phase: Equatable, Sendable {
        case idle
        case scanning
        case connecting(peripheralID: UUID)
        case connected(peripheralID: UUID)
    }

    enum DiscoveryAction: Equatable, Sendable {
        case ignore
        case connect
    }

    private(set) var foregroundRequested = false
    private(set) var phase: Phase = .idle

    var hasIntent: Bool { foregroundRequested }

    mutating func requestForeground() {
        foregroundRequested = true
        if phase == .idle { phase = .scanning }
    }

    mutating func cancelForeground() -> Bool {
        foregroundRequested = false
        switch phase {
        case .connecting, .connected:
            return true
        case .scanning:
            phase = .idle
        case .idle:
            break
        }
        return false
    }

    mutating func discovered(peripheralID: UUID, knownPeripheralID: UUID?) -> DiscoveryAction {
        guard phase == .scanning, foregroundRequested else { return .ignore }
        if let knownPeripheralID, knownPeripheralID != peripheralID { return .ignore }
        phase = .connecting(peripheralID: peripheralID)
        return .connect
    }

    mutating func didConnect(peripheralID: UUID) {
        guard phase == .connecting(peripheralID: peripheralID) else { return }
        phase = .connected(peripheralID: peripheralID)
    }

    mutating func didDisconnect() {
        phase = foregroundRequested ? .scanning : .idle
    }
}
