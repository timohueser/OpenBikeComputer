import SwiftUI
import OBCDomain
import OBCTransport
import OBCUI

/// The app's root: the B2 launch gate (bond check → quiet reconnect, or the
/// D1–D5 pairing flow) in front of the main screen (B3). Holds only the two
/// seams — `any DeviceTransport` + `any BondStore` — chosen by the composition
/// root, and owns the navigation stack the main screen pushes into.
struct RootView: View {
    @State private var launchModel: LaunchFlowModel
    @State private var mainModel: MainScreenModel
    @State private var path: [MainDestination] = []

    init(transport: any DeviceTransport, bondStore: any BondStore) {
        _launchModel = State(initialValue: LaunchFlowModel(transport: transport, bondStore: bondStore))
        _mainModel = State(initialValue: MainScreenModel(transport: transport))
    }

    var body: some View {
        LaunchFlowView(model: launchModel) {
            NavigationStack(path: $path) {
                MainScreenView(
                    model: mainModel,
                    // TODO(B6): once GPX/TCX decoders register, pass
                    // `RouteImporter.supportedFileExtensions` instead.
                    importFileExtensions: ["gpx", "tcx"],
                    onImportFile: { _ in
                        // TODO(B6): decode + land on the E1 import screen.
                    },
                    onSelectRoute: { route in
                        path.append(.route(id: route.id, name: route.name))
                    },
                    onSelectRide: { ride in
                        path.append(.ride(id: ride.id, name: ride.name))
                    },
                    onSettings: {
                        // TODO(B8): the settings screen (G).
                    }
                )
                .navigationDestination(for: MainDestination.self) { destination in
                    DetailPlaceholderView(destination: destination)
                }
            }
        }
    }
}

/// Where a card tap lands until the real detail screens exist (E2 planned =
/// B4, E3 tracked = B7).
enum MainDestination: Hashable {
    case route(id: RouteID, name: String)
    case ride(id: RideID, name: String)
}

private struct DetailPlaceholderView: View {
    let destination: MainDestination

    private var title: String {
        switch destination {
        case .route(_, let name), .ride(_, let name): name
        }
    }

    var body: some View {
        VStack(spacing: 10) {
            Text(title)
                .font(.obcSerif(size: 26))
                .foregroundStyle(OBCTheme.ink)
            Text("route detail · B4 lands here")
                .font(.caption2)
                .foregroundStyle(.secondary)
                .accessibilityIdentifier("detailPlaceholder")
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(OBCTheme.parchment.ignoresSafeArea())
    }
}

#if DEBUG
import OBCMock

#Preview("Bonded (main)") {
    let control = MockControl(scenario: .happyPath)
    RootView(transport: MockTransport(control: control), bondStore: MockBondStore(control: control))
}

#Preview("First run (pairing)") {
    let control = MockControl(scenario: .noDevice)
    RootView(transport: MockTransport(control: control), bondStore: MockBondStore(control: control))
}

#Preview("Out of range (S4)") {
    let control = MockControl(scenario: .outOfRange)
    RootView(transport: MockTransport(control: control), bondStore: MockBondStore(control: control))
}
#endif
