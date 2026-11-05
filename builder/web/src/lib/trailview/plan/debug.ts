// The debug mesh of a built cell: its faces as fills and edges and its graph as lines, in world units,
// each vertex with the colour of the debug tool. The plan worker makes it; draw.ts draws it over the view.
import earcut from 'earcut';
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
/** The fill's alpha over the surface's own, so the view shows through the faces. */
export const FILL_ALPHA = 0.5;
/** A fill is cut by the squares of this grid, so that no edge is longer than its diagonal, 4 m: the vertices drape as the lines' do. */
export const FILL_GRID_M = 2.8;
/** The alpha of the cell border, so that two neighbours show their shared border, and of a way in a tunnel. */
const FAINT = 0.4;
/** The sizes of the marks of the graph's points and crossings, in metres. */
const JUNCTION_M = 1.5, JOINT_M = 0.75, END_M = 1.5, CROSSING_M = 3;
const FAILED = '#ff0000';

/** The bridge's `vocabulary()`. */
export interface Vocabulary {
    stages: string[];
    /** Words per entry of the bridge's arrays. */
    words: Record<'wanted' | 'way' | 'point' | 'crossing' | 'face' | 'ring' | 'edge' | 'corner', number>;
    way_flags: Record<'link' | 'synthetic_deck', number>;
    families: { name: string; colour: string }[];
    levels: string[];
    networks: string[];
    points: string[];
    crossings: { name: string; letter: string }[];
    sources: string[];
    owners: string[];
    /** In surface code order; `fill` is '#rrggbbaa', null for no fill. */
    surfaces: { name: string; fill: string | null }[];
    /** The fill of a junction's face where it differs from its surface's. */
    junction_surfaces: { surface: number; fill: string }[];
    rules: string[];
    edges: { name: string; colour: string }[];
    colours: Record<'link' | 'synthetic_deck' | 'junction' | 'joint' | 'end' | 'crossing', string>;
}

/** The flat plan of a `CellPlan`: positions in frame units from the cell's origin. */
export interface FlatPlan {
    samples: Int32Array; ways: Uint32Array; points: Uint32Array; crossings: Uint32Array;
    vertices: Int32Array; faces: Uint32Array; rings: Uint32Array; ring_vertices: Uint32Array; edges: Uint32Array;
}

export interface DebugMesh {
    /** Vertex pairs of a line list. */
    lines: Int32Array<ArrayBuffer>;
    /** Vertex triples of a triangle list. */
    fills: Int32Array<ArrayBuffer>;
    /** Faces with a fill colour that gave no triangles: they show their edges only. */
    unfilled: number;
}

/** '#rrggbb' or '#rrggbbaa', its alpha times `alpha`, as the bytes of WGSL's unorm8x4: red lowest. */
export function packed(hex: string, alpha = 1): number {
    const own = hex.length === 9 ? parseInt(hex.slice(7, 9), 16) / 255 : 1;
    return (parseInt(hex.slice(1, 3), 16) | parseInt(hex.slice(3, 5), 16) << 8 | parseInt(hex.slice(5, 7), 16) << 16 | Math.round(own * alpha * 255) << 24) >>> 0;
}

type P = [number, number];

/** Writes lines and fills in world units from points in frame units of a cell, clipped to its square and in steps. */
class Mesh {
    readonly words: number[] = [];
    readonly fills: number[] = [];
    private readonly step: number;
    private readonly grid: number;
    private readonly origin: [number, number];

    constructor(cell: { x: number; y: number }) {
        const size = CELL_FRAME / FRAME_PER_UNIT;
        this.origin = [cell.x * size, cell.y * size];
        this.step = EDGE_STEP_M / metresPerUnit((cell.y + 0.5) * size) * FRAME_PER_UNIT;
        this.grid = this.frame(FILL_GRID_M);
    }

    private at(p: P): [number, number] {
        return [this.origin[0] + Math.round(p[0] / FRAME_PER_UNIT), this.origin[1] + Math.round(p[1] / FRAME_PER_UNIT)];
    }

    /**
     * A face, its outer ring first and then its holes, cut by the grid's squares inside the cell square
     * and each part triangulated, so the count grows with the area and the outline, not with the
     * shape of a triangulation of the whole face. Returns whether it gave a triangle.
     */
    fill(rings: P[][], colour: number): boolean {
        const g = this.grid, outer = rings[0], range = (k: 0 | 1) => {
            const v = outer.map(p => p[k]);
            return [Math.max(0, Math.floor(Math.min(...v) / g)), Math.min(Math.ceil(CELL_FRAME / g), Math.ceil(Math.max(...v) / g))];
        };
        const [[i0, i1], [j0, j1]] = [range(0), range(1)];
        let filled = false;
        for (let i = i0; i < i1; i++) {
            const column = rings.map(r => clipRing(r, 0, i * g, Math.min((i + 1) * g, CELL_FRAME)));
            if (column[0].length < 3) continue;
            for (let j = j0; j < j1; j++) {
                const [own, ...holes] = column.map(r => clipRing(r, 1, j * g, Math.min((j + 1) * g, CELL_FRAME)));
                const kept = holes.filter(h => h.length >= 3);
                // A square inside a hole: the clipped hole covers the clipped outer ring.
                if (own.length < 3 || Math.abs(area(own)) - kept.reduce((sum, h) => sum + Math.abs(area(h)), 0) <= 1) continue;
                const part = [own, ...kept], coords = part.flat(2), starts: number[] = [];
                part.reduce((at, ring, k) => (k && starts.push(at), at + ring.length), 0);
                const triangles = earcut(coords, starts);
                const at = (k: number) => this.at([coords[2 * k], coords[2 * k + 1]]);
                for (let t = 0; t < triangles.length; t += 3) this.fills.push(...at(triangles[t]), colour, ...at(triangles[t + 1]), colour, ...at(triangles[t + 2]), colour);
                filled ||= triangles.length > 0;
            }
        }
        return filled;
    }

