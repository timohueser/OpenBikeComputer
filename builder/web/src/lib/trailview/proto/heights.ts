// PROTOTYPE (crossing look study, never merged): heights set by hand at A1, the road under the railway at
// Attinghausen. The feature tiles get other heights before any worker reads them (stream/terrain.ts), so the ground
// plan, the stamps and the structures agree: the ways about the underpass sink, and the three tracks lie lower.
import { movedHeights } from '../stream/features';
import { VARIANT, world } from './places';

/** Metres per unit at A1, and the middle of the box under the tracks in world units: 0.8 m west and 7.3 m north of the place. */
export const A1_M = 0.025517977931996578;
const PLACE = world(46.8657, 8.6347);
export const A1: [number, number] = [Math.round(PLACE[0] - 0.8 / A1_M), Math.round(PLACE[1] - 7.3 / A1_M)];
/**
 * Variant a (build A): the road sinks by all of the room. The roads go down by 2.9 m (the carriageway under the tracks at
 * 445.5 to 446.1 m) and the rails by 2.83 m (451.5 m). Variant c (build B): shared. The roads go down by 1.5 m (446.9 to
 * 447.5 m) and the rails by 1.43 m (452.9 m). The bake has the roads at 448.4 to 449.0 m and the rails at 454.3 m.
 */
export const A1_ROAD = VARIANT === 'A' ? -2.9 : VARIANT === 'B' ? -1.5 : 0, A1_RAIL = VARIANT === 'A' ? -2.83 : VARIANT === 'B' ? -1.43 : 0;
/** The ways keep the whole of it to the first distance from the box and none past the second: the roads, the tracks. */
const ROAD_M = [25, 75], RAIL_M = [80, 330];
const WAYS = ['main-road', 'road', 'path', 'tunnel'];

function offset(kind: string, x: number, y: number): number {
    const rail = kind === 'railway';
    if (!rail && !WAYS.includes(kind)) return 0;
    const [whole, none] = rail ? RAIL_M : ROAD_M, d = Math.hypot(x - A1[0], y - A1[1]) * A1_M;
    return (rail ? A1_RAIL : A1_ROAD) * Math.min(1, Math.max(0, (none - d) / (none - whole)));
}

let checked = false;

/** A feature tile's body, with the heights of the variant where the tile comes near A1. */
export function protoTile(z: number, x: number, y: number, body: ArrayBuffer | undefined): ArrayBuffer | undefined {
    if (!body || (!A1_ROAD && !A1_RAIL)) return body;
    const side = 2 ** (30 - z), reach = RAIL_M[1] / A1_M;
    if (A1[0] < x * side - reach || A1[0] > (x + 1) * side + reach || A1[1] < y * side - reach || A1[1] > (y + 1) * side + reach) return body;
    const bytes = new Uint8Array(body);
    if (bytes[0] === 0x1f && bytes[1] === 0x8b) {
        console.warn('PROTO a gzip feature tile keeps its heights');
        return body;
    }
    if (!checked) {
        // Once: with no offset the bytes must come back as they are.
        checked = true;
        const same = movedHeights(bytes, { level: z, x, y }, () => 0);
        console.log(`PROTO heights walk ${same.length === bytes.length && same.every((value, i) => value === bytes[i]) ? 'ok' : 'DIFFERS'} on ${z}/${x}/${y}, ${bytes.length} bytes`);
    }
    return movedHeights(bytes, { level: z, x, y }, offset).buffer as ArrayBuffer;
}
