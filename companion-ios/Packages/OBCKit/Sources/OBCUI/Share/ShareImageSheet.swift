#if os(iOS)
import SwiftUI
import UIKit

/// A photo the share image can show: its thumbnail for the choice, and a loader for the full
/// image the card draws.
public struct SharePhoto {
    let thumbnail: UIImage
    let load: @MainActor () async -> UIImage?

    public init(thumbnail: UIImage, load: @escaping @MainActor () async -> UIImage?) {
        self.thumbnail = thumbnail
        self.load = load
    }
}

/// The share image preview: the rendered image, the photo and profile choices, then the system
/// share sheet.
public struct ShareImageSheet: View {
    private let content: ShareCardContent
    private let photos: [SharePhoto]
    /// The chosen photo, or nil for none.
    @State private var photoIndex: Int?
    @State private var showsProfile = false
    /// Snapshots by size: a photo or profile choice changes the map size, and a size seen once is not
    /// fetched again.
    @State private var maps: [String: UIImage] = [:]
    /// Full photos by choice index. A photo that does not load draws its thumbnail.
    @State private var fullPhotos: [Int: UIImage] = [:]
    @State private var image: UIImage?
    @Environment(\.dismiss) private var dismiss

    public init(content: ShareCardContent, photos: [SharePhoto] = []) {
        self.content = content
        self.photos = photos
        _photoIndex = State(initialValue: photos.isEmpty ? nil : 0)
    }

    public var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 18) {
                preview
                if !photos.isEmpty || content.hasProfile {
                    HStack(alignment: .top, spacing: 16) {
                        if !photos.isEmpty { photoChoice }
                        Spacer(minLength: 0)
                        if content.hasProfile { profileChoice }
                    }
                }
                Spacer(minLength: 0)
                if let image {
                    ShareLink(
                        item: Image(uiImage: image),
                        preview: SharePreview(content.title, image: Image(uiImage: image))
                    ) {
                        Label("Share", systemImage: "square.and.arrow.up")
                    }
                    .buttonStyle(.obcPrimary)
                    .accessibilityIdentifier("shareImage.share")
                }
            }
            .padding(20)
            .background(OBCTheme.parchment.ignoresSafeArea())
            .navigationTitle("Share image")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
        }
        .tint(OBCTheme.tint)
        .task(id: [photoIndex ?? -1, showsProfile ? 1 : 0]) { await render() }
    }

    private var preview: some View {
        Group {
            if let image {
                Image(uiImage: image).resizable()
            } else {
                OBCSkeleton(cornerRadius: OBCTheme.radiusPanel)
            }
        }
        .aspectRatio(ShareCard.size.width / ShareCard.size.height, contentMode: .fit)
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
        .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.line))
        .frame(maxWidth: .infinity)
        .accessibilityIdentifier("shareImage.preview")
    }

    private var photoChoice: some View {
        VStack(alignment: .leading, spacing: 8) {
            OBCEyebrow("Photo")
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    choiceTile(selected: photoIndex == nil) {
                        Text("None")
                            .font(.system(size: 13, weight: .medium))
                            .foregroundStyle(OBCTheme.inkSoft)
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                            .background(OBCTheme.panel)
                    } action: { photoIndex = nil }
                    ForEach(photos.indices, id: \.self) { index in
                        choiceTile(selected: photoIndex == index) {
                            Image(uiImage: photos[index].thumbnail).resizable().scaledToFill()
                        } action: { photoIndex = index }
                    }
                }
                .padding(2)
            }
        }
    }

    private var profileChoice: some View {
        VStack(alignment: .leading, spacing: 8) {
            OBCEyebrow("Profile")
            Toggle("Profile", isOn: $showsProfile)
                .labelsHidden()
                .tint(OBCTheme.forest)
                .accessibilityIdentifier("shareImage.profile")
        }
    }

    private func choiceTile(
        selected: Bool, @ViewBuilder label: () -> some View, action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            label()
                .frame(width: 64, height: 64)
                .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusSmall))
                .overlay(
                    RoundedRectangle(cornerRadius: OBCTheme.radiusSmall)
                        .strokeBorder(selected ? OBCTheme.forest : OBCTheme.line, lineWidth: selected ? 2.5 : 1)
                )
        }
        .buttonStyle(.plain)
    }

    /// Renders from the choices as they were at the start. `MKMapSnapshotter` ignores
    /// cancellation, so a render whose choices changed during the snapshot drops its image and
    /// leaves the frame to the newer render.
    private func render() async {
        var photo: UIImage?
        if let index = photoIndex {
            if fullPhotos[index] == nil { fullPhotos[index] = await photos[index].load() ?? photos[index].thumbnail }
            photo = fullPhotos[index]
        }
        let showsProfile = showsProfile
        let size = ShareCard.mapSize(hasPhoto: photo != nil, showsProfile: showsProfile)
        let key = "\(size.width)x\(size.height)"
        var map = maps[key]
        if map == nil {
            map = await ShareMapSnapshot.image(for: content.stages, size: size)
            maps[key] = map
        }
        guard !Task.isCancelled else { return }
        image = ShareCard(content: content, map: map, photo: photo, showsProfile: showsProfile).render()
    }
}
#endif
