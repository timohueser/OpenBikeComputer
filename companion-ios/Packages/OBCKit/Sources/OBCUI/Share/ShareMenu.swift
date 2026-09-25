#if os(iOS)
import SwiftUI
import UIKit

/// Export a ride or trip as a GPX file or an image.
public struct ShareMenu: View {
    private let gpx: GPXFile
    /// Built when the sheet opens: it measures the whole line.
    private let image: () -> ShareCardContent
    private var photos: [SharePhoto] = []
    @State private var imageShown = false

    public init(gpx: GPXFile, image: @autoclosure @escaping () -> ShareCardContent) {
        self.gpx = gpx
        self.image = image
    }

    /// The share image offers the ride's photos.
    public func photos(from model: RidePhotosModel?) -> ShareMenu {
        var menu = self
        menu.photos = (model?.photos ?? []).compactMap { photo in
            guard let data = model?.thumbnails[photo.assetID], let thumbnail = UIImage(data: data) else { return nil }
            return SharePhoto(thumbnail: thumbnail) {
                (try? await model?.fullImage(photo.assetID)).flatMap { $0 }.flatMap(UIImage.init(data:))
            }
        }
        return menu
    }

    public var body: some View {
        Menu {
            ShareLink(item: gpx, preview: SharePreview(gpx.fileName)) {
                Label("GPX file", systemImage: "doc")
            }
            .accessibilityIdentifier("share.gpx")
            Button { imageShown = true } label: {
                Label("Image", systemImage: "photo")
            }
            .accessibilityIdentifier("share.image")
        } label: {
            Image(systemName: "square.and.arrow.up")
        }
        .accessibilityLabel("Share")
        .accessibilityIdentifier("share.menu")
        .sheet(isPresented: $imageShown) {
            ShareImageSheet(content: image(), photos: photos)
        }
    }
}
#endif
