import Testing
import OBCDomain
@testable import OBCTransport

/// The pure whole-trip upload planner (TR8, issue #657): the skip / replace /
/// fresh partition and the precheck slot math — no transport, no model.
struct TripUploadPlannerTests {
    private func stage(_ id: String, upToDate: Bool = false, committed: UInt16? = nil) -> TripUploadPlanner.StageInput {
        TripUploadPlanner.StageInput(
            routeID: RouteID(id), isUpToDate: upToDate,
            committedObjectID: committed.map(DeviceObjectID.init))
    }

    // MARK: Partition

    @Test
    func partitionsStagesIntoSkipReplaceFresh() {
        let plan = TripUploadPlanner.plan(
            stages: [
                stage("a", upToDate: true, committed: 7),   // on device, current → skip
                stage("b", committed: 12),                    // on device, outdated → replace
                stage("c"),                                   // absent → fresh
            ],
            tripObjectID: nil,
            deviceRouteCount: 2, deviceTripCount: 0
        )
        #expect(plan.stages.map(\.action) == [.skip, .replace(DeviceObjectID(12)), .fresh])
        #expect(plan.uploadStages.count == 2)
        #expect(!plan.allStagesSkip)
        #expect(plan.tripObject == .fresh)  // no existing device trip link
    }

    @Test
    func upToDateBeatsAPresentLink() {
        // A stage that's both up-to-date and on the device is a skip, not a replace.
        let plan = TripUploadPlanner.plan(
            stages: [stage("a", upToDate: true, committed: 7)],
            tripObjectID: DeviceObjectID(3),
            deviceRouteCount: 1, deviceTripCount: 1
        )
        #expect(plan.stages.map(\.action) == [.skip])
        #expect(plan.allStagesSkip)
        #expect(plan.tripObject == .replace(DeviceObjectID(3)))
    }

    // MARK: Precheck math

    @Test
    func residentMenuLimitsAreNotStorageLimits() {
        // The flat-store benchmark card has 249 routes. The device intentionally
        // keeps only the newest 64 resident for its menu, but the store itself has
        // room for 1,916 catalog entries. With no advertised admission cap the
        // planner must proceed and let the device remain the storage authority.
        let plan = TripUploadPlanner.plan(
            stages: [stage("a"), stage("b")],
            tripObjectID: nil,
            deviceRouteCount: 249, deviceTripCount: 0
        )
        #expect(plan.precheck.freeRouteSlots == .max)
        #expect(plan.precheck.freeTripSlots == .max)
        #expect(plan.precheck.fits)
    }

    @Test
    func precheckFitsWhenFreshStagesHaveSlots() {
        let plan = TripUploadPlanner.plan(
            stages: [stage("a"), stage("b")],  // 2 fresh
            tripObjectID: nil,
            deviceRouteCount: 60, deviceTripCount: 0,
            routeCapacity: 64, tripCapacity: 16
        )
        #expect(plan.precheck.freshRoutesNeeded == 2)
        #expect(plan.precheck.freeRouteSlots == 4)
        #expect(plan.precheck.fits)
        #expect(plan.precheck.routeSlotDeficit == 0)
    }

    @Test
    func precheckFailsWhenFreshStagesOutrunSlots() {
        let plan = TripUploadPlanner.plan(
            stages: [stage("a"), stage("b"), stage("c")],  // 3 fresh
            tripObjectID: nil,
            deviceRouteCount: 63, deviceTripCount: 0,
            routeCapacity: 64, tripCapacity: 16
        )
        // Only one free slot for three fresh routes.
        #expect(plan.precheck.freeRouteSlots == 1)
        #expect(!plan.precheck.fits)
        #expect(plan.precheck.routeSlotDeficit == 2)
    }

    @Test
    func replacedAndSkippedStagesNeverCountAgainstSlots() {
        // A device at the route cap still fits a trip whose stages are all
        // replace-by-id / skip (replace is cap-exempt on the device).
        let plan = TripUploadPlanner.plan(
            stages: [stage("a", upToDate: true, committed: 1), stage("b", committed: 2)],
            tripObjectID: DeviceObjectID(9),
            deviceRouteCount: 64, deviceTripCount: 5,
            routeCapacity: 64, tripCapacity: 16
        )
        #expect(plan.precheck.freshRoutesNeeded == 0)
        #expect(plan.precheck.fits)
    }

    @Test
    func precheckFailsWhenANewTripHasNoTripSlot() {
        let plan = TripUploadPlanner.plan(
            stages: [stage("a", upToDate: true, committed: 1)],  // no fresh routes
            tripObjectID: nil,  // a new trip object
            deviceRouteCount: 0, deviceTripCount: 16,  // trip catalog full
            routeCapacity: 64, tripCapacity: 16
        )
        #expect(plan.precheck.needsNewTripSlot)
        #expect(plan.precheck.tripSlotExhausted)
        #expect(!plan.precheck.fits)
        // The route side is fine — the deficit is the trip slot.
        #expect(plan.precheck.routeSlotDeficit == 0)
    }

    // MARK: Idempotent re-run

    @Test
    func reRunOfALandedTripIsAllSkips() {
        // Every stage up-to-date + the trip already on the device → a pure-skip
        // plan that fits trivially (nothing fresh to send).
        let plan = TripUploadPlanner.plan(
            stages: [stage("a", upToDate: true, committed: 1), stage("b", upToDate: true, committed: 2)],
            tripObjectID: DeviceObjectID(9),
            deviceRouteCount: 2, deviceTripCount: 1
        )
        #expect(plan.allStagesSkip)
        #expect(plan.uploadStages.isEmpty)
        #expect(plan.tripObject == .replace(DeviceObjectID(9)))
        #expect(plan.precheck.fits)
    }
}
