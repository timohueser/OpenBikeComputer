// PROTOTYPE road system (a study; never merges). The cross-section of a road, track, path or railway:
// per side, the points across it from the carriageway's edge outward, then the rows of its band. The
// sweep (corridor.ts) joins the points of two stations into faces. corridor.wgsl reads the materials
// and the flags.
import { PATH, PROFILES, RAIL, RAIL_CODE, ROAD, SURFACES, walkOf, type Line } from '../terrain/bins';
import { LEFT, RIGHT } from '../terrain/town';
import { kerbParts } from './details';

export const CLASS_NAMES = Object.keys(PROFILES);
export const SURFACE_NAMES = Object.keys(SURFACES) as (keyof typeof SURFACES)[];
export const classOf = (line: Line) => CLASS_NAMES[line.profile];
export const surfaceName = (line: Line) => SURFACE_NAMES[(line.code & 15) - 1];

/** Face materials, as MAT_* of corridor.wgsl. */
export const MAT = {
    /** The look of the ground under the face: the band. */
    GROUND: 1,
    /** The road paint of `road_surface` for the line's surface code: carriageway, track bed, path tread. */
    PAINT: 2,
    SHOULDER: 3,
    KERB: 4,
    SIDEWALK: 5,
    BALLAST: 6,
    STEEL: 7,
    POST: 8,
    WALL: 9,
    CONCRETE: 10,
} as const;

/** Vertex flags of a corridor face, as CF_* of corridor.wgsl: the height is an offset over the sampled stamped ground, or over the drawn mesh. */
export const CF = { SAMPLE: 1, MESH: 2, BARRIER: 4 } as const;

/**
 * The paved shoulder or verge strip past a carriageway without a sidewalk, by surface; the stamp holds the
 * ground flat across it (shoulder_of in stamp-core.wgsl has the same numbers by surface code).
 */
export const SHOULDER_M: Partial<Record<keyof typeof SURFACES, number>> = { main: 0.6, road: 0.4, gravel: 0.3, dirt: 0.25, grass: 0.2, oneway: 0.6 };
/** A carriageway lies this far over the stamped core (its tolerance is 2 cm), so it covers the ground where no hole is cut. */
export const LIFT_M = 0.03;
/** The crown of a paved carriageway: its middle over its edges, per metre of half-width. A bend's banking takes it over (`carriageway`). */
export const CROWN = 0.025;
/** A town road's kerb: its rise and the width of its granite top, as RISE_M and KERB_M of sidewalks.ts. */
export const KERB_RISE_M = 0.12, KERB_TOP_M = 0.18;
/**
 * The band's rows past the outer edge of the road: the stamped ground along the road, so its creases run with
 * the road; the last row sinks under the drawn mesh, where the terrain takes over. corridor.wgsl draws the
 * corridor a little toward the eye, so the band wins over the terrain where the two coincide.
 */
export const BAND_M = [0.35, 1.0, 2.0], BAND_OUTER_M = 3.2, PATH_BAND_M = [0.3, 0.8], PATH_BAND_OUTER_M = 1.6, BAND_SINK_M = 0.05;

/** A point across a side: metres out from the line's middle, metres up from its height, and the material and flags of the face that ends at it. */
export interface Part { a: number; dz: number; mat: number; flags?: number }

/** Whether a line gets a corridor: roads, tracks, paths and railways on the ground. */
export const corridorLine = (line: Line) => (line.kind === ROAD || line.kind === RAIL || line.kind === PATH) && !line.gallery;

const PAVED_ROAD = ['main', 'road', 'oneway', 'paving'];

/**
 * The carriageway's points across, from left (+a) to right: its edges and its crown, `h` metres each side.
 * The edges lie on the plane of the bend's banking (the sweep adds it); the crown's outer half rotates
 * with the banking until it is one plane, as a road's runoff does.
 */
export function carriageway(line: Line, h = line.h, bank = 0): Part[] {
    const crown = line.kind === ROAD && PAVED_ROAD.includes(surfaceName(line)) ? CROWN * h * Math.max(0, 1 - Math.abs(bank) / CROWN) : 0;
    // A parabola, in four facets on a wide road, so the flat-shaded faces show no sharp ridge in the middle.
    return (line.h >= 3 ? [1, 0.5, 0, -0.5, -1] : [1, 0, -1]).map(f => ({ a: f * h, dz: LIFT_M + crown * (1 - f * f), mat: MAT.PAINT }));
}

/** The points of one side past the carriageway's edge, then the band; `side` 1 is left, -1 right, at segment v. */
export function sideOf(line: Line, side: 1 | -1, v: number, h = line.h): Part[] {
    const out: Part[] = [], walk = (line.sidewalks?.[v] ?? 0) & (side > 0 ? LEFT : RIGHT) ? walkOf(line) : 0;
    let a = h, dz = LIFT_M;
    if (walk > 0) {
        out.push(...kerbParts(a, dz, walk));
        a += walk;
        dz += KERB_RISE_M;
    } else if (line.kind === RAIL || line.code >= RAIL_CODE) {
        out.push({ a: a + 0.6, dz: -0.3, mat: MAT.BALLAST });
        a += 0.6;
        dz = -0.3;
    } else {
        const shoulder = line.kind === ROAD ? SHOULDER_M[surfaceName(line)] ?? 0 : 0;
        if (shoulder > 0) {
            out.push({ a: a + shoulder, dz: dz - 0.02, mat: MAT.SHOULDER });
            a += shoulder;
            dz -= 0.02;
        }
    }
    const path = line.kind === PATH;
    for (const d of path ? PATH_BAND_M : BAND_M) out.push({ a: a + d, dz: 0, mat: MAT.GROUND, flags: CF.SAMPLE });
    out.push({ a: a + (path ? PATH_BAND_OUTER_M : BAND_OUTER_M), dz: -BAND_SINK_M, mat: MAT.GROUND, flags: CF.MESH });
    return out;
}
