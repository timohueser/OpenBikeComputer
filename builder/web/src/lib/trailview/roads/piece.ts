// A corridor mesh's piece, as corridor.wgsl reads it: the faces of one square of an owner, an L16 child or
// for its details an L18 square, in 16 bytes per vertex and 16-bit indices. A face takes its material from its first vertex, so faces that meet in a
// point of a cross-section share its vertex. Its instances of parts.ts lie behind its vertices.
import { CHILD, size } from '../terrain/lod';

/** Bytes of a vertex: x and y (int16, their high bits in steps about the piece's middle), z (float32), and the two words of `vertex`. */
export const ROAD_VERTEX = 16;
/** World units per step of a vertex: about 2 mm, as a rail's web is 18 mm wide. As CORRIDOR_STEP of corridor.wgsl. */
const STEP_UNITS = 1 / 12;
/** Low bits of x and of y in the look word, and the most steps from a middle: ±10922 units, 2730 past the edge of an L16 square. */
const FINE_BITS = 2, STEPS_MOST = 2 ** (15 + FINE_BITS);
/** Steps of a vertex's half-width and across, and steps per metre of its `along`, as corridor.wgsl reads them. */
const HALF_STEP_M = 0.025, HALF_STEPS = 511, ACROSS_RANGE_M = 16, ACROSS_STEPS = 2047, ALONG_STEPS = 64, ALONG_MOST = 2 ** 20 - 1;
/** Indices of a piece address at most this many vertices. */
const VERTICES = 65536;
/** A cross-section or a dead end adds fewer vertices than this. */
const ROOM = 1024;
/** Steps per world unit of an instance's point past its whole units, of its turn and per unit of its grade. As PART_* of corridor.wgsl. */
export const PART_FINE = 16, PART_TURN = 65536, PART_GRADE = 64;
const PART_GRADE_MOST = 127;
/** Numbers per instance that `place` keeps. */
const PLACE_FIELDS = 5;
/** A look word's fields, from its low bit: material (7 bits), surface code (4), flags (5), the band's priority (3), half-width (9), then the low bits of x and of y. */
const CODE_SHIFT = 7, FLAGS_SHIFT = 11, PRIO_SHIFT = 16, HALF_SHIFT = 19;
export const FINE_SHIFT = 28;

/** An index range of a piece, and the vertices that it reaches. */
export interface PieceRange { first: number; count: number; vertices: number }

/** The instances of a part in a piece: slots of ROAD_VERTEX bytes from the start of its `vertices`. */
export interface PartRange { part: number; first: number; count: number }

/** The faces of a square near the eye, far from it, and those that cast; they share the vertices. */
export interface RoadPiece {
    /** The level of its square, and the square's middle in world units: its vertices count from it. */
    level: number;
    x: number;
    y: number;
    /** The heights that its vertices can reach. */
    low: number;
    high: number;
    /** World units that its vertices reach past its square. */
    over: number;
    /** Its vertices, then its instances. */
    vertices: ArrayBuffer;
    indices: Uint16Array<ArrayBuffer>;
    near: PieceRange;
    far: PieceRange;
    casters: PieceRange;
    parts: PartRange[];
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
    /** The vertices made per station of a line. */
    readonly made = new Map<number, { a: number; dz: number; flags: number; index: number }[]>();
    /** Per part, PLACE_FIELDS numbers per instance. */
    private readonly placed = new Map<number, number[]>();

    /** A piece about the middle of a square of `level`; with `casts`, its near faces cast. */
    constructor(private readonly x: number, private readonly y: number, private readonly level = CHILD, private readonly casts = false) {}

    /** Whether a cross-section still fits. */
    get roomy(): boolean {
        return this.count + ROOM <= VERTICES;
    }

