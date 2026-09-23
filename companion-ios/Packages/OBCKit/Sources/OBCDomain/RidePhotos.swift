import Foundation

/// A photo the rider added to a ride: a reference to the photo library and the time it was
/// taken. Its place on the ride comes from the ride's current points, so it follows a trim or a
/// split.
public struct RidePhoto: Identifiable, Equatable, Sendable {
    /// The photo library's identifier for the asset.
    public let assetID: String
    public let takenAt: Date

    public var id: String { assetID }

    public init(assetID: String, takenAt: Date) {
        self.assetID = assetID
        self.takenAt = takenAt
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

/// Places photos on a ride by the time they were taken.
///
/// The time decides the place, because a geotag cannot tell the legs of an out-and-back apart.
/// The geotag only marks a photo whose location is far from that place.
public enum RidePhotoPlacement {
    /// A geotag farther than this from the photo's place on the ride is "off the track".
    public static let offTrackMeters = 300.0
    /// A photo taken this long before the start or after the end sits at the start or the end.
    public static let margin: TimeInterval = 10 * 60

    public struct Placed: Identifiable, Equatable, Sendable {
        public let photo: RidePhoto
        /// Metres along the ride's `MeasuredLine`.
        public let distanceMeters: Double
        public let coordinate: Coordinate
        public let locationOffTrack: Bool

        public var id: String { photo.assetID }

        public init(photo: RidePhoto, distanceMeters: Double, coordinate: Coordinate, locationOffTrack: Bool) {
            self.photo = photo
            self.distanceMeters = distanceMeters
            self.coordinate = coordinate
            self.locationOffTrack = locationOffTrack
        }
    }

    /// The times a photo can have: the ride's first point to its last, widened by `margin`.
    public static func window(of points: [RidePoint]) -> ClosedRange<Date>? {
        guard let first = points.first?.timestamp, let last = points.last?.timestamp, first <= last
        else { return nil }
        return first.addingTimeInterval(-margin)...last.addingTimeInterval(margin)
    }

    /// The candidates taken in the ride's window, placed and in time order. `line` is the ride's
    /// `MeasuredLine`; each photo costs one binary search.
    public static func place(_ candidates: [PhotoCandidate], on points: [RidePoint], line: MeasuredLine) -> [Placed] {
        candidates
            .compactMap { candidate -> Placed? in
                guard let (distance, coordinate) = position(at: candidate.takenAt, points: points, line: line)
                else { return nil }
                let offTrack = candidate.location.map { $0.distance(to: coordinate) > offTrackMeters } ?? false
                return Placed(
                    photo: RidePhoto(assetID: candidate.assetID, takenAt: candidate.takenAt),
                    distanceMeters: distance, coordinate: coordinate, locationOffTrack: offTrack
                )
            }
            .sorted { $0.photo.takenAt < $1.photo.takenAt }
    }

    /// The ride's added photos on its current points. A photo outside their window drops out.
    public static func place(_ photos: [RidePhoto], on points: [RidePoint], line: MeasuredLine) -> [Placed] {
        place(photos.map { PhotoCandidate(assetID: $0.assetID, takenAt: $0.takenAt) }, on: points, line: line)
    }

    /// Where the rider was at `time`, interpolated between points; nil outside `window`. A time in
    /// a recording pause is where the rider stopped, and a time in the margin is the start or the end.
    static func position(at time: Date, points: [RidePoint], line: MeasuredLine) -> (Double, Coordinate)? {
        guard let window = window(of: points), window.contains(time) else { return nil }
        if time <= points[0].timestamp { return (0, points[0].coordinate) }
        if time >= points[points.count - 1].timestamp { return (line.length, points[points.count - 1].coordinate) }
        var low = 0
        var high = points.count - 1
        while high - low > 1 {
            let mid = (low + high) / 2
            if points[mid].timestamp <= time { low = mid } else { high = mid }
        }
        let a = points[low], b = points[high]
        let interval = b.timestamp.timeIntervalSince(a.timestamp)
        guard !b.segmentStart, interval > 0 else {
            return time < b.timestamp ? (line.vertices[low].distance, a.coordinate) : (line.vertices[high].distance, b.coordinate)
        }
        let t = time.timeIntervalSince(a.timestamp) / interval
        let coordinate = Coordinate(
            latitude: a.coordinate.latitude + (b.coordinate.latitude - a.coordinate.latitude) * t,
            longitude: a.coordinate.longitude + (b.coordinate.longitude - a.coordinate.longitude) * t
        )
        let distance = line.vertices[low].distance + (line.vertices[high].distance - line.vertices[low].distance) * t
        return (distance, coordinate)
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
