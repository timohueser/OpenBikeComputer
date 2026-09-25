import SwiftUI
import OBCDomain
#if canImport(UIKit)
import UIKit
#endif

/// The photo suggestion and persistent add action share one permission prompt and picker.
struct RidePhotoOfferRow: View {
    @Bindable var model: RidePhotosModel
    @State private var gridShown = false
    @Environment(\.openURL) private var openURL

    var body: some View {
        // The stack stays when the row goes, so the sheet it presents is never orphaned.
        VStack(spacing: 0) {
            if let offer = model.offer {
                OBCQuietRow(
                    systemImage: "photo.on.rectangle",
                    title: offer.title,
                    onOpen: { Task { if await model.openOffer() { gridShown = true } } },
                    onDismiss: { withAnimation(.snappy) { model.dismissOffer() } }
                )
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("photos.offer")
            } else {
                Button {
                    Task { if await model.openOffer() { gridShown = true } }
                } label: {
                    Label("Add photos", systemImage: "photo.on.rectangle")
                        .font(.system(.subheadline))
                        .foregroundStyle(OBCTheme.secondary)
                        .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                        .padding(.horizontal, 14)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("photos.addMore")
            }
        }
        .sheet(isPresented: $gridShown, onDismiss: model.closeGrid) {
            RidePhotoGridSheet(model: model)
        }
        .alert("OBC cannot see your photos", isPresented: $model.accessDenied) {
            #if os(iOS)
            Button("Open Settings") {
                if let url = URL(string: UIApplication.openSettingsURLString) { openURL(url) }
            }
            #endif
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Allow photo access for OBC in Settings.")
        }
    }
}

/// The pick grid in its sheet. Nothing is added until the rider taps Add.
struct RidePhotoGridSheet: View {
    @Bindable var model: RidePhotosModel
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 16) {
                    if let picks = model.picks {
                        if picks.isEmpty {
                            OBCEmptyStateView(
                                glyph: .muted(systemImage: "photo.on.rectangle"),
                                title: model.photos.isEmpty ? "No photos from this ride" : "No more photos from this ride",
                                message: model.access == .limited
                                    ? "OBC sees only the photos you chose. None of them are from this ride."
                                    : "There are no more photos to add from the time of this ride."
                            )
                            .padding(.top, 40)
                        } else {
                            RidePhotoGrid(picks: picks, selected: $model.selected)
                        }
                        if model.access == .limited {
                            Button("Choose more photos…") { Task { await model.chooseMore() } }
                                .buttonStyle(.obcGhost)
                                .padding(.horizontal, 20)
                                .accessibilityIdentifier("photos.chooseMore")
                        }
                    } else {
                        ProgressView().padding(.top, 40)
                    }
                }
                .padding(.bottom, 20)
            }
            .background(OBCTheme.page.ignoresSafeArea())
            .navigationTitle("Photos from this ride")
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Add \(model.selected.count)") {
                        model.addSelected()
                        dismiss()
                    }
                    .fontWeight(.semibold)
                    .disabled(model.selected.isEmpty)
                    .accessibilityIdentifier("photos.add")
                }
            }
        }
        .tint(OBCTheme.tint)
        .task { await model.loadPicks() }
        // A new set of picks restarts the loop; closing the sheet cancels it.
        .task(id: model.picks?.map(\.id)) { await model.loadPickThumbnails() }
    }
}

/// The ride detail's photo strip, and the viewer it opens.
struct RidePhotoStripSection: View {
    let model: RidePhotosModel
    /// The ride's track, for the viewer's map.
    let preview: TrackPreview?
    @State private var viewerStart: ViewerStart?

    private struct ViewerStart: Identifiable {
        let index: Int
        var id: Int { index }
    }

    var body: some View {
        VStack(spacing: 0) {
            if !model.photos.isEmpty {
                RidePhotoStrip(photos: model.photos, thumbnails: model.thumbnails) {
                    viewerStart = ViewerStart(index: $0)
                }
                .padding(.top, 18)
            }
        }
        .task(id: model.photos.map(\.id)) { await model.fillThumbnails() }
        #if os(iOS)
        .fullScreenCover(item: $viewerStart) { start in
            RidePhotoViewer(model: model, preview: preview, index: start.index)
        }
        #endif
    }
}
