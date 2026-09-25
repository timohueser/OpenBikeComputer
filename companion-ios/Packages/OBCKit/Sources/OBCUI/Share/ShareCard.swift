#if os(iOS)
import OBCDomain
import SwiftUI
import UIKit

/// What a share image shows: a name, a date line, four stats, the line and its elevation.
public struct ShareCardContent {
    let title: String
    let dateLine: String
    let stats: [OBCStat]
    /// The line as the map draws it: a ride is one solid stage.
    let stages: [MultiTrackPreviewView.Stage]
    let elevations: [Double]

    public init(ride: Ride) {
        let summary = ride.summary
        title = summary.name
        dateLine = summary.date.formatted(.dateTime.weekday(.abbreviated).day().month(.abbreviated).year())
        stats = [
            OBCStat(value: OBCFormat.distanceValue(meters: summary.distanceMeters), unit: "km", key: "Distance"),
            OBCStat(value: OBCFormat.movingTime(summary.movingTime), unit: "h", key: "Moving"),
            OBCStat(value: OBCFormat.speedValue(mps: summary.averageSpeedMps), unit: "kph", key: "Avg"),
            OBCStat(value: OBCFormat.climbValue(meters: summary.climbMeters), unit: "m", key: "Climb"),
        ]
        stages = [.ride(ride.points.map(\.coordinate))]
        // The detail screen's profile, on the same distance axis.
        elevations = MeasuredLine.elevationProfile(ridePoints: ride.points)
    }

    /// The trip review's line and totals. During the trip the distance reads "156 of 217 km".
    init(trip: Trip, review: TripReview, runs: [LineRun], dateLine: String) {
        let totals = review.totals
        title = trip.name
        self.dateLine = dateLine
        stats = [
            OBCStat(
                value: OBCFormat.distanceValue(meters: totals.distanceMeters),
                unit: review.currentDay == nil ? "km" : "of \(OBCFormat.distance(meters: review.plannedMeters))",
                key: "Distance"),
            OBCStat(value: OBCFormat.movingTime(totals.movingTime), unit: "h", key: "Moving"),
            OBCStat(
                value: OBCFormat.speedValue(mps: totals.movingTime > 0 ? totals.distanceMeters / totals.movingTime : 0),
                unit: "kph", key: "Avg"),
            OBCStat(value: OBCFormat.climbValue(meters: totals.climbMeters), unit: "m", key: "Climb"),
        ]
        stages = runs.map(MultiTrackPreviewView.Stage.init)
        elevations = trip.measuredLine.elevationProfile(count: RouteStats.profileSampleCount)
    }

    var hasProfile: Bool { elevations.count > 1 }
}

/// The share image in points: the device's rust title bar, then edge-to-edge bands of photo,
/// map, profile and stats. It renders at 3x: 1080 × 1350 pixels, a 4:5 portrait that Instagram
/// and messages show uncropped. It is always in the Day colours: the image leaves the app, and a
/// light card reads the same in every app that shows it.
struct ShareCard: View {
    static let size = CGSize(width: 360, height: 450)
    static let profileHeight: CGFloat = 44
    static let barHeight: CGFloat = 22

    let content: ShareCardContent
    /// The map snapshot at exactly `mapSize`, or nil for the grid fallback.
    let map: UIImage?
    let photo: UIImage?
    let showsProfile: Bool

    /// The photo gives up height to the profile: below about 140 points MapKit drops the
    /// Apple Maps attribution from the snapshot.
    static func photoHeight(showsProfile: Bool) -> CGFloat { showsProfile ? 168 : 203 }

    /// The snapshot size the card shows uncropped, so the Apple Maps attribution stays visible.
    static func mapSize(hasPhoto: Bool, showsProfile: Bool) -> CGSize {
        let photo = hasPhoto ? photoHeight(showsProfile: showsProfile) : 0
        let profile = showsProfile ? profileHeight : 0
        let stats: CGFloat = hasPhoto ? 72 : 150
        return CGSize(width: size.width, height: size.height - barHeight - photo - profile - stats)
    }

