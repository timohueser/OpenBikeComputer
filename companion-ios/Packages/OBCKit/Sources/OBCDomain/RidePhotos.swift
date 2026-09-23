import Foundation

/// A photo the rider added to a ride: a reference to the photo library, and its place on the
/// ride. The library keeps the photo; the ride keeps the reference and a thumbnail.
public struct RidePhoto: Identifiable, Equatable, Sendable {
    /// The photo library's identifier for the asset.
    public let assetID: String
    public let takenAt: Date
    /// Metres along the ride's `MeasuredLine`.
    public let distanceMeters: Double

    public var id: String { assetID }

    public init(assetID: String, takenAt: Date, distanceMeters: Double) {
        self.assetID = assetID
        self.takenAt = takenAt
        self.distanceMeters = distanceMeters
    }
}

/// A photo from the library, before it is placed on a ride.
public struct PhotoCandidate: Equatable, Sendable {
    public let assetID: String
    public let takenAt: Date
    /// The photo's geotag, when it has one.
    public let location: Coordinate?

    public init(assetID: String, takenAt: Date, location: Coordinate? = nil) {
        self.assetID = assetID
        self.takenAt = takenAt
        self.location = location
    }
}

/// Places library photos on a ride by their geotag and their time.
public enum RidePhotoPlacement {
    /// A photo taken this long before the start or after the end still belongs to the ride.
    public static let margin: TimeInterval = 10 * 60
    /// A geotag this close to the track places the photo on the track.
    public static let geotagReachMeters = 300.0

    public struct Placed: Equatable, Sendable {
        public let photo: RidePhoto
        /// The geotag is far from the track, so the photo is placed by its time.
        public let locationOffTrack: Bool
    }

    /// The times a candidate can have: the ride's first to last point, widened by `margin`.
    public static func window(for points: [RidePoint]) -> ClosedRange<Date>? {
        guard let first = points.first?.timestamp, let last = points.last?.timestamp, first <= last
        else { return nil }
        return first.addingTimeInterval(-margin)...last.addingTimeInterval(margin)
    }

    /// The candidates that belong to the ride, placed and in time order.
    ///
    /// A geotag within reach wins over the time: a camera clock can be wrong, a nearby geotag
    /// rarely is. A far geotag is a wrong location more often than a wrong photo, so a photo
    /// taken during the ride stays, placed by time. In the margin, a far geotag drops the photo.
    public static func place(_ candidates: [PhotoCandidate], on points: [RidePoint]) -> [Placed] {
        guard let window = window(for: points) else { return [] }
        let line = MeasuredLine(ridePoints: points)
        let ride = points[0].timestamp...points[points.count - 1].timestamp
        return candidates
            .filter { window.contains($0.takenAt) }
            .compactMap { candidate -> Placed? in
                let byTime = distance(at: candidate.takenAt, points: points, line: line)
                var distance = byTime
                var offTrack = false
                if let location = candidate.location {
                    // Searched from the time position, so an out-and-back keeps the leg the
                    // photo was taken on.
                    let projection = line.projection(of: location, near: byTime, window: line.length)
                    if projection.error <= geotagReachMeters {
                        distance = projection.distance
                    } else if ride.contains(candidate.takenAt) {
                        offTrack = true
                    } else {
                        return nil
                    }
                }
                let photo = RidePhoto(assetID: candidate.assetID, takenAt: candidate.takenAt, distanceMeters: distance)
                return Placed(photo: photo, locationOffTrack: offTrack)
            }
            .sorted { $0.photo.takenAt < $1.photo.takenAt }
    }

    /// The distance the ride had covered at `time`, interpolated between points and clamped to
    /// the ride. A time in a recording gap stays at the end of the piece before it.
    static func distance(at time: Date, points: [RidePoint], line: MeasuredLine) -> Double {
        guard time > points[0].timestamp else { return 0 }
        guard time < points[points.count - 1].timestamp else { return line.length }
        var low = 0
        var high = points.count - 1
        while high - low > 1 {
            let mid = (low + high) / 2
            if points[mid].timestamp <= time { low = mid } else { high = mid }
        }
        let a = line.vertices[low], b = line.vertices[high]
        let span = points[high].timestamp.timeIntervalSince(points[low].timestamp)
        let t = span > 0 ? time.timeIntervalSince(points[low].timestamp) / span : 0
        return a.distance + (b.distance - a.distance) * t
    }
}

/// A one-time offer on a synced ride. It shows until the rider uses it or dismisses it, and then
/// never again for that ride.
public enum RideQuietRow: String, CaseIterable, Sendable {
    case photos
}

/// What the phone adds to a synced ride. The device copy never has it.
public struct RideJournal: Equatable, Sendable {
    /// In time order.
    public private(set) var photos: [RidePhoto]
    public private(set) var closedRows: Set<RideQuietRow>

    public init(photos: [RidePhoto] = [], closedRows: Set<RideQuietRow> = []) {
        self.photos = photos.sorted { $0.takenAt < $1.takenAt }
        self.closedRows = closedRows
    }

    /// Adds photos in time order. A photo the ride already has keeps its place.
    public mutating func add(_ new: [RidePhoto]) {
        let known = Set(photos.map(\.assetID))
        photos = (photos + new.filter { !known.contains($0.assetID) }).sorted { $0.takenAt < $1.takenAt }
    }

    public mutating func remove(_ assetID: String) {
        photos.removeAll { $0.assetID == assetID }
    }

    public mutating func close(_ row: RideQuietRow) {
        closedRows.insert(row)
    }
}
