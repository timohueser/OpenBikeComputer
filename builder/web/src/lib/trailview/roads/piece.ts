// A corridor mesh's piece, as corridor.wgsl reads it: the faces of one L16 child of an owner, in 16 bytes
// per vertex and 16-bit indices. A face takes its material from its first vertex, so faces that meet in a
// point of a cross-section share its vertex.

/** Bytes of a vertex: x and y (int16 in STEP_UNITS about the piece's middle), z (float32), and the two words of `vertex`. */
export const ROAD_VERTEX = 16;
/** World units per step of a vertex: ±10922 units about a middle, 2730 units past the edge of an L16 square. As CORRIDOR_STEP of corridor.wgsl. */
const STEP_UNITS = 1 / 3;
/** Steps of a vertex's half-width and across, and steps per metre of its `along`, as corridor.wgsl reads them. */
const HALF_STEP_M = 0.025, HALF_STEPS = 511, ACROSS_RANGE_M = 16, ACROSS_STEPS = 2047, ALONG_STEPS = 64, ALONG_MOST = 2 ** 20 - 1;
/** Indices of a piece address at most this many vertices. */
const VERTICES = 65536;
/** A cross-section or a dead end adds fewer vertices than this. */
const ROOM = 1024;
/** A look word's fields, from its low bit: material (7 bits), surface code (4), flags (5), the band's priority (3), half-width (9). */
const CODE_SHIFT = 7, FLAGS_SHIFT = 11, PRIO_SHIFT = 16, HALF_SHIFT = 19;

/** An index range of a piece, and the vertices that it reaches. */
export interface PieceRange { first: number; count: number; vertices: number }

/** The faces of an L16 child near the eye, far from it, and those that cast, the tail of the far range; they share the vertices. */
export interface RoadPiece {
    /** The middle of the child's square in world units: its vertices count from it. */
    x: number;
    y: number;
    /** The heights that its vertices can reach. */
    low: number;
    high: number;
    /** World units that its vertices reach past the child's square. */
    over: number;
    vertices: ArrayBuffer;
    indices: Uint16Array<ArrayBuffer>;
    near: PieceRange;
    far: PieceRange;
    casters: PieceRange;
}

export const lookWord = (mat: number, code: number, flags: number, prio: number, half: number) =>
    (mat | (code << CODE_SHIFT) | (flags << FLAGS_SHIFT) | (prio << PRIO_SHIFT) | (Math.min(HALF_STEPS, Math.round(half / HALF_STEP_M)) << HALF_SHIFT)) >>> 0;

/** Collects the vertices and the ranges of a piece. */
export class PieceBuilder {
    private bytes = new ArrayBuffer(4096 * ROAD_VERTEX);
    private packed = new Uint32Array(this.bytes);
    private floats = new Float32Array(this.bytes);
    private low = Infinity;
    private high = -Infinity;
    /** World units from the middle to the farthest vertex, along x or y. */
    private out = 0;
    count = 0;
    readonly near: number[] = [];
    readonly far: number[] = [];
    /** Faces of the far range that also cast. */
    readonly casters: number[] = [];
    /** The vertices made per station of a line. */
    readonly made = new Map<number, { a: number; dz: number; flags: number; index: number }[]>();

    /** A piece about a middle, of a square `half` world units each way. */
    constructor(private readonly x: number, private readonly y: number, private readonly half = 0) {}

    /** Whether a cross-section still fits. */
    get roomy(): boolean {
        return this.count + ROOM <= VERTICES;
    }

    /** A vertex at world point x, y, height z, with its `look` word, `across` metres from the line and `along` a repeat count from its base. */
    vertex(x: number, y: number, z: number, look: number, across: number, along: number): number {
        if (4 * (this.count + 1) > this.packed.length) this.grow();
        // Near the pole a wide way's band can pass the range: it holds at the edge, and never wraps.
        const at = 4 * this.count, step = (value: number) => Math.max(-32768, Math.min(32767, Math.round(value / STEP_UNITS))) & 0xffff;
        this.packed[at] = (step(x - this.x) | (step(y - this.y) << 16)) >>> 0;
        this.floats[at + 1] = z;
        this.packed[at + 2] = look;
        const reach = Math.max(-1, Math.min(1, across / ACROSS_RANGE_M));
        this.packed[at + 3] = ((Math.round(reach * ACROSS_STEPS) & 0xfff) | (Math.min(ALONG_MOST, Math.max(0, Math.round(along * ALONG_STEPS))) << 12)) >>> 0;
        this.low = Math.min(this.low, z);
        this.high = Math.max(this.high, z);
        this.out = Math.max(this.out, Math.abs(x - this.x), Math.abs(y - this.y));
        return this.count++;
    }

    private grow(): void {
        const old = new Uint8Array(this.bytes, 0, this.count * ROAD_VERTEX);
        this.bytes = new ArrayBuffer(2 * this.bytes.byteLength);
        new Uint8Array(this.bytes).set(old);
        this.packed = new Uint32Array(this.bytes);
        this.floats = new Float32Array(this.bytes);
    }

    /** The piece, `reach` metres of height past its vertices either way. */
    piece(reach: number): RoadPiece {
        const { near, far, casters, count } = this, drawn = near.length + far.length, total = drawn + casters.length;
        const indices = new Uint16Array(total + (total & 1));
        indices.set(near);
        indices.set(far, near.length);
        indices.set(casters, drawn);
        const reached = (list: number[]) => {
            const seen = new Uint8Array(count);
            let vertices = 0;
            for (const index of list) {
                vertices += 1 - seen[index];
                seen[index] = 1;
            }
            return vertices;
        };
        return {
            x: this.x, y: this.y, low: this.low - reach, high: this.high + reach, over: Math.max(0, this.out - this.half), vertices: this.bytes.slice(0, count * ROAD_VERTEX), indices,
            near: { first: 0, count: near.length, vertices: reached(near) }, far: { first: near.length, count: far.length + casters.length, vertices: reached([...far, ...casters]) }, casters: { first: drawn, count: casters.length, vertices: reached(casters) },
        };
    }
}
