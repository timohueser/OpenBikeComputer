/** Decides whether the terrain hides a point of a line from the eye of a still. */
import { CURVATURE } from './frame/frame';

/**
 * The first sample of the terrain lies FIRST_M across from the eye, and each next one STEP of its distance
 * farther. So the step follows what the image resolves: 1 % of the distance is about 11 pixels of a still.
 * A ridge with flanks of 1 in 2 that falls between two samples rises less than 3 pixels above the line of sight.
 */
const FIRST_M = 10, STEP = 0.01;
/** The ground nearer to the point than this, in metres across, is the point's own ground and hides nothing. */
const OWN_M = 30;
/**
 * The terrain hides a point only where it rises above the line of sight by SLACK_M plus SLACK_PIXELS pixels of
 * the image at the distance of that ground. The slack keeps a line on a slope that faces the eye visible.
 */
const SLACK_M = 2, SLACK_PIXELS = 3;

/** The distances across from the eye, in metres, at which `terrainHides` reads the terrain on the way to a point `across` metres away. */
export function sampleDistances(across: number): number[] {
    const out: number[] = [];
    for (let d = FIRST_M; d < across - OWN_M; d *= 1 + STEP) out.push(d);
    return out;
}

/**
 * Whether the terrain hides the point `to` from an eye at the origin, in metres east, north and up. `ground[k]`
 * is the terrain height at `along[k]` metres across from the eye on the way to the point (`sampleDistances`),
 * and heights are as a map has them: this takes off the curvature of the Earth. NaN hides nothing. `pixel` is
 * metres per image pixel at 1 m from the eye. Trees, houses, bridges and water are not terrain, so they hide nothing.
 */
export function terrainHides(to: readonly [number, number, number], along: ArrayLike<number>, ground: ArrayLike<number>, pixel: number): boolean {
    const across = Math.hypot(to[0], to[1]), drop = (d: number) => CURVATURE * d * d, rise = to[2] - drop(across);
    for (let k = 0; k < along.length; k++) {
        const d = along[k], t = d / across;
        if (across - d < OWN_M) break;
        if (ground[k] - drop(d) > t * rise + SLACK_M + SLACK_PIXELS * pixel * Math.hypot(d, t * rise)) return true;
    }
    return false;
}
