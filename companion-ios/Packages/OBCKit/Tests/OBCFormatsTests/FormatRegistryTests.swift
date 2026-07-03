import XCTest
import OBCDomain
@testable import OBCFormats

// The format registries route by file extension; stub codecs isolate that
// routing behavior from actual parsing (each real decoder/encoder has its own tests).

private struct StubRouteDecoder: RouteFileDecoder {
    let fileExtensions: Set<String>
    let name: String

    func decode(_ data: Data) throws -> ImportedRoute {
        guard !data.isEmpty else { throw FormatError.malformed(reason: "empty file") }
        return ImportedRoute(name: name, points: [
            RoutePoint(coordinate: Coordinate(latitude: 47.0, longitude: 11.0))
        ])
    }
}

private struct StubRideEncoder: RideFileEncoder {
    let fileExtension: String

    func encode(_ ride: Ride) throws -> Data {
        Data("\(fileExtension):\(ride.summary.name):\(ride.points.count)".utf8)
    }
}

private func makeRide(name: String = "Morning loop") -> Ride {
    let summary = RideSummary(id: RideID("r1"), name: name, date: .distantPast, distanceMeters: 1_000)
    let points = [
        RidePoint(timestamp: .distantPast, coordinate: Coordinate(latitude: 47.0, longitude: 11.0)),
        RidePoint(timestamp: .distantPast + 1, coordinate: Coordinate(latitude: 47.1, longitude: 11.1), elevationMeters: 512),
    ]
    return Ride(summary: summary, points: points)
}

final class RouteImporterTests: XCTestCase {
    private let importer = RouteImporter(decoders: [
        StubRouteDecoder(fileExtensions: ["gpx"], name: "from-gpx"),
        StubRouteDecoder(fileExtensions: ["tcx"], name: "from-tcx"),
    ])

    func testRoutesToDecoderByExtensionCaseInsensitive() throws {
        let route = try importer.importRoute(from: Data([1]), fileExtension: "GPX")
        XCTAssertEqual(route.name, "from-gpx")
        XCTAssertEqual(try importer.importRoute(from: Data([1]), fileExtension: "tcx").name, "from-tcx")
    }

    func testUnsupportedExtensionThrowsH5() {
        XCTAssertThrowsError(try importer.importRoute(from: Data([1]), fileExtension: "fit")) { error in
            XCTAssertEqual(error as? FormatError, .unsupportedFileType(fileExtension: "fit"))
        }
    }

    func testSupportedExtensionsUnionAllDecoders() {
        XCTAssertEqual(importer.supportedFileExtensions, ["gpx", "tcx"])
    }

    func testDecoderErrorsPropagate() {
        XCTAssertThrowsError(try importer.importRoute(from: Data(), fileExtension: "gpx")) { error in
            XCTAssertEqual(error as? FormatError, .malformed(reason: "empty file"))
        }
    }
}

final class RideExporterTests: XCTestCase {
    private let exporter = RideExporter(
        encoders: [StubRideEncoder(fileExtension: "gpx"), StubRideEncoder(fileExtension: "fit")],
        defaultFileExtension: "gpx"
    )

    func testExportsDefaultFormat() throws {
        let file = try exporter.export(makeRide())
        XCTAssertEqual(file.fileExtension, "gpx")
        XCTAssertEqual(String(decoding: file.data, as: UTF8.self), "gpx:Morning loop:2")
    }

    func testExportsExplicitFormat() throws {
        let file = try exporter.export(makeRide(), as: "FIT")
        XCTAssertEqual(file.fileExtension, "fit")
        XCTAssertEqual(String(decoding: file.data, as: UTF8.self), "fit:Morning loop:2")
    }

    func testUnknownFormatThrows() {
        XCTAssertThrowsError(try exporter.export(makeRide(), as: "tcx")) { error in
            XCTAssertEqual(error as? FormatError, .unsupportedFileType(fileExtension: "tcx"))
        }
    }
}
