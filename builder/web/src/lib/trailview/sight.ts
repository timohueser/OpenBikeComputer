/** Decides whether the terrain hides a point of a line from the eye of a still. */

/** The terrain heights that the test reads between the eye and each point. */
export const SAMPLES = 128;
/** The ground nearer to the point than this, in metres across, is the point's own ground and hides nothing. */
const OWN_M = 30;
/**
 * The terrain hides a point only where it rises above the line of sight by SLACK_M plus SLACK_PIXELS pixels of
 * the image at the distance of that ground. The slack keeps a line on a slope that faces the eye visible.
 */
const SLACK_M = 2, SLACK_PIXELS = 3;

/**
 * Whether the terrain hides the point `to` from an eye at the origin; both in metres east, north and up, with the
 * curvature of the Earth taken off. `ground[k]` is the terrain height at (k + 1) / (ground.length + 1) of the way
 * to the point, in the same frame; NaN hides nothing. `pixel` is metres per image pixel at 1 m from the eye.
 * Trees, houses, bridges and water are not terrain, so they hide nothing.
 */
export function terrainHides(to: readonly [number, number, number], ground: ArrayLike<number>, pixel: number): boolean {
    const across = Math.hypot(to[0], to[1]), reach = Math.hypot(across, to[2]);
    for (let k = 0; k < ground.length; k++) {
        const t = (k + 1) / (ground.length + 1);
        if ((1 - t) * across < OWN_M) break;
        if (ground[k] > t * to[2] + SLACK_M + SLACK_PIXELS * pixel * t * reach) return true;
    }
    return false;
}
