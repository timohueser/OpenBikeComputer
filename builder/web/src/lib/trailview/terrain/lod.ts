import { UNITS, type Anchor } from '../frame/space';

/** Cells along a node edge in the one grid mesh. */
export const GRID = 64;
/** The finest level: about 0.4 m per texel in the Alps. */
export const FINEST = 18;
/** The coarsest level with pages: the quadrants of tile 0/0/0. */
export const COARSEST = 1;
/** The morph band of a CDLOD level, in levels of log2 distance, as MORPH in drawn-height.wgsl. */
const MORPH = 0.3;
/** Nodes along an edge of the LOD map window of each level. */
export const MAP = 32;
/** A node splits this far, in levels, before its finest point reaches the next level, so float32 shaders see exact plateaus. */
const MARGIN = 1e-3;
/**
 * CDLOD ranges in node sizes. Below the least, a node can reach past the morph band of its parent's
 * level, and neighbours would not meet; above the most, a level leaves its LOD map window.
 */
export const RANGE = { least: 4.6, most: 8 } as const;
/** The near field: metres about the eye where the level-17 nodes and the kinds' near meshes draw. `nearReady()` waits for it. */
export const NEAR_M = 2000;
/** The field about the eye that `eyeReady()` waits for: the view's position is final once it draws. */
export const EYE_M = 500;
/** The coarse cover lies this many levels above the drawn level: few pages, so a turn never finds a hole. */
const COVER = 3;

/** Node `x`, `y` of a level: the Mercator tile of zoom `level`. */
export interface Node { level: number; x: number; y: number }

/** What the selection looks from: the eye, and its height above the ground. */
export interface Eye { anchor: Anchor; above: number }

/** Units along a node edge. */
export const size = (level: number) => 2 ** (30 - level);
/**
 * The level of the nodes that hold the structures and the corridors: each piece belongs to the one whose
 * square holds its anchor, so every piece draws once and no node clips. A corridor's mesh splits per child.
 */
export const OWNER = 15, CHILD = 16;
export const key = ({ level, x, y }: Node) => `${level}/${x}/${y}`;

/**
 * A LOD rule: which nodes split, and the continuous LOD at a point. A level-k node draws where the
 * LOD is in [k − 1, k], and its vertices morph by k − LOD, so neighbours meet. `lod` is the twin of
 * `lod()` in drawn-height.wgsl.
 */
export interface Rule {
    /** The finest level that draws. */
    readonly finest: number;
    /** The distance in metres where the LOD is 0. */
    readonly range: number;
    /** Whether some point of the node wants a level finer than its own + `coarser`. */
    split(node: Node, coarser?: number): boolean;
    /** Whether some point of a drawn node morphs toward its parent's level, so the node reads its parent's page. */
    morphs(node: Node): boolean;
    lod(s: [number, number]): number;
}

/** The eye in units. */
const eyeUnits = ({ whole, part }: Anchor) => [whole[0] + part[0], whole[1] + part[1]];

/** The nearest and farthest LOD-space distance from the eye to a node, with the eye's height above the ground. */
export function distances(eye: Eye, { level, x, y }: Node): [number, number] {
    const [ex, ey] = eyeUnits(eye.anchor), s = size(level), unit = eye.anchor.unit;
    const near = (lo: number, e: number) => Math.max(lo - e, 0, e - lo - s), far = (lo: number, e: number) => Math.max(Math.abs(lo - e), Math.abs(lo + s - e));
    return [
        Math.hypot(near(x * s, ex) * unit, near(y * s, ey) * unit, eye.above),
        Math.hypot(far(x * s, ex) * unit, far(y * s, ey) * unit, eye.above),
    ];
}

/**
 * CDLOD (Strugar 2009) by distance: the LOD is log2(range / d) with a plateau at each whole level and
 * a morph band of MORPH at its far end. `factor` is the range of a level in its node sizes.
 */
export function cdlod(eye: Eye, factor: number, finest = FINEST): Rule {
    const range = factor * UNITS * eye.anchor.unit;
    const plateau = (q: number) => {
        const k = Math.floor(q);
        return Math.min(finest, Math.max(COARSEST, k - 1 + Math.min(1, (q - k) / MORPH)));
    };
    return {
        finest, range,
        split: (node, coarser = 0) => node.level + coarser < finest && Math.log2(range / distances(eye, node)[0]) > node.level + 1 + coarser - MARGIN,
        morphs: node => Math.log2(range / distances(eye, node)[1]) < node.level + MORPH + MARGIN,
        lod: ([sx, sy]) => plateau(Math.log2(range / Math.hypot(sx, sy, eye.above))),
    };
}

/**
 * The drawn nodes, and the nodes whose pages they read: the coarse cover first, then their own
 * pages, and their parents' where they morph.
 */
export interface Selection { nodes: Node[]; wanted: Node[] }

/**
 * The quadtree selection: from the four nodes of level 1 down, a node inside `bounds` (units: west,
 * north, east, south) that `visible` keeps either splits or draws. A culled node takes its subtree.
 * The coarse cover is the same selection COVER levels coarser and in every direction, so that the
 * pages under any new view are resident before the view turns there.
 */
export function select(rule: Rule, bounds: [number, number, number, number], visible: (node: Node) => boolean): Selection {
    const nodes: Node[] = [], wanted = new Map<string, Node>();
    const visit = (node: Node, cover: boolean) => {
        const s = size(node.level);
        if (node.x * s >= bounds[2] || (node.x + 1) * s <= bounds[0] || node.y * s >= bounds[3] || (node.y + 1) * s <= bounds[1]) return;
        if (!cover && !visible(node)) return;
        if (rule.split(node, cover ? COVER : 0)) {
            for (let q = 0; q < 4; q++) visit({ level: node.level + 1, x: 2 * node.x + (q & 1), y: 2 * node.y + (q >> 1) }, cover);
            return;
        }
        if (cover) {
            wanted.set(key(node), node);
            return;
        }
        nodes.push(node);
        wanted.set(key(node), node);
        if (node.level > COARSEST && rule.morphs(node)) {
            const parent = { level: node.level - 1, x: node.x >> 1, y: node.y >> 1 };
            wanted.set(key(parent), parent);
        }
    };
    for (let q = 0; q < 4; q++) visit({ level: COARSEST, x: q & 1, y: q >> 1 }, true);
    for (let q = 0; q < 4; q++) visit({ level: COARSEST, x: q & 1, y: q >> 1 }, false);
    return { nodes, wanted: [...wanted.values()] };
}
