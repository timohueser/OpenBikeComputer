import SwiftUI
import OBCDomain
#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

/// The ride's photos in time order, one tap from the viewer.
public struct RidePhotoStrip: View {
    let photos: [RidePhoto]
    let thumbnails: [String: Data]
    let onOpen: (Int) -> Void

    public init(photos: [RidePhoto], thumbnails: [String: Data], onOpen: @escaping (Int) -> Void) {
        self.photos = photos
        self.thumbnails = thumbnails
        self.onOpen = onOpen
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            OBCEyebrow("Photos · \(photos.count)")
            ScrollView(.horizontal, showsIndicators: false) {
                LazyHStack(spacing: 6) {
                    ForEach(Array(photos.enumerated()), id: \.element.id) { index, photo in
                        Button { onOpen(index) } label: {
                            PhotoThumbnail(data: thumbnails[photo.assetID])
                                .frame(width: 84, height: 84)
                                .clipShape(RoundedRectangle(cornerRadius: TrackRow.sketchRadius))
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Photo \(index + 1) of \(photos.count)")
                        .accessibilityHint("Opens the photo")
                        .accessibilityIdentifier("photos.strip.\(index)")
                    }
                }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("detail.photoStrip")
    }
}

/// The pick grid: every candidate in time order, a tap toggles it.
public struct RidePhotoGrid: View {
    let picks: [RidePhotosModel.Pick]
    @Binding var selected: Set<String>

    public init(picks: [RidePhotosModel.Pick], selected: Binding<Set<String>>) {
        self.picks = picks
        _selected = selected
    }

    public var body: some View {
        LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 3), count: 3), spacing: 3) {
            ForEach(picks) { pick in
                let isSelected = selected.contains(pick.id)
                Button {
                    if isSelected { selected.remove(pick.id) } else { selected.insert(pick.id) }
                } label: {
                    Color.clear
                        .aspectRatio(1, contentMode: .fit)
                        .overlay { PhotoThumbnail(data: pick.thumbnail) }
                        .clipped()
                        .overlay { if !isSelected { OBCTheme.surface.opacity(0.45) } }
                        .overlay(alignment: .bottomLeading) { caption(pick) }
                        .overlay(alignment: .topTrailing) { checkmark(isSelected) }
                }
                .buttonStyle(.plain)
                .accessibilityLabel(pickLabel(pick))
                .accessibilityAddTraits(isSelected ? .isSelected : [])
                .accessibilityIdentifier("photos.pick.\(pick.id)")
            }
        }
    }

    private func pickLabel(_ pick: RidePhotosModel.Pick) -> String {
        let time = pick.placed.photo.takenAt.formatted(date: .omitted, time: .shortened)
        return pick.placed.locationOffTrack ? "Photo at \(time), location off the track" : "Photo at \(time)"
    }

    private func caption(_ pick: RidePhotosModel.Pick) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            if pick.placed.locationOffTrack {
                Text("Location off the track")
                    .font(.system(.caption2, weight: .semibold))
            }
            Text(pick.placed.photo.takenAt.formatted(date: .omitted, time: .shortened))
                .font(.system(.caption2, weight: .semibold).monospacedDigit())
        }
        .foregroundStyle(.white)
        .padding(.horizontal, 6)
        .padding(.vertical, 5)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(LinearGradient(colors: [.clear, .black.opacity(0.5)], startPoint: .top, endPoint: .bottom))
    }

    /// The list's selection mark: an amber disc with an ink check, or an empty ring.
    private func checkmark(_ isSelected: Bool) -> some View {
        Image(systemName: isSelected ? "checkmark.circle.fill" : "circle")
            .font(.system(.title3, weight: .semibold))
            .symbolRenderingMode(.palette)
            .foregroundStyle(isSelected ? OBCTheme.onAmber : .white, isSelected ? OBCTheme.amber : .black.opacity(0.2))
            .shadow(color: .black.opacity(0.25), radius: 2)
            .padding(6)
            .obcFixedGeometryType()
    }
}

/// A cached thumbnail, filling its frame, or a placeholder when there is none.
struct PhotoThumbnail: View {
    let data: Data?

    var body: some View {
        OBCTheme.surface2.overlay {
            if let data, let image = Image(photoData: data) {
                image.resizable().scaledToFill()
            } else {
                Image(systemName: "photo")
                    .font(.system(.body))
                    .foregroundStyle(OBCTheme.secondary)
            }
        }
        .clipped()
    }
}

extension Image {
    init?(photoData data: Data) {
        #if canImport(UIKit)
        guard let image = UIImage(data: data) else { return nil }
        self.init(uiImage: image)
        #elseif canImport(AppKit)
        guard let image = NSImage(data: data) else { return nil }
        self.init(nsImage: image)
        #else
        return nil
        #endif
    }
}
