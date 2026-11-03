// The debug mesh of a built cell: its plan as lines (and later fills) in world units, each vertex with
// the colour of the debug tool. The plan worker makes it; draw.ts draws it over the view.
import { metresPerUnit } from '../terrain/bins';

/** Frame units per world unit: the crate's frame is 2^34 of the square, the viewer's units 2^30. */
const FRAME_PER_UNIT = 16;
/** A cell is a zoom-16 tile: 2^18 frame units. */
const CELL_FRAME = 2 ** 18;
/** Words of a debug vertex: x and y in world units, and the colour as sRGB bytes with alpha, red lowest. */
export const DEBUG_WORDS = 3;
/**
 * Each vertex drapes on the drawn surface, so a line goes in steps of at most this: from 60 m, a height
 * error of 1 m at 50 m from the middle moves a point by 0.8 m on the screen; at these steps, by centimetres.
 */
export const EDGE_STEP_M = 2;
/** The alpha of the cell border, so that two neighbours show their shared border, and of a way in a tunnel. */
const FAINT = 0.4;
/** The sizes of the marks of the graph's points and crossings, in metres. */
const JUNCTION_M = 1.5, JOINT_M = 0.75, END_M = 1.5, CROSSING_M = 3;
const FAILED = '#ff0000';

/** The bridge's `vocabulary()`. */
export interface Vocabulary {
    stages: string[];
    families: { name: string; colour: string }[];
    levels: string[];
    networks: string[];
    points: string[];
    crossings: { name: string; letter: string }[];
    sources: string[];
    colours: Record<'link' | 'synthetic_deck' | 'junction' | 'joint' | 'end' | 'crossing', string>;
}

/** The flat graph of a `CellPlan`: positions in frame units from the cell's origin. */
export interface FlatGraph { samples: Int32Array; ways: Uint32Array; points: Uint32Array; crossings: Uint32Array }

export interface DebugMesh {
    /** Vertex pairs of a line list. */
    lines: Int32Array<ArrayBuffer>;
    /** Vertex triples of a triangle list. */
    fills: Int32Array<ArrayBuffer>;
}

/** As WAY_WORDS, POINT_WORDS and CROSSING_WORDS of the crate's transport. */
const WAY_WORDS = 6, POINT_WORDS = 5, CROSSING_WORDS = 5;
const WAY_LINK = 1, WAY_SYNTHETIC_DECK = 2;

/** '#rrggbb' and an alpha as the bytes of WGSL's unorm8x4: red lowest. */
export function packed(hex: string, alpha = 1): number {
    return (parseInt(hex.slice(1, 3), 16) | parseInt(hex.slice(3, 5), 16) << 8 | parseInt(hex.slice(5, 7), 16) << 16 | Math.round(alpha * 255) << 24) >>> 0;
}

/** Writes lines in world units from points in frame units of a cell, clipped to its square and in steps. */
class Lines {
    readonly words: number[] = [];
    private readonly step: number;
    private readonly origin: [number, number];

    constructor(cell: { x: number; y: number }) {
        const size = CELL_FRAME / FRAME_PER_UNIT;
        this.origin = [cell.x * size, cell.y * size];
        this.step = EDGE_STEP_M / metresPerUnit((cell.y + 0.5) * size) * FRAME_PER_UNIT;
    }

    /** Metres as frame units at the cell. */
    frame(metres: number): number {
        return metres / EDGE_STEP_M * this.step;
    }

    line(a: [number, number], b: [number, number], colour: number): void {
        const clipped = clip(a, b);
        if (!clipped) return;
        const [p, q] = clipped, steps = Math.max(1, Math.ceil(Math.hypot(q[0] - p[0], q[1] - p[1]) / this.step));
        const at = (k: number) => [this.origin[0] + Math.round((p[0] + (q[0] - p[0]) * k / steps) / FRAME_PER_UNIT), this.origin[1] + Math.round((p[1] + (q[1] - p[1]) * k / steps) / FRAME_PER_UNIT)];
        for (let k = 0; k < steps; k++) this.words.push(...at(k), colour, ...at(k + 1), colour);
    }

    ring(points: [number, number][], colour: number): void {
        points.forEach((p, i) => this.line(p, points[(i + 1) % points.length], colour));
    }

