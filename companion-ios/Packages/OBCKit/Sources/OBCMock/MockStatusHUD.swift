#if DEBUG
import SwiftUI
import OBCDomain
import OBCTransport

/// A one-line status tag overlaid on the root view in Debug builds: the active scenario and
/// the live connection state. XCUITests assert `mockScenarioTag` and `mockConnectionTag`.
public struct MockStatusHUD: View {
    private let control: MockControl
    @State private var connection: ConnectionState

    public init(control: MockControl) {
        self.control = control
        _connection = State(initialValue: control.connection)
    }

    public var body: some View {
        HStack(spacing: 6) {
            Text(control.scenario.rawValue)
                .accessibilityIdentifier("mockScenarioTag")
            Text("·")
            Text(connection.launchToken)
                .accessibilityIdentifier("mockConnectionTag")
        }
        .font(.caption2.monospaced())
        .padding(.horizontal, 8)
        .padding(.vertical, 3)
        .background(.black.opacity(0.55), in: Capsule())
        .foregroundStyle(.white)
        .task {
            // The connection stream replays the latest value, so the HUD is correct at once.
            for await state in control.stateMulticast.stream() {
                connection = state
            }
        }
    }
}

extension ConnectionState {
    /// The stable token `-OBCConnection` uses, also shown in the HUD.
    public var launchToken: String {
        switch self {
        case .disconnected: return "disconnected"
        case .connecting: return "connecting"
        case .connected: return "connected"
        case .outOfRange: return "outOfRange"
        }
    }
}
#endif
