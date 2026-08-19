import Foundation
import OBCDomain

/// The device's object-store capacities the whole-trip precheck reckons against
/// (TR8). The wire doesn't advertise them, so these mirror the shipping target's
/// firmware constants (`obc-app`: the 512 KB LM20 profile) — the client-side
/// pre-flight that keeps a trip that can't fit from failing `storageFull` at the
/// last stage. The device's own `storageFull` reject stays the backstop; a
/// device on the trimmed `nrf-mem` profile simply hits that backstop sooner.
public enum DeviceStorage {
    /// `MAX_ROUTES` on the shipping target (`firmware/obc-app/src/route.rs`).
    public static let routeCapacity = 64
    /// `MAX_TRIPS` — the epic-locked trip cap (spec §7.4: 16).
    public static let tripCapacity = 16
}

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
    ///   - routeCapacity / tripCapacity: the device catalog caps.
    public static func plan(
        stages: [StageInput],
        tripObjectID: DeviceObjectID?,
        deviceRouteCount: Int,
        deviceTripCount: Int,
        routeCapacity: Int = DeviceStorage.routeCapacity,
        tripCapacity: Int = DeviceStorage.tripCapacity
    ) -> TripUploadPlan {
        let stagePlans = stages.map { TripStagePlan(routeID: $0.routeID, action: $0.action) }
        let freshRoutes = stagePlans.reduce(0) { $0 + ($1.action == .fresh ? 1 : 0) }
        let tripAction: TripObjectAction = tripObjectID.map(TripObjectAction.replace) ?? .fresh
        let precheck = TripUploadPrecheck(
            freshRoutesNeeded: freshRoutes,
            freeRouteSlots: max(0, routeCapacity - deviceRouteCount),
            needsNewTripSlot: tripAction.isFresh,
            freeTripSlots: max(0, tripCapacity - deviceTripCount)
        )
        return TripUploadPlan(stages: stagePlans, tripObject: tripAction, precheck: precheck)
    }
}
