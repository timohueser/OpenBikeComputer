import Foundation

/// Statistics derived from a parsed route's geometry: `[RoutePoint]` in, numbers out.
/// Device-stored routes do not come through here. Their stats ride in `RouteSummary` and
/// `RouteDetail` as the device or fixture reports them.
public struct RouteStats: Equatable, Sendable {
    public var distanceMeters: Double
    public var elevationGainMeters: Double
    /// Total descent in metres (same hysteresis as the gain).
    public var elevationLossMeters: Double
    /// Elevation samples for the profile card, downsampled to `profileSampleCount`.
    public var elevationProfile: [Double]
    /// Steepest sustained climb over a ~100 m window, in percent. `nil` when the
    /// source carried no elevation.
    public var maxGradePercent: Double?

    /// Elevation-noise hysteresis: climb only accumulates once the track has
    /// risen this far above its last confirmed elevation.
    public static let climbHysteresisMeters = 3.0
    /// Grades are measured over windows at least this long, so a single noisy sample cannot
    /// spike the MAX stat.
    public static let gradeWindowMeters = 100.0

    public static func compute(from points: [RoutePoint], profileSampleCount: Int = 64) -> RouteStats {
        var cumulative: [Double] = [0]
        cumulative.reserveCapacity(points.count)
        for i in 1..<max(points.count, 1) {
            cumulative.append(cumulative[i - 1] + points[i - 1].coordinate.routeDistance(to: points[i].coordinate))
        }
        let distance = cumulative.last ?? 0

        // Climb + descent with hysteresis: ignore jitter smaller than the threshold.
        var climb = 0.0
        var descent = 0.0
        var confirmed: Double?
        for point in points {
            if point.elevationIncomplete { confirmed = nil }
            guard let elevation = point.elevationMeters else { confirmed = nil; continue }
            guard let last = confirmed else {
                confirmed = elevation
                continue
            }
            if elevation >= last + climbHysteresisMeters {
                climb += elevation - last
                confirmed = elevation
            } else if elevation <= last - climbHysteresisMeters {
                descent += last - elevation
                confirmed = elevation
            }
        }

        // Steepest sustained climb: grade over the smallest window ≥ 100 m.
        var maxGrade: Double?
        var windowStart = 0
        for i in 1..<max(points.count, 1) {
            if points[i].elevationMeters == nil || points[i].elevationIncomplete { windowStart = i; continue }
            if points[windowStart].elevationMeters == nil { windowStart = i; continue }
            while cumulative[i] - cumulative[windowStart] >= gradeWindowMeters,
                windowStart + 1 < i,
                cumulative[i] - cumulative[windowStart + 1] >= gradeWindowMeters {
                windowStart += 1
            }
            let run = cumulative[i] - cumulative[windowStart]
            guard run >= gradeWindowMeters,
                let from = points[windowStart].elevationMeters,
                let to = points[i].elevationMeters
            else { continue }
            let grade = (to - from) / run * 100
            if grade > (maxGrade ?? -.infinity) { maxGrade = grade }
        }

        let elevations = points.allSatisfy { $0.elevationMeters != nil && !$0.elevationIncomplete } ? points.compactMap(\.elevationMeters) : []
        return RouteStats(
            distanceMeters: distance,
            elevationGainMeters: climb,
            elevationLossMeters: descent,
            elevationProfile: downsample(elevations, to: profileSampleCount),
            maxGradePercent: maxGrade
        )
    }

    /// Uniform-stride downsample that always keeps the endpoints. Profile-grade, not
    /// analysis-grade.
    public static func downsample(_ samples: [Double], to maxCount: Int) -> [Double] {
        guard maxCount > 1, samples.count > maxCount else { return samples }
        let stride = Double(samples.count - 1) / Double(maxCount - 1)
        return (0..<maxCount).map { samples[Int((Double($0) * stride).rounded())] }
    }

    public init(
        distanceMeters: Double,
        elevationGainMeters: Double,
        elevationLossMeters: Double = 0,
        elevationProfile: [Double] = [],
        maxGradePercent: Double? = nil
    ) {
        self.distanceMeters = distanceMeters
        self.elevationGainMeters = elevationGainMeters
        self.elevationLossMeters = elevationLossMeters
        self.elevationProfile = elevationProfile
        self.maxGradePercent = maxGradePercent
    }
}
