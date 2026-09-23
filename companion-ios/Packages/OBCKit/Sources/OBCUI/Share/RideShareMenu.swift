#if os(iOS)
import SwiftUI
import UIKit

/// The ride detail's share button: the GPX file, the share image, and save as route.
public struct RideShareMenu: View {
    private let gpx: RideGPXFile
    private let photos: [UIImage]
    /// Nil when the ride cannot become a route; see `Ride.plannedRoute()`.
    private let onSaveAsRoute: (() -> Void)?
    @State private var imageShown = false

    public init(gpx: RideGPXFile, photos: [UIImage] = [], onSaveAsRoute: (() -> Void)?) {
        self.gpx = gpx
        self.photos = photos
        self.onSaveAsRoute = onSaveAsRoute
    }

    public var body: some View {
        Menu {
            ShareLink(item: gpx, preview: SharePreview(gpx.fileName)) {
                Label("GPX file", systemImage: "doc")
            }
            .accessibilityIdentifier("rideShare.gpx")
            Button { imageShown = true } label: {
                Label("Image", systemImage: "photo")
            }
            .accessibilityIdentifier("rideShare.image")
            Button { onSaveAsRoute?() } label: {
                Label("Save as route", systemImage: "point.topleft.down.to.point.bottomright.curvepath")
                if onSaveAsRoute == nil { Text("A gap in the ride is too long to join") }
            }
            .disabled(onSaveAsRoute == nil)
            .accessibilityIdentifier("rideShare.saveAsRoute")
        } label: {
            Image(systemName: "square.and.arrow.up")
        }
        .accessibilityLabel("Share")
        .accessibilityIdentifier("rideShare.menu")
        .sheet(isPresented: $imageShown) {
            ShareImageSheet(content: ShareCardContent(ride: gpx.ride), photos: photos)
        }
    }
}
#endif
