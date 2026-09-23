import Foundation
import OBCDomain

/// Exports a tracked ride to GPX 1.1, the app-side mirror of the firmware's
/// `track_to_gpx` (`obc-route/src/track.rs`), down to the sensor extensions: the
/// `gpxtpx` namespace on the root `<gpx>`, and per point a
/// `gpxtpx:TrackPointExtension` carrying `<gpxtpx:hr>` and `<gpxtpx:cad>` plus a bare
/// `<power>`, the de-facto Strava form. Each element is omitted when its field is
/// absent, and the whole `<extensions>` block when all three are.
///
/// It encodes from the canonical `Ride`, so a point may carry no elevation; `<ele>`
/// is then omitted, never a sentinel. The recorded segment-start flag is preserved,
/// so exports keep their pause boundaries. Every point carries its ISO 8601 UTC
/// `<time>`: Strava and Garmin Connect refuse an activity without times, or read it
/// as a route.
public struct GPXRideEncoder: RideFileEncoder {
    public let fileExtension = "gpx"

    public init() {}

    public func encode(_ ride: Ride) throws -> Data {
        var xml = ""
        xml += "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"
        xml += "<gpx version=\"1.1\" creator=\"OpenBikeComputer\""
        xml += " xmlns=\"http://www.topografix.com/GPX/1/1\""
        xml += " xmlns:gpxtpx=\"http://www.garmin.com/xmlschemas/TrackPointExtension/v1\">\n"
        xml += "<metadata><time>\(Self.time(ride.points.first?.timestamp ?? ride.summary.date))</time></metadata>\n"
        xml += "<trk><name>\(Self.escaped(ride.summary.name))</name>\n"
        xml += "<trkseg>\n"

        for (index, point) in ride.points.enumerated() {
            if index > 0 && point.segmentStart {
                xml += "</trkseg>\n<trkseg>\n"
            }
            xml += "<trkpt lat=\"\(Self.degrees(point.coordinate.latitude))\""
            xml += " lon=\"\(Self.degrees(point.coordinate.longitude))\">"
            if let ele = point.elevationMeters {
                xml += "<ele>\(Self.ele(ele))</ele>"
            }
            xml += "<time>\(Self.time(point.timestamp))</time>"
            xml += Self.extensions(point)
            xml += "</trkpt>\n"
        }

        xml += "</trkseg>\n"
        xml += "</trk>\n</gpx>\n"
        return Data(xml.utf8)
    }

    /// The per-point `<extensions>` block, or `""` when the point carries no sensor
    /// sample. `hr` and `cad` nest inside a `TrackPointExtension`; `power` is a bare
    /// sibling. The wrapper is dropped for power-only points, matching the firmware.
    private static func extensions(_ point: RidePoint) -> String {
        guard point.heartRate != nil || point.cadence != nil || point.power != nil else { return "" }
        var block = "<extensions>"
        if point.heartRate != nil || point.cadence != nil {
            block += "<gpxtpx:TrackPointExtension>"
            if let hr = point.heartRate { block += "<gpxtpx:hr>\(hr)</gpxtpx:hr>" }
            if let cad = point.cadence { block += "<gpxtpx:cad>\(cad)</gpxtpx:cad>" }
            block += "</gpxtpx:TrackPointExtension>"
        }
        if let power = point.power { block += "<power>\(power)</power>" }
        block += "</extensions>"
        return block
    }

    /// Fixed 6-decimal degrees on the v3 sample's microdegree grid, matching firmware.
    static func degrees(_ value: Double) -> String {
        let scaled = Int64((value * 1e6).rounded())
        let sign = scaled < 0 ? "-" : ""
        let magnitude = scaled.magnitude
        let whole = magnitude / 1_000_000
        let fracDigits = String(magnitude % 1_000_000)
        let frac = String(repeating: "0", count: 6 - fracDigits.count) + fracDigits
        return "\(sign)\(whole).\(frac)"
    }

    /// "2026-09-11T08:00:00Z". The formatter is UTC by default.
    private static func time(_ date: Date) -> String {
        date.formatted(.iso8601)
    }

    /// Elevation to the whole metre, the ride object's own quantum.
    static func ele(_ value: Double) -> String {
        String(Int(value.rounded()))
    }

    /// Minimal XML escaping for the track name: the same three entities the firmware
    /// escapes.
    static func escaped(_ text: String) -> String {
        var out = ""
        out.reserveCapacity(text.count)
        for ch in text {
            switch ch {
            case "&": out += "&amp;"
            case "<": out += "&lt;"
            case ">": out += "&gt;"
            default: out.append(ch)
            }
        }
        return out
    }
}
