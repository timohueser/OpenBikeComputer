// Packs the models that a generator makes into model files (model-file.ts), in the build. It measures each model as the
// instances did on the device: from the same float vertices, so the pools and the model records hold the same bytes.
import { MeshoptEncoder } from 'meshoptimizer/encoder';
import { MAGIC, OUTLINE_FLOATS, quantize, SINK_SLOPES, VERSION, type FloatMesh, type Mesh, type Packed, type Shape } from './model-file';

/** A tree's sink table: its slopes, SINK_STEP apart from SINK_STEP, as in emit.wgsl. */
const SINK_STEP = 0.25;
/** Frames of an impostor (8 × 8, FRAMES of hemi.wgsl). */
const FRAMES = OUTLINE_FLOATS / 4;

/** A model from its float tiers, finest first, and a tree's crown base in model units. */
export function pack(tiers: FloatMesh[], crown?: number): Packed {
    const extent = Math.max(...tiers.map(mesh => largest(mesh.position)));
    const radius = Math.max(0, ...tiers.map(({ position: p }) => {
        let most = 0;
        for (let i = 0; i < p.length; i += 3) most = Math.max(most, Math.hypot(p[i], p[i + 1], p[i + 2] - 0.5));
        return most;
    }));
    const spread = Math.max(0, ...tiers.map(({ position: p }) => {
        let most = 0;
        for (let i = 0; i < p.length; i += 3) most = Math.max(most, Math.hypot(p[i], p[i + 1]));
        return most;
    }));
    // The model record holds these values as float32; the proxy took its extent from the record.
    const hull = proxy(tiers[tiers.length - 1]), f32 = Math.fround;
    const shape: Shape = {
        extent: f32(extent), radius: f32(radius), spread: f32(spread), trunk: tiers.some(mesh => mesh.trunk), crown: crown === undefined ? undefined : f32(crown),
        sinks: sinks(tiers, crown).map(f32), proxy: hull && packMesh(hull, f32(extent)), outlines: outlines(tiers[tiers.length - 1], radius),
    };
    return { tiers: tiers.map(mesh => packMesh(mesh, extent)), shape };
}

const packMesh = (mesh: FloatMesh, extent: number): Mesh => ({ vertices: quantize(mesh, extent), index: Uint32Array.from(mesh.index) });

/** A model file of models: the layout of specs/trailview-models.md. */
export async function encodeModels(models: Packed[]): Promise<Uint8Array<ArrayBuffer>> {
    await MeshoptEncoder.ready;
    const words: number[] = [], floats = new Set<number>(), blobs: Uint8Array[] = [];
    const u32 = (value: number) => words.push(value), f32 = (value: number) => (floats.add(words.length), words.push(value));
    u32(MAGIC);
    u32(VERSION);
    u32(models.length);
    for (const { tiers, shape } of models) {
        [shape.extent, shape.radius, shape.spread, shape.crown ?? NaN, ...shape.sinks].forEach(f32);
        const meshes = shape.proxy ? [...tiers, shape.proxy] : tiers;
        u32((shape.trunk ? 1 : 0) | (shape.proxy ? 2 : 0));
        u32(tiers.length);
        for (const { vertices, index } of meshes) {
            u32(vertices.length / 16);
            u32(index.length);
        }
        shape.outlines.forEach(f32);
        for (const { vertices, index } of meshes) blobs.push(MeshoptEncoder.encodeVertexBuffer(vertices, vertices.length / 16, 16), MeshoptEncoder.encodeIndexSequence(new Uint8Array(index.buffer, index.byteOffset, index.byteLength), index.length, 4));
    }
    const head = words.length * 4, size = blobs.reduce((sum, blob) => sum + 4 + Math.ceil(blob.length / 4) * 4, head);
    const out = new Uint8Array(size), view = new DataView(out.buffer);
    words.forEach((value, k) => (floats.has(k) ? view.setFloat32(4 * k, value, true) : view.setUint32(4 * k, value, true)));
    let at = head;
    for (const blob of blobs) {
        view.setUint32(at, blob.length, true);
        out.set(blob, at + 4);
        at += 4 + Math.ceil(blob.length / 4) * 4;
    }
    return out;
}

/** The largest absolute value. */
function largest(values: ArrayLike<number>): number {
    let most = 0;
    for (let i = 0; i < values.length; i++) most = Math.max(most, Math.abs(values[i]));
    return most;
}

/**
 * The proxy of a crown in the far shadow map: a diamond of 8 faces about its crown's vertices, from its
 * lowest to its highest point and widest where its widest vertex lies. A mesh whose faces cover less
 * than half of the diamond's outline seen from above, such as a bare crown's branches, has none.
 */
