#if os(iOS)
import SwiftUI
import UIKit
import OBCDomain
import OBCTransport

/// The full-screen photo viewer: swipe through the ride's photos, with the current photo's pin
/// highlighted on the map below.
struct RidePhotoViewer: View {
    let model: RidePhotosModel
    let preview: TrackPreview?
    @State var index: Int
    /// Screen-size images of the current photo and its neighbours, by asset id.
    @State private var images: [String: Data] = [:]
    @State private var deleted: Set<String> = []
    @State private var notShared: Set<String> = []
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL

    var body: some View {
        VStack(spacing: 0) {
            header
            TabView(selection: $index) {
                ForEach(Array(model.photos.enumerated()), id: \.element.id) { offset, photo in
                    page(photo).tag(offset)
                }
            }
            .tabViewStyle(.page(indexDisplayMode: .never))
            if model.placed.indices.contains(index) {
                let placed = model.placed[index]
                Text("\(OBCFormat.distance(meters: placed.distanceMeters)) · \(placed.photo.takenAt.formatted(date: .omitted, time: .shortened))")
                    .font(.system(.footnote, weight: .medium).monospacedDigit())
                    .foregroundStyle(.white.opacity(0.85))
                    .padding(.vertical, 10)
                    .accessibilityIdentifier("photos.viewer.caption")
            }
            MapTrackPreviewView(preview, photoPins: model.pinCoordinates, highlightedPhoto: index)
                .frame(height: 150)
                .padding(.horizontal, 16)
                .padding(.bottom, 8)
        }
        .background(Color.black.ignoresSafeArea())
        .task(id: index) { await load() }
    }

    private var header: some View {
        HStack {
            Button { dismiss() } label: {
                Image(systemName: "xmark")
                    .font(.system(.callout, weight: .semibold))
                    .frame(width: 44, height: 44)
            }
            .accessibilityLabel("Close")
            .accessibilityIdentifier("photos.viewer.close")
            Spacer()
            Text("\(index + 1) of \(model.photos.count)")
                .font(.system(.footnote, weight: .medium).monospacedDigit())
            Spacer()
            Color.clear.frame(width: 44, height: 44)
        }
        .foregroundStyle(.white)
        .padding(.horizontal, 8)
    }

    @ViewBuilder
    private func page(_ photo: RidePhoto) -> some View {
        if deleted.contains(photo.assetID) {
            unavailable(photo, "This photo is no longer in your library.") {
                Button("Remove", role: .destructive) { remove(photo) }
                    .buttonStyle(.obcDestructive)
                    .accessibilityIdentifier("photos.viewer.remove")
            }
        } else if notShared.contains(photo.assetID) {
            unavailable(photo, "This photo is not shared with the app.") {
                Button("Open Settings") {
                    if let url = URL(string: UIApplication.openSettingsURLString) { openURL(url) }
                }
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("photos.viewer.settings")
            }
        } else if let data = images[photo.assetID] ?? model.thumbnails[photo.assetID],
                  let image = Image(photoData: data) {
            image.resizable().scaledToFit()
        } else {
            ProgressView().tint(.white)
        }
    }

    private func unavailable(_ photo: RidePhoto, _ message: String, @ViewBuilder action: () -> some View) -> some View {
        VStack(spacing: 16) {
            PhotoThumbnail(data: model.thumbnails[photo.assetID])
                .frame(width: 160, height: 160)
                .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusSmall))
                .opacity(0.6)
            Text(message)
                .font(.system(.subheadline))
                .foregroundStyle(.white)
            action().frame(width: 180)
        }
        .padding(24)
    }

    /// Loads the current photo, then its neighbours, and frees every other full image. A photo
    /// that did not load for another reason keeps its thumbnail.
    private func load() async {
        let photos = model.photos
        let near = [index, index + 1, index - 1].filter(photos.indices.contains).map { photos[$0].assetID }
        images = images.filter { near.contains($0.key) }
        for id in near where images[id] == nil && !deleted.contains(id) && !notShared.contains(id) {
            guard !Task.isCancelled else { return }
            do {
                if let data = try await model.fullImage(id) { images[id] = data } else { deleted.insert(id) }
            } catch is PhotoNotShared {
                notShared.insert(id)
            } catch {}
        }
    }

    private func remove(_ photo: RidePhoto) {
        model.remove(photo.assetID)
        if model.photos.isEmpty {
            dismiss()
        } else {
            index = min(index, model.photos.count - 1)
        }
    }
}
#endif
