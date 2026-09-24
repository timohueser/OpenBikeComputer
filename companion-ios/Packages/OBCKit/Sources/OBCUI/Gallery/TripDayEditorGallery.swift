#if DEBUG
import SwiftUI
import OBCDomain

/// The day editor's sheet on the sample Alps line: split mode, where one file becomes the trip
/// and the stepper cuts it, and edit mode on the two-day trip with its stops. The map behind the
/// sheet is the shared marker map; the gallery shows the sheet alone.
struct TripDayEditorGallerySection: View {
    @State private var split = Self.model(GalleryStops.oneFile(), isSplitMode: true)
    @State private var edit = Self.model(GalleryStops.trip(waypoints: true), isSplitMode: false)
    @State private var discardShown = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            OBCEyebrow("Split mode")
            sheet(split)
            OBCEyebrow("Edit mode")
            sheet(edit)
        }
    }

    private static func model(_ trip: Trip, isSplitMode: Bool) -> TripDayEditorModel {
        TripDayEditorModel(
            trip: trip, isSplitMode: isSplitMode,
            finder: StopFinder(search: GalleryStopSearch(stops: GalleryStops.nearby)),
            onSave: { _ in }
        )!
    }

    private func sheet(_ model: TripDayEditorModel) -> some View {
        DayEditorSheet(
            model: model, discardShown: $discardShown, sheetHeight: .constant(0), peekHeight: .constant(0), onClose: {})
            .frame(height: 560)
            .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusSheet))
            .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusSheet).strokeBorder(OBCTheme.hairline))
            .onAppear { model.loadStops() }
    }
}
#endif
