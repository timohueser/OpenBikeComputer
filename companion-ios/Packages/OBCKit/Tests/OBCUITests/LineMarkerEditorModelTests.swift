import Testing
import SwiftUI
import OBCDomain
@testable import OBCUI

/// The drag rules behind the marker control: markers never cross, a map move projects
/// within its window, VoiceOver steps are whole moves, and every move is reported once.
@MainActor
struct LineMarkerEditorModelTests {
    /// Latitude `meters` north of 47° N.
    private func north(_ meters: Double) -> Double { 47 + meters / 111_320 }

    /// A straight 10 km line north, 100 m of elevation per kilometre.
    private func model(events: @escaping (LineMarkerEvent) -> Void = { _ in }) -> LineMarkerEditorModel {
        let line = MeasuredLine(
            coordinates: (0...10).map { Coordinate(latitude: north(Double($0) * 1000), longitude: 8) },
            elevations: (0...10).map { Double($0) * 100 }
        )
        return LineMarkerEditorModel(
            line: line,
            markers: [LineMarker(id: 2, distance: 6_000, name: "Day 2 end"), LineMarker(id: 1, distance: 3_000, name: "Day 1 end")],
            segmentColors: [.red, .green, .blue],
            onEvent: events
        )
    }

    @Test
    func markersAreSortedAndNeverCross() {
        let model = model()
        #expect(model.markers.map(\.id) == [1, 2])
        model.move(1, to: 8_000)
        #expect(model.markers[0].distance == model.markers[1].distance, "the first stops at the second")
        model.move(2, to: -500)
        #expect(model.markers[1].distance == model.markers[0].distance)
        model.move(2, to: 99_000)
        #expect(abs(model.markers[1].distance - model.line.length) < 1e-9, "and inside the line")
    }

    @Test
    func aDragReportsBeginEveryMoveAndEnd() {
        var events: [LineMarkerEvent] = []
        let model = model { events.append($0) }
        model.begin(1)
        #expect(model.activeID == 1)
        model.move(1, to: 3_500)
        model.move(1, to: 3_500)
        model.move(1, to: 4_000)
        model.end()
        #expect(model.activeID == nil)
        #expect(events == [
            .began(1), .moved(1, distance: 3_500), .moved(1, distance: 4_000), .ended(1, distance: 4_000),
        ])
    }

    @Test
    func aMapMoveProjectsWithinTheWindow() {
        let model = model()
        // A finger beside the 3.4 km point, a little east of the line.
        model.move(1, toward: Coordinate(latitude: north(3_400), longitude: 8.001), window: 1_000)
        #expect(abs(model.markers[0].distance - 3_400) < 10)
        // A finger far ahead moves the marker one window, not to the finger.
        model.move(1, toward: Coordinate(latitude: north(9_000), longitude: 8), window: 1_000)
        #expect(abs(model.markers[0].distance - 4_400) < 10)
    }

    @Test
    func aVoiceOverNudgeIsOneWholeMove() {
        var events: [LineMarkerEvent] = []
        let model = model { events.append($0) }
        model.nudge(2, by: LineMarkerEditorModel.nudgeMeters)
        #expect(abs(model.markers[1].distance - 7_000) < 1e-9)
        #expect(events == [.began(2), .moved(2, distance: 7_000), .ended(2, distance: 7_000)])
    }

    @Test
    func theLabelReadsDistanceAndElevation() {
        let model = model()
        #expect(model.label(for: 1, locale: Locale(identifier: "en_US")) == "km 3.0 · 300 m")
        #expect(model.color(endingAt: 2) == .green, "a marker takes the colour of the segment it ends")
    }
}
