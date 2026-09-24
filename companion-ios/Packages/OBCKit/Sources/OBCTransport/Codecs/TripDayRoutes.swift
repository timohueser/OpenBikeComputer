import Foundation
import OBCDomain

/// One day of a trip as an upload sends it: the day's cut of the line, encoded as an OBCR route
/// with the trip's bike type. Distance and climb come from the encoded header, so the phone shows
/// the figures and the estimate the device shows for the same day route.
public struct TripDayRoute: Equatable, Sendable {
    public let day: Int
    /// The OBCR name: the day's own name, or "Day 2 Ulrichen"; at most 48 bytes.
    public let name: String
    public let points: [RoutePoint]
    public let payload: Data
    public let crc32: UInt32
    public let distanceMeters: Double
    public let elevationGainMeters: Double
    public let estimatedDuration: TimeInterval
    /// Metres along the route where it joins the trip's main line: past the spur back from a
    /// stop, else 0.
    public let joinMeters: UInt32
    /// Metres along the route where it leaves the main line for a spur to a stop; `UInt32.max`
    /// for a day that ends on the line.
    public let leaveMeters: UInt32
}

extension TripDayRoute {
    /// The day as a route summary: the transfer blob and the list rows read it.
    public func summary(tripID: TripID) -> RouteSummary {
        RouteSummary(
            id: RouteID("\(tripID.rawValue)/day-\(day)"),
            name: name,
            distanceMeters: distanceMeters,
            elevationGainMeters: elevationGainMeters,
            estimatedDuration: estimatedDuration,
            pointCount: points.count,
            trackPreview: TrackPreview.normalizing(points.map(\.coordinate)))
    }

    /// The day as the route detail shows it, with the profile and the steepest grade of its cut.
    public func detail(tripID: TripID) -> RouteDetail {
        let stats = RouteStats.compute(from: points)
        return RouteDetail(
            summary: summary(tripID: tripID), elevationProfile: stats.elevationProfile,
            maxGradePercent: stats.maxGradePercent)
    }
}

extension Trip {
    /// The day routes an upload of this trip sends, in ride order. A re-cut that leaves a day's
    /// bytes unchanged leaves its CRC unchanged, so the upload skips that day.
    public func dayRoutes() -> [TripDayRoute] {
        dayLines().enumerated().map { day, cut in
            let points = cut.points
            let name = Self.routeName(day: day, title: dayEnds[day].title, place: dayEnds[day].name)
            let payload = RouteObjectCodec.encode(points: points, waypoints: [], name: name, bikeType: bikeType)
            let totals = RouteObjectCodec.totals(of: payload)
            let distance = Double(totals?.distanceMeters ?? 0)
            let ascent = Double(totals?.ascentMeters ?? 0)
            // The route's own metres, as the device measures them along the decoded points.
            let along = MeasuredLine(coordinates: points.map(\.coordinate)).vertices
            return TripDayRoute(
                day: day, name: name, points: points, payload: payload, crc32: CRC32.checksum(payload),
                distanceMeters: distance, elevationGainMeters: ascent,
                estimatedDuration: bikeType.estimatedDuration(distanceMeters: distance, ascentMeters: ascent),
                joinMeters: UInt32(along[cut.joinIndex].distance.rounded()),
                leaveMeters: cut.leaveIndex.map { UInt32(along[$0].distance.rounded()) } ?? .max)
        }
    }

    /// The day's own name, or "Day N ‹place›", within the OBCR name cap. The cap cuts on a
    /// character boundary, and never cuts the "Day N".
    static func routeName(day: Int, title: String?, place: String?) -> String {
        if let title, !title.isEmpty { return capped("", title) }
        let number = "Day \(day + 1)"
        guard let place, !place.isEmpty else { return number }
        return capped(number + " ", place)
    }

    private static func capped(_ prefix: String, _ text: String) -> String {
        var name = prefix
        for character in text {
            guard name.utf8.count + String(character).utf8.count <= RouteObjectCodec.nameCap else { break }
            name.append(character)
        }
        return name.trimmingCharacters(in: .whitespaces)
    }

    /// The trip object an upload writes after `days`, naming each day by its device object id.
    public func tripObject(days: [TripDayRoute], dayObjectIDs: [DeviceObjectID]) -> TripObjectCodec.Trip {
        TripObjectCodec.Trip(
            key: key, name: name,
            startDate: UInt16(clamping: max(startDay?.daysSince1970 ?? 0, 0)),
            days: zip(days, dayObjectIDs).map { day, id in
                TripObjectCodec.Day(routeID: id, joinMeters: day.joinMeters, leaveMeters: day.leaveMeters)
            })
    }
}

extension TripStats {
    /// The totals of a trip's day routes.
    public init(days: [TripDayRoute]) {
        self.init(
            distanceMeters: days.reduce(0) { $0 + $1.distanceMeters },
            elevationGainMeters: days.reduce(0) { $0 + $1.elevationGainMeters },
            dayCount: days.count)
    }
}
