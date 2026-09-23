import CoreLocation
import Foundation
import OBCDomain
import OBCTransport
import Photos
import PhotosUI
import UIKit

/// The rider's photo library through PhotoKit. PhotoKit has no read-only level, so this asks for
/// read and write and never writes.
struct PhotoKitLibrary: PhotoLibrary {
    struct LoadFailed: Error {}

    func access() -> PhotoAccess {
        Self.access(PHPhotoLibrary.authorizationStatus(for: .readWrite))
    }

    func requestAccess() async -> PhotoAccess {
        Self.access(await PHPhotoLibrary.requestAuthorization(for: .readWrite))
    }

    func candidates(takenIn range: ClosedRange<Date>) async -> [PhotoCandidate] {
        let options = PHFetchOptions()
        options.predicate = NSPredicate(
            format: "creationDate >= %@ AND creationDate <= %@",
            range.lowerBound as NSDate, range.upperBound as NSDate
        )
        options.sortDescriptors = [NSSortDescriptor(key: "creationDate", ascending: true)]
        var candidates: [PhotoCandidate] = []
        PHAsset.fetchAssets(with: .image, options: options).enumerateObjects { asset, _, _ in
            guard let date = asset.creationDate else { return }
            let location = asset.location.map {
                Coordinate(latitude: $0.coordinate.latitude, longitude: $0.coordinate.longitude)
            }
            candidates.append(PhotoCandidate(assetID: asset.localIdentifier, takenAt: date, location: location))
        }
        return candidates
    }

    func image(_ assetID: String, maxPixels: Int) async throws -> Data? {
        guard let asset = PHAsset.fetchAssets(withLocalIdentifiers: [assetID], options: nil).firstObject else {
            // Limited or revoked access hides a photo the same way a delete does.
            if access() == .full { return nil }
            throw PhotoNotShared()
        }
        let options = PHImageRequestOptions()
        // One callback with the final image, fetched from iCloud when the phone has no copy.
        options.deliveryMode = .highQualityFormat
        options.isNetworkAccessAllowed = true
        let size = CGSize(width: maxPixels, height: maxPixels)
        let data: Data? = await withCheckedContinuation { continuation in
            PHImageManager.default().requestImage(
                for: asset, targetSize: size, contentMode: .aspectFit, options: options
            ) { image, _ in
                continuation.resume(returning: image?.jpegData(compressionQuality: 0.85))
            }
        }
        guard let data else { throw LoadFailed() }
        return data
    }

    @MainActor func chooseMore() async {
        let window = UIApplication.shared.connectedScenes
            .compactMap { ($0 as? UIWindowScene)?.keyWindow }.first
        guard var top = window?.rootViewController else { return }
        while let presented = top.presentedViewController { top = presented }
        await withCheckedContinuation { continuation in
            PHPhotoLibrary.shared().presentLimitedLibraryPicker(from: top) { _ in continuation.resume() }
        }
    }

    private static func access(_ status: PHAuthorizationStatus) -> PhotoAccess {
        switch status {
        case .authorized: .full
        case .limited: .limited
        case .notDetermined: .notDetermined
        default: .denied
        }
    }
}
