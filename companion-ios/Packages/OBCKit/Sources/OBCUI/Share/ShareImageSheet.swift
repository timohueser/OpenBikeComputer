#if os(iOS)
import SwiftUI
import UIKit

/// The share image preview: the rendered image, the photo and profile choices, then the system
/// share sheet.
public struct ShareImageSheet: View {
    private let content: ShareCardContent
    private let photos: [UIImage]
    /// The chosen photo, or nil for none.
    @State private var photoIndex: Int?
    @State private var showsProfile = false
    /// Snapshots by size: a photo or profile choice changes the map size, and a size seen once is not
    /// fetched again.
    @State private var maps: [String: UIImage] = [:]
    @State private var image: UIImage?
    @Environment(\.dismiss) private var dismiss

    public init(content: ShareCardContent, photos: [UIImage] = []) {
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
                            Image(uiImage: photos[index]).resizable().scaledToFill()
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

    private func render() async {
        let photo = photoIndex.map { photos[$0] }
        let size = ShareCard.mapSize(hasPhoto: photo != nil, showsProfile: showsProfile)
        let key = "\(size.width)x\(size.height)"
        if maps[key] == nil {
            maps[key] = await ShareMapSnapshot.image(for: content.coordinates, size: size)
        }
        image = ShareCard(content: content, map: maps[key], photo: photo, showsProfile: showsProfile).render()
    }
}
#endif
