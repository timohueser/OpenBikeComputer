// The small things that a corridor repeats, such as sleepers and the logs of trail steps: one mesh each, drawn
// as instances that a piece holds behind its vertices (`PieceBuilder.place`), in the same pool. A new part is
// one row of PARTS.
import { LIFT_M, RAIL_CODE, SURFACES } from '../terrain/bins';
import { lookWord } from './piece';
import { BED_M, LOOK } from './section';

/** Bytes of a vertex of a part's mesh: metres across to the left, ahead and up, and a look word. */
export const PART_VERTEX = 16;

/** A face's corners [across, ahead, up], counter-clockwise seen from outside. */
type Face = [number, number, number][];

/** A box `long` metres ahead and 2 × `half` across, from `low` to `high` metres up, open below. */
function box(long: number, half: number, low: number, high: number): Face[] {
    const [b, f] = [-long / 2, long / 2], [r, l] = [-half, half];
    return [
        [[l, b, high], [r, b, high], [r, f, high], [l, f, high]],
        [[r, b, low], [r, f, low], [r, f, high], [r, b, high]],
        [[l, f, low], [l, b, low], [l, b, high], [l, f, high]],
        [[r, f, low], [l, f, low], [l, f, high], [r, f, high]],
        [[l, b, low], [r, b, low], [r, b, high], [l, b, high]],
    ];
}

/** A sleeper's top over a track's plane, which stands SLEEPER_UP_M over the ballast, and its width along the track and its depth, which the ballast hides in part. */
const SLEEPER_UP_M = 0.05, SLEEPER_W = 0.26, SLEEPER_DEPTH_M = 0.12;
export const SLEEPER_TOP_M = LIFT_M + BED_M + SLEEPER_UP_M;
/** The middle of a rail from a track's middle, and half the length of a sleeper, of each gauge. As track-looks.wgsl. */
export const RAIL_GAUGE_M = 0.7525, NARROW_GAUGE_M = 0.53, SLEEPER_HALF_M = 1.3, NARROW_SLEEPER_HALF_M = 0.9;
const NARROW_CODE = Object.keys(SURFACES).indexOf('narrow') + 1, PATH_CODE = Object.keys(SURFACES).indexOf('path') + 1;
const sleeper = (half: number) => box(SLEEPER_W, half, SLEEPER_TOP_M - SLEEPER_DEPTH_M, SLEEPER_TOP_M);

/**
 * A log of trail steps: a round timber of radius LOG_R_M, its axis LOG_AXIS_M over the way's core, its foot
 * LOG_SUNK_M under it, in LOG_FACETS facets: an odd number, so that each cap is whole quads. LOG_HALF_M: the
 * half-lengths of the logs, shortest first, a part each.
 */
const LOG_R_M = 0.08, LOG_AXIS_M = 0.03, LOG_SUNK_M = 0.03, LOG_FACETS = 7;
export const LOG_HALF_M = [0.3, 0.35, 0.45, 0.55, 0.7, 0.85];

/** A log `half` metres each side of the way's middle: round over the ground, open below, its ends capped. */
function log(half: number): Face[] {
    const low = Math.asin((LOG_AXIS_M + LOG_SUNK_M) / LOG_R_M), [r, l] = [-half, half];
    const arc = Array.from({ length: LOG_FACETS + 1 }, (_, k) => -low + (k * (Math.PI + 2 * low)) / LOG_FACETS).map(angle => [Math.cos(angle) * LOG_R_M, LIFT_M + LOG_AXIS_M + Math.sin(angle) * LOG_R_M]);
    const round = arc.slice(1).map(([ahead, up], k): Face => [[r, ...arc[k]], [l, ...arc[k]], [l, ahead, up], [r, ahead, up]] as Face);
    const caps = Array.from({ length: (LOG_FACETS - 1) / 2 }, (_, i) => 2 * i + 1).flatMap(k => [[0, k, k + 1, k + 2].map(i => [r, ...arc[i]]), [k + 2, k + 1, k, 0].map(i => [l, ...arc[i]])] as Face[]);
    return [...round, ...caps];
}

/** The parts, as `place` takes them. LOG + k: a log of LOG_HALF_M[k]. */
export const PART = { SLEEPER: 0, SLEEPER_NARROW: 1, TIMBER: 2, TIMBER_NARROW: 3, ROLLER: 4, LOG: 5 } as const;
/** Each part's faces, material and surface code. */
const PARTS: [Face[], number, number][] = [
    [sleeper(SLEEPER_HALF_M), LOOK.SLEEPER, RAIL_CODE],
    [sleeper(NARROW_SLEEPER_HALF_M), LOOK.SLEEPER, NARROW_CODE],
    [sleeper(SLEEPER_HALF_M), LOOK.TIMBER, RAIL_CODE],
    [sleeper(NARROW_SLEEPER_HALF_M), LOOK.TIMBER, NARROW_CODE],
    // A funicular's roller, under its cable.
    [box(0.1, 0.16, SLEEPER_TOP_M - 0.05, SLEEPER_TOP_M + 0.06), LOOK.RAIL, RAIL_CODE],
    ...LOG_HALF_M.map((half): [Face[], number, number] => [log(half), LOOK.LOG, PATH_CODE]),
];

/** The meshes of the parts in one vertex and one index buffer, with each part's range of indices. */
export function partMeshes(): { vertices: ArrayBuffer; indices: Uint16Array<ArrayBuffer>; ranges: { first: number; count: number; vertices: number }[] } {
    const words: number[] = [], indices: number[] = [];
    const ranges = PARTS.map(([faces, mat, code]) => {
        const first = indices.length;
        for (const face of faces) {
            const base = words.length / 4;
            indices.push(base, base + 1, base + 2, base, base + 2, base + 3);
            words.push(...face.flatMap(point => [...point, lookWord(mat, code, 0, 0)]));
        }
        return { first, count: indices.length - first, vertices: 4 * faces.length };
    });
    const vertices = new ArrayBuffer(words.length * 4), floats = new Float32Array(vertices), packed = new Uint32Array(vertices);
    words.forEach((word, i) => {
        if (i % 4 === 3) packed[i] = word;
        else floats[i] = word;
    });
    return { vertices, indices: new Uint16Array(indices), ranges };
}
