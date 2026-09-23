import Testing
import Foundation
@testable import OBCDomain

/// The trip review: rides grouped by trip day, the ridden parts of the line, the totals, and the
/// offer to even out the days after a day that ended far from its plan.
struct TripReviewTests {
    /// Planar metres east and north of a fixed origin at 46.5° N.
    private func coordinate(_ x: Double, _ y: Double = 0) -> Coordinate {
        Coordinate(latitude: 46.5 + y / 111_320, longitude: 8 + x / (111_320 * cos(46.5 * Double.pi / 180)))
    }

    /// A straight file along y = 0, a point every 100 m.
    private func file(_ from: Double, _ to: Double) -> [RoutePoint] {
        stride(from: from, through: to, by: to >= from ? 100 : -100).map { RoutePoint(coordinate: coordinate($0)) }
    }

    /// One day per `(from, to)` pair in km. A file that starts away from the last end is a transfer.
    private func trip(_ days: [(Double, Double)]) -> Trip {
        Trip.joining(
            days.map { file($0.0 * 1_000, $0.1 * 1_000) },
            id: TripID("t"), name: "T", bikeType: .road, now: Date(timeIntervalSince1970: 0))
    }

    /// A track along y = 0 from `from` to `to` km, a sample every 20 m.
    private func track(_ from: Double, _ to: Double) -> [RidePoint] {
        stride(from: from * 1_000, through: to * 1_000, by: to >= from ? 20 : -20).map { point(coordinate($0)) }
    }

    private func point(_ coordinate: Coordinate, elevation: Double? = nil) -> RidePoint {
        RidePoint(timestamp: Date(timeIntervalSince1970: 0), coordinate: coordinate, elevationMeters: elevation)
    }

    private func ride(
        _ id: String, day: Int, of trip: Trip, km: Double = 10, hour: Double = 0, key: UInt64? = nil
    ) -> RideSummary {
        RideSummary(
            id: RideID(id), name: id, date: Date(timeIntervalSince1970: hour * 3_600), distanceMeters: km * 1_000,
            movingTime: 1_800, climbMeters: 100,
            trip: RideTrip(key: key ?? trip.key, dayIndex: day, dayCount: trip.dayCount, name: trip.name))
    }

    @Test
    func ridesGroupByTripKeyAndDayAndOnlyThoseCount() throws {
        let trip = trip([(0, 10), (10, 20), (20, 30)])
        let rides = [
            ride("late", day: 1, of: trip, km: 4, hour: 30),
            ride("d1", day: 0, of: trip, km: 10.2, hour: 1),
            ride("early", day: 1, of: trip, km: 6, hour: 26),
            ride("other trip", day: 0, of: trip, key: trip.key &+ 1),
            RideSummary(id: RideID("loose"), name: "loose", date: .now, distanceMeters: 50_000),
        ]
        let review = try #require(TripReview(trip: trip, rides: rides, tracks: [:]))

        #expect(review.days.map { $0.rides.map(\.id.rawValue) } == [["d1"], ["early", "late"], []])
        #expect(review.days[1].totals.distanceMeters == 10_000, "a day with two rides counts both")
        #expect(review.totals == RideTotals(rides.prefix(3)))
        #expect(abs(review.plannedMeters - 30_000) < 1)
        #expect(abs(review.days[2].plannedMeters - 10_000) < 1)
        #expect(review.currentDay == 2)
    }

    @Test
    func theReviewStartsWithTheFirstRideAndEndsWithTheLastDay() throws {
        let trip = trip([(0, 10), (10, 20)])
        #expect(TripReview(trip: trip, rides: [ride("other", day: 0, of: trip, key: 99)], tracks: [:]) == nil)
        let done = try #require(TripReview(trip: trip, rides: [ride("d2", day: 1, of: trip)], tracks: [:]))
        #expect(done.currentDay == nil)
    }

