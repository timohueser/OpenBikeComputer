import Foundation
import Testing
import OBCDomain
import OBCTransport
@testable import OBCUI

/// Save as route and the shared file name, the app half. The ride-to-route transform itself is
/// pinned in `RideToRouteTests`.
@MainActor @Suite struct RideShareModelTests {
    private static let ride = Ride(
        summary: RideSummary(
            id: RideID("r"), name: "Lunch / Loop", date: Date(), distanceMeters: 1_112, bikeType: .gravel
        ),
        points: [
            RidePoint(timestamp: Date(), coordinate: Coordinate(latitude: 48.00, longitude: 8.0), elevationMeters: 100),
            RidePoint(timestamp: Date(), coordinate: Coordinate(latitude: 48.01, longitude: 8.0), elevationMeters: 200),
        ]
    )

    private static func flow(_ library: any LibraryStore) -> ImportFlowModel {
        ImportFlowModel(decode: { _, _ in throw CocoaError(.fileReadCorruptFile) }, library: library, isBonded: { true })
    }

    private static func open(_ flow: ImportFlowModel) throws {
        flow.open(
            route: try #require(ride.plannedRoute()),
            fileName: GPXFile.fileName(for: ride.summary.name),
            fileData: Data(),
            source: .ride(ride.summary.date),
            bikeType: ride.summary.bikeType
        )
    }

    @Test func saveAsRouteOpensTheImportLanding() throws {
        let flow = Self.flow(InMemoryLibraryStore())

        try Self.open(flow)

        #expect(flow.pendingImport?.route.name == "Lunch / Loop")
        #expect(flow.pendingImport?.fileName == "Lunch - Loop.gpx")
        #expect(flow.pendingImport?.source == .ride(Self.ride.summary.date))
        #expect(flow.pendingImport?.bikeType == .gravel)
        #expect(flow.collision == nil)
    }

    /// The second save of one ride meets the first: the import's update-or-add dialog, never a
    /// silent duplicate.
    @Test func saveAsRouteTakesTheImportNameCollisionRule() throws {
        let library = InMemoryLibraryStore()
        let saved = try #require(Self.ride.plannedRoute())
        library.savePlannedRoute(PlannedRouteRecord(
            summary: RouteSummary(id: RouteID("saved"), name: "lunch / loop", distanceMeters: 1_112, elevationGainMeters: 100),
            route: saved, sourceFileName: "Lunch - Loop.gpx", sourceFileData: Data()
        ))
        let flow = Self.flow(library)

        try Self.open(flow)

        #expect(flow.pendingImport == nil)
        #expect(flow.collision?.existing.id == RouteID("saved"))
    }

    @Test func gpxFileNameIsCleanedForTheFileSystem() {
        #expect(GPXFile.fileName(for: "Furka: day 2/3") == "Furka- day 2-3.gpx")
        #expect(GPXFile.fileName(for: " .hidden ") == "hidden.gpx")
        #expect(GPXFile.fileName(for: "  ") == "Ride.gpx")
    }

    /// A ride name has no length limit, but a file name has 255 bytes. The cut keeps whole
    /// characters, here four-byte emoji.
    @Test func gpxFileNameIsCutToTheFileSystemLimit() {
        let long = GPXFile.fileName(for: String(repeating: "a", count: 300))
        #expect(long == String(repeating: "a", count: GPXFile.maxBaseBytes) + ".gpx")

        let emoji = GPXFile.fileName(for: String(repeating: "\u{1F6B2}", count: 300))
        #expect(emoji.hasSuffix(".gpx"))
        #expect(emoji.utf8.count <= GPXFile.maxBaseBytes + 4)
        #expect(emoji.dropLast(4).allSatisfy { $0 == "\u{1F6B2}" })
    }
}
