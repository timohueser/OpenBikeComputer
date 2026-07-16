import Foundation
import Testing
import OBCDomain
@testable import OBCFormats

/// The ride → GPX encoder (SE4 #711): a Strava-shaped export whose sensor
/// extensions mirror the firmware's on-device `track_to_gpx` (epic #707, SE3) —
/// the `gpxtpx` namespace on the root, a per-point `gpxtpx:TrackPointExtension`
/// (`hr`/`cad`) plus a bare `<power>`, each element omitted when absent and the
/// whole block omitted when all three are. A sensor-less ride (a v1 download)
/// exports the plain track a pre-sensor build did — no regression.
struct GPXRideEncoderTests {
    private let encoder = GPXRideEncoder()

    private func point(
        _ latE7: Int, _ lonE7: Int, ele: Double?,
        hr: Int? = nil, cad: Int? = nil, pwr: Int? = nil
    ) -> RidePoint {
        RidePoint(
            timestamp: .distantPast,
            coordinate: Coordinate(latitude: Double(latE7) / 1e7, longitude: Double(lonE7) / 1e7),
            elevationMeters: ele, heartRate: hr, cadence: cad, power: pwr)
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
            point(480_000_000, 78_000_000, ele: 214, hr: 140, cad: 84, pwr: 205),
            point(480_010_000, 78_012_000, ele: nil),
            point(480_020_000, 78_030_000, ele: 219, pwr: 215),
            point(480_030_000, 78_040_000, ele: 220, hr: 150),
        ])

        let expected = """
            <?xml version="1.0" encoding="UTF-8"?>
            <gpx version="1.1" creator="OpenBikeComputer" xmlns="http://www.topografix.com/GPX/1/1" xmlns:gpxtpx="http://www.garmin.com/xmlschemas/TrackPointExtension/v1">
            <trk><name>Feierabend &amp; Sensors</name>
            <trkseg>
            <trkpt lat="48.0000000" lon="7.8000000"><ele>214</ele><extensions><gpxtpx:TrackPointExtension><gpxtpx:hr>140</gpxtpx:hr><gpxtpx:cad>84</gpxtpx:cad></gpxtpx:TrackPointExtension><power>205</power></extensions></trkpt>
            <trkpt lat="48.0010000" lon="7.8012000"></trkpt>
            <trkpt lat="48.0020000" lon="7.8030000"><ele>219</ele><extensions><power>215</power></extensions></trkpt>
            <trkpt lat="48.0030000" lon="7.8040000"><ele>220</ele><extensions><gpxtpx:TrackPointExtension><gpxtpx:hr>150</gpxtpx:hr></gpxtpx:TrackPointExtension></extensions></trkpt>
            </trkseg>
            </trk>
            </gpx>

            """  // trailing newline: the file ends "</gpx>\n"

        #expect(String(decoding: try encoder.encode(mixed), as: UTF8.self) == expected)
    }

    @Test func sensorlessRideHasNoExtensionBlocks() throws {
        let plain = ride(name: "Plain Ride", points: [
            point(470_000_000, 110_000_000, ele: 500),
            point(470_050_000, 110_010_000, ele: nil),
        ])

        let expected = """
            <?xml version="1.0" encoding="UTF-8"?>
            <gpx version="1.1" creator="OpenBikeComputer" xmlns="http://www.topografix.com/GPX/1/1" xmlns:gpxtpx="http://www.garmin.com/xmlschemas/TrackPointExtension/v1">
            <trk><name>Plain Ride</name>
            <trkseg>
            <trkpt lat="47.0000000" lon="11.0000000"><ele>500</ele></trkpt>
            <trkpt lat="47.0050000" lon="11.0010000"></trkpt>
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
            point(470_000_000, 110_000_000, ele: 500, hr: 120),
        ]))
        #expect(file.fileExtension == "gpx")
        let gpx = String(decoding: file.data, as: UTF8.self)
        #expect(gpx.contains("<gpxtpx:hr>120</gpxtpx:hr>"))
    }
}
