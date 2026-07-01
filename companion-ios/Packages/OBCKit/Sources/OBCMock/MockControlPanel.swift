#if DEBUG
import SwiftUI
import OBCDomain
import OBCTransport

/// The in-app dev control panel (B1P): every `MockControl` knob, live, while you
/// click through the app. Deliberately utilitarian — a grouped list of pickers
/// and buttons, not a designed screen. Opened via shake (or `-OBCShowDevPanel`);
/// B8 adds the hidden Settings row.
///
/// The panel holds `@State` mirrors of the knobs (initialized from the live
/// control, pushed back on change); picking a scenario re-applies the whole
/// preset and re-reads every mirror.
public struct MockControlPanel: View {
    private let control: MockControl
    @Environment(\.dismiss) private var dismiss

    @State private var scenario: Scenario
    @State private var connection: ConnectionState
    @State private var battery: Double
    @State private var latencyMilliseconds: Int
    @State private var throughputBytesPerSec: Int
    @State private var radio: RadioState

    public init(control: MockControl) {
        self.control = control
        _scenario = State(initialValue: control.scenario)
        _connection = State(initialValue: control.connection)
        _battery = State(initialValue: Double(control.battery))
        _latencyMilliseconds = State(initialValue: control.latency.wholeMilliseconds)
        _throughputBytesPerSec = State(initialValue: control.throughputBytesPerSec)
        _radio = State(initialValue: control.radio)
    }

    public var body: some View {
        NavigationStack {
            Form {
                scenarioSection
                linkSection
                timingSection
                faultsSection
                eventsSection
                fixturesSection
                deviceSection
            }
            .accessibilityIdentifier("devPanel")
            .navigationTitle("Mock control")
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                        .accessibilityIdentifier("devPanel.done")
                }
            }
        }
    }

    // MARK: Sections

    private var scenarioSection: some View {
        Section("Scenario") {
            Picker("Preset", selection: $scenario) {
                ForEach(Scenario.allCases, id: \.self) { Text($0.rawValue).tag($0) }
            }
            .accessibilityIdentifier("devPanel.scenario")
            .onChange(of: scenario) { _, newValue in
                control.apply(newValue)
                readBackKnobs()
            }
        }
    }

    private var linkSection: some View {
        Section("Link") {
            Picker("Connection", selection: $connection) {
                ForEach([ConnectionState.disconnected, .connecting, .connected, .outOfRange], id: \.self) {
                    Text($0.launchToken).tag($0)
                }
            }
            .accessibilityIdentifier("devPanel.connection")
            .onChange(of: connection) { _, newValue in control.connection = newValue }

            Picker("Radio", selection: $radio) {
                ForEach([RadioState.on, .off, .unauthorized], id: \.self) {
                    Text(String(describing: $0)).tag($0)
                }
            }
            .onChange(of: radio) { _, newValue in control.radio = newValue }

            LabeledContent("Battery \(Int(battery))%") {
                Slider(value: $battery, in: 0...100, step: 1)
                    .onChange(of: battery) { _, newValue in control.battery = Int(newValue) }
            }
        }
    }

    private var timingSection: some View {
        Section("Timing") {
            Picker("Latency", selection: $latencyMilliseconds) {
                ForEach(options([0, 180, 500, 1_000, 3_000], including: latencyMilliseconds), id: \.self) {
                    Text($0 == 0 ? "none" : "\($0) ms").tag($0)
                }
            }
            .onChange(of: latencyMilliseconds) { _, newValue in
                control.latency = .milliseconds(newValue)
            }

            Picker("Throughput", selection: $throughputBytesPerSec) {
                ForEach(options([50_000, 200_000, 500_000, 2_000_000], including: throughputBytesPerSec), id: \.self) {
                    Text("\($0 / 1_000) KB/s").tag($0)
                }
            }
            .onChange(of: throughputBytesPerSec) { _, newValue in
                control.throughputBytesPerSec = newValue
            }
        }
    }

    private var faultsSection: some View {
        Section {
            Button("Fail next op (read)") { control.failNextOp(.readFailed) }
            Button("Fail next op (write)") { control.failNextOp(.writeFailed) }
            Button("Drop next transfer at 50%") { control.dropTransfer(atFraction: 0.5) }
            Button("Arm pairing timeout") { control.failPairing(.timeout) }
            Button("Arm pairing rejection") { control.failPairing(.rejected) }
        } header: {
            Text("Faults (one-shot)")
        } footer: {
            Text("Armed faults fire on the next matching operation, then clear.")
        }
    }

    private var eventsSection: some View {
        Section("Events") {
            Button("Add a synthetic ride") {
                control.emit(.rideAdded(RideSummary(
                    id: RideID("injected-\(UUID().uuidString.prefix(8))"),
                    name: "Injected ride",
                    date: .now,
                    distanceMeters: 12_400,
                    movingTime: 1_845,
                    averageSpeedMps: 6.7,
                    climbMeters: 210
                )))
            }
        }
    }

    private var fixturesSection: some View {
        Section("Fixtures") {
            ForEach(["default", "empty", "large"], id: \.self) { name in
                Button("Load “\(name)”") {
                    control.loadFixtures(name)
                    readBackKnobs()
                }
            }
        }
    }

    private var deviceSection: some View {
        Section("Reported device") {
            LabeledContent("Name", value: control.deviceInfo.name)
            LabeledContent("Firmware", value: control.deviceInfo.firmwareVersion)
        }
    }

    // MARK: Helpers

    /// Re-read every knob mirror from the control (after a preset/fixture apply).
    private func readBackKnobs() {
        connection = control.connection
        battery = Double(control.battery)
        latencyMilliseconds = control.latency.wholeMilliseconds
        throughputBytesPerSec = control.throughputBytesPerSec
        radio = control.radio
    }

    /// Fixed picker options, plus the current value if it's nonstandard (so the
    /// picker never shows an empty selection).
    private func options(_ fixed: [Int], including current: Int) -> [Int] {
        fixed.contains(current) ? fixed : (fixed + [current]).sorted()
    }
}

/// A one-line status tag for automation + at-a-glance state: the active scenario
/// and the live connection state. Overlaid on the root view in Debug builds;
/// XCUITests assert `mockScenarioTag` / `mockConnectionTag` to prove a launch
/// argument booted the right state.
public struct MockStatusHUD: View {
    private let control: MockControl
    @State private var scenario: Scenario
    @State private var connection: ConnectionState

    public init(control: MockControl) {
        self.control = control
        _scenario = State(initialValue: control.scenario)
        _connection = State(initialValue: control.connection)
    }

    public var body: some View {
        HStack(spacing: 6) {
            Text(scenario.rawValue)
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
            // The connection stream replays the latest value, so the HUD is
            // correct immediately and tracks every change; the scenario is
            // re-read on each event (it only changes via the panel).
            for await state in control.stateMulticast.stream() {
                connection = state
                scenario = control.scenario
            }
        }
    }
}

extension ConnectionState {
    /// The stable token used by `-OBCConnection` and shown in the HUD.
    public var launchToken: String {
        switch self {
        case .disconnected: return "disconnected"
        case .connecting: return "connecting"
        case .connected: return "connected"
        case .outOfRange: return "outOfRange"
        }
    }
}

extension Duration {
    fileprivate var wholeMilliseconds: Int {
        Int(components.seconds) * 1_000 + Int(components.attoseconds / 1_000_000_000_000_000)
    }
}
#endif
