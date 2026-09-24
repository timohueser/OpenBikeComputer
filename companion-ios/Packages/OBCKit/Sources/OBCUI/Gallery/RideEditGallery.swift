#if DEBUG
import SwiftUI
import OBCDomain

/// Ride edit mode on the sample Furka line, and the merge-suggestion row.
struct RideEditGallerySection: View {
    @State private var editShown = false
    @State private var lastEdit = "No edit yet"

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            RideMergeSuggestion(load: { Self.lunch }, onMerge: { lastEdit = "Merged" }, onDismiss: {})
            Button("Open edit mode") { editShown = true }
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("gallery.rideEdit")
            Text(lastEdit)
                .font(.system(.caption).monospacedDigit())
                .foregroundStyle(OBCTheme.secondary)
        }
        #if os(iOS)
        .fullScreenCover(isPresented: $editShown) { editor }
        #else
        .sheet(isPresented: $editShown) { editor }
        #endif
    }

    private var editor: some View {
        RideEditView(ride: Self.ride, nextRide: Self.lunch) { edit in
            editShown = false
            lastEdit = edit.map { "\($0)" } ?? "Cancelled"
        }
    }

    /// The sample line as a ride at 5 m/s from 08:00.
    static let ride: Ride = {
        let start = Date(timeIntervalSince1970: 1_790_748_000)
        let points = SampleLine.alps.vertices.map {
            RidePoint(timestamp: start.addingTimeInterval($0.distance / 5), coordinate: $0.coordinate,
                      elevationMeters: $0.elevation)
        }
        return Ride(summary: RideSummary(id: RideID("gallery-ride"), name: "Day 2 Ulrichen", date: start,
                                         distanceMeters: SampleLine.alps.length), points: points)
    }()

    static let lunch = RideSummary(
        id: RideID("gallery-lunch"), name: "Day 2 Ulrichen (2)",
        date: ride.points.last!.timestamp.addingTimeInterval(2_700), distanceMeters: 12_400
    )
}
#endif
