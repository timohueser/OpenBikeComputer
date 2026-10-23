// The anchored mesh of a node: models and cables whose vertices hang from anchors. An anchor is a span
// between two points of the drawn surface, a height over each and a sag; a vertex lies a share t along
// it, offset in the span's frame: x across to the right, y along, z up. A model on the surface hangs
// from a point: the span's far end only turns it, and its vertices lie at t = 0. The GPU finds the
// heights of the anchors once per frame (`anchor_heights` in structures.wgsl).
import type { Model } from '../models/shape';
import { metresPerUnit } from '../terrain/bins';

/** Words of an anchor, as `Anchor` in structures.wgsl: a and b (int32 world units), the heights over them and the sag (float32), and flags. */
export const ANCHOR_WORDS = 8;
/** Anchor flags: the anchor is a point, and its far end only gives the heading. */
export const POINT = 1;
/** Words of a vertex, as `AnchoredIn` in structures.wgsl: the offset in 1/OFFSET_SCALE m (sint16x4, w the side of a cable), the anchor and t (uint16 each), the colour and flags (unorm8x4). */
export const ANCHORED_VERTEX = 4;
export const OFFSET_SCALE = 256;
/** Vertex flags, after those of mesh.ts: the face takes the colour of the rock under it, tinted; the vertex is an edge of a cable's ribbon. */
export const STONE = 32, CABLE = 64;
/** A point's far end lies this many world units off: its heading is good to 1/HEADING rad. */
const HEADING = 4096;

/** The vertices and indices of an anchored mesh, whose anchors are those of its node. */
export interface AnchoredMesh {
    vertices: ArrayBuffer;
    indices: Uint32Array<ArrayBuffer>;
}

/**
 * A model on the surface, as kinds hand it over: its foot at world point x, y, its +y turned `yaw`
 * radians counter-clockwise from east, scaled; `flags` as the vertex flags, and whether it draws only
 * near the eye.
 */
export interface Instance { x: number; y: number; yaw: number; scale: number; model: Model; flags?: number; near?: boolean }

/** Collects the vertices that hang from anchors; the anchors are in `anchorWords`, which several builders of a node share. */
export class AnchoredBuilder {
    private readonly words: number[] = [];
    private readonly indices: number[] = [];

    constructor(private readonly anchorWords: number[]) {}

    /** A span from a to b in world units, `ha` and `hb` metres over the drawn surface, sagging `sag` metres at its middle. */
    span(a: [number, number], b: [number, number], ha: number, hb: number, sag: number): number {
        this.anchorWords.push(Math.round(a[0]), Math.round(a[1]), Math.round(b[0]), Math.round(b[1]), ha, hb, sag, 0);
        return this.anchorWords.length / ANCHOR_WORDS - 1;
    }

    /** A point on the drawn surface at world point x, y, whose +y points `yaw` radians counter-clockwise from east. */
    point(x: number, y: number, yaw: number): number {
        const at = this.span([x, y], [x + HEADING * Math.cos(yaw), y - HEADING * Math.sin(yaw)], 0, 0, 0);
        this.anchorWords[at * ANCHOR_WORDS + 7] = POINT;
        return at;
    }

    /** A vertex t along an anchor, at offset x, y, z metres in its frame, `w` the side of a cable's edge. */
    vertex(anchor: number, t: number, [x, y, z]: [number, number, number], colour: number, w = 0): number {
        const q = (v: number) => Math.max(-32768, Math.min(32767, Math.round(v * OFFSET_SCALE))) & 0xffff;
        this.words.push((q(x) | (q(y) << 16)) >>> 0, (q(z) | ((w & 0xffff) << 16)) >>> 0, (anchor | (Math.round(Math.min(1, Math.max(0, t)) * 65535) << 16)) >>> 0, colour >>> 0);
        return this.words.length / ANCHORED_VERTEX - 1;
    }

    triangle(a: number, b: number, c: number): void {
        this.indices.push(a, b, c);
    }

    /** A model t along an anchor, turned `turn` radians about z in the anchor's frame and moved to `at` there. `flags` go to every face. */
    model(model: Model, anchor: number, t = 0, turn = 0, at: [number, number, number] = [0, 0, 0], scale = 1, flags = 0): void {
        // Faces of one colour share their corners.
        const c = Math.cos(turn) * scale, s = Math.sin(turn) * scale, p = model.points, made = new Map<string, number>();
        model.colours.forEach((colour, f) => {
            const v = [0, 1, 2].map(k => {
                const i = 9 * f + 3 * k, name = `${p[i]} ${p[i + 1]} ${p[i + 2]} ${colour}`;
                return made.get(name) ?? made.set(name, this.vertex(anchor, t, [at[0] + c * p[i] - s * p[i + 1], at[1] + s * p[i] + c * p[i + 1], at[2] + scale * p[i + 2]], (colour | (flags << 24)) >>> 0)).get(name)!;
            });
            this.triangle(v[0], v[1], v[2]);
        });
    }

    /** A model on the surface at the foot of an instance. */
    place({ x, y, yaw, scale, model, flags = 0 }: Instance): void {
        this.model(model, this.point(x, y, yaw), 0, 0, [0, 0, 0], scale, flags);
    }

    /**
     * A cable along a span, `across` metres to the right of it, in `segments` pieces: a ribbon that the
     * vertex shader turns to the eye and widens to a pixel.
     */
    cable(anchor: number, across: number, segments: number, colour: number): void {
        let last: number[] = [];
        for (let k = 0; k <= segments; k++) {
            const edge = [-1, 1].map(w => this.vertex(anchor, k / segments, [across, 0, 0], (colour | (CABLE << 24)) >>> 0, w));
            if (last.length) {
                this.triangle(last[0], last[1], edge[1]);
                this.triangle(last[0], edge[1], edge[0]);
            }
            last = edge;
        }
    }

    mesh(): AnchoredMesh | null {
        return this.indices.length ? { vertices: new Uint32Array(this.words).buffer, indices: new Uint32Array(this.indices) } : null;
    }
}

/** Anchor words as `Anchor` in structures.wgsl reads them. */
export function anchorBytes(words: number[]): ArrayBuffer {
    const anchors = new ArrayBuffer(words.length * 4), ints = new Int32Array(anchors), floats = new Float32Array(anchors);
    words.forEach((word, i) => {
        if (i % ANCHOR_WORDS < 4 || i % ANCHOR_WORDS === 7) ints[i] = word;
        else floats[i] = word;
    });
    return anchors;
}

/** Metres between two world points. */
export const metresBetween = (a: [number, number], b: [number, number]) => Math.hypot(b[0] - a[0], b[1] - a[1]) * metresPerUnit((a[1] + b[1]) / 2);
