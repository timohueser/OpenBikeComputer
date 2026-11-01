// The mesh of the structures of a node, as structures.wgsl reads it: flat faces in vertex colours,
// at heights from the bake, and some vertices on the drawn surface.
import { marked, metresPerUnit, mitre, RAIL_CODE, SLEEPER_M, type Line } from '../terrain/bins';
import { stationsOf as curveStations } from '../roads/stations';

/**
 * Words of a vertex, as `StructureIn` of structures.wgsl: x, y (int32 world units), z, colour and flags
 * (unorm8x4), across and half-width (snorm16x2 of ±ROAD_RANGE_M), and a track's sleepers from its start.
 */
export const STRUCTURE_VERTEX = 6;
const ROAD_RANGE_M = 16;
/** As NO_SLEEPERS of roads.wgsl: a face without a count of sleepers. */
export const NO_SLEEPERS = -1e9;
/**
 * Vertex flags in the colour's alpha: z is metres above the drawn surface; the face is a road, its colour's
 * red the surface code; the face is the dark void of a tunnel's opening, drawn only where the terrain draws
 * coarser than level 17; the face lies under a gallery's roof, in an even light; the face is a lamp; the
 * face lies in a tunnel's bore, lit by its lamps and near a portal by daylight. 32 and 64 are flags of
 * anchored.ts.
 */
export const GROUND = 1, PAINT = 2, VOID = 4, INSIDE = 8, LAMP = 16, BORE = 128;
/** On a road face, bits of the colour's green, as structures.wgsl reads them: its way has the lines of `marked`; a corridor draws it near the eye, and the paint hands over as on the ground. */
const MARKED_FACE = 1, CORRIDOR_FACE = 2;

export interface StructureMesh {
    vertices: ArrayBuffer;
    indices: Uint32Array<ArrayBuffer>;
}

/** A point of a cross-section: metres across to the left of the line, and metres up. */
export type Corner = [number, number];

/**
 * A place on a line: its point in world units, its height, the unit vector across it to the left
 * in world units per metre, with the length of a mitre, and on a track its sleepers from its start.
 */
export interface Station { x: number; y: number; z: number; ax: number; ay: number; sleepers?: number }

/** A mitre grows at most this much in a sharp bend. */
const MITRE = 3;

/** The stations of a line's vertices, made once each; a track's count its sleepers. */
export function stationsOf(line: Line): (v: number) => Station {
    const along = [0], n = line.z.length, made = new Map<number, Station>();
    if (line.code >= RAIL_CODE) for (let v = 1; v < n; v++) along.push(along[v - 1] + Math.hypot(line.xy[2 * v] - line.xy[2 * v - 2], line.xy[2 * v + 1] - line.xy[2 * v - 1]) * metresPerUnit(line.xy[2 * v + 1]));
    return v => made.get(v) ?? made.set(v, { ...station(line, v), sleepers: line.code >= RAIL_CODE ? deckSleepers(line, along[v], along[n - 1]) : undefined }).get(v)!;
}

/** The station of vertex v of a line: across to the left of its mitre, per metre. */
export function station(line: Line, v: number): Station {
    const [ux, uy] = mitre(line, v), n = line.z.length, w = v + 1 < n ? v : v - 1;
    const dx = line.xy[2 * w + 2] - line.xy[2 * w], dy = line.xy[2 * w + 3] - line.xy[2 * w + 1], l = Math.hypot(dx, dy) || 1;
    const x = line.xy[2 * v], y = line.xy[2 * v + 1], scale = 1 / Math.max((ux * dx + uy * dy) / l, 1 / MITRE) / metresPerUnit(y);
    return { x, y, z: line.z[v], ax: uy * scale, ay: -ux * scale };
}

/** The station a share u of the way from s to t. */
export const mix = (s: Station, t: Station, u: number): Station => ({
    x: s.x + (t.x - s.x) * u, y: s.y + (t.y - s.y) * u, z: s.z + (t.z - s.z) * u, ax: s.ax + (t.ax - s.ax) * u, ay: s.ay + (t.ay - s.ay) * u,
    sleepers: s.sleepers === undefined || t.sleepers === undefined ? undefined : s.sleepers + (t.sleepers - s.sleepers) * u,
});

/**
 * The sleepers of a deck or a bore of a track `metres` along its line of `total` metres, as `sleepers_at` of
 * stamp-core.wgsl counts them on the ground at its ends: from an end that meets the ground.
 */
export function deckSleepers(line: Line, metres: number, total: number): number | undefined {
    return line.abut?.[0] ? metres / SLEEPER_M : line.abut?.[1] ? -(total - metres) / SLEEPER_M : undefined;
}

/**
 * Calls `post` with the stations about each of the places that divide a segment from s to t, `length`
 * metres long, about every `spacing` metres, from its start: `width` metres apart.
 */
export function posts(s: Station, t: Station, length: number, post: (p: Station, q: Station) => void, width: number, spacing: number): void {
    const n = Math.max(1, Math.round(length / spacing)), half = width / 2 / Math.max(length, 1e-3);
    for (let k = 0; k < n; k++) post(mix(s, t, Math.max(0, k / n - half)), mix(s, t, k / n + half));
}

/** The colour of a hex `#rrggbb` as unorm8 bytes, with flags in alpha. */
export const rgb = (hex: string, flags = 0) => (parseInt(hex.slice(1, 3), 16) | (parseInt(hex.slice(3, 5), 16) << 8) | (parseInt(hex.slice(5, 7), 16) << 16) | (flags << 24)) >>> 0;

/** The colour of a road face of a line. */
export const paintOf = (line: Line, flags: number, corridor = false) => (rgb('#000000', flags) | line.code | (((marked(line) ? MARKED_FACE : 0) | (corridor ? CORRIDOR_FACE : 0)) << 8)) >>> 0;

/** A line's stations on its curve, as the corridors have them, with their metres along it and a track's sleepers. */
export function curveStops(line: Line): (Station & { along: number })[] {
    const list = curveStations(line, () => line.h), total = list[list.length - 1].along;
    return list.map(s => ({ ...s, sleepers: line.code >= RAIL_CODE ? deckSleepers(line, s.along, total) : undefined }));
}

/**
 * Collects faces. Each face is wound counter-clockwise seen from the side that `out` points to, in
 * metres east, north and up, so the back faces cull.
 */
export class MeshBuilder {
    private readonly words: number[] = [];
    private readonly indices: number[] = [];

    /** A vertex at a station's corner, or at a point. */
    vertex(x: number, y: number, z: number, colour: number, across = 0, half = 0, sleepers = NO_SLEEPERS): number {
        const snorm = (metres: number) => Math.round(Math.max(-1, Math.min(1, metres / ROAD_RANGE_M)) * 32767) & 0xffff;
        this.words.push(Math.round(x), Math.round(y), z, colour, (snorm(across) | (snorm(half) << 16)) >>> 0, sleepers);
        return this.words.length / STRUCTURE_VERTEX - 1;
    }

    at(s: Station, [a, up]: Corner, colour: number, across = 0, half = 0): number {
        return this.vertex(s.x + s.ax * a, s.y + s.ay * a, s.z + up, colour, across, half, s.sleepers);
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
            else if (k === 3 || k === 4) uints[i] = word;
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
