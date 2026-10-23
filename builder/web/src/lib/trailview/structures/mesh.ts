// The mesh of the structures of a node, as structures.wgsl reads it: flat faces in vertex colours,
// at heights from the bake, and some vertices on the drawn surface.
import { metresPerUnit, mitre, type Line } from '../terrain/bins';

/** Words of a vertex, as `StructureIn` of structures.wgsl: x, y (int32 world units), z, colour and flags (unorm8x4), across and half-width. */
export const STRUCTURE_VERTEX = 6;
/**
 * Vertex flags in the colour's alpha: z is metres above the drawn surface; the face is a road, its colour's
 * red the surface code; the face takes no light, as the dark inside of a tunnel.
 */
export const GROUND = 1, PAINT = 2, VOID = 4;

export interface StructureMesh {
    vertices: ArrayBuffer;
    indices: Uint32Array<ArrayBuffer>;
}

/** A point of a cross-section: metres across to the left of the line, and metres up. */
export type Corner = [number, number];

/**
 * A place on a line: its point in world units, its height, and the unit vector across it to the left
 * in world units per metre, with the length of a mitre.
 */
export interface Station { x: number; y: number; z: number; ax: number; ay: number }

/** A mitre grows at most this much in a sharp bend. */
const MITRE = 3;

/** The station of vertex v of a line: across to the left of its mitre, per metre. */
export function station(line: Line, v: number): Station {
    const [ux, uy] = mitre(line, v), n = line.z.length, w = v + 1 < n ? v : v - 1;
    const dx = line.xy[2 * w + 2] - line.xy[2 * w], dy = line.xy[2 * w + 3] - line.xy[2 * w + 1], l = Math.hypot(dx, dy) || 1;
    const x = line.xy[2 * v], y = line.xy[2 * v + 1], scale = 1 / Math.max((ux * dx + uy * dy) / l, 1 / MITRE) / metresPerUnit(y);
    return { x, y, z: line.z[v], ax: uy * scale, ay: -ux * scale };
}

/** The station a share u of the way from s to t. */
export const mix = (s: Station, t: Station, u: number): Station => ({ x: s.x + (t.x - s.x) * u, y: s.y + (t.y - s.y) * u, z: s.z + (t.z - s.z) * u, ax: s.ax + (t.ax - s.ax) * u, ay: s.ay + (t.ay - s.ay) * u });

/**
 * Calls `post` with the stations about each place every `spacing` metres along a line between s and t,
 * `width` metres apart; s lies `along` metres along the line and t `length` metres after it.
 */
export function posts(s: Station, t: Station, along: number, length: number, post: (p: Station, q: Station) => void, width: number, spacing: number): void {
    for (let k = Math.ceil(along / spacing); k * spacing < along + length; k++) {
        const u = (k * spacing - along) / length, half = width / 2 / Math.max(length, 1e-3);
        post(mix(s, t, Math.max(0, u - half)), mix(s, t, Math.min(1, u + half)));
    }
}

/** The colour of a hex `#rrggbb` as unorm8 bytes, with flags in alpha. */
export const rgb = (hex: string, flags = 0) => (parseInt(hex.slice(1, 3), 16) | (parseInt(hex.slice(3, 5), 16) << 8) | (parseInt(hex.slice(5, 7), 16) << 16) | (flags << 24)) >>> 0;

/**
 * Collects faces. Each face is wound counter-clockwise seen from the side that `out` points to, in
 * metres east, north and up, so the back faces cull.
 */
export class MeshBuilder {
    private readonly words: number[] = [];
    private readonly indices: number[] = [];

    /** A vertex at a station's corner, or at a point. */
    vertex(x: number, y: number, z: number, colour: number, across = 0, half = 0): number {
        this.words.push(Math.round(x), Math.round(y), z, colour, across, half);
        return this.words.length / STRUCTURE_VERTEX - 1;
    }

    at(s: Station, [a, up]: Corner, colour: number, across = 0, half = 0): number {
        return this.vertex(s.x + s.ax * a, s.y + s.ay * a, s.z + up, colour, across, half);
    }

    /** A quad of four vertices in order around it, turned to face `out` (east, north, up). */
    quad(v: [number, number, number, number], out: [number, number, number]): void {
        const facing = this.facing(v[0], v[1], v[2], v[3], out);
        this.indices.push(...(facing ? [v[0], v[1], v[2], v[0], v[2], v[3]] : [v[0], v[2], v[1], v[0], v[3], v[2]]));
    }

    triangle(a: number, b: number, c: number, out: [number, number, number]): void {
        this.indices.push(...(this.facing(a, b, c, c, out) ? [a, b, c] : [a, c, b]));
    }

    /** The quad between two stations from corner a to corner b, facing `out` in the cross-section's plane: [across, up]. */
    strip(s: Station, t: Station, a: Corner, b: Corner, colour: number, out: Corner): void {
        const o = this.outward(s, t, out);
        this.quad([this.at(s, a, colour), this.at(s, b, colour), this.at(t, b, colour), this.at(t, a, colour)], o);
    }

    mesh(): StructureMesh | null {
        if (!this.indices.length) return null;
        const vertices = new ArrayBuffer(this.words.length * 4), floats = new Float32Array(vertices), ints = new Int32Array(vertices), uints = new Uint32Array(vertices);
        this.words.forEach((word, i) => {
            const k = i % STRUCTURE_VERTEX;
            if (k < 2) ints[i] = word;
            else if (k === 3) uints[i] = word;
            else floats[i] = word;
        });
        return { vertices, indices: new Uint32Array(this.indices) };
    }

    /** The direction east, north, up of a direction [across, up] at the middle of two stations. */
    outward(s: Station, t: Station, [a, up]: Corner): [number, number, number] {
        const ax = (s.ax + t.ax) / 2, ay = (s.ay + t.ay) / 2, l = Math.hypot(ax, ay) || 1;
        return [a * ax / l, -a * ay / l, up];
    }

    /** Whether a, b, c, d in order turn counter-clockwise seen from `out`, by their diagonals. */
    private facing(a: number, b: number, c: number, d: number, out: [number, number, number]): boolean {
        const [p, q, r, s] = [a, b, c, d].map(i => this.point(i));
        const u = r.map((v, k) => v - p[k]), w = s.map((v, k) => v - q[k]);
        const n = [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]];
        return n[0] * out[0] + n[1] * out[1] + n[2] * out[2] >= 0;
    }

    /** A vertex in metres east, north and up of the first vertex; one on the drawn surface lies far below the others. */
    private point(i: number): number[] {
        const w = this.words, m = metresPerUnit(w[1]), at = STRUCTURE_VERTEX * i;
        return [(w[at] - w[0]) * m, -(w[at + 1] - w[1]) * m, (w[at + 3] >>> 24) & GROUND ? -1e4 : w[at + 2]];
    }
}
