import Foundation

/// Link lifecycle the UI observes (surfaced by `DeviceTransport.state`). The BLE
/// link is intermittent **by design**, so this is a first-class domain type, not
/// an error condition — `.outOfRange` degrades the UI, never blocks.
public enum ConnectionState: Equatable, Sendable {
    case disconnected
    case connecting
    case connected
    case outOfRange
}
