#if DEBUG
import SwiftUI
import OBCDomain
import OBCTransport

/// The component gallery: every kit component with sample data, for on-simulator
/// screenshot review and quick visual checks. Debug-only; reach it with
/// `-OBCShowUIGallery`.
public struct OBCComponentGallery: View {
    @State private var tab = 0
    @State private var toastShown = false
    @State private var renameShown = false
    @State private var confirmShown = false
    @State private var name = "Trailhead"
    @State private var progress = 0.62
    @State private var waypointsExpanded = true
    @State private var rideLibrary = Self.sampleRideLibrary()
    @State private var selectedPhotos: Set<String> = ["p0", "p1", "p3", "p4", "p5"]
    @State private var dayNote = Self.sampleDayNote()

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
                    RouteCard(
                        title: "Sugar River Trail",
                        subtitle: "38.1 km · 210 m ↑ · 1h 55m",
                        preview: .obcSample,
                        onDevice: .upToDate,
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
                section("Ride Library Header") {
                    RideLibraryHeader(model: rideLibrary) {}
                        .task { await rideLibrary.loadMapLines() }
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

                section("Marker-on-line editor") {
                    LineMarkerGallerySection()
                }

                section("Trip stops sheet") {
                    TripStopsGallerySection()
                }

                section("Ride edit") {
                    RideEditGallerySection()
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
                    OBCGroupedSection {
                        OBCListRow(label: "Add to Alps traverse", detail: "Becomes Day 4", showsChevron: true) {}
                        OBCBikeTypeRow(type: .gravel) { _ in }
                    }
                }

                section("Trip Day Rows") {
                    OBCGroupedSection {
                        TripDayRow(color: OBCTheme.stageColor(index: 0), number: 1, title: "Furka Pass",
                                   detail: "Mon 29 Sep · 82.0 km · 1,640 m ↑ · 5h 10m")
                        TripDayRow(color: OBCTheme.stageColor(index: 1), number: 2, title: nil,
                                   detail: "Tue 30 Sep · 74.0 km · 2,100 m ↑ · 5h 0m", showsDivider: false)
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

                #if os(iOS)
                section("Share image (offline map fallback)") {
                    ForEach([false, true], id: \.self) { showsProfile in
                        ShareCard(content: Self.sampleShareContent, map: nil, photo: nil, showsProfile: showsProfile)
                            .overlay(Rectangle().strokeBorder(OBCTheme.line))
                    }
                }
                #endif

                #if os(iOS)
                section("Ride photos") {
                    OBCQuietRow(systemImage: "photo.on.rectangle", title: "Add 6 photos from this ride", onOpen: {}, onDismiss: {})
                    ElevationProfileView(
                        samples: [220, 260, 240, 380, 330, 470, 360, 450, 390, 410], ticks: [0.12, 0.3, 0.34, 0.55, 0.8, 0.93]
                    )
                    RidePhotoStrip(photos: Self.samplePhotos, thumbnails: Self.sampleThumbnails) { _ in }
                    RidePhotoGrid(
                        picks: Self.samplePhotos.enumerated().map { index, photo in
                            RidePhotosModel.Pick(
                                placed: RidePhotoPlacement.Placed(
                                    photo: photo, distanceMeters: 0, coordinate: Coordinate(latitude: 0, longitude: 0),
                                    locationOffTrack: index == 2
                                ),
                                thumbnail: Self.sampleThumbnails[photo.assetID]
                            )
                        },
                        selected: $selectedPhotos
                    )
                }
                #endif

                section("Day note") {
                    // Live: the row opens the writer, and the entry shows what it saved.
                    DayNoteOfferRow(model: dayNote, photos: nil)
                    DayNoteEntry(model: dayNote, photos: nil)
                    DayNoteText(
                        header: "Tue 30 Sep · Andermatt → Ulrichen · 74 km",
                        note: "Furka in the fog, then sun on the way down. Wild camp by the lake, storm at 3."
                    )
                }
                .task { await dayNote.start() }

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

    /// A full launch or pairing screen shrunk into a browsable gallery cell.
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

    #if os(iOS)
    static let sampleShareContent = ShareCardContent(ride: Ride(
        summary: RideSummary(
            id: RideID("gallery"), name: "Kettle Moraine Loop", date: Date(timeIntervalSince1970: 1_790_000_000),
            distanceMeters: 58_200, movingTime: 10_260, averageSpeedMps: 5.67, climbMeters: 812
        ),
        points: zip(TrackPreview.obcSample.coordinates, [260, 280, 310, 350, 330, 300, 290, 270, 265, 262, 260])
            .map { RidePoint(timestamp: Date(), coordinate: $0, elevationMeters: $1) }
    ))
    #endif

    /// Two seasons of loops around the sample track, so the chips and the year menu have choices.
    static func sampleRideLibrary() -> RideLibraryModel {
        let store = InMemoryLibraryStore()
        let loop = TrackPreview.obcSample.coordinates
        let rides: [(String, String, BikeType, Double)] = [
            ("Kettle Moraine Loop", "2026-06-30T08:12:00Z", .gravel, 0),
            ("Blue Mounds Backroads", "2026-05-17T07:40:00Z", .road, 0.03),
            ("Sugar River Trail", "2026-04-02T09:05:00Z", .road, -0.02),
            ("Emma Carlin Singletrack", "2025-09-20T15:00:00Z", .mtb, 0.05),
        ]
        for (name, iso, type, shift) in rides {
            let date = ISO8601DateFormatter().date(from: iso)!
            let points = loop.map {
                RidePoint(timestamp: date, coordinate: Coordinate(latitude: $0.latitude + shift, longitude: $0.longitude + shift))
            }
            store.saveRide(Ride(
                summary: RideSummary(id: RideID(name), name: name, date: date, distanceMeters: 42_300,
                                     movingTime: 8_100, climbMeters: 520, bikeType: type),
                points: points
            ))
        }
        let model = RideLibraryModel(library: store)
        model.rides = store.rideSummaries()
        return model
    }

    /// A Day 2 ride in a fresh in-memory library, so the row and the writer run for real.
    static func sampleDayNote() -> DayNoteModel {
        let library = InMemoryLibraryStore()
        let ride = RideSummary(
            id: RideID("gallery-day-2"), name: "Ulrichen", date: Date(timeIntervalSince1970: 1_790_000_000),
            distanceMeters: 74_300, trip: RideTrip(key: 7, dayIndex: 1, dayCount: 3, name: "Alps traverse")
        )
        let points = [RidePoint(timestamp: ride.date, coordinate: Coordinate(latitude: 46.6, longitude: 8.6))]
        try? library.saveRide(Ride(summary: ride, points: points))
        return DayNoteModel(ride: ride, points: points, library: library)
    }

    #if os(iOS)
    static let samplePhotos = (0..<6).map {
        RidePhoto(assetID: "p\($0)", takenAt: Date(timeIntervalSince1970: 1_790_000_000 + Double($0) * 1_500))
    }

    /// Sky gradients through a day, rendered once.
    static let sampleThumbnails: [String: Data] = Dictionary(uniqueKeysWithValues: samplePhotos.enumerated().compactMap { index, photo in
        let t = Double(index) / 5
        let sky = LinearGradient(
            colors: [Color(red: 0.95 - 0.2 * t, green: 0.75, blue: 0.55 + 0.3 * t), OBCTheme.water],
            startPoint: .top, endPoint: .bottom
        )
        .frame(width: 160, height: 120)
        return ImageRenderer(content: sky).uiImage?.jpegData(compressionQuality: 0.8).map { (photo.assetID, $0) }
    })
    #endif

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
