#if DEBUG
import SwiftUI
import OBCDomain

/// The day editor on the sample Alps line: split mode, where one file becomes the trip and the
/// stepper cuts it, and edit mode on the two-day trip with its stops.
struct TripDayEditorGallerySection: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            OBCEyebrow("Split mode")
            editor(GalleryStops.oneFile(), isSplitMode: true)
            OBCEyebrow("Edit mode")
            editor(GalleryStops.trip(waypoints: true), isSplitMode: false)
        }
    }

    private func editor(_ trip: Trip, isSplitMode: Bool) -> some View {
        NavigationStack {
            TripDayEditorView(
                model: TripDayEditorModel(
                    trip: trip, isSplitMode: isSplitMode,
                    finder: StopFinder(search: GalleryStopSearch(stops: GalleryStops.nearby)),
                    onSave: { _ in }
                )!,
                onClose: {}
            )
        }
        .frame(height: 720)
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusSheet))
        .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusSheet).strokeBorder(OBCTheme.line))
    }
}
#endif
