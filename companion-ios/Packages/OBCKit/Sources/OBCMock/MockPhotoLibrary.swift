#if DEBUG
import CoreGraphics
import Foundation
import ImageIO
import OBCDomain
import OBCTransport
import UniformTypeIdentifiers

/// The simulator's photo library: twelve drawn photos spread over whatever ride asks for them.
/// The fifth has a geotag far from any ride. Limited access sees every third photo until the
/// rider chooses more, and then sees all of them.
public final class MockPhotoLibrary: PhotoLibrary, @unchecked Sendable {
    public static let photoCount = 12
    private let lock = NSLock()
    private var current: PhotoAccess
    private var choseMore = false
    /// The last photo was deleted after it was added: it lists and has a thumbnail, but its
    /// full image is gone.
    private let lastPhotoGone: Bool

    public init(access: PhotoAccess, lastPhotoGone: Bool = false) {
        current = access
        self.lastPhotoGone = lastPhotoGone
    }

    public func access() -> PhotoAccess {
        lock.withLock { current }
    }

    public func requestAccess() async -> PhotoAccess {
        lock.withLock {
            if current == .notDetermined { current = .full }
            return current
        }
    }

    public func candidates(takenIn range: ClosedRange<Date>) async -> [PhotoCandidate] {
        let span = range.upperBound.timeIntervalSince(range.lowerBound)
        return (0..<Self.photoCount).compactMap { index in
            guard isVisible(index) else { return nil }
            let time = range.lowerBound.addingTimeInterval(span * (Double(index) + 0.5) / Double(Self.photoCount))
            let far = index == 4 ? Coordinate(latitude: 0, longitude: 0) : nil
            return PhotoCandidate(assetID: Self.assetID(index), takenAt: time, location: far)
        }
    }

    public func image(_ assetID: String, maxPixels: Int) async throws -> Data? {
        guard let index = (0..<Self.photoCount).first(where: { Self.assetID($0) == assetID }) else { return nil }
        guard isVisible(index) else { throw PhotoNotShared() }
        if lastPhotoGone, index == Self.photoCount - 1, maxPixels > 400 { return nil }
        return Self.drawPhoto(index, longEdge: maxPixels)
    }

    @MainActor public func chooseMore() async {
        lock.withLock { choseMore = true }
    }

    private func isVisible(_ index: Int) -> Bool {
        lock.withLock { current == .full || (current == .limited && (choseMore || index % 3 == 0)) }
    }

    private static func assetID(_ index: Int) -> String { "mock-photo-\(index + 1)" }

    /// A landscape: a sky gradient whose hue walks through the day, a ridge and a sun.
    private static func drawPhoto(_ index: Int, longEdge: Int) -> Data? {
        let width = min(longEdge, 1_200), height = width * 3 / 4
        guard let context = CGContext(
            data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue
        ) else { return nil }
        let t = CGFloat(index) / CGFloat(photoCount - 1)
        let w = CGFloat(width), h = CGFloat(height)
        let colors = [
            CGColor(red: 0.95 - 0.2 * t, green: 0.75 - 0.1 * t, blue: 0.55 + 0.3 * t, alpha: 1),
            CGColor(red: 0.35 + 0.3 * t, green: 0.55, blue: 0.85 - 0.3 * t, alpha: 1),
        ] as CFArray
        if let gradient = CGGradient(colorsSpace: nil, colors: colors, locations: [0, 1]) {
            context.drawLinearGradient(gradient, start: .zero, end: CGPoint(x: 0, y: h), options: [])
        }
        context.setFillColor(CGColor(red: 1, green: 0.95, blue: 0.8, alpha: 0.9))
        context.fillEllipse(in: CGRect(x: w * (0.15 + 0.7 * t) - h * 0.08, y: h * 0.62, width: h * 0.16, height: h * 0.16))
        for (layer, shade) in [(0, 0.42), (1, 0.28)] {
            context.setFillColor(CGColor(red: 0.2 * shade * 2, green: 0.35 * shade * 2, blue: 0.25 * shade * 2, alpha: 1))
            context.move(to: .zero)
            for step in 0...12 {
                let x = w * CGFloat(step) / 12
                let peak = sin(CGFloat(step + index * 3 + layer * 5) * 1.3) * 0.12 + 0.45 - 0.14 * CGFloat(layer)
                context.addLine(to: CGPoint(x: x, y: h * peak))
            }
            context.addLine(to: CGPoint(x: w, y: 0))
            context.fillPath()
        }
        guard let image = context.makeImage() else { return nil }
        let data = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(data, UTType.jpeg.identifier as CFString, 1, nil)
        else { return nil }
        CGImageDestinationAddImage(destination, image, [kCGImageDestinationLossyCompressionQuality: 0.8] as CFDictionary)
        guard CGImageDestinationFinalize(destination) else { return nil }
        return data as Data
    }
}
#endif
