// The cross-section of a road, track, path or railway: per side, the points across it from the
// carriageway's edge outward, then the rows of its band. The sweep (sweep.ts) joins the points of two
// stations into faces, and corridor.wgsl reads their materials and flags.
import { BRIDGE, KERB_RISE_M, LIFT_M, marked, MOTORWAYS, PATH, PROFILES, RAIL, RAIL_CODE, ROAD, SETTS, SHOULDER_M, SURFACES, walkOf, type Line } from '../terrain/bins';
import { LEFT, RIGHT } from '../terrain/town';
import { isDeck } from './decks';

const CLASS_NAMES = Object.keys(PROFILES), SURFACE_NAMES = Object.keys(SURFACES) as (keyof typeof SURFACES)[];
export const classOf = (line: Line) => CLASS_NAMES[line.profile];
export const surfaceName = (line: Line) => SURFACE_NAMES[(line.code & 15) - 1];
/** The surface code of a way's mesh. A one-way road of a minor class keeps code `road` in its record, and loses its middle line only here. */
export const drawnCode = (line: Line) => (line.oneway !== undefined && surfaceName(line) === 'road' ? SURFACE_NAMES.indexOf('oneway') + 1 : line.code);

export { KERB_RISE_M, LIFT_M };

/** Face materials, as MAT_* of corridor.wgsl and details.wgsl: the ground under the face, the stone of a town street, and what stands beside a road. */
export const MAT = { GROUND: 1, KERB: 2, SIDEWALK: 3, RAIL: 40, RAIL_POST: 41, RAIL_BLOCK: 42, DELI: 44, DELI_BAND: 45, REFLECTOR: 46, WALL: 48, CAP: 49 } as const;
/** The looks of the ways, as LOOK_* of looks.wgsl. */
export const LOOK = { GIVE_WAY: 15, ZEBRA: 16, ZEBRA_SETT: 17, ZEBRA_PAVERS: 18, ZEBRA_CONCRETE: 19, ASPHALT: 20, MAIN: 21, MOTORWAY: 22, CONCRETE: 23, SETT: 24, PAVERS: 25, GRAVEL: 26, DIRT: 27, TRAIL: 28, BALLAST: 29, STRIPS: 30, TREAD: 31, RISER: 32, SLEEPER: 33, TIMBER: 34, RAIL: 35, RAIL_HEAD: 36, RACK: 37, CABLE: 38 } as const;
/**
 * Vertex flags, as CF_* of corridor.wgsl. MESH: the foot of a backfill or of a cheek of steps under the drawn mesh.
 * FILL: a backfill's row at its height, or under the drawn mesh where that lies lower. NO_LEFT_LINE, NO_RIGHT_LINE:
 * a carriageway without its edge line on that side, at a junction's mouth.
 */
export const CF = { MESH: 2, FILL: 3, NO_LEFT_LINE: 8, NO_RIGHT_LINE: 16 } as const;

/** The top of a track's ballast over its line, and the width of its slope on each side. */
export const BED_M = 0.25;
const BED_SLOPE_M = 0.4;
/** The bed of paving over the core: seen at a grazing angle, the terrain across a narrow core would rise through it. */
const PAVED_BED_M = 0.05;
/** The crown of a paved carriageway: its middle over its edges, per metre of half-width. A bend's banking takes it over. */
const CROWN = 0.025;
/** How far a paved shoulder lies under the carriageway's edge. */
const SHOULDER_DROP_M = 0.02;
/** The width of the top of a town road's kerb. */
const KERB_TOP_M = 0.16;
/** A lowered kerb's fall: its top stays 2.5 cm over the carriageway. */
const KERB_DROP_M = KERB_RISE_M - 0.025;
/** Ballast spilt past a track's bed. */
export const SPILL_M = 0.35;
/** A paved way narrower than this on each side frays into the ground, as `w.half` in paved_look of looks.wgsl. */
const NARROW_HALF_M = 2;
/** A junction's faces are one surface with no edge in its look: this far across a way this wide on each side. */
const FACE_ACROSS_M = 2, FACE_HALF_M = 12;
/**
 * The curtain past a side's outer edge: one row of the ground, this far out, down under the drawn mesh. The fit of stamp.wgsl
 * holds the terrain under a way's faces, and where they stand over it, the curtain closes the step.
 */
const CURTAIN_OUT_M = 0.1;

