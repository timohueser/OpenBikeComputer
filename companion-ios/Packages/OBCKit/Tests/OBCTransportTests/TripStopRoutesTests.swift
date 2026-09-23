import Testing
import Foundation
@testable import OBCDomain
@testable import OBCTransport

/// Days that end at a stop off the line: the out and back and the via, as the phone's figures
/// and the uploaded day routes show them, through line changes; and gaps inside a day.
struct TripStopRoutesTests {
    /// Planar metres east and north of a fixed origin at 46.5° N.
    private static func coordinate(_ x: Double, _ y: Double = 0) -> Coordinate {
        Coordinate(latitude: 46.5 + y / 111_320, longitude: 8 + x / (111_320 * cos(46.5 * Double.pi / 180)))
    }

    /// A straight file east along y = 0, a point every 100 m.
    private func file(_ from: Double, _ to: Double, y: Double = 0) -> [RoutePoint] {
        stride(from: from, through: to, by: 100).map { RoutePoint(coordinate: Self.coordinate($0, y), elevationMeters: 500) }
    }

    /// The phone's router as a fake: a straight road between the two points, or a failure.
    struct FakeRouter: LegRouter {
        var failure: LegRouteFailure?

        func route(
            from: Coordinate, to: Coordinate, bikeType: BikeType, onDownload: @escaping @Sendable () -> Void
        ) async throws -> [RoutePoint] {
            onDownload()
            if let failure { throw failure }
            let steps = max(Int(from.distance(to: to) / 50), 1)
            return (0...steps).map { i in
                let t = Double(i) / Double(steps)
                return RoutePoint(
                    coordinate: Coordinate(
                        latitude: from.latitude + (to.latitude - from.latitude) * t,
                        longitude: from.longitude + (to.longitude - from.longitude) * t),
                    elevationMeters: 500)
            }
        }
    }

    /// Three days of 10 km; Day 1 ends at Camp Ulrichen, 400 m north of km 11.2.
    private func trip() -> Trip {
        var trip = Trip.joining(
            [file(0, 10_000), file(10_000, 20_000), file(20_000, 30_000)],
            id: TripID("t"), name: "T", bikeType: .road, now: Date(timeIntervalSince1970: 0))
        let camp = Stop(name: "Camp Ulrichen", coordinate: Self.coordinate(11_200, 400), kind: .campsite)
        let ended = trip.endDay(0, at: trip.place([camp], near: 10_000)[0])
        #expect(ended)
        return trip
    }

    /// The phone's day figures match the device's view of the uploaded day routes.
    private func expectFiguresMatchTheUpload(_ trip: Trip) {
        let phone = trip.dayStats().map(\.distanceMeters)
        let device = trip.dayRoutes().map(\.distanceMeters)
        #expect(phone.count == device.count)
        for (p, d) in zip(phone, device) {
            #expect(abs(p - d) <= d * 0.01, "phone \(p) m against device \(d) m")
        }
    }

    @Test
    func anOutAndBackAddsTheSpurBothWaysAndLeavesTheLine() async throws {
        var trip = trip()
        let route = try await trip.routeOutAndBack(0, with: FakeRouter())
        #expect(abs(trip.extraMeters(route, at: 0) - 800) < 5, "400 m out, 400 m back")
        let set = trip.setStopRoute(0, route)
        #expect(set)
        #expect(trip.line.count == 301, "the main line does not change")
        expectFiguresMatchTheUpload(trip)

        let days = trip.dayRoutes()
        #expect(abs(days[0].distanceMeters - 11_600) < 20)
        #expect(abs(days[1].distanceMeters - (8_800 + 400)) < 20)
        #expect(days[0].points.last?.coordinate.distance(to: Self.coordinate(11_200, 400)) ?? .infinity < 1, "Day 1 ends at the camp")
        #expect(days[1].points.first?.coordinate.distance(to: Self.coordinate(11_200, 400)) ?? .infinity < 1, "Day 2 starts there")

        let object = trip.tripObject(days: days, dayObjectIDs: [DeviceObjectID(1), DeviceObjectID(2), DeviceObjectID(3)])
        #expect(object.days[0].joinMeters == 0)
        #expect(abs(Double(object.days[0].leaveMeters) - 11_200) < 20, "Day 1 leaves the line at the junction")
        #expect(abs(Double(object.days[1].joinMeters) - 400) < 20, "Day 2 joins it after the spur back")
        #expect(object.days[1].leaveMeters == .max)
        #expect(object.days[2] == TripObjectCodec.Day.whole(DeviceObjectID(3)))
    }

    @Test
    func aViaRoutesThroughTheStopAndIsTheMainLine() async throws {
        var trip = trip()
        let route = try await trip.routeVia(0, with: FakeRouter())
        guard case .via(_, _, let leave, let rejoin) = route else { Issue.record("not a via"); return }
        #expect(abs(leave - 9_200) < 1 && abs(rejoin - 13_200) < 1, "2 km either side of the stop's line point")
        let legs = 2 * (2_000.0 * 2_000 + 400 * 400).squareRoot()
        #expect(abs(trip.extraMeters(route, at: 0) - (legs - 4_000)) < 5)
        let set = trip.setStopRoute(0, route)
        #expect(set)
        expectFiguresMatchTheUpload(trip)

        let days = trip.dayRoutes()
        let camp = Self.coordinate(11_200, 400)
        #expect(days[0].points.last?.coordinate.distance(to: camp) ?? .infinity < 1)
        #expect(days[1].points.first?.coordinate.distance(to: camp) ?? .infinity < 1)
        #expect(!days[0].points.contains { $0.coordinate.distance(to: Self.coordinate(10_500)) < 1 }, "the old section is not ridden")
        #expect(days.allSatisfy { $0.joinMeters == 0 && $0.leaveMeters == .max }, "a via is main line")
    }

