import Foundation
import OBCDomain

/// Exports a trip's planned line to GPX 1.1: one track named after the trip, with one segment
/// per day in ride order. GPX gives a segment no name, so a day is only its segment. A planned
/// point has no time, and `<ele>` is omitted where the line has no elevation.
public enum GPXTripEncoder {
    public static func encode(_ trip: Trip) -> Data {
        var xml = ""
        xml += "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"
        xml += "<gpx version=\"1.1\" creator=\"OpenBikeComputer\" xmlns=\"http://www.topografix.com/GPX/1/1\">\n"
        xml += "<trk><name>\(GPXRideEncoder.escaped(trip.name))</name>\n"
        for day in trip.dayLines() {
            xml += "<trkseg>\n"
            for point in day {
                xml += "<trkpt lat=\"\(GPXRideEncoder.degrees(point.coordinate.latitude))\""
                xml += " lon=\"\(GPXRideEncoder.degrees(point.coordinate.longitude))\">"
                if let ele = point.elevationMeters {
                    xml += "<ele>\(GPXRideEncoder.ele(ele))</ele>"
                }
                xml += "</trkpt>\n"
            }
            xml += "</trkseg>\n"
        }
        xml += "</trk>\n</gpx>\n"
        return Data(xml.utf8)
    }
}