/** A point across a way: metres out from the line, metres up, the material of the face that ends at it, its flags, whether the far range keeps it, and whether it ends the curtain. */
export interface Part { a: number; dz: number; mat: number; flags: number; far: boolean; curtain?: boolean }

/** Whether a line gets a corridor: roads, tracks and paths on the ground, and the decks of road bridges. */
export const corridorLine = (line: Line) => ((line.kind === ROAD || line.kind === RAIL || line.kind === PATH) && !line.gallery) || isDeck(line);

const PAVED_ROAD = ['main', 'road', 'oneway', 'paving'];

/** The look of a line's carriageway, by its class, its surface code and its OSM surface. */
export function lookOf(line: Line): number {
    if (line.kind === RAIL || line.code >= RAIL_CODE) return LOOK.BALLAST;
    const tag = line.surface ?? '', name = surfaceName(line), cls = classOf(line);
    if (SETTS.includes(tag)) return LOOK.SETT;
    if (tag === 'paving_stones' || name === 'paving' || cls === 'pedestrian') return LOOK.PAVERS;
    if (tag.startsWith('concrete')) return cls === 'track' ? LOOK.STRIPS : LOOK.CONCRETE;
    if (name === 'gravel') return LOOK.GRAVEL;
    if (name === 'dirt' || name === 'grass') return line.kind === PATH ? LOOK.TRAIL : LOOK.DIRT;
    if (name === 'path') return LOOK.TRAIL;
    return !marked(line) ? LOOK.ASPHALT : MOTORWAYS.includes(cls) ? LOOK.MOTORWAY : LOOK.MAIN;
}

/** Whether a way's look frays into the ground at its edges. */
export const frays = (line: Line) => ([LOOK.GRAVEL, LOOK.DIRT, LOOK.TRAIL] as number[]).includes(lookOf(line));

/**
 * The surface of a junction's faces on a road of this line's look: its own material without lines or ruts, and
 * the metres across and half-width that give it. A way of soil has none: only its ruts are its surface.
 */
export function junctionFace(line: Line): { mat: number; across: number; half: number } | undefined {
    const look = lookOf(line);
    if (look === LOOK.DIRT || look === LOOK.TRAIL) return undefined;
    return { mat: look === LOOK.GRAVEL ? look : LOOK.ASPHALT, across: FACE_ACROSS_M, half: FACE_HALF_M };
}

/** A shape's points from an edge to the middle, mirrored to the other edge. The far range keeps the edges and the middle. */
function mirrored(points: [number, number][], mat: number): Part[] {
    const last = points.length - 1, part = ([a, dz]: [number, number], i: number, sign: number) => ({ a: sign * a, dz: LIFT_M + dz, mat, flags: 0, far: i === 0 || i === last });
    return [...points.map((point, i) => part(point, i, 1)), ...points.slice(0, -1).reverse().map((point, i) => part(point, last - 1 - i, -1))];
}

/**
 * The carriageway's points from left (+a) to right, `h` metres each side: its edges and its crown, a
 * track's ruts or a railway's ballast bed. The edges lie on the plane of the bend's banking (the sweep adds
 * it). The crown's outer half turns with the banking until it is one plane, as a road's runoff does.
 */
export function carriageway(line: Line, h: number, bank: number): Part[] {
    const mat = lookOf(line), r = Math.min(1, h / line.h);
    if (mat === LOOK.BALLAST) return mirrored([[h, 0], [h - BED_SLOPE_M * r, BED_M], [0, BED_M]], mat);
    // Ruts worn into a track, the middle left higher; a narrower station pulls them in.
    if (mat === LOOK.DIRT && line.h >= 1.2) return mirrored([[h, 0], [1.0 * r, 0], [0.75 * r, -0.04], [0.5 * r, 0], [0, 0.025]], mat);
    if (mat === LOOK.GRAVEL && line.h >= 1.3) return mirrored([[h, 0], [1.05 * r, 0], [0.78 * r, -0.025], [0.5 * r, 0], [0, 0.01]], mat);
    const paved = mat === LOOK.PAVERS || mat === LOOK.SETT || mat === LOOK.CONCRETE || mat === LOOK.STRIPS || (line.kind === PATH && mat !== LOOK.TRAIL);
    const bed = classOf(line) !== 'steps' && paved ? PAVED_BED_M : 0;
    const crown = (line.kind === ROAD || line.kind === BRIDGE) && PAVED_ROAD.includes(surfaceName(line)) ? CROWN * h * Math.max(0, 1 - Math.abs(bank) / CROWN) : 0;
    // A parabola, in four facets on a wide road, so the flat-shaded faces show no sharp ridge in the middle.
    const shape = line.h >= 3 ? [1, 0.5, 0] : [1, 0];
    // A narrow paved way frays into the ground, unless a kerb or a shoulder lies on each side.
    const flags = h < NARROW_HALF_M && shoulderOf(line) > 0 && !frays(line) ? CF.NO_LEFT_LINE | CF.NO_RIGHT_LINE : 0;
    return mirrored(shape.map(f => [f * h, bed + crown * (1 - f * f)]), mat).map(part => ({ ...part, flags }));
}