    @Test
    func aDayEndCannotMoveIntoAVia() async throws {
        var trip = trip()
        trip.setStopRoute(0, try await trip.routeVia(0, with: FakeRouter()))
        #expect(abs((trip.endRange(of: 1)?.lowerBound ?? 0) - (13_200 + Trip.minimumDayMeters)) < 1)
        let added = trip.addDayEnd(at: 12_000)
        #expect(added == 1)
        #expect(trip.dayEnds[1].distance > 13_200, "a new end in the next day stays past the rejoin")
    }

    @Test
    func aRouterFailureLeavesTheDayOnTheLine() async {
        let trip = trip()
        await #expect(throws: LegRouteFailure.noRoad) { try await trip.routeOutAndBack(0, with: FakeRouter(failure: .noRoad)) }
        await #expect(throws: LegRouteFailure.noConnection) { try await trip.routeVia(0, with: FakeRouter(failure: .noConnection)) }
        #expect(trip.dayEnds[0].stopRoute == nil)
        #expect(trip.dayEnds[0].name == "Camp Ulrichen", "the day end on the line keeps the stop's name")
        #expect(abs(trip.dayEnds[0].distance - 11_200) < 1)
    }

    @Test
    func stopRoutesFollowALineChangeWhileTheirJunctionsStayOnIt() async throws {
        var trip = trip()
        let via = try await trip.routeVia(0, with: FakeRouter())
        trip.setStopRoute(0, via)
        trip.append(file(30_000, 40_000), name: "Extra")
        guard case .via(_, _, let keptLeave, let keptRejoin)? = trip.dayEnds[0].stopRoute else {
            Issue.record("an appended day drops the via")
            return
        }
        #expect(abs(keptLeave - 9_200) < 1 && abs(keptRejoin - 13_200) < 1, "an appended day leaves the via as it is")

        var reversed = trip
        reversed.reverse()
        guard case .via(let to, let from, let leave, let rejoin)? = reversed.dayEnds[2].stopRoute,
            case .via(let oldTo, let oldFrom, _, _) = via
        else { Issue.record("the via did not survive the reverse"); return }
        #expect(abs(leave - (40_000 - 13_200)) < 1 && abs(rejoin - (40_000 - 9_200)) < 1)
        #expect(to == oldFrom.reversed() && from == oldTo.reversed(), "the legs swap and turn around")
        expectFiguresMatchTheUpload(reversed)

        var moved = trip
        moved.line = moved.line.map { point in
            RoutePoint(coordinate: Coordinate(latitude: point.coordinate.latitude + 100 / 111_320, longitude: point.coordinate.longitude))
        }
        moved.reproject()
        #expect(moved.dayEnds[0].stopRoute == nil, "junctions 100 m off the new line: the day ends on it again")
        #expect(moved.dayEnds[0].stop != nil)
    }

    @Test
    func aGapInsideADayBridgesByRouteAndAGapAtADayEndStays() async throws {
        // Files meet 150 m apart: a gap, not a transfer. Joining the days puts it inside a day.
        var trip = Trip.joining(
            [file(0, 10_000), file(10_150, 20_000), file(23_000, 30_000)],
            id: TripID("t"), name: "T", bikeType: .road, now: Date(timeIntervalSince1970: 0))
        #expect(trip.gapsInsideDays().isEmpty, "every gap sits at a day end")
        #expect(trip.endsAtTransfer(1), "3 km apart: a transfer")
        let removedTransfer = trip.removeDayEnd(1)
        #expect(!removedTransfer, "a transfer never moves inside a day")
        let removed = trip.removeDayEnd(0)
        #expect(removed)

        let gaps = trip.gapsInsideDays()
        #expect(gaps.count == 1)
        let gap = try #require(gaps.first)
        #expect(gap.day == 0 && abs(gap.meters - 150) < 1)

        await #expect(throws: LegRouteFailure.noRoad) { try await trip.routeBridge(gap, with: FakeRouter(failure: .noRoad)) }
        let straight = trip.dayRoutes()[0].distanceMeters
        #expect(abs(straight - 19_800 - 150) < 20, "unbridged, the day rides the gap as a straight line")

        let leg = try await trip.routeBridge(gap, with: FakeRouter())
        let bridged = trip.bridge(gap, with: leg)
        #expect(bridged)
        #expect(trip.gapsInsideDays().isEmpty)
        #expect(trip.pieceStarts.count == 1, "the gap at the transfer stays")
        #expect(trip.endsAtTransfer(0))
        let again = trip.bridge(gap, with: leg)
        #expect(!again, "a bridged gap is gone")
        expectFiguresMatchTheUpload(trip)
    }
}
