import Foundation
import Testing
import OBCDomain
@testable import OBCFormats

/// The ride → GPX encoder (SE4 #711): a Strava-shaped export whose sensor
/// extensions mirror the firmware's on-device `track_to_gpx` (epic #707, SE3) —
/// the `gpxtpx` namespace on the root, a per-point `gpxtpx:TrackPointExtension`
/// (`hr`/`cad`) plus a bare `<power>`, each element omitted when absent and the
/// whole block omitted when all three are. Segment flags and the v3 microdegree grid are retained.
struct GPXRideEncoderTests {
    private let encoder = GPXRideEncoder()

    private func point(
        _ latE6: Int, _ lonE6: Int, ele: Double?, segmentStart: Bool = false,
        hr: Int? = nil, cad: Int? = nil, pwr: Int? = nil
    ) -> RidePoint {
        RidePoint(
            timestamp: .distantPast,
            coordinate: Coordinate(latitude: Double(latE6) / 1e6, longitude: Double(lonE6) / 1e6),
            elevationMeters: ele, heartRate: hr, cadence: cad, power: pwr, segmentStart: segmentStart)
    }

    private func ride(name: String, points: [RidePoint]) -> Ride {
        Ride(
            summary: RideSummary(id: RideID("r"), name: name, date: .distantPast,
                                 distanceMeters: 1_000),
            points: points)
    }

    @Test func emitsStravaShapedSensorExtensions() throws {
        // One of each extension shape: all-present, none, power-only (no
        // TrackPointExtension wrapper), hr-only. `&` in the name exercises escaping.
        let mixed = ride(name: "Feierabend & Sensors", points: [
            point(48_000_000, 7_800_000, ele: 214, hr: 140, cad: 84, pwr: 205),
            point(48_001_000, 7_801_200, ele: nil),
            point(48_002_000, 7_803_000, ele: 219, pwr: 215),
            point(48_003_000, 7_804_000, ele: 220, segmentStart: true, hr: 150),
        ])

        let expected = """
            <?xml version="1.0" encoding="UTF-8"?>
            <gpx version="1.1" creator="OpenBikeComputer" xmlns="http://www.topografix.com/GPX/1/1" xmlns:gpxtpx="http://www.garmin.com/xmlschemas/TrackPointExtension/v1">
            <trk><name>Feierabend &amp; Sensors</name>
            <trkseg>
            <trkpt lat="48.000000" lon="7.800000"><ele>214</ele><extensions><gpxtpx:TrackPointExtension><gpxtpx:hr>140</gpxtpx:hr><gpxtpx:cad>84</gpxtpx:cad></gpxtpx:TrackPointExtension><power>205</power></extensions></trkpt>
            <trkpt lat="48.001000" lon="7.801200"></trkpt>
            <trkpt lat="48.002000" lon="7.803000"><ele>219</ele><extensions><power>215</power></extensions></trkpt>
            </trkseg>
            <trkseg>
            <trkpt lat="48.003000" lon="7.804000"><ele>220</ele><extensions><gpxtpx:TrackPointExtension><gpxtpx:hr>150</gpxtpx:hr></gpxtpx:TrackPointExtension></extensions></trkpt>
            </trkseg>
            </trk>
            </gpx>

            """  // trailing newline: the file ends "</gpx>\n"

        #expect(String(decoding: try encoder.encode(mixed), as: UTF8.self) == expected)
    }

    @Test func sensorlessRideHasNoExtensionBlocks() throws {
        let plain = ride(name: "Plain Ride", points: [
            point(47_000_000, 11_000_000, ele: 500),
            point(47_005_000, 11_001_000, ele: nil),
        ])

        let expected = """
            <?xml version="1.0" encoding="UTF-8"?>
            <gpx version="1.1" creator="OpenBikeComputer" xmlns="http://www.topografix.com/GPX/1/1" xmlns:gpxtpx="http://www.garmin.com/xmlschemas/TrackPointExtension/v1">
            <trk><name>Plain Ride</name>
            <trkseg>
            <trkpt lat="47.000000" lon="11.000000"><ele>500</ele></trkpt>
            <trkpt lat="47.005000" lon="11.001000"></trkpt>
            </trkseg>
            </trk>
            </gpx>

            """

        let gpx = String(decoding: try encoder.encode(plain), as: UTF8.self)
        #expect(gpx == expected)
        // The defining "no regression" property: not a single per-point
        // extension is emitted for a ride that carries no sensor data.
        #expect(!gpx.contains("<extensions>"))
        #expect(!gpx.contains("gpxtpx:"))
    }

    @Test func registersThroughTheExporter() throws {
        // The B7 seam: the encoder plugs into `RideExporter` by extension.
        let exporter = RideExporter(encoders: [GPXRideEncoder()], defaultFileExtension: "gpx")
        let file = try exporter.export(ride(name: "Loop", points: [
            point(47_000_000, 11_000_000, ele: 500, hr: 120),
        ]))
        #expect(file.fileExtension == "gpx")
        let gpx = String(decoding: file.data, as: UTF8.self)
        #expect(gpx.contains("<gpxtpx:hr>120</gpxtpx:hr>"))
    }
}
