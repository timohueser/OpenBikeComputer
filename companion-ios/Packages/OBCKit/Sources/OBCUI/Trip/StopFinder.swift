import Foundation
import OBCDomain

/// Campsites and hotels near points of a trip line, from Apple Maps. One answer serves every
/// later ask within ``reuseMeters`` of the point it was asked for, for the app session, so a day
/// end moved back and forth costs no new request. Apple throttles searches per device.
///
/// Ask when a day end is tapped or a handle is released, never while a handle moves.
@MainActor
public final class StopFinder {
    /// A day end lists the stops this far from it.
    public static let radiusMeters = 5_000.0
    /// An answer serves every later ask this close to the point it was asked for. The distance is
    /// straight, not along the line, so the two legs of an out-and-back share an answer and the
    /// two sides of a transfer do not.
    static let reuseMeters = 1_000.0
    /// The search radius around the asked point, so that one answer holds every stop within
    /// ``radiusMeters`` of any point it serves.
    static let requestRadiusMeters = radiusMeters + reuseMeters

    private let search: any StopSearch
    /// The asked points and their requests, in flight or answered. A failed one is removed, so
    /// the next ask tries again.
    private var requests: [(point: Coordinate, task: Task<[Stop], any Error>)] = []

    public init(search: any StopSearch) {
        self.search = search
    }

    /// Campsites and hotels within ``radiusMeters`` of the point `distance` metres along `line`.
    public func stops(near distance: Double, on line: MeasuredLine) async throws -> [Stop] {
        guard line.vertices.count > 1 else { return [] }
        let point = line.coordinate(at: min(max(distance, 0), line.length))
        let request: Task<[Stop], any Error>
        if let known = requests.first(where: { $0.point.distance(to: point) <= Self.reuseMeters }) {
            request = known.task
        } else {
            request = Task { [search] in try await search.stops(near: point, radius: Self.requestRadiusMeters) }
            requests.append((point, request))
        }
        do {
            return try await request.value.filter { $0.coordinate.distance(to: point) <= Self.radiusMeters }
        } catch {
            requests.removeAll { $0.task == request }
            throw error
        }
    }

    /// Stops near several points, asked one after the other, in the order of `distances`: the
    /// candidates of the day editor's auto split.
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
