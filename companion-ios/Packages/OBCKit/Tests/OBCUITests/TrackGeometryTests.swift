import XCTest
import OBCDomain
@testable import OBCUI

/// Geometry rules behind the drawing components: the letterbox transform in `TrackPreviewView`
/// and the waypoint-marker placement on the grid preview. `@MainActor` because the helpers are
/// statics on `@MainActor` SwiftUI views, and the returned transform closure is non-Sendable.
@MainActor
final class TrackGeometryTests: XCTestCase {
    // MARK: Letterbox fitting
    func testWideTrackLetterboxesVertically() {
        // Aspect 2 (wide) into a 100x100 box with a 10pt inset gives 80x40, centered.
        let preview = TrackPreview(
            points: [.init(x: 0, y: 0), .init(x: 1, y: 1)],
            aspectRatio: 2
        )
        let transform = TrackPreviewView.fittingTransform(
            for: preview, in: CGSize(width: 100, height: 100), inset: 10
        )
        let topLeft = transform(.init(x: 0, y: 0))
        let bottomRight = transform(.init(x: 1, y: 1))
        XCTAssertEqual(topLeft.x, 10, accuracy: 0.001)
        XCTAssertEqual(topLeft.y, 30, accuracy: 0.001)
        XCTAssertEqual(bottomRight.x, 90, accuracy: 0.001)
        XCTAssertEqual(bottomRight.y, 70, accuracy: 0.001)
    }

    func testTallTrackLetterboxesHorizontally() {
        // Aspect 0.5 (tall) into the same box gives 40x80, centered.
        let preview = TrackPreview(
            points: [.init(x: 0, y: 0), .init(x: 1, y: 1)],
            aspectRatio: 0.5
        )
        let transform = TrackPreviewView.fittingTransform(
            for: preview, in: CGSize(width: 100, height: 100), inset: 10
        )
        let topLeft = transform(.init(x: 0, y: 0))
        XCTAssertEqual(topLeft.x, 30, accuracy: 0.001)
        XCTAssertEqual(topLeft.y, 10, accuracy: 0.001)
    }

    func testATopInsetFitsTheTrackBelowTheTag() {
        // Aspect 0.5 into 100x100 with a 10pt inset and a 20pt band: 30x60, centred below the band.
        let preview = TrackPreview(
            points: [.init(x: 0, y: 0), .init(x: 1, y: 1)],
            aspectRatio: 0.5
        )
        let transform = TrackPreviewView.fittingTransform(
            for: preview, in: CGSize(width: 100, height: 100), inset: 10, topInset: 20
        )
        XCTAssertEqual(transform(.init(x: 0, y: 0)).y, 30, accuracy: 0.001)
        XCTAssertEqual(transform(.init(x: 1, y: 1)).y, 90, accuracy: 0.001)
        XCTAssertEqual(transform(.init(x: 0, y: 0)).x, 35, accuracy: 0.001)
    }

    func testABottomInsetKeepsTheTrackAboveTheChipStrip() {
        // A wide track into 96x72 with a 6pt inset and a 20pt strip: every point stays above 72 - 20 - 6.
        let preview = TrackPreview(
            points: [.init(x: 0, y: 0), .init(x: 1, y: 1)],
            aspectRatio: 1
        )
        let transform = TrackPreviewView.fittingTransform(
            for: preview, in: CGSize(width: 96, height: 72), inset: 6, bottomInset: 20
        )
        XCTAssertEqual(transform(.init(x: 0, y: 0)).y, 6, accuracy: 0.001)
        XCTAssertEqual(transform(.init(x: 1, y: 1)).y, 46, accuracy: 0.001)
    }

    func testCenterPointStaysCentered() {
        let preview = TrackPreview(points: [.init(x: 0.5, y: 0.5)], aspectRatio: 1.7)
        let transform = TrackPreviewView.fittingTransform(
            for: preview, in: CGSize(width: 128, height: 116), inset: 8
        )
        let center = transform(.init(x: 0.5, y: 0.5))
        XCTAssertEqual(center.x, 64, accuracy: 0.001)
        XCTAssertEqual(center.y, 58, accuracy: 0.001)
    }

    func testDegenerateAspectFallsBackToSquare() {
        let preview = TrackPreview(points: [.init(x: 0, y: 0)], aspectRatio: 0)
        let transform = TrackPreviewView.fittingTransform(
            for: preview, in: CGSize(width: 100, height: 100), inset: 10
        )
        // Doesn't crash; maps the unit square onto the full inset box.
        let p = transform(.init(x: 1, y: 1))
        XCTAssertEqual(p.x, 90, accuracy: 0.001)
        XCTAssertEqual(p.y, 90, accuracy: 0.001)
    }

    // MARK: Waypoint markers
    func testMarkerIndexClampsToPolyline() {
        XCTAssertEqual(TrackPreviewView.Marker.pointIndex(fraction: 0, pointCount: 11), 0)
        XCTAssertEqual(TrackPreviewView.Marker.pointIndex(fraction: 1, pointCount: 11), 10)
        XCTAssertEqual(TrackPreviewView.Marker.pointIndex(fraction: 0.5, pointCount: 11), 5)
        // Out-of-range fractions clamp instead of indexing out of bounds.
        XCTAssertEqual(TrackPreviewView.Marker.pointIndex(fraction: 1.4, pointCount: 11), 10)
        XCTAssertEqual(TrackPreviewView.Marker.pointIndex(fraction: -0.2, pointCount: 11), 0)
    }
}
