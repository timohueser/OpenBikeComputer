/**
 * What deleting a trip *with its routes* may actually delete — computed up front, so the
 * confirmation can say the truth before anything happens.
 *
 * Pure bookkeeping over the dashboard's trip views (`dashboard.trips`), separated from the page
 * for the same reason `elevation.ts` is: the rules are easy to state and easy to get subtly
 * wrong, so they live where a unit test can hold them still.
 *
 * The rules:
 *
 *  1. A route that is **also a stage of another trip** is never deleted with this one — the other
 *     trip still points at it, and deleting it would leave that trip a dangling stage. Shared
 *     stages are excluded from the deletable set and named in the dialog's note.
 *  2. **Unreadable stage lists poison the whole offer.** If any *other* trip's `detail` is null,
 *     nothing can prove a route of this trip isn't shared with it — so the offer degrades to
 *     "delete the trip only", with the reason. Same when this trip's own list is unreadable:
 *     there is no route list to offer.
 *  3. Duplicate stage ids within the trip count once, and ids that no longer resolve to a route
 *     on the device (dangling stages) are not "routes" at all — nothing to delete, nothing to
 *     count in the dialog's numbers.
 */

/** The slice of a `TripView` this module reads — structural, so tests need no protocol types. */
export interface TripStages {
    readonly objectId: bigint;
    readonly name: string;
    /** The trip's stage ids, or null when the trip object could not be read. */
    readonly detail: { readonly stages: readonly bigint[] } | null;
}

export type TripDeletePlan =
    | {
          /** Only "delete the trip only" is offered; `reason` (when set) says why in one line. */
          readonly offer: "trip-only";
          readonly reason: string | null;
      }
    | {
          readonly offer: "both";
          /** The route ids deleted by the second option — deduped, existing, not shared. */
          readonly deletable: readonly bigint[];
          /** The trip's unique stage ids that exist as routes — the dialog's "its N routes". */
          readonly routeCount: number;
          /** "2 of its 4 routes are also in “Other trip” and will stay" — null when none are. */
          readonly note: string | null;
      };

/**
 * Decide what the delete-trip confirmation may offer for `trip`, given every trip on the card
 * (`allTrips` includes `trip` itself) and the ids of the routes that actually exist.
 */
export function planTripDelete(
    trip: TripStages,
    allTrips: readonly TripStages[],
    existingRouteIds: ReadonlySet<bigint>,
): TripDeletePlan {
    if (trip.detail === null) {
        return { offer: "trip-only", reason: "This trip's own stage list could not be read." };
    }
    const others = allTrips.filter((t) => t.objectId !== trip.objectId);
    if (others.some((t) => t.detail === null)) {
        return {
            offer: "trip-only",
            reason: "Another trip's stage list could not be read, so no routes are deleted with this one.",
        };
    }

    // Unique stage ids that are still routes on the device, in stage order.
    const routes = [...new Set(trip.detail.stages)].filter((id) => existingRouteIds.has(id));
    if (routes.length === 0) return { offer: "trip-only", reason: null };

    // A route is shared when any other trip lists it too; remember who, for the note.
    const sharedIn = new Map<bigint, TripStages[]>();
    for (const other of others) {
        for (const id of new Set(other.detail?.stages ?? [])) {
            if (routes.includes(id)) sharedIn.set(id, [...(sharedIn.get(id) ?? []), other]);
        }
    }
    const deletable = routes.filter((id) => !sharedIn.has(id));
    if (deletable.length === 0) {
        return { offer: "trip-only", reason: "All of its routes are also in other trips and would stay anyway." };
    }

    return { offer: "both", deletable, routeCount: routes.length, note: sharedNote(routes.length, sharedIn) };
}

/**
 * The dialog's up-front sentence about the routes that stay: how many, and with whom. Only
 * reached from the "both" offer, where at least one route is shared and at least one is not —
 * so `0 < sharedIn.size < routeCount`, and "of its N routes" is always plural.
 */
function sharedNote(routeCount: number, sharedIn: ReadonlyMap<bigint, readonly TripStages[]>): string | null {
    if (sharedIn.size === 0) return null;
    const names = [...new Set([...sharedIn.values()].flat().map((t) => t.name || `Trip ${t.objectId}`))];
    const listed =
        names.length === 1
            ? `“${names[0]}”`
            : `${names
                  .slice(0, -1)
                  .map((n) => `“${n}”`)
                  .join(", ")} and “${names[names.length - 1]}”`;
    const verb = sharedIn.size === 1 ? "is" : "are";
    return `${sharedIn.size} of its ${routeCount} routes ${verb} also in ${listed} and will stay.`;
}