/** The kerb of a town street from the carriageway's edge at `a`, `dz`: its face, bevel and top, the sidewalk, and the edging stone at its outer edge. */
function kerb(a: number, dz: number, walk: number): Part[] {
    const point = (at: number, up: number, mat: number, far = false): Part => ({ a: at, dz: up, mat, flags: 0, far });
    return [
        point(a, dz + 0.1, MAT.KERB), point(a + 0.025, dz + 0.125, MAT.KERB, true), point(a + KERB_TOP_M, dz + 0.125, MAT.KERB), point(a + KERB_TOP_M + 0.01, dz + KERB_RISE_M, MAT.KERB),
        point(a + walk - 0.1, dz + KERB_RISE_M, MAT.SIDEWALK, true), point(a + walk - 0.1, dz + 0.135, MAT.KERB), point(a + walk, dz + 0.135, MAT.KERB), point(a + walk + 0.01, dz + 0.07, MAT.KERB, true),
    ];
}

/** A side's points with the kerb before its sidewalk lowered by a share of its drop; the sidewalk's own level and its outer stone stay. */
export function lowered(parts: Part[], share: number): Part[] {
    if (!share) return parts;
    const walk = parts.findIndex(({ mat }) => mat === MAT.SIDEWALK);
    return parts.map((part, i) => (i < walk ? { ...part, dz: part.dz - share * KERB_DROP_M } : part));
}

/** The width of the sidewalk on a side of a line at segment v. */
const walkAt = (line: Line, side: 1 | -1, v: number) => ((line.sidewalks?.[v] ?? 0) & (side > 0 ? LEFT : RIGHT) ? walkOf(line) : 0);

/** The shoulder of a line, which the stamp holds flat. */
export const shoulderOf = (line: Line) => (line.kind === ROAD ? SHOULDER_M[surfaceName(line)] ?? 0 : 0);

/** The points of a side from the carriageway's edge out to the core's outer edge `a`, which the stamp holds flat. */
function core(line: Line, side: 1 | -1, v: number, h: number): { out: Part[]; a: number } {
    const walk = walkAt(line, side, v);
    if (walk > 0) return { out: kerb(h, LIFT_M, walk), a: h + walk };
    if (line.kind === RAIL || line.code >= RAIL_CODE) return { out: [{ a: h + SPILL_M, dz: LIFT_M - SHOULDER_DROP_M, mat: LOOK.BALLAST, flags: 0, far: true }], a: h + SPILL_M };
    const shoulder = shoulderOf(line);
    if (shoulder <= 0) return { out: [], a: h };
    // A paved strip beside a main road, a verge of the ground beside a minor one.
    const look = lookOf(line), paved = look === LOOK.MAIN || look === LOOK.MOTORWAY || look === LOOK.ASPHALT;
    const mat = paved ? look : look === LOOK.CONCRETE || look === LOOK.STRIPS || look === LOOK.SETT || look === LOOK.PAVERS ? LOOK.ASPHALT : MAT.GROUND;
    return { out: [{ a: h + shoulder, dz: LIFT_M - SHOULDER_DROP_M, mat, flags: 0, far: true }], a: h + shoulder };
}

/**
 * One side of a line, side 1 its left and -1 its right, in metres out from its middle: the points past the
 * carriageway's edge, then the curtain, and `edge`, the core's outer edge where the curtain starts. A deck has no curtain.
 */
export function sideOf(line: Line, side: 1 | -1, v: number, h: number): { parts: Part[]; edge: number } {
    const { out, a } = core(line, side, v, h);
    if (!isDeck(line)) out.push({ a: a + CURTAIN_OUT_M, dz: 0, mat: MAT.GROUND, flags: CF.FILL, far: true, curtain: true });
    return { parts: out, edge: a };
}
