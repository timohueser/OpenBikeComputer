import Foundation
import OBCDomain

/// What a whole-trip upload does to one stage: up to date by CRC is a skip, on the device but
/// outdated is a replace in place by id, and absent is a fresh upload the device assigns an id for.
public enum TripStageAction: Equatable, Sendable {
    /// The device's copy is byte-identical to what an upload would send, so no bytes move.
    case skip
    /// On the device under this id but outdated: replace that object in place.
    case replace(DeviceObjectID)
    /// Not on the device: a fresh upload, which consumes a free route slot.
    case fresh

    /// Whether this action moves bytes.
    public var isUpload: Bool { self != .skip }
}

/// How the trip object itself lands, uploaded last: replace the existing device trip in place, or
/// create a fresh one, which consumes a free trip slot.
public enum TripObjectAction: Equatable, Sendable {
    case fresh
    case replace(DeviceObjectID)

    public var isFresh: Bool { self == .fresh }
}

/// One stage's slot in the plan: its library id plus its verdict, in ride order.
public struct TripStagePlan: Equatable, Sendable {
    public let routeID: RouteID
    public let action: TripStageAction

    public init(routeID: RouteID, action: TripStageAction) {
        self.routeID = routeID
        self.action = action
    }
}

/// The precheck the queue runs before any bytes flow: fresh uploads against free slots, on both
/// the route and the trip catalogs. A trip that cannot fit fails up front with guidance, rather
/// than filling the device partway through.
public struct TripUploadPrecheck: Equatable, Sendable {
    /// Stages that would be fresh uploads; each needs a free route slot.
    public let freshRoutesNeeded: Int
    /// Free route slots on the device (`routeCapacity − routes currently stored`).
    public let freeRouteSlots: Int
    /// Whether the trip object itself is a new trip, which needs a free trip slot. A
    /// replace-by-id trip push is exempt from the cap.
    public let needsNewTripSlot: Bool
    /// Free trip slots on the device (`tripCapacity − trips currently stored`).
    public let freeTripSlots: Int

    public init(
        freshRoutesNeeded: Int, freeRouteSlots: Int,
        needsNewTripSlot: Bool, freeTripSlots: Int
    ) {
        self.freshRoutesNeeded = freshRoutesNeeded
        self.freeRouteSlots = freeRouteSlots
        self.needsNewTripSlot = needsNewTripSlot
        self.freeTripSlots = freeTripSlots
    }

    /// How many route slots short the device is; zero fits. The headline the precheck-failure copy
    /// quotes.
    public var routeSlotDeficit: Int { max(0, freshRoutesNeeded - freeRouteSlots) }

    /// Whether the new trip object has nowhere to land.
    public var tripSlotExhausted: Bool { needsNewTripSlot && freeTripSlots < 1 }

    /// The whole trip fits: every fresh stage has a slot, and the trip object has one when it is
    /// new.
    public var fits: Bool { routeSlotDeficit == 0 && !tripSlotExhausted }
}

/// The full plan a whole-trip upload executes: the per-stage queue in ride order, how the trip
/// object lands, and the precheck. A pure value: the queue driver turns it into transfers, and the
/// precheck gates it.
public struct TripUploadPlan: Equatable, Sendable {
    public let stages: [TripStagePlan]
    public let tripObject: TripObjectAction
    public let precheck: TripUploadPrecheck

    public init(stages: [TripStagePlan], tripObject: TripObjectAction, precheck: TripUploadPrecheck) {
        self.stages = stages
        self.tripObject = tripObject
        self.precheck = precheck
    }

    /// The stages that actually move bytes, skips excluded, in order.
    public var uploadStages: [TripStagePlan] { stages.filter { $0.action.isUpload } }

    /// Every stage is already up-to-date on the device.
    public var allStagesSkip: Bool { stages.allSatisfy { $0.action == .skip } }
}

/// Partitions a trip's stages into skip, replace and fresh, and does the precheck math. Pure: the
/// model feeds it a per-stage snapshot of the reconcile state and the device catalog counts, and
/// it never touches the transport.
public enum TripUploadPlanner {
    /// One stage's reconcile snapshot, as `MainScreenModel` reads it.
    public struct StageInput: Equatable, Sendable {
        public let routeID: RouteID
        /// The device is proven to hold this stage's current content, so the stage is skipped.
        public let isUpToDate: Bool
        /// The device object id this stage is currently stored under, when a valid scoped link
        /// points at a still-present catalog entry. Nil means absent, so a fresh upload; present
        /// but not up to date means a replace by id.
        public let committedObjectID: DeviceObjectID?

        public init(routeID: RouteID, isUpToDate: Bool, committedObjectID: DeviceObjectID?) {
            self.routeID = routeID
            self.isUpToDate = isUpToDate
            self.committedObjectID = committedObjectID
        }

        /// This stage's queue verdict.
        var action: TripStageAction {
            if isUpToDate { return .skip }
            if let committedObjectID { return .replace(committedObjectID) }
            return .fresh
        }
    }

    /// Build the plan and the precheck for a trip.
    ///
    /// - Parameters:
    ///   - stages: the trip's stages, in ride order, each with its reconcile snapshot.
    ///   - tripObjectID: the trip object's current device id when a valid scoped link points at a
    ///     still-present trip-catalog entry, which makes the push a replace by id. Nil means a
    ///     fresh trip object.
    ///   - deviceRouteCount, deviceTripCount: how many routes and trips the device currently
    ///     stores, from the last catalog reconcile.
    ///   - routeCapacity, tripCapacity: optional admission caps, passed only when a transport
    ///     explicitly knows them. The protocol does not advertise the store's free entry count, so
    ///     production leaves these nil and the device's atomic refusal stays the authority. The
    ///     device's route and trip menu limits are bounded on-device snapshots, not storage limits.
    public static func plan(
        stages: [StageInput],
        tripObjectID: DeviceObjectID?,
        deviceRouteCount: Int,
        deviceTripCount: Int,
        routeCapacity: Int? = nil,
        tripCapacity: Int? = nil
    ) -> TripUploadPlan {
        let stagePlans = stages.map { TripStagePlan(routeID: $0.routeID, action: $0.action) }
        let freshRoutes = stagePlans.reduce(0) { $0 + ($1.action == .fresh ? 1 : 0) }
        let tripAction: TripObjectAction = tripObjectID.map(TripObjectAction.replace) ?? .fresh
        let precheck = TripUploadPrecheck(
            freshRoutesNeeded: freshRoutes,
            freeRouteSlots: routeCapacity.map { max(0, $0 - deviceRouteCount) } ?? .max,
            needsNewTripSlot: tripAction.isFresh,
            freeTripSlots: tripCapacity.map { max(0, $0 - deviceTripCount) } ?? .max
        )
        return TripUploadPlan(stages: stagePlans, tripObject: tripAction, precheck: precheck)
    }
}