    /** A regular polygon of `n` corners about c with the radius r in frame units, the first at `turn` of a circle. */
    mark(c: [number, number], r: number, n: number, colour: number, turn = 0): void {
        this.ring(Array.from({ length: n }, (_, i) => {
            const a = 2 * Math.PI * (i / n + turn);
            return [c[0] + r * Math.cos(a), c[1] + r * Math.sin(a)] as [number, number];
        }), colour);
    }
}

/** The part of segment a–b inside the cell square [0, CELL_FRAME]² (Liang–Barsky), or undefined. */
function clip(a: [number, number], b: [number, number]): [[number, number], [number, number]] | undefined {
    let t0 = 0, t1 = 1;
    const d = [b[0] - a[0], b[1] - a[1]];
    for (const [p, q] of [[-d[0], a[0]], [d[0], CELL_FRAME - a[0]], [-d[1], a[1]], [d[1], CELL_FRAME - a[1]]]) {
        if (p === 0) {
            if (q < 0) return undefined;
            continue;
        }
        const t = q / p;
        if (p < 0) t0 = Math.max(t0, t);
        else t1 = Math.min(t1, t);
        if (t0 > t1) return undefined;
    }
    return [[a[0] + t0 * d[0], a[1] + t0 * d[1]], [a[0] + t1 * d[0], a[1] + t1 * d[1]]];
}

const square = (lines: Lines, colour: number) => lines.ring([[0, 0], [CELL_FRAME, 0], [CELL_FRAME, CELL_FRAME], [0, CELL_FRAME]], colour);

/**
 * The debug mesh of a cell's graph: each way's samples in its family's colour (red a link, orange a
 * synthetic deck, faint in a tunnel), a square at each owned junction, a cross at a joint, a ring at an
 * end, a diamond at a crossing, and the faint cell border. Every line stays inside the cell square.
 */
export function debugMesh(flat: FlatGraph, vocabulary: Vocabulary, cell: { x: number; y: number }): DebugMesh {
    const lines = new Lines(cell), c = vocabulary.colours;
    const family = vocabulary.families.map(({ colour }) => [packed(colour), packed(colour, FAINT)]);
    const tunnel = vocabulary.levels.indexOf('tunnel');
    for (let w = 0; w < flat.ways.length; w += WAY_WORDS) {
        const [, kind, level, flags, first, count] = flat.ways.subarray(w, w + WAY_WORDS);
        const faint = level === tunnel ? 1 : 0;
        const colour = flags & WAY_LINK ? packed(c.link) : flags & WAY_SYNTHETIC_DECK ? packed(c.synthetic_deck) : family[kind][faint];
        for (let s = first; s + 1 < first + count; s++) {
            lines.line([flat.samples[2 * s], flat.samples[2 * s + 1]], [flat.samples[2 * s + 2], flat.samples[2 * s + 3]], colour);
        }
    }
    const marks = [
        (p: [number, number]) => lines.mark(p, lines.frame(JUNCTION_M) * Math.SQRT2, 4, packed(c.junction), 1 / 8),
        (p: [number, number]) => {
            const r = lines.frame(JOINT_M), colour = packed(c.joint);
            lines.line([p[0] - r, p[1]], [p[0] + r, p[1]], colour);
            lines.line([p[0], p[1] - r], [p[0], p[1] + r], colour);
        },
        (p: [number, number]) => lines.mark(p, lines.frame(END_M), 8, packed(c.end)),
    ];
    for (let i = 0; i < flat.points.length; i += POINT_WORDS) {
        const [kind, , x, y, owned] = flat.points.subarray(i, i + POINT_WORDS);
        if (owned) marks[kind]([x | 0, y | 0]);
    }
    for (let i = 0; i < flat.crossings.length; i += CROSSING_WORDS) {
        const [x, y, , , owned] = flat.crossings.subarray(i, i + CROSSING_WORDS);
        if (owned) lines.mark([x | 0, y | 0], lines.frame(CROSSING_M), 4, packed(c.crossing));
    }
    square(lines, packed('#000000', FAINT));
    return { lines: new Int32Array(Uint32Array.from(lines.words, w => w >>> 0).buffer), fills: new Int32Array(0) };
}

/** The red outline of a cell that failed, so that a failed cell never hides. */
export function failedMesh(cell: { x: number; y: number }): DebugMesh {
    const lines = new Lines(cell);
    square(lines, packed(FAILED));
    return { lines: new Int32Array(Uint32Array.from(lines.words, w => w >>> 0).buffer), fills: new Int32Array(0) };
}