export function proxy(mesh: FloatMesh): FloatMesh | undefined {
    const p = mesh.position, from = mesh.trunk ?? 0, count = p.length / 3;
    if (count <= from) return undefined;
    let [low, high, middle, widest] = [Infinity, -Infinity, 0, 0];
    const side = [0, 0, 0, 0];
    for (let k = from; k < count; k++) {
        const [x, y, z] = [p[3 * k], p[3 * k + 1], p[3 * k + 2]];
        [low, high] = [Math.min(low, z), Math.max(high, z)];
        side[0] = Math.max(side[0], x), side[1] = Math.max(side[1], y), side[2] = Math.max(side[2], -x), side[3] = Math.max(side[3], -y);
        if (Math.hypot(x, y) > widest) [widest, middle] = [Math.hypot(x, y), z];
    }
    let cover = 0;
    for (let i = 0; i < mesh.index.length; i += 3) {
        const [a, b, c] = [mesh.index[i], mesh.index[i + 1], mesh.index[i + 2]].map(v => 3 * v);
        if (Math.min(a, b, c) < 3 * from) continue;
        cover += Math.abs((p[b] - p[a]) * (p[c + 1] - p[a + 1]) - (p[c] - p[a]) * (p[b + 1] - p[a + 1])) / 2;
    }
    if (cover < (side[0] + side[2]) * (side[1] + side[3]) / 4) return undefined;
    const position = [0, 0, high, 0, 0, low, side[0], 0, middle, 0, side[1], middle, -side[2], 0, middle, 0, -side[3], middle];
    return { position, normal: new Array(18).fill(0), color: new Array(18).fill(1), index: [0, 2, 3, 0, 3, 4, 0, 4, 5, 0, 5, 2, 1, 3, 2, 1, 4, 3, 1, 5, 4, 1, 2, 5] };
}

/**
 * Per frame of a model's impostor (8 × 8, FRAMES of hemi.wgsl), the trapezoid of the frame's square
 * (−1 to 1 over the sphere of `radius` about (0, 0, 0.5)) that holds the mesh seen along the frame: its
 * bottom and top, and its half widths there, the least in area.
 */
export function outlines(mesh: FloatMesh, radius: number): Float32Array<ArrayBuffer> {
    const BANDS = 32, p = mesh.position, n = p.length / 3, out = new Float32Array(FRAMES * 4);
    const xs = new Float64Array(n), ys = new Float64Array(n);
    for (let frame = 0; frame < FRAMES; frame++) {
        // hemi_decode and view_axes of hemi.wgsl.
        const ex = (frame % 8) / 3.5 - 1, ey = Math.floor(frame / 8) / 3.5 - 1, px = (ex + ey) / 2, py = (ex - ey) / 2;
        const pz = 1 - Math.abs(px) - Math.abs(py), norm = Math.hypot(px, py, pz), d = [px / norm, py / norm, pz / norm];
        const flat = Math.hypot(d[0], d[1]), right = Math.abs(d[2]) < 0.999 ? [-d[1] / flat, d[0] / flat, 0] : [1, 0, 0];
        const up = [d[1] * right[2] - d[2] * right[1], d[2] * right[0] - d[0] * right[2], d[0] * right[1] - d[1] * right[0]];
        let low = Infinity, high = -Infinity;
        for (let k = 0; k < n; k++) {
            const x = p[3 * k], y = p[3 * k + 1], z = p[3 * k + 2] - 0.5;
            xs[k] = Math.abs(x * right[0] + y * right[1] + z * right[2]) / radius;
            ys[k] = (x * up[0] + y * up[1] + z * up[2]) / radius;
            low = Math.min(low, ys[k]);
            high = Math.max(high, ys[k]);
        }
        // The widest |x| in each of BANDS bands of height, which the trapezoid holds at both ends of the band.
        const span = Math.max(high - low, 1e-6), widest = new Float64Array(BANDS).fill(-1);
        for (let k = 0; k < n; k++) {
            const band = Math.min(BANDS - 1, Math.floor(((ys[k] - low) / span) * BANDS));
            widest[band] = Math.max(widest[band], xs[k]);
        }
        // The least w0 + w1 where w0 (1 − t) + w1 t ≥ the widest at both ends t of every band: convex in w1.
        const bottom = (top: number) => {
            let w = 0;
            for (let band = 0; band < BANDS; band++) {
                const x = widest[band];
                if (x < 0) continue;
                // At the band's foot; its top binds harder unless it is the trapezoid's top.
                w = Math.max(w, (x - (top * band) / BANDS) / (1 - band / BANDS));
                if (band < BANDS - 1) w = Math.max(w, (x - (top * (band + 1)) / BANDS) / (1 - (band + 1) / BANDS));
            }
            return w;
        };
        // The top band's top needs w1 at least its widest.
        let a = widest[BANDS - 1], b = Math.max(...widest);
        for (let step = 0; step < 24; step++) {
            const m1 = a + (b - a) / 3, m2 = b - (b - a) / 3;
            if (m1 + bottom(m1) <= m2 + bottom(m2)) b = m2;
            else a = m1;
        }
        out.set([low, high, bottom(b) + 1e-4, b + 1e-4], 4 * frame);
    }
    return out;
}

/**
 * A tree's sink table (slope_sink of emit.wgsl): per slope SINK_STEP × (k + 1), the most that the drop
 * of the ground under a crown vertex, the slope × its reach, exceeds its height above the crown's base;
 * a vertex below the base counts as at it. Over the vertices of every tier but their trunks'.
 */
export function sinks(tiers: FloatMesh[], crown: number | undefined): number[] {
    const out = new Array<number>(SINK_SLOPES).fill(0);
    if (crown === undefined) return out;
    for (const { position: p, trunk = 0 } of tiers)
        for (let i = 3 * trunk; i < p.length; i += 3) {
            const reach = Math.hypot(p[i], p[i + 1]), above = Math.max(0, p[i + 2] - crown);
            for (let k = 0; k < SINK_SLOPES; k++) out[k] = Math.max(out[k], SINK_STEP * (k + 1) * reach - above);
        }
    return out;
}
