import Foundation
import OBCDomain

/// An advertisement from one nearby OBC. The identifier selects the radio peer, never its name.
public struct PairingDevice: Identifiable, Equatable, Sendable {
    public let id: UUID
    public let name: String

    public init(id: UUID, name: String) {
        self.id = id
        self.name = name
    }
}

/// First-use discovery keeps the candidate scan separate from connecting and authentication.
public protocol DevicePairing: Sendable {
    /// Collect nearby OBC advertisements before choosing. Cancellation stops the scan.
    func scanForPairing() async throws -> [PairingDevice]
    /// Connect only the selected candidate and discover its unprotected characteristics.
    func discover(_ candidate: PairingDevice) async throws
}
