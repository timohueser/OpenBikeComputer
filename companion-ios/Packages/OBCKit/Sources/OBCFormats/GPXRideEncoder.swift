import Foundation
import OBCDomain

/// Exports a tracked ride to **GPX 1.1** — the app-side mirror of the firmware's
/// on-device `track_to_gpx` (`obc-route/src/track.rs`), down to the sensor
/// extensions (epic #707, SE3/SE4): the `gpxtpx` namespace on the root `<gpx>`,
/// and per point a `gpxtpx:TrackPointExtension` carrying `<gpxtpx:hr>` / `<gpxtpx:cad>`
/// plus a bare `<power>` (the de-facto Strava form). Each element is omitted when
/// its field is absent; the whole `<extensions>` block when all three are — so a
/// sensor-less ride (a v1 download) produces exactly the plain track a
/// pre-sensor export did (no regression).
///
/// The app export encodes from the canonical `Ride` (decoded from the ride
/// object), so — unlike the device, which reads the raw track log — a point may
/// carry no elevation (`nil`): `<ele>` is omitted then, never a sentinel. The
/// ride object carries no segment breaks, so the track is one `<trkseg>`.
public struct GPXRideEncoder: RideFileEncoder {
    public let fileExtension = "gpx"

    public init() {}

    public func encode(_ ride: Ride) throws -> Data {
        var xml = ""
        xml += "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"
        xml += "<gpx version=\"1.1\" creator=\"OpenBikeComputer\""
        xml += " xmlns=\"http://www.topografix.com/GPX/1/1\""
        xml += " xmlns:gpxtpx=\"http://www.garmin.com/xmlschemas/TrackPointExtension/v1\">\n"
        xml += "<trk><name>\(Self.escaped(ride.summary.name))</name>\n"
        xml += "<trkseg>\n"

        for point in ride.points {
            xml += "<trkpt lat=\"\(Self.degrees(point.coordinate.latitude))\""
            xml += " lon=\"\(Self.degrees(point.coordinate.longitude))\">"
            if let ele = point.elevationMeters {
                xml += "<ele>\(Self.ele(ele))</ele>"
            }
            xml += Self.extensions(point)
            xml += "</trkpt>\n"
        }

        xml += "</trkseg>\n"
        xml += "</trk>\n</gpx>\n"
        return Data(xml.utf8)
    }

    /// The per-point `<extensions>` block, or `""` when the point carries no
    /// sensor sample. `hr`/`cad` nest inside a `TrackPointExtension`; `power` is
    /// a bare sibling. The wrapper itself is dropped when both `hr` and `cad`
    /// are absent (power-only points), matching the firmware.
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

    /// Fixed 7-decimal degrees via exact integer math on the ride object's
    /// `1e-7°` grid — no float-formatting drift (mirrors the firmware's
    /// integer-exact `write_deg`, at the ride object's finer precision).
    private static func degrees(_ value: Double) -> String {
        let scaled = Int64((value * 1e7).rounded())
        let sign = scaled < 0 ? "-" : ""
        let magnitude = scaled.magnitude
        let whole = magnitude / 10_000_000
        let fracDigits = String(magnitude % 10_000_000)
        let frac = String(repeating: "0", count: 7 - fracDigits.count) + fracDigits
        return "\(sign)\(whole).\(frac)"
    }

    /// Elevation to the whole metre — the ride object's own quantum.
    private static func ele(_ value: Double) -> String {
        String(Int(value.rounded()))
    }

    /// Minimal XML escaping for the track name — the same three entities the
    /// firmware escapes.
    private static func escaped(_ text: String) -> String {
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
