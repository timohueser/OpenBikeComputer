#if DEBUG
import SwiftUI
import OBCDomain

/// The day editor on the sample Alps line: split mode, where one file becomes the trip and the
/// stepper cuts it, and edit mode on the two-day trip with its stops.
struct TripDayEditorGallerySection: View {
    @State private var split = Self.model(GalleryStops.oneFile(), isSplitMode: true)
    @State private var edit = Self.model(GalleryStops.trip(waypoints: true), isSplitMode: false)

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            OBCEyebrow("Split mode")
            editor(split)
            OBCEyebrow("Edit mode")
            editor(edit)
        }
    }

    private static func model(_ trip: Trip, isSplitMode: Bool) -> TripDayEditorModel {
        TripDayEditorModel(
            trip: trip, isSplitMode: isSplitMode,
            finder: StopFinder(search: GalleryStopSearch(stops: GalleryStops.nearby)),
            onSave: { _ in }
        )!
    }

    private func editor(_ model: TripDayEditorModel) -> some View {
        NavigationStack {
            TripDayEditorView(model: model, onClose: {})
        }
        .frame(height: 720)
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusSheet))
        .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusSheet).strokeBorder(OBCTheme.line))
    }
}
#endif
