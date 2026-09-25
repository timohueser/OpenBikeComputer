import Foundation
import OBCDomain

/// A point on the ridden distance axis. A segment start has no incoming line.
public struct ReplayPoint: Codable, Sendable {
    public let latitude: Double
    public let longitude: Double
    public let elevation: Double?
    public let distance: Double
    public let segmentStart: Bool
}

public struct ReplayDay: Codable, Sendable {
    public let name: String
    public let distance: Double
}

public struct ReplayPhoto: Identifiable, Sendable {
    public let id: String
    public let distance: Double
    public let thumbnailData: Data?
}

/// The renderer's immutable input. Distances exclude recording gaps and trip transfers.
public struct ReplayContent: Sendable {
    public let title: String
    public let points: [ReplayPoint]
    public let totalDistance: Double
    public let durationSeconds: Double
    public let days: [ReplayDay]
    public let photos: [ReplayPhoto]

    public struct Ride: Sendable {
        public let points: [RidePoint]
        public let photos: [RidePhoto]
        public let thumbnails: [String: Data]

        public init(points: [RidePoint], photos: [RidePhoto] = [], thumbnails: [String: Data] = [:]) {
            self.points = points
            self.photos = photos
            self.thumbnails = thumbnails
        }
    }

    /// Keep the entry hidden until at least one drawable recorded segment exists.
    public static func hasUsableGeometry(_ points: [RidePoint]) -> Bool {
        var previous: RidePoint?
        for point in points {
            guard point.coordinate.isValidGeographic else {
                previous = nil
                continue
            }
            if let previous, !point.segmentStart,
               replayDistance(previous.coordinate, point.coordinate) > 0 {
                return true
            }
            previous = point
        }
        return false
    }

    public static func ride(title: String, ride: Ride) -> Self? {
        make(title: title, days: [(name: "", rides: [ride])], showDays: false)
    }

    public static func trip(title: String, days: [(name: String, rides: [Ride])]) -> Self? {
        make(title: title, days: days, showDays: true)
    }

    private static func make(
        title: String, days: [(name: String, rides: [Ride])], showDays: Bool
    ) -> Self? {
        var allPoints: [ReplayPoint] = []
        var replayDays: [ReplayDay] = []
        var replayPhotos: [ReplayPhoto] = []
        var photoIDs = Set<String>()
        var offset = 0.0
        for day in days {
            var dayHasTrack = false
            for ride in day.rides {
                let valid = validPoints(ride.points)
                guard hasUsableGeometry(valid) else { continue }
                let line = MeasuredLine(
                    coordinates: valid.map(\.coordinate),
                    elevations: valid.map(\.elevationMeters),
                    pieceStarts: valid.indices.filter { $0 > 0 && valid[$0].segmentStart },
                    distanceBetween: replayDistance
                )
                guard line.length.isFinite, line.length > 0 else { continue }
                if showDays, !dayHasTrack {
                    replayDays.append(ReplayDay(name: day.name, distance: offset))
                    dayHasTrack = true
                }
                for (index, vertex) in line.vertices.enumerated() {
                    let point = valid[index]
                    allPoints.append(ReplayPoint(
                        latitude: vertex.coordinate.latitude,
                        longitude: vertex.coordinate.longitude,
                        elevation: point.elevationMeters?.isFinite == true ? point.elevationMeters : nil,
                        distance: offset + vertex.distance,
                        segmentStart: index == 0 || line.pieceStarts.contains(index)
                    ))
                }
                for placed in RidePhotoPlacement.place(ride.photos, on: valid, line: line) {
                    guard photoIDs.insert(placed.id).inserted else { continue }
                    replayPhotos.append(ReplayPhoto(
                        id: placed.id, distance: offset + placed.distanceMeters,
                        thumbnailData: ride.thumbnails[placed.id]
                    ))
                }
                offset += line.length
            }
        }
        guard offset.isFinite, offset > 0 else { return nil }
        let duration = showDays ? min(240, max(60, Double(replayDays.count) * 60)) : 60
        return Self(title: title, points: allPoints, totalDistance: offset,
                    durationSeconds: duration, days: replayDays,
                    photos: replayPhotos.sorted { $0.distance < $1.distance })
    }

    /// Dropping an invalid fix also starts a new piece, so no line crosses the missing position.
    private static func validPoints(_ points: [RidePoint]) -> [RidePoint] {
        var valid: [RidePoint] = []
        valid.reserveCapacity(points.count)
        var gap = true
        for point in points {
            guard point.coordinate.isValidGeographic else {
                gap = true
                continue
            }
            valid.append(RidePoint(
                timestamp: point.timestamp, coordinate: point.coordinate,
                elevationMeters: point.elevationMeters?.isFinite == true ? point.elevationMeters : nil,
                heartRate: point.heartRate, cadence: point.cadence, power: point.power,
                segmentStart: gap || point.segmentStart
            ))
            gap = false
        }
        return valid
    }

    private static func replayDistance(_ a: Coordinate, _ b: Coordinate) -> Double {
        abs(a.longitude - b.longitude) > 180 ? a.distance(to: b) : a.routeDistance(to: b)
    }
}
