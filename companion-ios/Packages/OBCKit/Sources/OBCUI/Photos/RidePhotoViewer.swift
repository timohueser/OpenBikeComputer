#if os(iOS)
import SwiftUI
import OBCDomain

/// The full-screen photo viewer: swipe through the ride's photos, with the current photo's pin
/// highlighted on the map below.
struct RidePhotoViewer: View {
    let model: RidePhotosModel
    let preview: TrackPreview?
    @State var index: Int
    /// Screen-size images by asset id, loaded as the rider swipes.
    @State private var images: [String: Data] = [:]
    @State private var missing: Set<String> = []
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 0) {
            header
            TabView(selection: $index) {
                ForEach(Array(model.photos.enumerated()), id: \.element.id) { offset, photo in
                    page(photo).tag(offset)
                }
            }
            .tabViewStyle(.page(indexDisplayMode: .never))
            if model.photos.indices.contains(index) {
                let photo = model.photos[index]
                Text("\(OBCFormat.distance(meters: photo.distanceMeters)) · \(photo.takenAt.formatted(date: .omitted, time: .shortened))")
                    .font(.obcMono(size: 13, weight: .medium))
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
                    .font(.system(size: 16, weight: .semibold))
                    .frame(width: 44, height: 44)
            }
            .accessibilityLabel("Close")
            .accessibilityIdentifier("photos.viewer.close")
            Spacer()
            Text("\(index + 1) of \(model.photos.count)")
                .font(.obcMono(size: 13, weight: .medium))
            Spacer()
            Color.clear.frame(width: 44, height: 44)
        }
        .foregroundStyle(.white)
        .padding(.horizontal, 8)
    }

    @ViewBuilder
    private func page(_ photo: RidePhoto) -> some View {
        if missing.contains(photo.assetID) {
            VStack(spacing: 16) {
                PhotoThumbnail(data: model.thumbnails[photo.assetID])
                    .frame(width: 160, height: 160)
                    .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusSmall))
                    .opacity(0.6)
                Text("This photo is no longer in your library.")
                    .font(.system(size: 15))
                    .foregroundStyle(.white)
                Button("Remove", role: .destructive) { remove(photo) }
                    .buttonStyle(.obcDestructive)
                    .frame(width: 160)
                    .accessibilityIdentifier("photos.viewer.remove")
            }
            .padding(24)
        } else if let data = images[photo.assetID] ?? model.thumbnails[photo.assetID],
                  let image = Image(photoData: data) {
            image.resizable().scaledToFit()
        } else {
            ProgressView().tint(.white)
        }
    }

    /// A photo that did not load, but is still in the library, keeps its thumbnail.
    private func load() async {
        guard model.photos.indices.contains(index) else { return }
        let id = model.photos[index].assetID
        guard images[id] == nil, !missing.contains(id) else { return }
        do {
            if let data = try await model.fullImage(id) {
                images[id] = data
            } else {
                missing.insert(id)
            }
        } catch {}
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
