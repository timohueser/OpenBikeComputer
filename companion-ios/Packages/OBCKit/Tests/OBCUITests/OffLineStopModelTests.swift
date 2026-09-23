import Testing
import Foundation
import OBCDomain
@testable import OBCUI

/// The day editor with the phone's router: a stop off the line offers out and back and via, a
/// pick sets the mode, a failure leaves the day on the line, and a gap inside a day bridges.
@MainActor
struct OffLineStopModelTests {
    /// Planar metres east and north of a fixed origin at 46.5° N.
    private func coordinate(_ x: Double, _ y: Double = 0) -> Coordinate {
        Coordinate(latitude: 46.5 + y / 111_320, longitude: 8 + x / (111_320 * cos(46.5 * Double.pi / 180)))
    }

    private func file(_ from: Double, _ to: Double) -> [RoutePoint] {
        stride(from: from, through: to, by: 100).map { RoutePoint(coordinate: coordinate($0), elevationMeters: 500) }
    }

    /// A straight road between the two points, or a failure.
    struct FakeLegRouter: LegRouter {
        var failure: LegRouteFailure?

        func route(
            from: Coordinate, to: Coordinate, bikeType: BikeType, onDownload: @escaping @Sendable () -> Void
        ) async throws -> [RoutePoint] {
            if let failure { throw failure }
            return [RoutePoint(coordinate: from), RoutePoint(coordinate: to)]
        }
    }

    private func editor(_ files: [[RoutePoint]], router: FakeLegRouter) -> TripDayEditorModel {
        let trip = Trip.joining(files, id: TripID("t"), name: "T", bikeType: .road, now: Date(timeIntervalSince1970: 0))
        return TripDayEditorModel(trip: trip, isSplitMode: false, finder: nil, router: router) { _ in }!
    }

    private func camp(on model: TripDayEditorModel, offset: Double) -> PlacedStop {
        let stop = Stop(name: "Camp Ulrichen", coordinate: coordinate(11_200, offset), kind: .campsite)
        return model.trip.place([stop], near: 10_000)[0]
    }

    @Test
    func aStopOffTheLineOffersBothModesAndAPickSetsOne() async throws {
        let model = editor([file(0, 10_000), file(10_000, 20_000), file(20_000, 30_000)], router: FakeLegRouter())
        model.endDay(at: camp(on: model, offset: 20))
        #expect(model.offLineStop == nil, "a stop on the line needs no choice")

        model.endDay(at: camp(on: model, offset: 400))
        let choice = try #require(model.offLineStop)
        #expect(abs(model.trip.dayEnds[0].distance - 11_200) < 1, "the day ends on the line until a pick")
        await choice.load()
        guard case .ready(_, let spur) = choice.outAndBack, case .ready(_, let via) = choice.via else {
            Issue.record("both modes route")
            return
        }
        #expect(abs(spur - 800) < 5)
        #expect(via < spur, "a via around a stop 400 m off costs less than going there and back")

        choice.pick(.outAndBack)
        guard case .outAndBack? = model.trip.dayEnds[0].stopRoute else {
            Issue.record("the pick sets the mode")
            return
        }
        #expect(model.notes[0] == "out and back \(OBCFormat.extraDistance(meters: spur))")
        #expect(model.handles.branches.count == 1, "the map draws the spur")
        #expect(abs(model.stats[0].distanceMeters - 11_600) < 5 && abs(model.stats[1].distanceMeters - 9_200) < 5)

        model.endDay(at: camp(on: model, offset: 400))
        #expect(model.offLineStop?.current == .outAndBack, "the same stop again offers the switch")
        #expect(model.trip.dayEnds[0].stopRoute != nil, "and keeps the mode until a pick")

        model.undo()
        #expect(model.trip.dayEnds[0].stopRoute == nil)
        #expect(model.handles.branches.isEmpty)
    }

    @Test
    func whenNeitherModeRoutesTheDayEndsOnTheLine() async throws {
        let model = editor([file(0, 10_000), file(10_000, 20_000)], router: FakeLegRouter(failure: .noConnection))
        model.endDay(at: camp(on: model, offset: 400))
        let choice = try #require(model.offLineStop)
        await choice.load()
        #expect(choice.failure == .noConnection)
        choice.endOnLine()
        #expect(model.trip.dayEnds[0].stopRoute == nil)
        #expect(model.trip.dayEnds[0].name == "Camp Ulrichen")
    }

    @Test
    func aGapInsideADayBridgesOrStaysAStraightLine() async {
        let model = editor([file(0, 10_000), file(10_100, 20_000)], router: FakeLegRouter())
        model.joinDay(0)
        #expect(model.gaps.count == 1)
        #expect(model.notes[0] == "straight line 100 m")
        #expect(model.canBridge(0))
        model.bridgeGap(in: 0)
        while model.bridging != nil { await Task.yield() }
        #expect(model.gaps.isEmpty)
        #expect(model.notes[0] == nil)
        #expect(model.trip.pieceStarts.isEmpty)
    }
}
