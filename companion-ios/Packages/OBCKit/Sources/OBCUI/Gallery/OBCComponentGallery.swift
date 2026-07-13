#if DEBUG
import SwiftUI
import OBCDomain

/// The B11 **component gallery** — every kit component with design-data
/// samples, for on-sim screenshot review (issue #240 acceptance) and quick
/// visual regression checks. Debug-only, like the mock dev tooling; reach it
/// with `-OBCShowUIGallery` (see `companion-ios/CLAUDE.md`).
public struct OBCComponentGallery: View {
    @State private var tab = 0
    @State private var toastShown = false
    @State private var renameShown = false
    @State private var confirmShown = false
    @State private var name = "Trailhead"
    @State private var progress = 0.62
    @State private var waypointsExpanded = true

    public init() {}

    public var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 30) {
                section("Device Top Bar") {
                    VStack(spacing: 0) {
                        DeviceTopBar(deviceName: "Trailhead", connection: .connected, batteryPercent: 82)
                        DeviceTopBar(deviceName: "Trailhead", connection: .connected, batteryPercent: 82, syncState: .syncing)
                        DeviceTopBar(deviceName: "Trailhead", connection: .connected, batteryPercent: 12, syncState: .done)
                        DeviceTopBar(deviceName: "Trailhead", connection: .outOfRange, batteryPercent: nil)
                    }
                    .padding(.horizontal, -20)
                }

                section("Nav Bar (serif large title)") {
                    OBCLargeTitleBar("Routes") {
                        OBCImportButton(fileExtensions: ["gpx", "tcx"]) { _ in }
                    }
                    .padding(.horizontal, -20)
                }

                section("Segmented Control") {
                    OBCSegmentedControl(selection: $tab, labels: ["Planned", "Tracked"])
                }

                section("GPS Track Preview") {
                    TrackPreviewView(.obcSample, style: .hero, tag: "Planned")
                        .frame(height: 214)
                    HStack(spacing: 16) {
                        TrackPreviewView(.obcSample)
                            .frame(width: 128, height: 116)
                        TrackPreviewView(nil)
                            .frame(width: 128, height: 116)
                    }
                }

                section("Route Card") {
                    RouteCard(
                        title: "Kettle Moraine Loop",
                        subtitle: "62.4 km · 840 m ↑ · 3h 20m",
                        preview: .obcSample
                    )
                    RouteCard(
                        title: "Blue Mounds Backroads",
                        subtitle: "Fri · 79.0 km · 4:12 · 18.8 kph",
                        preview: .obcSample
                    )
                    RouteCardFullBleed(
                        title: "Kettle Moraine Loop",
                        subtitle: "Southern Unit · gravel & forest doubletrack",
                        preview: .obcSample,
                        stats: [
                            OBCStat(value: "62.4", unit: "km", key: "Distance"),
                            OBCStat(value: "840", unit: "m", key: "Climb"),
                            OBCStat(value: "3:20", key: "Est."),
                        ],
                        tag: "Planned"
                    )
                }

                section("Skeleton Loader") {
                    RouteCardSkeleton()
                }

                section("Stats") {
                    OBCStatStrip([
                        OBCStat(value: "62.4", unit: "km", key: "Distance"),
                        OBCStat(value: "840", unit: "m", key: "Climb"),
                        OBCStat(value: "3:20", key: "Est. time"),
                        OBCStat(value: "4", key: "Points"),
                    ])
                    OBCStatGrid([
                        OBCStat(value: "58.2", unit: "km", key: "Distance"),
                        OBCStat(value: "2:51", key: "Moving"),
                        OBCStat(value: "20.4", unit: "kph", key: "Avg"),
                        OBCStat(value: "812", unit: "m", key: "Climb"),
                    ])
                }

                section("Elevation Profile") {
                    OBCEyebrow("Elevation profile")
                    ElevationProfileView(samples: [220, 260, 240, 380, 330, 470, 360, 450, 390, 410])
                }

                section("Disclosure Row + Waypoints Dropdown") {
                    OBCDisclosureRow(
                        systemImage: "mappin.and.ellipse",
                        label: "Waypoints",
                        value: "\(Self.sampleWaypoints.count)",
                        isExpanded: $waypointsExpanded
                    ) {
                        WaypointsDropdownContent(waypoints: Self.sampleWaypoints)
                    }
                }

                section("Buttons") {
                    Button {} label: { Label("Upload to Trailhead", systemImage: "square.and.arrow.up") }
                        .buttonStyle(.obcPrimary)
                    Button("Save to Planned") {}.buttonStyle(.obcGhost)
                    Button("Pair now") {}.buttonStyle(.obcWarm)
                    Button("Delete route") {}.buttonStyle(.obcDestructive)
                    Button("Disabled") {}.buttonStyle(.obcPrimary).disabled(true)
                }

                section("Progress Bar") {
                    OBCProgressBar(value: progress)
                    Button("Advance") { progress = progress >= 1 ? 0.1 : progress + 0.2 }
                        .buttonStyle(.obcGhost)
                }

                section("Banners + Toast") {
                    OBCInlineBanner(
                        systemImage: "wifi.slash",
                        title: "Trailhead is out of range.",
                        message: "Showing your last sync."
                    )
                    OBCInlineBanner(
                        tone: .warning,
                        systemImage: "exclamationmark.triangle",
                        title: "Sync interrupted.",
                        message: "Got 2 of 5 rides.",
                        actionTitle: "Resume"
                    )
                    Button("Show toast") { toastShown = true }.buttonStyle(.obcGhost)
                }

                section("Grouped List") {
                    OBCGroupedSection("Device", footer: "Renaming updates the device at the next sync.") {
                        OBCListRow(icon: "pencil", iconColor: OBCTheme.forest, label: "Name", value: name, showsChevron: true) { renameShown = true }
                        OBCListRow(icon: "arrow.triangle.2.circlepath", iconColor: OBCTheme.wood, label: "Firmware update", comingSoon: true)
                        OBCListRow(icon: "xmark.circle", iconColor: OBCTheme.warning, label: "Forget this device", showsDivider: false) { confirmShown = true }
                    }
                }

                section("Connected Services") {
                    OBCConnectedServicesBlock(services: [
                        OBCServiceStatus(name: "Strava", systemImage: "bolt.fill", tileColor: OBCTheme.coral, state: .uploaded("Uploaded on import")),
                        OBCServiceStatus(name: "Komoot", systemImage: "location.circle", tileColor: OBCTheme.wood, state: .notUploaded("Not uploaded")),
                    ])
                }

                section("Launch & Pairing (B2)") {
                    launchScreen { LaunchConnectingView(deviceName: "Trailhead") }
                    launchScreen { PairIntroView(onStart: {}) }
                    launchScreen {
                        PairScanningView(
                            discovered: .init(name: "Trailhead"),
                            onTapDevice: {},
                            onCancel: {}
                        )
                    }
                    launchScreen { PairedView(deviceName: "Trailhead", onContinue: {}) }
                    launchScreen { PairFailedView(failure: .timeout, onRetry: {}, onHelp: {}) }
                    launchScreen { RadioBlockedView(block: .off, onBrowseLibrary: {}) }
                }

                section("Empty / Error Layout") {
                    OBCEmptyStateView(
                        glyph: .trackTile,
                        title: "No planned routes yet",
                        message: "Tap + to import a .gpx from Files, or share one from Komoot, Strava, or any app.",
                        actionTitle: "Import a route",
                        actionSystemImage: "plus"
                    )
                    OBCEmptyStateView(
                        glyph: .warning(systemImage: "exclamationmark.triangle"),
                        title: "Couldn't read Trailhead",
                        message: "The connection dropped mid-read. Your saved routes are still here.",
                        actionTitle: "Retry"
                    )
                }
            }
            .padding(20)
        }
        .background(OBCTheme.parchment)
        .obcToast(isPresented: $toastShown, message: "You're up to date — no new rides on Trailhead.")
        .obcRenameAlert("Rename device", isPresented: $renameShown, name: $name) {}
        .obcDestructiveConfirm(
            "Forget Trailhead?",
            isPresented: $confirmShown,
            message: "The app deletes its pairing. Nothing on the device is touched.",
            actionTitle: "Forget device"
        ) {}
        .accessibilityIdentifier("uiGallery")
    }

    /// A full launch/pairing screen shrunk into a browsable gallery cell.
    private func launchScreen(@ViewBuilder _ content: () -> some View) -> some View {
        content()
            .frame(height: 620)
            .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusLarge))
            .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusLarge).strokeBorder(OBCTheme.line))
    }

    private func section(_ title: String, @ViewBuilder content: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            OBCEyebrow(title)
            content()
        }
    }

    static let sampleWaypoints = [
        Waypoint(index: 0, name: "Ottawa Lake trailhead", note: "Start · parking & water", distanceAlongMeters: 0, coordinate: .init(latitude: 42.9, longitude: -88.6)),
        Waypoint(index: 1, name: "Emma Carlin junction", note: "Water · trail crossing", distanceAlongMeters: 14200, coordinate: .init(latitude: 42.9, longitude: -88.5)),
        Waypoint(index: 2, name: "Bald Bluff overlook", note: "Summit · 12% pitch before", distanceAlongMeters: 31600, coordinate: .init(latitude: 42.9, longitude: -88.4)),
        Waypoint(index: 3, name: "Ottawa Lake", note: "Finish", distanceAlongMeters: 62400, coordinate: .init(latitude: 42.9, longitude: -88.6)),
    ]
}

#Preview("Gallery") {
    OBCComponentGallery()
}
#endif