    /** Metres as frame units at the cell. */
    frame(metres: number): number {
        return metres / EDGE_STEP_M * this.step;
    }

    line(a: [number, number], b: [number, number], colour: number): void {
        const clipped = clip(a, b);
        if (!clipped) return;
        const [p, q] = clipped, steps = Math.max(1, Math.ceil(Math.hypot(q[0] - p[0], q[1] - p[1]) / this.step));
        const at = (k: number) => this.at([p[0] + (q[0] - p[0]) * k / steps, p[1] + (q[1] - p[1]) * k / steps]);
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

/** The part of a ring between `lo` and `hi` on axis `k` (Sutherland–Hodgman): a concave ring may keep edges along the bounds. */
function clipRing(ring: P[], k: 0 | 1, lo: number, hi: number): P[] {
    let out = ring;
    for (const [bound, below] of [[lo, false], [hi, true]] as const) {
        const inside = (p: P) => (below ? p[k] <= bound : p[k] >= bound), next: P[] = [];
        out.forEach((p, i) => {
            const q = out[(i + 1) % out.length];
            if (inside(p)) next.push(p);
            if (inside(p) !== inside(q)) {
                const t = (bound - p[k]) / (q[k] - p[k]);
                next.push([p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1])]);
            }
        });
        if (next.length < 3) return [];
        out = next;
    }
    return out;
}

/** The signed area of a ring. */
const area = (ring: P[]) => ring.reduce((sum, p, i) => sum + p[0] * ring[(i + 1) % ring.length][1] - ring[(i + 1) % ring.length][0] * p[1], 0) / 2;

/** The faces of a plan: each face with a fill in triangles, each edge in its kind's colour. Returns the count of faces left unfilled. */
function faces(mesh: Mesh, flat: FlatPlan, vocabulary: Vocabulary): number {
    const { face: FACE_WORDS, ring: RING_WORDS, edge: EDGE_WORDS } = vocabulary.words, junction = vocabulary.owners.indexOf('junction');
    const fill = vocabulary.surfaces.map(({ fill }) => (fill ? packed(fill, FILL_ALPHA) : undefined));
    const junctionFill = new Map(vocabulary.junction_surfaces.map(({ surface, fill }) => [surface, packed(fill, FILL_ALPHA)]));
    const v = flat.vertices, point = (i: number): P => [v[2 * i], v[2 * i + 1]];
    let unfilled = 0;
    for (let f = 0; f < flat.faces.length; f += FACE_WORDS) {
        const [, owner, , surface] = flat.faces.subarray(f, f + 4), [first, count] = flat.faces.subarray(f + FACE_WORDS - 2, f + FACE_WORDS);
        const colour = (owner === junction ? junctionFill.get(surface) : undefined) ?? fill[surface];
        if (colour === undefined) continue;
        const rings = Array.from({ length: count }, (_, r) => {
            const [start, n] = flat.rings.subarray(RING_WORDS * (first + r), RING_WORDS * (first + r) + 2);
            return Array.from(flat.ring_vertices.subarray(start, start + n), point);
        });
        if (!mesh.fill(rings, colour)) unfilled++;
    }
    const border = vocabulary.edges.findIndex(e => e.name === 'cell_border');
    const edge = vocabulary.edges.map(({ colour }, kind) => packed(colour, kind === border ? FAINT : 1));
    for (let e = 0; e < flat.edges.length; e += EDGE_WORDS) mesh.line(point(flat.edges[e]), point(flat.edges[e + 1]), edge[flat.edges[e + 4]]);
    return unfilled;
}

const square = (lines: Mesh, colour: number) => lines.ring([[0, 0], [CELL_FRAME, 0], [CELL_FRAME, CELL_FRAME], [0, CELL_FRAME]], colour);

/**
 * The debug mesh of a cell's plan: the faces (`faces`), then over them each way's samples in its family's colour (red a link, orange a
 * synthetic deck, faint in a tunnel), a square at each owned junction, a cross at a joint, a ring at an
 * end, a diamond at a crossing, and the faint cell border. Every line stays inside the cell square.
 */
export function debugMesh(flat: FlatPlan, vocabulary: Vocabulary, cell: { x: number; y: number }): DebugMesh {
    const lines = new Mesh(cell), unfilled = faces(lines, flat, vocabulary), c = vocabulary.colours, { way: WAY_WORDS, point: POINT_WORDS, crossing: CROSSING_WORDS } = vocabulary.words;
    const { link: WAY_LINK, synthetic_deck: WAY_SYNTHETIC_DECK } = vocabulary.way_flags;
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
    return { ...words(lines), unfilled };
}

const words = (mesh: Mesh) => ({ lines: new Int32Array(Uint32Array.from(mesh.words, w => w >>> 0).buffer), fills: new Int32Array(Uint32Array.from(mesh.fills, w => w >>> 0).buffer) });

/** The red outline of a cell that failed, so that a failed cell never hides. */
export function failedMesh(cell: { x: number; y: number }): DebugMesh {
    const lines = new Mesh(cell);
    square(lines, packed(FAILED));
    return { ...words(lines), unfilled: 0 };
}
