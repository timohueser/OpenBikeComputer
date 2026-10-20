import { UNITS, type Anchor } from '../frame/space';

/** Cells along a node edge in the one grid mesh. */
export const GRID = 64;
/** Page texels along a node edge: a stored tile of zoom z holds the pages of its four nodes of level z + 1. */
export const TEXELS = 256;
/** The finest level: about 0.4 m per texel in the Alps. */
export const FINEST = 18;
/** The coarsest level with pages: the quadrants of tile 0/0/0. */
export const COARSEST = 1;
/** The morph band of a CDLOD level, in levels of log2 distance. */
export const MORPH = 0.3;
/** Nodes along an edge of the LOD map window of each level. */
export const MAP = 32;
/** A node splits this far, in levels, before its finest point reaches the next level, so float32 shaders see exact plateaus. */
const MARGIN = 1e-3;
/**
 * CDLOD ranges in node sizes. Below the least, a node can reach past the morph band of its parent's
 * level, and neighbours would not meet; above the most, a level leaves its LOD map window.
 */
export const RANGE = { least: 4.6, most: 8 } as const;
/** Clipmap ring half-widths in nodes: even, so a ring aligns with the nodes of the next coarser level. */
export const RINGS = [6, 4] as const;
/** A clipmap level is active while its ring reaches this many times the eye's height above the ground. */
const RING_REACH = 2.5;

/** Node `x`, `y` of a level: the Mercator tile of zoom `level`. */
export interface Node { level: number; x: number; y: number }

/** What the selection looks from: the eye, and its height above the ground. */
export interface Eye { anchor: Anchor; above: number }

/** Units along a node edge. */
export const size = (level: number) => 2 ** (30 - level);
export const key = ({ level, x, y }: Node) => `${level}/${x}/${y}`;

/**
 * A LOD rule: which nodes split, and the continuous LOD at a point. A level-k node draws where the
 * LOD is in [k − 1, k], and its vertices morph by k − LOD, so neighbours meet. `lod` is the twin of
 * `lod()` in drawn-height.wgsl.
 */
export interface Rule {
    readonly name: 'cdlod' | 'clipmap';
    /** The finest level that draws. */
    readonly finest: number;
    /** CDLOD: the distance in metres where the LOD is 0. Clipmap: the ring half-width in nodes. */
    readonly range: number;
    split(node: Node): boolean;
    /** Whether some point of a drawn node morphs toward its parent's level, so the node reads its parent's page. */
    morphs(node: Node): boolean;
    lod(s: [number, number]): number;
    /** Clipmap: the ring centre of a level in whole units. */
    centre(level: number): [number, number];
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
        name: 'cdlod', finest, range,
        split: node => node.level < finest && Math.log2(range / distances(eye, node)[0]) > node.level + 1 - MARGIN,
        morphs: node => Math.log2(range / distances(eye, node)[1]) < node.level + MORPH + MARGIN,
        lod: ([sx, sy]) => plateau(Math.log2(range / Math.hypot(sx, sy, eye.above))),
        centre: () => [0, 0],
    };
}

/**
 * Geometry clipmaps (Losasso and Hoppe 2004) on the node grid: each level is a square ring of
 * `half` nodes about the eye, snapped to the next coarser level's nodes, that blends to that level
 * over a band of its outer nodes. A level is active while its ring reaches far enough below a high eye.
 */
export function clipmap(eye: Eye, half: number, finest = FINEST): Rule {
    const [ex, ey] = eyeUnits(eye.anchor), unit = eye.anchor.unit;
    let top = finest;
    while (top > COARSEST + 1 && half * size(top) * unit < RING_REACH * eye.above) top--;
    const centre = (level: number): [number, number] => {
        const step = size(level - 1);
        return [Math.round(ex / step) * step, Math.round(ey / step) * step];
    };
    return {
        name: 'clipmap', finest: top, range: half, centre,
        split: ({ level, x, y }) => {
            if (level >= top) return false;
            const [cx, cy] = centre(level + 1), reach = half * size(level + 1), s = size(level);
            return x * s >= cx - reach && (x + 1) * s <= cx + reach && y * s >= cy - reach && (y + 1) * s <= cy + reach;
        },
        morphs: ({ level, x, y }) => {
            const s = size(level), far = Math.max(Math.abs(x * s - ex), Math.abs((x + 1) * s - ex), Math.abs(y * s - ey), Math.abs((y + 1) * s - ey));
            return far > (half / 2 + 0.55 - MARGIN) * s;
        },
        lod: ([sx, sy]) => {
            const u = [sx / unit + ex, -sy / unit + ey], e = Math.max(Math.abs(sx), Math.abs(sy)) / unit;
            for (let level = top; level > COARSEST; level--) {
                const [cx, cy] = centre(level), s = size(level);
                if (Math.max(Math.abs(u[0] - cx), Math.abs(u[1] - cy)) > half * s) continue;
                return level - Math.min(1, Math.max(0, (e / s - (half / 2 + 0.55)) / (half / 2 - 1.6)));
            }
            return COARSEST;
        },
    };
}

/** The drawn nodes, and the nodes whose pages they read: their own, and their parents' where they morph. */
export interface Selection { nodes: Node[]; wanted: Node[] }

/**
 * The quadtree selection: from the four nodes of level 1 down, a node inside `bounds` (units: west,
 * north, east, south) that `visible` keeps either splits or draws. A culled node takes its subtree.
 */
export function select(rule: Rule, bounds: [number, number, number, number], visible: (node: Node) => boolean): Selection {
    const nodes: Node[] = [], wanted = new Map<string, Node>();
    const visit = (node: Node) => {
        const s = size(node.level);
        if (node.x * s >= bounds[2] || (node.x + 1) * s <= bounds[0] || node.y * s >= bounds[3] || (node.y + 1) * s <= bounds[1]) return;
        if (!visible(node)) return;
        if (rule.split(node)) {
            for (let q = 0; q < 4; q++) visit({ level: node.level + 1, x: 2 * node.x + (q & 1), y: 2 * node.y + (q >> 1) });
            return;
        }
        nodes.push(node);
        wanted.set(key(node), node);
        if (node.level > COARSEST && rule.morphs(node)) {
            const parent = { level: node.level - 1, x: node.x >> 1, y: node.y >> 1 };
            wanted.set(key(parent), parent);
        }
    };
    for (let q = 0; q < 4; q++) visit({ level: COARSEST, x: q & 1, y: q >> 1 });
    return { nodes, wanted: [...wanted.values()] };
}
