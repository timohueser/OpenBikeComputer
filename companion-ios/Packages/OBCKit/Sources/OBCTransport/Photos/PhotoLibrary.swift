import Foundation
import OBCDomain

/// The photo may still be in the library, but the rider's access setting hides it from the app.
public struct PhotoNotShared: Error {
    public init() {}
}

/// What the app may see of the rider's photo library.
public enum PhotoAccess: Sendable {
    case notDetermined
    case denied
    /// Only the photos the rider chose.
    case limited
    case full

    public var canRead: Bool { self == .limited || self == .full }
}

/// The rider's photo library, read only. The composition root picks PhotoKit on a phone and a
/// fake in the simulator and tests.
public protocol PhotoLibrary: Sendable {
    func access() -> PhotoAccess
    /// Asks the rider for access; call it only from a rider's tap.
    func requestAccess() async -> PhotoAccess
    /// The photos the app can see that were taken in `range`, in time order.
    func candidates(takenIn range: ClosedRange<Date>) async -> [PhotoCandidate]
    /// A JPEG of the photo, at most `maxPixels` on its long edge. Nil when the photo is deleted
    /// from the library. Throws `PhotoNotShared` when the app cannot see the photo, and another
    /// error when the photo did not load.
    func image(_ assetID: String, maxPixels: Int) async throws -> Data?
    /// Shows the system picker that adds photos to limited access.
    @MainActor func chooseMore() async
}
