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

/** The looks of the ways, materials 20 to 39, as LOOK_* of looks.wgsl; from TREAD on, the detail meshes of tracks.ts. */
export const LOOK = {
    ASPHALT: 20, MAIN: 21, MOTORWAY: 22, CONCRETE: 23, SETT: 24, PAVERS: 25, GRAVEL: 26, DIRT: 27, TRAIL: 28, BALLAST: 29,
    TREAD: 30, RISER: 31, SLEEPER: 32, SLEEPER_WOOD: 33, RAIL: 34, RAIL_HEAD: 35, RACK: 36, CABLE: 37,
} as const;
/** The top of a track's ballast over its line, and the width of its slope on each side. */
export const BED_M = 0.25, BED_SLOPE_M = 0.4;
/** The bed of paving over the core. */
export const PAVED_BED_M = 0.05;

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
const MOTORWAYS = ['motorway', 'motorway_link', 'trunk', 'trunk_link'];
const MAINS = ['primary', 'primary_link', 'secondary', 'secondary_link', 'tertiary', 'tertiary_link'];
const SETTS = ['sett', 'cobblestone', 'unhewn_cobblestone', 'stone'];

/** The look of a line's carriageway, by its class, its surface code and its OSM surface. */
export function lookOf(line: Line): number {
    if (line.kind === RAIL || line.code >= RAIL_CODE) return LOOK.BALLAST;
    const tag = line.surface ?? '', name = surfaceName(line), cls = classOf(line);
    if (SETTS.includes(tag)) return LOOK.SETT;
    if (tag === 'paving_stones' || name === 'paving' || cls === 'pedestrian') return LOOK.PAVERS;
    if (tag.startsWith('concrete')) return LOOK.CONCRETE;
    if (name === 'gravel') return LOOK.GRAVEL;
    if (name === 'dirt' || name === 'grass') return line.kind === PATH ? LOOK.TRAIL : LOOK.DIRT;
    if (name === 'path') return LOOK.TRAIL;
    return MOTORWAYS.includes(cls) ? LOOK.MOTORWAY : MAINS.includes(cls) ? LOOK.MAIN : LOOK.ASPHALT;
}

/**
 * The carriageway's points across, from left (+a) to right, `h` metres each side: its edges and its crown, a
 * track's ruts or a railway's ballast bed. The edges lie on the plane of the bend's banking (the sweep adds
 * it); the crown's outer half rotates with the banking until it is one plane, as a road's runoff does. The
 * shape is the line's, so two stations of a line have the same points.
 */
export function carriageway(line: Line, h = line.h, bank = 0): Part[] {
    const mat = lookOf(line), r = Math.min(1, h / line.h);
    const half = (pts: [number, number][]): Part[] => [...pts.map(([a, dz]) => ({ a, dz: LIFT_M + dz, mat })), ...pts.slice(0, -1).reverse().map(([a, dz]) => ({ a: -a, dz: LIFT_M + dz, mat }))];
    // A bed of ballast: its top BED_M over the line, its slopes down to the core at the edges.
    if (mat === LOOK.BALLAST) return half([[h, 0], [h - BED_SLOPE_M * r, BED_M], [0, BED_M]]);
    // Ruts worn into a track, the middle left higher; a narrower station (a joint's taper) pulls them in.
    if (mat === LOOK.DIRT && line.h >= 1.2) return half([[h, 0], [1.0 * r, 0], [0.75 * r, -0.04], [0.5 * r, 0], [0, 0.025]]);
    if (mat === LOOK.GRAVEL && line.h >= 1.3) return half([[h, 0], [1.05 * r, 0], [0.78 * r, -0.025], [0.5 * r, 0], [0, 0.01]]);
    // Paving and narrow paved paths lie on their own bed over the core: seen at a grazing angle, the drawn
    // terrain's mesh across a narrow core would rise through them.
    const bed = classOf(line) === 'steps' ? 0 : mat === LOOK.PAVERS || mat === LOOK.SETT || mat === LOOK.CONCRETE || (line.kind === PATH && mat !== LOOK.TRAIL) ? PAVED_BED_M : 0;
    const crown = line.kind === ROAD && PAVED_ROAD.includes(surfaceName(line)) ? CROWN * h * Math.max(0, 1 - Math.abs(bank) / CROWN) : 0;
    // A parabola, in four facets on a wide road, so the flat-shaded faces show no sharp ridge in the middle.
    return (line.h >= 3 ? [1, 0.5, 0, -0.5, -1] : [1, 0, -1]).map(f => ({ a: f * h, dz: LIFT_M + bed + crown * (1 - f * f), mat }));
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
        // The formation beside the bed: ballast spilt into the ground (looks.wgsl past the half-width).
        out.push({ a: a + 0.35, dz: LIFT_M - 0.02, mat: LOOK.BALLAST });
        a += 0.35;
        dz = LIFT_M - 0.02;
    } else {
        const shoulder = line.kind === ROAD ? SHOULDER_M[surfaceName(line)] ?? 0 : 0;
        if (shoulder > 0) {
            // A paved strip beside a main road, a gravel verge beside a minor one (both in the road's look past
            // its half-width); a track has none: the stamp's flat shoulder is ground.
            const look = lookOf(line), paved = look === LOOK.MAIN || look === LOOK.MOTORWAY || look === LOOK.ASPHALT;
            out.push({ a: a + shoulder, dz: dz - 0.02, mat: paved ? look : look === LOOK.CONCRETE || look === LOOK.SETT || look === LOOK.PAVERS ? LOOK.ASPHALT : MAT.GROUND });
            a += shoulder;
            dz -= 0.02;
        }
    }
    const path = line.kind === PATH;
    for (const d of path ? PATH_BAND_M : BAND_M) out.push({ a: a + d, dz: 0, mat: MAT.GROUND, flags: CF.SAMPLE });
    out.push({ a: a + (path ? PATH_BAND_OUTER_M : BAND_OUTER_M), dz: -BAND_SINK_M, mat: MAT.GROUND, flags: CF.MESH });
    return out;
}
