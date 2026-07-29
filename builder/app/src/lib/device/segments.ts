/**
 * The trip preview's shared distance axis: several stage tracks, concatenated in trip order into
 * the one polyline the chart-room modal already knows how to window, hover and zoom
 * (`elevation.ts`), plus the bookkeeping that keeps each stage addressable on that axis — where
 * it starts, how long its drawn track is, which point indices are its, and where the seams
 * between stages fall (the profile's thin boundary rules).
 *
 * Pure geometry, like `elevation.ts`: the modal derives everything else (window echo per stage,
 * per-stage profile tint, merged waypoint positions) from this one result.
 *
 * The axis is `cumulativeDistances` over the **concatenated** points, so the straight-line jump
 * between one stage's end and the next stage's start — usually zero for a tour, where each day
 * starts where the last ended — is part of the axis, exactly as it would be had the stages been
 * one recorded track.
 *
 * Deliberate tradeoff for a *discontinuous* trip (a real gap between stages): the map marks the
 * jump with a thin dashed connector (the modal draws it from consecutive ranges here), so the
 * hover dot visibly travels a transfer leg — but the elevation profile still draws one straight
 * interpolated ramp across the jump's span. Masking that ramp would mean splitting the profile's
 * SVG path per stage; the seam rules already say where the stages meet, so the ramp stays until a
 * discontinuous trip is something riders actually build.
 */

import type { RouteWaypoint } from "../convert/bridge";
import { cumulativeDistances, type ProfilePoint } from "./elevation";

/** One stage of a trip as the preview draws it: the decoded track, the stage's band color, and
 *  the stage's own waypoints (distances measured from *its* start — see {@link waypointDistanceM}). */
export interface TrackSegment {
    readonly name: string;
    /** The stage color — the same palette entry the trip band's dot uses. */
    readonly color: string;
    readonly points: readonly ProfilePoint[];
    readonly waypoints: readonly RouteWaypoint[];
}

/** The concatenated axis and each segment's place on it. All distances metres, all on the drawn
 *  (post-decimation) polyline. */
export interface SegmentAxis {
    /** Every segment's points, in order — the one track the modal windows and hovers. */
    readonly points: ProfilePoint[];
    /** `cumulativeDistances(points)` — computed once here, shared by every consumer. */
    readonly cum: number[];
    /** The whole axis's length. */
    readonly totalM: number;
    /** Per segment: its `[first, last]` point indices in {@link points}, or null for a segment
     *  that brought no points. */
    readonly ranges: ReadonlyArray<readonly [number, number] | null>;
    /** Per segment: the axis distance at its first point (for an empty segment: where it would
     *  have started). `offsetsM[0] === 0`. */
    readonly offsetsM: number[];
    /** Per segment: the drawn length of its own track — `cum[last] - cum[first]`, 0 if empty. */
    readonly lengthsM: number[];
    /** The seams between consecutive non-empty segments — the axis distance where a later
     *  segment begins. One entry per internal boundary; empty for 0 or 1 drawable segments. */
    readonly boundariesM: number[];
}

/** Concatenate stage tracks into one shared axis. */
export function concatSegments(segments: ReadonlyArray<readonly ProfilePoint[]>): SegmentAxis {
    const points: ProfilePoint[] = [];
    const spans: Array<readonly [number, number] | null> = [];
    for (const segment of segments) {
        if (segment.length === 0) {
            spans.push(null);
            continue;
        }
        const first = points.length;
        points.push(...segment);
        spans.push([first, points.length - 1]);
    }
    const cum = cumulativeDistances(points);
    const totalM = cum.length ? cum[cum.length - 1] : 0;

    const offsetsM: number[] = [];
    const lengthsM: number[] = [];
    const boundariesM: number[] = [];
    let cursorM = 0;
    let drawnBefore = false;
    for (const span of spans) {
        if (span === null) {
            offsetsM.push(cursorM);
            lengthsM.push(0);
            continue;
        }
        const [first, last] = span;
        offsetsM.push(cum[first]);
        lengthsM.push(cum[last] - cum[first]);
        if (drawnBefore) boundariesM.push(cum[first]);
        drawnBefore = true;
        cursorM = cum[last];
    }
    return { points, cum, totalM, ranges: spans, offsetsM, lengthsM, boundariesM };
}

/**
 * A stage waypoint's position on the shared axis: its stored `distAlongM` — measured from the
 * stage's own start on the RAW pre-decimation track, so it can slightly exceed the drawn stage's
 * length — clamped into the stage's drawn span, then offset by everything before the stage. The
 * same clamp the single-route modal applies (`clampedDistM`), per stage.
 *
 * On a degenerate axis (no distance at all) the raw value is passed through, as before: there is
 * no span to clamp into, and the caption should say what was stored rather than 0.
 */
export function waypointDistanceM(axis: SegmentAxis, segment: number, distAlongM: number): number {
    if (axis.totalM <= 0) return distAlongM;
    const clamped = Math.max(0, Math.min(distAlongM, axis.lengthsM[segment] ?? 0));
    return (axis.offsetsM[segment] ?? 0) + clamped;
}