    @Test
    func aTransferCountsNothingPlannedOrRidden() throws {
        // Day 2 starts 5 km on by train.
        let trip = trip([(0, 10), (15, 25)])
        #expect(trip.endsAtTransfer(0))
        let review = try #require(TripReview(
            trip: trip, rides: [ride("d1", day: 0, of: trip), ride("d2", day: 1, of: trip, hour: 24)],
            tracks: [RideID("d1"): track(0, 10), RideID("d2"): track(15, 25)]))
        #expect(abs(review.plannedMeters - 20_000) < 1)
        #expect(review.ridden.count == 1)
        #expect(abs(review.ridden[0].upperBound - 20_000) < 1, "the ridden part runs on over the gap")
    }

    @Test
    func theRiddenPartFollowsTheTrackAndSkipsADetour() throws {
        let trip = trip([(0, 10), (10, 20)])
        // Day 1 rides 0–4 km, leaves the line 1 km north until 6 km, and rides on to 8 km.
        let detour = track(0, 4) + [point(coordinate(4_000, 1_000)), point(coordinate(6_000, 1_000))] + track(6, 8)
        // Day 2 starts again at 7.5 km, so the two rides overlap.
        let review = try #require(TripReview(
            trip: trip, rides: [ride("d1", day: 0, of: trip), ride("d2", day: 1, of: trip, hour: 24)],
            tracks: [RideID("d1"): detour, RideID("d2"): track(7.5, 20)]))
        #expect(review.ridden.count == 2)
        #expect(abs(review.ridden[0].upperBound - 4_000) < Trip.coverSampleMeters)
        #expect(abs(review.ridden[1].lowerBound - 6_000) < Trip.coverSampleMeters)
        #expect(abs(review.ridden[1].upperBound - 20_000) < 1)
        #expect(abs(review.days[0].endedAt! - 8_000) < 1)
    }

    @Test
    func theReturnOfAnOutAndBackStaysOnTheReturnLeg() throws {
        // Out 10 km, back to the start, then 10 km the other way.
        let trip = trip([(0, 10), (10, 0), (0, -10)])
        let review = try #require(TripReview(
            trip: trip, rides: [ride("d2", day: 1, of: trip)], tracks: [RideID("d2"): track(10, 0)]))
        #expect(review.ridden.count == 1)
        #expect(abs(review.ridden[0].lowerBound - 10_000) < Trip.coverSampleMeters)
        #expect(abs(review.ridden[0].upperBound - 20_000) < 1)
        #expect(abs(try #require(review.days[1].endedAt) - 20_000) < 1)
    }

    @Test
    func aSparseTrackCoversTheLineInOnePart() throws {
        let trip = trip([(0, 10)])
        // A sample every 1 km, farther apart than the join allowance alone.
        let sparse = stride(from: 0.0, through: 10_000, by: 1_000).map { point(coordinate($0)) }
        let review = try #require(TripReview(
            trip: trip, rides: [ride("d1", day: 0, of: trip)], tracks: [RideID("d1"): sparse]))
        #expect(review.ridden.count == 1)
        #expect(abs(review.ridden[0].upperBound - 10_000) < 1)
    }

    @Test
    func theMapRunsSplitAtTheRiddenBoundAndCrossATransfer() throws {
        let trip = trip([(0, 10), (15, 25)])
        let review = try #require(TripReview(
            trip: trip, rides: [ride("d1", day: 0, of: trip)], tracks: [RideID("d1"): track(0, 4.04)]))
        let runs = trip.runs(ridden: review.ridden)
        #expect(runs.map(\.kind) == [.ridden, .planned, .transfer, .planned])
        #expect(runs[0].coordinates.last!.distance(to: runs[1].coordinates.first!) == 0)
        #expect(runs[0].coordinates.last!.distance(to: coordinate(4_040)) < 1, "the cut falls inside a segment")
        #expect(runs[2].coordinates.map { $0.distance(to: coordinate(10_000)) }.first! < 1)
        #expect(runs[2].coordinates.map { $0.distance(to: coordinate(15_000)) }.last! < 1)
    }

    @Test
    func theHighPointAndTheBiggestDayComeFromTheRides() throws {
        let trip = trip([(0, 10), (10, 20), (20, 30)])
        let tracks = [
            RideID("d1"): [point(coordinate(0), elevation: 500), point(coordinate(5_000), elevation: 2_431)],
            RideID("d2"): [point(coordinate(10_000), elevation: 900)],
        ]
        let one = try #require(TripReview(trip: trip, rides: [ride("d1", day: 0, of: trip, km: 12)], tracks: tracks))
        #expect(one.highPoint?.elevationMeters == 2_431)
        #expect(one.biggestDay == nil, "one ridden day is no contest")
        let two = try #require(TripReview(
            trip: trip, rides: [ride("d1", day: 0, of: trip, km: 12), ride("d2", day: 1, of: trip, km: 9, hour: 24)],
            tracks: tracks))
        #expect(two.biggestDay == 0)
    }

    @Test
    func anEarlyStopOffersToEvenOutTheDaysUpToTheNextTransfer() throws {
        // Days 1–3 on one piece, then a train, then days 4–5.
        let trip = trip([(0, 10), (10, 20), (20, 30), (31, 41), (41, 51)])
        let tracks = [RideID("d1"): track(0, 4)]
        let review = try #require(TripReview(trip: trip, rides: [ride("d1", day: 0, of: trip)], tracks: tracks))
        let offer = try #require(review.rebalance)
        #expect(offer.day == 0)
        #expect(offer.ride == RideID("d1"))
        #expect(offer.days == 1...2, "days after the train keep their plan")
        #expect(abs(offer.fixedBefore - 4_000) < 1)
        #expect(abs(offer.shortfall - 6_000) < 1)

        // Riding on past the day end offers it too: the days after it are shorter.
        let far = try #require(TripReview(
            trip: trip, rides: [ride("d1", day: 0, of: trip)], tracks: [RideID("d1"): track(0, 16)]))
        #expect(far.rebalance.map { $0.shortfall < 0 } == true)
    }

    @Test
    func noOfferWhenTheDayEndedNearItsPlanOrNothingCanShare() {
        let trip = trip([(0, 10), (10, 20), (20, 30), (31, 41)])
        func offer(day: Int, to km: Double, earlier: [RideSummary] = []) -> RebalanceOffer? {
            let start = trip.dayEnds[safe: day - 1]?.distance ?? 0
            let rides = earlier + [ride("d", day: day, of: trip, hour: 100)]
            return TripReview(trip: trip, rides: rides, tracks: [RideID("d"): track(start / 1_000, km)])?.rebalance
        }
        let limitKm = TripReview.rebalanceMinMeters / 1_000
        #expect(offer(day: 0, to: 10 - limitKm + 0.5) == nil, "within the threshold")
        #expect(offer(day: 1, to: 14) == nil, "one day left before the train")
        #expect(offer(day: 2, to: 24) == nil, "the day ends at a transfer")
        // An early day 1 is old news once day 2 is ridden to plan.
        let early = ride("e", day: 0, of: trip, hour: 1)
        #expect(offer(day: 0, to: 4) != nil)
        #expect(TripReview(
            trip: trip, rides: [early, ride("d", day: 1, of: trip, hour: 24)],
            tracks: [RideID("e"): track(0, 4), RideID("d"): track(10, 20)])?.rebalance == nil)
    }
}

extension Array {
    fileprivate subscript(safe index: Int) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}
