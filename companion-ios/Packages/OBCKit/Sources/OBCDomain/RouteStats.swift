import Foundation

/// Statistics derived from a parsed route's geometry — what the import landing
/// (E1) shows before the route exists anywhere else, and what `RouteSummary`
/// construction on save reuses. Pure geometry: `[RoutePoint]` in, numbers out.
///
/// Device-stored routes don't come through here — their stats ride in
/// `RouteSummary` / `RouteDetail` as the device (or fixture) reports them.
public struct RouteStats: Equatable, Sendable {
    public var distanceMeters: Double
    public var elevationGainMeters: Double
    /// Elevation samples for the profile card, downsampled to `profileSampleCount`.
    public var elevationProfile: [Double]
    /// Steepest sustained climb over a ~100 m window, in percent. `nil` when the
    /// source carried no elevation.
    public var maxGradePercent: Double?
    /// Planned-ride estimate: 16 km/h on the flat plus a minute per 10 m of
    /// climb — the touring rule of thumb, not a fitness model.
    public var estimatedDuration: TimeInterval

    /// Elevation-noise hysteresis: climb only accumulates once the track has
    /// risen this far above its last confirmed elevation.
    public static let climbHysteresisMeters = 2.0
    /// Grades are measured over windows at least this long, so single noisy
    /// samples can't spike the MAX stat.
    public static let gradeWindowMeters = 100.0

    public static func compute(from points: [RoutePoint], profileSampleCount: Int = 64) -> RouteStats {
        // Cumulative distance along the track.
        var cumulative: [Double] = [0]
        cumulative.reserveCapacity(points.count)
        for i in 1..<max(points.count, 1) {
            cumulative.append(cumulative[i - 1] + points[i - 1].coordinate.distance(to: points[i].coordinate))
        }
        let distance = cumulative.last ?? 0

        // Climb with hysteresis: ignore jitter smaller than the threshold.
        var climb = 0.0
        var confirmed: Double?
        for point in points {
            guard let elevation = point.elevationMeters else { continue }
            guard let last = confirmed else {
                confirmed = elevation
                continue
            }
            if elevation >= last + climbHysteresisMeters {
                climb += elevation - last
                confirmed = elevation
            } else if elevation <= last - climbHysteresisMeters {
                confirmed = elevation
            }
        }

        // Steepest sustained climb: grade over the smallest window ≥ 100 m.
        var maxGrade: Double?
        var windowStart = 0
        for i in 1..<max(points.count, 1) {
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

        let elevations = points.compactMap(\.elevationMeters)
        let estimateMinutes = distance / 1000 / 16 * 60 + climb / 10
        return RouteStats(
            distanceMeters: distance,
            elevationGainMeters: climb,
            elevationProfile: downsample(elevations, to: profileSampleCount),
            maxGradePercent: maxGrade,
            estimatedDuration: estimateMinutes * 60
        )
    }

    /// Uniform-stride downsample, always keeping the endpoints (mirrors
    /// `TrackPreview.normalizing`'s approach — profile-grade, not analysis-grade).
    public static func downsample(_ samples: [Double], to maxCount: Int) -> [Double] {
        guard maxCount > 1, samples.count > maxCount else { return samples }
        let stride = Double(samples.count - 1) / Double(maxCount - 1)
        return (0..<maxCount).map { samples[Int((Double($0) * stride).rounded())] }
    }

    public init(
        distanceMeters: Double,
        elevationGainMeters: Double,
        elevationProfile: [Double] = [],
        maxGradePercent: Double? = nil,
        estimatedDuration: TimeInterval = 0
    ) {
        self.distanceMeters = distanceMeters
        self.elevationGainMeters = elevationGainMeters
        self.elevationProfile = elevationProfile
        self.maxGradePercent = maxGradePercent
        self.estimatedDuration = estimatedDuration
    }
}
