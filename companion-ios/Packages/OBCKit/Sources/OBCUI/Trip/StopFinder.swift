import Foundation
import Observation
import OBCDomain

/// Campsites and hotels near points of a trip line, from Apple Maps. It asks once per
/// ``bucketMeters`` of line and keeps each answer for the app session, so a day end moved back
/// and forth costs no new request. Apple throttles searches per device.
///
/// Ask when a day end is tapped or a handle is released, never while a handle moves.
@MainActor @Observable
public final class StopFinder {
    /// One request covers this much of the line.
    public static let bucketMeters = 1_000.0
    /// The search radius around the middle of a bucket. No point of a bucket is more than half a
    /// bucket from its middle, so one answer holds every stop within 2.5 km of any point of it.
    public static let radiusMeters = 3_000.0

    @ObservationIgnored private let search: any StopSearch
    /// One request per bucket middle, in flight or answered. A failed one is removed, so the
    /// next ask tries again.
    @ObservationIgnored private var requests: [Coordinate: Task<[Stop], any Error>] = [:]
    /// Every stop an answer held: the pins near the line.
    public private(set) var known: Set<Stop> = []

    public init(search: any StopSearch) {
        self.search = search
    }

    /// Campsites and hotels near the point `distance` metres along `line`.
    public func stops(near distance: Double, on line: MeasuredLine) async throws -> [Stop] {
        guard line.vertices.count > 1 else { return [] }
        let bucket = (min(max(distance, 0), line.length) / Self.bucketMeters).rounded(.down)
        let center = line.coordinate(at: min((bucket + 0.5) * Self.bucketMeters, line.length))
        let request = requests[center] ?? Task { [search] in
            let stops = try await search.stops(near: center, radius: Self.radiusMeters)
            self.known.formUnion(stops)
            return stops
        }
        requests[center] = request
        do {
            return try await request.value
        } catch {
            requests[center] = nil
            throw error
        }
    }

    /// Stops near several points, asked one after the other, in the order of `distances`.
    public func stops(near distances: [Double], on line: MeasuredLine) async throws -> [[Stop]] {
        var answers: [[Stop]] = []
        for distance in distances { answers.append(try await stops(near: distance, on: line)) }
        return answers
    }

    /// Places of any kind that match `query`, in the box around `line`. Not kept: a typed search
    /// is rare.
    public func places(matching query: String, along line: MeasuredLine) async throws -> [Stop] {
        let coordinates = line.vertices.map(\.coordinate)
        guard let south = coordinates.map(\.latitude).min(), let north = coordinates.map(\.latitude).max(),
            let west = coordinates.map(\.longitude).min(), let east = coordinates.map(\.longitude).max()
        else { return [] }
        return try await search.places(
            matching: query,
            southWest: Coordinate(latitude: south, longitude: west),
            northEast: Coordinate(latitude: north, longitude: east))
    }
}