    /** A vertex at world point x, y, height z, with its `look` word, `across` metres from the line and `along` a repeat count from its base. */
    vertex(x: number, y: number, z: number, look: number, across: number, along: number): number {
        if (4 * (this.count + 1) > this.packed.length) this.grow();
        // Near the pole a wide way's band can pass the range: it holds at the edge, and never wraps.
        const at = 4 * this.count, step = (value: number) => Math.max(-STEPS_MOST, Math.min(STEPS_MOST - 1, Math.round(value / STEP_UNITS)));
        const sx = step(x - this.x), sy = step(y - this.y), low = 2 ** FINE_BITS - 1;
        this.packed[at] = (((sx >> FINE_BITS) & 0xffff) | ((sy >> FINE_BITS) << 16)) >>> 0;
        this.floats[at + 1] = z;
        this.packed[at + 2] = (look | ((sx & low) << FINE_SHIFT) | ((sy & low) << (FINE_SHIFT + FINE_BITS))) >>> 0;
        const reach = Math.max(-1, Math.min(1, across / ACROSS_RANGE_M));
        this.packed[at + 3] = ((Math.round(reach * ACROSS_STEPS) & 0xfff) | (Math.min(ALONG_MOST, Math.max(0, Math.round(along * ALONG_STEPS))) << 12)) >>> 0;
        this.low = Math.min(this.low, z);
        this.high = Math.max(this.high, z);
        this.out = Math.max(this.out, Math.abs(x - this.x), Math.abs(y - this.y));
        return this.count++;
    }

    /** An instance of a part at world point x, y, height z: `turn` radians from east to its left, on a way that rises `grade` per metre ahead. */
    place(part: number, x: number, y: number, z: number, turn: number, grade: number): void {
        (this.placed.get(part) ?? this.placed.set(part, []).get(part)!).push(x, y, z, turn, grade);
        this.low = Math.min(this.low, z);
        this.high = Math.max(this.high, z);
        this.out = Math.max(this.out, Math.abs(x - this.x), Math.abs(y - this.y));
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
        const { near, far, count, placed } = this, total = near.length + far.length;
        const instances = [...placed.values()].reduce((sum, list) => sum + list.length / PLACE_FIELDS, 0), vertices = new ArrayBuffer((count + instances) * ROAD_VERTEX);
        new Uint8Array(vertices).set(new Uint8Array(this.bytes, 0, count * ROAD_VERTEX));
        const ints = new Int32Array(vertices), floats = new Float32Array(vertices), packed = new Uint32Array(vertices), parts: PartRange[] = [];
        let slot = count;
        for (const [part, list] of placed) {
            parts.push({ part, first: slot, count: list.length / PLACE_FIELDS });
            for (let i = 0; i < list.length; i += PLACE_FIELDS, slot++) {
                const [x, y, z, turn, grade] = list.slice(i, i + PLACE_FIELDS), [wx, wy] = [Math.floor(x), Math.floor(y)], fine = (value: number) => Math.min(PART_FINE - 1, Math.round(value * PART_FINE));
                const steps = Math.round(((turn / (2 * Math.PI)) % 1 + 1) % 1 * PART_TURN) % PART_TURN, rise = Math.max(-PART_GRADE_MOST, Math.min(PART_GRADE_MOST, Math.round(grade * PART_GRADE)));
                ints.set([wx, wy], 4 * slot);
                floats[4 * slot + 2] = z;
                packed[4 * slot + 3] = (steps | (fine(x - wx) << 16) | (fine(y - wy) << 20) | ((rise & 255) << 24)) >>> 0;
            }
        }
        const indices = new Uint16Array(total + (total & 1));
        indices.set(near);
        indices.set(far, near.length);
        const reached = (list: number[]) => {
            const seen = new Uint8Array(count);
            let vertices = 0;
            for (const index of list) {
                vertices += 1 - seen[index];
                seen[index] = 1;
            }
            return vertices;
        };
        const closeBy = { first: 0, count: near.length, vertices: reached(near) };
        return {
            level: this.level, x: this.x, y: this.y, low: this.low - reach, high: this.high + reach, over: Math.max(0, this.out - size(this.level) / 2), vertices, indices, parts,
            near: closeBy, far: { first: near.length, count: far.length, vertices: reached(far) }, casters: this.casts ? closeBy : { first: total, count: 0, vertices: 0 },
        };
    }
}
