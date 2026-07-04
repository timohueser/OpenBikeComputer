import XCTest
import OBCDomain
@testable import OBCUI

/// Geometry rules behind the drawing components — the letterbox transform in
/// `TrackPreviewView` and the waypoint-marker placement on the grid preview
/// (`TrackPreviewView.Marker.middleWaypointPins`).
/// `@MainActor` because the helpers are statics on `@MainActor` SwiftUI views
/// (and the returned transform closure is non-Sendable, so it must stay there).
@MainActor
final class TrackGeometryTests: XCTestCase {
    // ------------------------------------------------------- letterbox fitting
    func testWideTrackLetterboxesVertically() {
        // aspect 2 (wide) into a 100×100 box with 10pt inset → 80×40 centered.
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
        // aspect 0.5 (tall) into 100×100 with 10pt inset → 40×80 centered.
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

    // ------------------------------------------------------- waypoint markers
    func testMarkerIndexClampsToPolyline() {
        XCTAssertEqual(TrackPreviewView.Marker.pointIndex(fraction: 0, pointCount: 11), 0)
        XCTAssertEqual(TrackPreviewView.Marker.pointIndex(fraction: 1, pointCount: 11), 10)
        XCTAssertEqual(TrackPreviewView.Marker.pointIndex(fraction: 0.5, pointCount: 11), 5)
        // Out-of-range fractions clamp instead of indexing out of bounds.
        XCTAssertEqual(TrackPreviewView.Marker.pointIndex(fraction: 1.4, pointCount: 11), 10)
        XCTAssertEqual(TrackPreviewView.Marker.pointIndex(fraction: -0.2, pointCount: 11), 0)
    }
}
