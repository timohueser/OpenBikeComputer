#if os(iOS)
import SwiftUI
import UIKit

/// The share button of a ride or a trip: the GPX file, the share image, and for a ride, save as
/// route.
public struct ShareMenu: View {
    private let gpx: GPXFile
    /// Built when the sheet opens: it measures the whole line.
    private let image: () -> ShareCardContent
    private var photos: [SharePhoto] = []
    private var offersSaveAsRoute = false
    private var onSaveAsRoute: (() -> Void)?
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

    /// Offers Save as route. A nil `action` shows it disabled, because the ride cannot become a
    /// route; see `Ride.plannedRoute()`.
    public func saveAsRoute(_ action: (() -> Void)?) -> ShareMenu {
        var menu = self
        menu.offersSaveAsRoute = true
        menu.onSaveAsRoute = action
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
            if offersSaveAsRoute {
                Button { onSaveAsRoute?() } label: {
                    Label("Save as route", systemImage: "point.topleft.down.to.point.bottomright.curvepath")
                    if onSaveAsRoute == nil { Text("A gap in the ride is too long to join") }
                }
                .disabled(onSaveAsRoute == nil)
                .accessibilityIdentifier("share.saveAsRoute")
            }
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