    /// With a photo, the heading sits on the photo: below the map, a band that also held the
    /// heading would leave a map too short for MapKit to draw its attribution.
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            titleBar
            if let photo {
                Image(uiImage: photo)
                    .resizable()
                    .scaledToFill()
                    .frame(width: Self.size.width, height: Self.photoHeight(showsProfile: showsProfile))
                    .clipped()
                    .overlay(alignment: .bottomLeading) {
                        heading(onPhoto: true)
                            .padding(20)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(LinearGradient(colors: [.clear, .black.opacity(0.5)], startPoint: .top, endPoint: .bottom))
                    }
            }
            mapView
            VStack(alignment: .leading, spacing: 0) {
                if showsProfile {
                    ElevationProfileView(samples: content.elevations, height: Self.profileHeight - 8, card: false)
                        .padding(.top, 8)
                        .padding(.horizontal, 20)
                }
                VStack(alignment: .leading, spacing: 14) {
                    if photo == nil { heading(onPhoto: false) }
                    statLine
                }
                .padding(.horizontal, 20)
                .padding(.top, photo == nil ? 18 : 14)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .background(OBCTheme.surface)
            .overlay(alignment: .top) { OBCTheme.hairline.frame(height: 1) }
        }
        .frame(width: Self.size.width, height: Self.size.height, alignment: .top)
        .background(OBCTheme.page)
        .environment(\.colorScheme, .light)
    }

    /// The device's title bar, as the ride was recorded on it.
    private var titleBar: some View {
        PixelText("OPENBIKECOMPUTER", scale: 2 / 3, color: OBCTheme.onRust)
            .padding(.horizontal, 20)
            .frame(width: Self.size.width, height: Self.barHeight, alignment: .leading)
            .background(OBCTheme.rust)
    }

    private func heading(onPhoto: Bool) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(content.dateLine.uppercased())
                .font(.system(.caption2, weight: .semibold).monospacedDigit())
                .kerning(1)
                .foregroundStyle(onPhoto ? .white.opacity(0.85) : OBCTheme.secondary)
            Text(content.title)
                .font(.system(.title, weight: .bold))
                .foregroundStyle(onPhoto ? .white : OBCTheme.ink)
                .lineLimit(1)
                .minimumScaleFactor(0.6)
        }
    }

    /// Four stats justified across one line: each takes the width it needs, so a long value
    /// never shrinks its neighbours.
    private var statLine: some View {
        HStack(alignment: .top, spacing: 0) {
            ForEach(Array(content.stats.enumerated()), id: \.offset) { index, stat in
                if index > 0 { Spacer(minLength: 8) }
                VStack(alignment: .leading, spacing: 2) {
                    (Text(stat.value)
                        .font(.obcStat(.body))
                        .foregroundColor(OBCTheme.ink)
                        + Text(stat.unit.map { " \($0)" } ?? "")
                        .font(.system(.caption2, weight: .medium).monospacedDigit())
                        .foregroundColor(OBCTheme.secondary))
                        .lineLimit(1)
                        .minimumScaleFactor(0.7)
                    OBCEyebrow(stat.key)
                }
            }
        }
    }

    private var mapView: some View {
        let size = Self.mapSize(hasPhoto: photo != nil, showsProfile: showsProfile)
        return Group {
            if let map {
                Image(uiImage: map).resizable()
            } else {
                // `ImageRenderer` cannot draw a live map, so without a snapshot it takes the grid.
                MultiTrackPreviewView(stages: content.stages, showsChrome: false, showsEnds: true)
                    .environment(\.obcIsOnline, false)
            }
        }
        .frame(width: size.width, height: size.height)
        .clipped()
    }
}

extension ShareCard {
    /// The share image at 1080 × 1350 pixels.
    @MainActor
    func render() -> UIImage? {
        let renderer = ImageRenderer(content: self)
        renderer.scale = 3
        return renderer.uiImage
    }
}
#endif
