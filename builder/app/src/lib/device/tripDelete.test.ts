/**
 * The delete-trip offer's rules (`tripDelete.ts`), held still: shared stages are protected,
 * unreadable stage lists degrade the offer, duplicates and dangling ids never inflate the counts.
 */

import { describe, expect, it } from "vitest";

import { planTripDelete, type TripStages } from "./tripDelete";

const trip = (objectId: number, name: string, stages: readonly number[] | null): TripStages => ({
    objectId: BigInt(objectId),
    name,
    detail: stages === null ? null : { stages: stages.map(BigInt) },
});

const routes = (...ids: number[]) => new Set(ids.map(BigInt));
const ids = (...values: number[]) => values.map(BigInt);

describe("planTripDelete", () => {
    it("offers both options with every unshared existing route deletable", () => {
        const t = trip(1, "Traverse", [2, 3, 4]);
        const plan = planTripDelete(t, [t], routes(2, 3, 4));
        expect(plan).toEqual({ offer: "both", deletable: ids(2, 3, 4), routeCount: 3, note: null });
    });

    it("protects a route that is also a stage of another trip, and says so", () => {
        const t = trip(1, "Traverse", [2, 3, 4, 5]);
        const other = trip(9, "Other trip", [3, 5, 77]);
        const plan = planTripDelete(t, [t, other], routes(2, 3, 4, 5, 77));
        expect(plan).toEqual({
            offer: "both",
            deletable: ids(2, 4),
            routeCount: 4,
            note: "2 of its 4 routes are also in “Other trip” and will stay.",
        });
    });

    it("names every trip a shared route belongs to", () => {
        const t = trip(1, "Traverse", [2, 3]);
        const a = trip(8, "Alps", [2]);
        const b = trip(9, "Baltic", [3]);
        const plan = planTripDelete(t, [t, a, b], routes(2, 3));
        expect(plan).toMatchObject({
            offer: "trip-only",
            reason: "All of its routes are also in other trips and would stay anyway.",
        });
        // With one route of three still deletable, both names appear in the note.
        const t2 = trip(1, "Traverse", [2, 3, 4]);
        const plan2 = planTripDelete(t2, [t2, a, b], routes(2, 3, 4));
        expect(plan2).toMatchObject({
            deletable: ids(4),
            note: "2 of its 3 routes are also in “Alps” and “Baltic” and will stay.",
        });
    });

    it("counts a duplicated stage id once", () => {
        const t = trip(1, "Loop twice", [2, 2, 3]);
        const plan = planTripDelete(t, [t], routes(2, 3));
        expect(plan).toEqual({ offer: "both", deletable: ids(2, 3), routeCount: 2, note: null });
    });

    it("ignores dangling stage ids — they are not routes", () => {
        const t = trip(1, "Traverse", [2, 3, 99]);
        const plan = planTripDelete(t, [t], routes(2, 3));
        expect(plan).toEqual({ offer: "both", deletable: ids(2, 3), routeCount: 2, note: null });
    });

    it("degrades to trip-only when another trip's stage list is unreadable", () => {
        const t = trip(1, "Traverse", [2, 3]);
        const broken = trip(9, "Mystery", null);
        expect(planTripDelete(t, [t, broken], routes(2, 3))).toEqual({
            offer: "trip-only",
            reason: "Another trip's stage list could not be read, so no routes are deleted with this one.",
        });
    });

    it("degrades to trip-only when this trip's own stage list is unreadable", () => {
        const t = trip(1, "Traverse", null);
        expect(planTripDelete(t, [t], routes(2, 3))).toEqual({
            offer: "trip-only",
            reason: "This trip's own stage list could not be read.",
        });
    });

    it("offers trip-only (no reason line) when nothing it lists is still on the device", () => {
        const t = trip(1, "Ghost tour", [98, 99]);
        expect(planTripDelete(t, [t], routes(2, 3))).toEqual({ offer: "trip-only", reason: null });
    });

    it("uses the singular for one protected route of one", () => {
        const t = trip(1, "Solo", [2]);
        const other = trip(9, "Other trip", [2, 3]);
        expect(planTripDelete(t, [t, other], routes(2, 3))).toMatchObject({
            offer: "trip-only",
            reason: "All of its routes are also in other trips and would stay anyway.",
        });
        const t2 = trip(1, "Pair", [2, 4]);
        expect(planTripDelete(t2, [t2, other], routes(2, 3, 4))).toMatchObject({
            note: "1 of its 2 routes is also in “Other trip” and will stay.",
        });
    });
});
