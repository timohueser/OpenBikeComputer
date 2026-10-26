import { defaultView, TIME_ZONE, type View } from '../trailview/view';
import type { Coordinate } from './geo';

/** A press that moves farther than this, in CSS pixels, is a drag; a shorter one is a click. */
export const DRAG_PX = 5;
/** The binoculars hang this far below the pointer, in CSS pixels. Their bottom is the drop point. */
export const HANG_PX = 36;
const DEGREES_PER_PX = 1.5, MAX_SWING = 35, SETTLE_MS = 140;
/** The first heading of a view. */
const HEADING = 0;

/** The engine is a chunk of its own: the planner loads it at the first press of the binoculars. */
let chunk: Promise<typeof import('./trailview-start')> | undefined;
export function loadTrailView(): Promise<typeof import('./trailview-start')> {
    return chunk ??= import('./trailview-start').catch(error => {
        chunk = undefined;
        throw error;
    });
}

/** The swing after the pointer moves `dx` pixels: the binoculars lag behind, so a move to the right turns them clockwise. */
export function swing(angle: number, dx: number): number {
    return Math.max(-MAX_SWING, Math.min(MAX_SWING, angle + dx * DEGREES_PER_PX));
}

/** The swing `ms` later, as it eases back to rest. */
export function settle(angle: number, ms: number): number {
    return angle * Math.exp(-ms / SETTLE_MS);
}

/** The drop point of the binoculars that hang from the pointer at `angle` degrees clockwise: their bottom. */
export function dropPoint([x, y]: [number, number], angle: number): [number, number] {
    const radians = angle * Math.PI / 180;
    return [x - HANG_PX * Math.sin(radians), y + HANG_PX * Math.cos(radians)];
}

export function inBounds([west, south, east, north]: readonly [number, number, number, number], [lon, lat]: Coordinate): boolean {
    return lon >= west && lon <= east && lat >= south && lat <= north;
}

/** A walk at the point, at noon of today in the Alps, in sunshine. */
export function viewAt([lon, lat]: Coordinate, now = new Date()): View {
    // en-CA formats a date as YYYY-MM-DD.
    const date = new Intl.DateTimeFormat('en-CA', { timeZone: TIME_ZONE }).format(now);
    return { ...defaultView([lon, lat, lon, lat], now), lon, lat, heading: HEADING, date, time: '12:00', weather: 'sunny' };
}
