import Testing
import SwiftUI
import OBCDomain
@testable import OBCUI

/// The drag rules behind the marker control: markers never cross, a map move projects
/// within its window, the profile shows the stretch the map shows, VoiceOver steps are whole
/// moves, and every move is reported once.
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
        )!
    }

    /// Planar metres east and north of 47° N, 8° E.
    private func point(_ x: Double, _ y: Double) -> Coordinate {
        Coordinate(latitude: 47 + y / 111_320, longitude: 8 + x / (111_320 * cos(47 * Double.pi / 180)))
    }

    @Test
    func markersAreSortedAndNeverCross() {
        let model = model()
        #expect(model.markers.map(\.id) == [1, 2])
        model.begin(1)
        model.move(1, to: 8_000)
        #expect(model.markers[0].distance == model.markers[1].distance, "the first stops at the second")
        model.end()
        model.begin(2)
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
        model.begin(1)
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

    @Test
    func aDragEndsOnceAndASecondFingerIsRefused() {
        var events: [LineMarkerEvent] = []
        let model = model { events.append($0) }
        #expect(model.begin(1))
        #expect(!model.begin(2), "one marker at a time")
        model.move(2, to: 8_000)
        #expect(model.markers[1].distance == 6_000, "the refused finger moves nothing")
        model.end()
        model.end()
        #expect(events == [.began(1), .ended(1, distance: 3_000)], "a cancel and its late end are one end")
        #expect(model.begin(2), "free again")
    }

    @Test
    func markersAndTheLineAreReplacedInOneCall() {
        var events: [LineMarkerEvent] = []
        let model = model { events.append($0) }
        model.begin(1)
        model.setMarkers([LineMarker(id: 7, distance: 9_000, name: "Day 1 end")], segmentColors: [.red, .blue])
        #expect(model.activeID == nil, "a replacement ends the drag first")
        #expect(events.last == .ended(1, distance: 3_000))
        #expect(model.markers.map(\.id) == [7])

        let shorter = MeasuredLine(coordinates: [point(0, 0), point(0, 4_000)], elevations: [0, 0])
        model.setLine(shorter, markers: [LineMarker(id: 7, distance: 9_000, name: "Day 1 end")], segmentColors: [.red, .blue])
        #expect(model.lineVersion == 1)
        #expect(abs(model.markers[0].distance - model.line.length) < 1e-9, "held inside the new line")
        #expect(model.profile.count == 2)
    }

    @Test
    func aLineWithoutASegmentIsRefused() {
        let dot = MeasuredLine(coordinates: [point(0, 0)])
        #expect(LineMarkerEditorModel(line: dot, markers: [], segmentColors: [.red]) == nil)
        #expect(LineMarkerEditorModel(line: MeasuredLine(coordinates: []), markers: [], segmentColors: [.red]) == nil)
    }

    @Test
    func coincidentHandlesGoToTheOneThatCanMove() {
        let model = model()
        model.setMarkers(
            [LineMarker(id: 1, distance: 9_999.99, name: "A"), LineMarker(id: 2, distance: 9_999.99, name: "B")],
            segmentColors: [.red, .green, .blue]
        )
        #expect(model.grab(among: [1, 2], forward: false) == 1, "backward, the first is free")
        #expect(model.grab(among: [1, 2], forward: true) == 2)
        model.begin(1)
        model.move(1, to: 5_000)
        #expect(model.markers[0].distance == 5_000)
    }

    @Test
    func onALoopTheMapGrabsTheHandleWhoseLegTheFingerFollows() {
        // A 1 km square ridden clockwise: the trim start and end share the corner at (0, 0).
        let loop = MeasuredLine(coordinates: [point(0, 0), point(0, 1_000), point(1_000, 1_000), point(1_000, 0), point(0, 0)])
        let model = LineMarkerEditorModel(
            line: loop,
            markers: [LineMarker(id: 1, distance: 0, name: "Trim start"), LineMarker(id: 2, distance: loop.length, name: "Trim end")],
            segmentColors: [.gray, .orange, .gray]
        )!
        // The finger moves north, up the first leg.
        #expect(model.grab(among: [1, 2], toward: point(2, 40), window: 100) == 1)
        // The finger moves east along the last leg, back toward the corner's approach.
        #expect(model.grab(among: [1, 2], toward: point(40, 2), window: 100) == 2)
    }

    @Test
    func theMapWindowKeepsAMarkerOnItsLeg() {
        // 10 km out and 10 km back on lanes 5 m apart.
        let outAndBack = MeasuredLine(coordinates: [point(0, 0), point(10_000, 0), point(10_000, 5), point(0, 5)])
        let model = LineMarkerEditorModel(
            line: outAndBack,
            markers: [LineMarker(id: 1, distance: 12_005, name: "Day 1 end")],
            segmentColors: [.red, .blue]
        )!
        // At the fit zoom the finger is 3 m off the return leg and moved 3 m this frame.
        model.begin(1)
        model.move(1, toward: point(7_995, 2), window: LineMarkerEditorModel.mapWindow(travelMeters: 3))
        #expect(abs(model.markers[0].distance - 12_010) < 1, "stays on the return leg")

        // A 1 km switchback with a 30 m hairpin, the marker 300 m down the return leg.
        let switchback = MeasuredLine(coordinates: [point(0, 0), point(1_000, 0), point(1_000, 30), point(0, 30)])
        let hairpin = LineMarkerEditorModel(
            line: switchback, markers: [LineMarker(id: 1, distance: 1_300, name: "Day 1 end")], segmentColors: [.red, .blue]
        )!
        hairpin.begin(1)
        hairpin.move(1, toward: point(720, 12), window: LineMarkerEditorModel.mapWindow(travelMeters: 20))
        #expect(hairpin.markers[0].distance > 1_250, "never the outbound leg at 720 m")
        #expect(LineMarkerEditorModel.mapWindow(travelMeters: 0) == 50)
        #expect(LineMarkerEditorModel.mapWindow(travelMeters: 5_000) == 2_000)
    }

    /// From the first metre in view to the last, unless the hidden part between the pieces is
    /// longer than the pieces: then only the piece nearest the map centre.
    @Test
    func theProfileShowsTheStretchInView() {
        // A snaking route leaves the view twice for short bends: one stretch.
        #expect(LineMarkerEditorModel.window(visible: [1_000...3_000, 3_500...5_000, 5_400...8_000], centre: 4_000)
            == 1_000...8_000)
        // A 40 km loop with both ends in view: 4 km shown, 36 km hidden.
        let ends = [0.0...2_000, 38_000.0...40_000]
        #expect(LineMarkerEditorModel.window(visible: ends, centre: 39_000) == 38_000...40_000)
        #expect(LineMarkerEditorModel.window(visible: ends, centre: 2_500) == 0...2_000, "the nearer piece")
        #expect(LineMarkerEditorModel.window(visible: [], centre: 0) == nil, "no line in view keeps the window")
    }

    @Test
    func aDragStaysInTheStretchInViewAndTheWindowHoldsUnderTheFinger() {
        let model = model()
        model.showVisible([2_000...5_000], centre: 3_500)
        #expect(model.window == 2_000...5_000)
        model.begin(1)
        model.move(1, to: 5_500)
        #expect(model.markers[0].distance == 5_000, "held at the edge of the stretch in view")
        model.showVisible([0...model.line.length], centre: 5_000)
        #expect(model.window == 2_000...5_000, "the window holds still under the finger")
        model.end()
        #expect(model.window == 0...model.line.length, "the map's stretch applies on release")
    }
}
