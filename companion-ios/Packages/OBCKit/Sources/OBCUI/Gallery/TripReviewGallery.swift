#if DEBUG && os(iOS)
import SwiftUI
import OBCDomain

/// The trip review's parts on the sample Alps line: the map with the ridden part, the totals, a
/// ridden day, a day with two rides, the transfer lines and the offer.
struct TripReviewGallerySection: View {
    @State private var transfer: TransferKind? = .train

    var body: some View {
        let line = SampleLine.alps
        let points = line.vertices.map(\.coordinate)
        let ridden = Array(points[...14]), planned = Array(points[14...])
        VStack(alignment: .leading, spacing: 18) {
            MultiTrackPreviewView(
                stages: [
                    .init(coordinates: planned, color: OBCTheme.inkSoft, dash: [5, 4]),
                    .init(coordinates: ridden, color: OBCTheme.trackStroke),
                ],
                pins: [
                    .init(coordinate: points[20], color: OBCTheme.ink),
                    .init(coordinate: points[6], color: OBCTheme.water),
                    .init(coordinate: points[12], color: OBCTheme.water),
                    .init(coordinate: points[14], color: OBCTheme.forest),
                ]
            )
            .frame(height: 200)
            TripReviewTotals(
                ridden: RideTotals([RideSummary(
                    id: RideID("a"), name: "a", date: .now, distanceMeters: 156_000, movingTime: 42_000, climbMeters: 3_720)]),
                plannedMeters: 217_000, isDone: false, highlights: "Furka 2,431 m · Biggest day 82.0 km")
            TripJournalDayEntry(
                number: 1, title: "Furka", header: "Mon 29 Sep · Andermatt → Ulrichen · 74 km",
                note: "Furka in the fog, then sun on the way down. Wild camp by the lake, storm at 3.",
                photos: OBCComponentGallery.samplePhotos, thumbnails: OBCComponentGallery.sampleThumbnails,
                rides: [Self.ride("Day 1 Ulrichen", 74_300)], onOpenRide: { _ in })
            TripJournalTransfer(kind: transfer, from: "Ulrichen", to: "Oberwald") { transfer = $0 }
            TripJournalTransfer(kind: nil, from: "Ulrichen", to: nil) { _ in }
            TripJournalDayEntry(
                number: 2, title: nil, header: "Tue 30 Sep · Oberwald → Brig · 46 km", note: "", photos: [],
                thumbnails: [:], rides: [Self.ride("Day 2 Brig", 28_100), Self.ride("Day 2 Brig (2)", 17_900)],
                onOpenRide: { _ in })
            OBCQuietRow(
                systemImage: "arrow.left.and.right", title: "Days 3–5 are longer now. Even them out?", onOpen: {},
                onDismiss: {})
            OBCGroupedSection {
                TripDayRow(color: OBCTheme.stageColor(index: 2), number: 3, title: "to Brig", detail: "Wed 1 Oct · 61.0 km")
                TripTransferRow(kind: transfer, meters: 34_000) { transfer = $0 }
                TripDayRow(
                    color: OBCTheme.stageColor(index: 3), number: 4, title: "to Spiez", detail: "Thu 2 Oct · 38.0 km",
                    showsDivider: false)
            }
        }
    }

    private static func ride(_ name: String, _ meters: Double) -> RideSummary {
        RideSummary(
            id: RideID(name), name: name, date: Date(timeIntervalSince1970: 1_790_000_000), distanceMeters: meters,
            movingTime: meters / 4.5, averageSpeedMps: 4.5, climbMeters: meters / 40)
    }
}
#endif
