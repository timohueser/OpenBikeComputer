import Foundation
import OBCDomain

/// What a whole-trip upload does to one stage — the queue's per-stage verdict
/// (spec / issue #657): up-to-date by CRC → skip, on device but outdated →
/// replace in place by id, absent → a fresh upload the device assigns an id for.
public enum TripStageAction: Equatable, Sendable {
    /// The device's copy is byte-identical to what an upload would send — no bytes.
    case skip
    /// On the device under this id but outdated — replace that object in place.
    case replace(DeviceObjectID)
    /// Not on the device — a fresh upload (consumes a free route slot).
    case fresh

    /// Whether this action moves bytes (everything but `skip`).
    public var isUpload: Bool { self != .skip }
}

/// How the **trip object itself** lands (uploaded last): replace the existing
/// device trip in place, or create a fresh one (consumes a free trip slot).
public enum TripObjectAction: Equatable, Sendable {
    case fresh
    case replace(DeviceObjectID)

    public var isFresh: Bool { self == .fresh }
}

/// One stage's slot in the plan — its library id plus its verdict, in ride order.
public struct TripStagePlan: Equatable, Sendable {
    public let routeID: RouteID
    public let action: TripStageAction

    public init(routeID: RouteID, action: TripStageAction) {
        self.routeID = routeID
        self.action = action
    }
}

/// The **precheck** the queue runs before any bytes flow (issue #657): fresh
/// uploads vs. free slots, on both the route and trip catalogs. A trip that
/// can't fit fails upfront with the "delete routes on the device" guidance —
/// never `storageFull` at stage 4.
public struct TripUploadPrecheck: Equatable, Sendable {
    /// Stages that would be **fresh** uploads (each needs a free route slot).
    public let freshRoutesNeeded: Int
    /// Free route slots on the device (`routeCapacity − routes currently stored`).
    public let freeRouteSlots: Int
    /// Whether the trip object itself is a **new** trip (needs a free trip slot);
    /// a replace-by-id trip push is cap-exempt.
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

    /// How many route slots short the device is (0 = fits) — the headline the
    /// precheck-failure copy quotes ("free N routes").
    public var routeSlotDeficit: Int { max(0, freshRoutesNeeded - freeRouteSlots) }

    /// Whether the new trip object has nowhere to land.
    public var tripSlotExhausted: Bool { needsNewTripSlot && freeTripSlots < 1 }

    /// The whole trip fits — every fresh stage has a slot and (if new) the trip
    /// object does too.
    public var fits: Bool { routeSlotDeficit == 0 && !tripSlotExhausted }
}

/// The full plan a whole-trip upload executes: the per-stage queue (ride order),
/// how the trip object lands, and the precheck. Pure value — the queue driver
/// (`TripUploadModel`) turns it into transfers, the precheck gates it.
public struct TripUploadPlan: Equatable, Sendable {
    public let stages: [TripStagePlan]
    public let tripObject: TripObjectAction
    public let precheck: TripUploadPrecheck

    public init(stages: [TripStagePlan], tripObject: TripObjectAction, precheck: TripUploadPrecheck) {
        self.stages = stages
        self.tripObject = tripObject
        self.precheck = precheck
    }

    /// The stages that actually move bytes (skips excluded) — the queue's upload
    /// steps, in order.
    public var uploadStages: [TripStagePlan] { stages.filter { $0.action.isUpload } }

    /// Every stage is already up-to-date on the device.
    public var allStagesSkip: Bool { stages.allSatisfy { $0.action == .skip } }
}

/// Partitions a trip's stages into skip / replace / fresh and does the precheck
/// math (issue #657). **Pure** — the model feeds it a per-stage snapshot of the
/// reconcile state and the device catalog counts; it never touches the transport.
public enum TripUploadPlanner {
    /// One stage's reconcile snapshot, as `MainScreenModel` reads it.
    public struct StageInput: Equatable, Sendable {
        public let routeID: RouteID
        /// The device is **proven** to hold this stage's current content (the C1
        /// up-to-date badge) → skip.
        public let isUpToDate: Bool
        /// The device object id this stage is **currently** stored under, when a
        /// valid scoped link points at a still-present catalog entry (`nil` when
        /// absent — a fresh upload). Present + not up-to-date → replace by id.
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

    /// Build the plan + precheck for a trip.
    ///
    /// - Parameters:
    ///   - stages: the trip's stages, **in ride order**, each with its reconcile
    ///     snapshot.
    ///   - tripObjectID: the trip object's current device id when a valid scoped
    ///     link points at a still-present trip-catalog entry — a replace-by-id trip
    ///     push; `nil` = a fresh trip object.
    ///   - deviceRouteCount / deviceTripCount: how many routes / trips the device
    ///     currently stores (from the last route/trip catalog reconcile).
    ///   - routeCapacity / tripCapacity: optional **admission** caps, only when a
    ///     transport explicitly knows them. Protocol v4 does not advertise the
    ///     flat store's free entry count, so production leaves these `nil` and
    ///     lets the device's atomic PUT refusal remain the authority. In
    ///     particular, `obc-app`'s `MAX_ROUTES = 64` / `MAX_TRIPS = 16` are
    ///     bounded on-device menu snapshots, not storage limits.
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
