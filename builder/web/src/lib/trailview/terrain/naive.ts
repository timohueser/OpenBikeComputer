import { fetchTile, requestSignal, type Bounds } from '../../planner/layers/archive';
import { heights, imagePixels, UNKNOWN_HEIGHT } from '../../planner/layers/terrain';
import type { Gpu } from '../gpu/device';
import { tileAt, tileLonLat, type Frame } from '../frame/frame';
import shader from './terrain.wgsl?raw';

/** The deepest zoom of the planner terrain. */
const MAX_ZOOM = 12;
/** Cells along a tile edge; a tile draws as four quadrants of HALF × HALF cells. */
const GRID = 128, HALF = GRID / 2;
/** A tile splits while the eye is nearer than this many tile edges. */
const SPLIT = 2;
const LOADS = 6;
/** The tile meshes are a fixed pool; the tiles that the selection used longest ago leave first. */
const MESH_BUDGET = 96 * 2 ** 20;
/** Draws per frame, as `tiles` in terrain.wgsl. */
const MAX_DRAWS = 1024;
const VERTEX_BYTES = 16;
/** Grid rows and columns that carry a skirt: the tile edges and the quadrant edges. */
const SKIRTS = [0, HALF, GRID];
const VERTICES = (GRID + 1) ** 2 + 2 * SKIRTS.length * (GRID + 1);
const QUADRANT_INDICES = 6 * HALF * HALF + 24 * HALF;

/** The heights of tile z/x/y, row by row from the north-west, and the bytes of its body; undefined for an absent tile. */
export type HeightTile = (z: number, x: number, y: number) => Promise<{ raster: Int16Array; size: number; bytes: number } | undefined>;

/** The Terrarium tiles of a `{z}/{x}/{y}` template. */
export function terrariumTiles(template: string): HeightTile {
    return async (z, x, y) => {
        const body = await fetchTile(template, z, x, y, requestSignal());
        if (!body) return undefined;
        const rgba = await imagePixels(new Uint8Array(body));
        return { raster: heights(rgba.data, rgba.width * rgba.height), size: rgba.width, bytes: body.byteLength };
    };
}

interface Tile {
    z: number;
    x: number;
    y: number;
    /** The tile centre in the frame. */
    centre: [number, number];
    /** The drawn heights at the grid vertices, NaN where unknown: the surface the eye walks on. */
    grid: Float32Array;
    /** Kept, so a new device gets the tile again without a request. */
    vertices: ArrayBuffer;
    buffer?: GPUBuffer;
    /** The last selection that wanted the tile. */
    used: number;
}

/** A tile, and the bits of the quadrants that it draws: 1 north-west, 2 north-east, 4 south-west, 8 south-east. */
export interface Draw { tile: Tile; quadrants: number }

/** Where the selection looks from: `above` is the eye's height above the ground, or the pose's height over an unknown ground. */
export interface Place { east: number; north: number; above: number }
/** The eye in the frame, at height z above sea level. */
export interface Eye extends Place { z: number }

/**
 * One mesh per Terrarium tile of the planner terrain. The tiles split by distance; a missing or
 * absent tile leaves its quadrant of the parent drawn, so the view never has a hole.
 */
export class NaiveTerrain {
    requests = 0;
    bytes = 0;
    failed = 0;
    private readonly tiles = new Map<string, Tile>();
    private readonly absent = new Set<string>();
    private readonly loading = new Set<string>();
    private readonly boxes = new Map<string, [number, number, number, number]>();
    private wanted: { key: string; z: number; x: number; y: number; distance: number }[] = [];
    private missing = 0;
    private selection = 0;
    private gpu?: Gpu;
    private pipeline?: GPURenderPipeline;
    private bindGroup?: GPUBindGroup;
    private indices?: GPUBuffer;
    private uniforms?: GPUBuffer;

    constructor(private readonly frame: Frame, private readonly heightTile: HeightTile, private readonly bounds: Bounds, private readonly changed: () => void) {}

    /** Compiles the pipeline for a device; a new device needs it again, and gets the tiles again from memory. */
    async attach(gpu: Gpu, frameUniform: GPUBuffer, depthFormat: GPUTextureFormat): Promise<void> {
        for (const tile of this.tiles.values()) tile.buffer = undefined;
        const { device } = gpu;
        const module = device.createShaderModule({ label: 'terrain', code: shader });
        const pipeline = await device.createRenderPipelineAsync({
            label: 'terrain',
            layout: 'auto',
            vertex: {
                module,
                buffers: [{ arrayStride: VERTEX_BYTES, attributes: [{ shaderLocation: 0, offset: 0, format: 'float32x3' }, { shaderLocation: 1, offset: 12, format: 'snorm8x4' }] }],
            },
            fragment: { module, targets: [{ format: gpu.format }] },
            primitive: { topology: 'triangle-list' },
            depthStencil: { format: depthFormat, depthWriteEnabled: true, depthCompare: 'greater' },
        });
        const indices = gridIndices();
        this.indices = gpu.buffer('terrain', indices.byteLength, GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST);
        device.queue.writeBuffer(this.indices, 0, indices);
        this.uniforms = gpu.buffer('uniforms', 16 * MAX_DRAWS, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
        this.bindGroup = device.createBindGroup({
            layout: pipeline.getBindGroupLayout(0),
            entries: [{ binding: 0, resource: { buffer: frameUniform } }, { binding: 1, resource: { buffer: this.uniforms } }],
        });
        this.pipeline = pipeline;
        this.gpu = gpu;
    }

    /** True when no tile is loading and every tile that the last selection wanted has arrived. */
    get idle(): boolean {
        return this.missing === 0 && this.loading.size === 0;
    }

    get resident(): number {
        return this.tiles.size;
    }

    /** The drawn height under a point, from the finest tile that knows it; undefined while no tile knows it. */
    heightAt(lon: number, lat: number): number | undefined {
        for (let z = MAX_ZOOM; z >= 0; z--) {
            const { x, y, u, v } = tileAt(lon, lat, z);
            const tile = this.tiles.get(`${z}/${x}/${y}`);
            const height = tile && surfaceAt(tile.grid, u * GRID, v * GRID);
            if (height !== undefined) return height;
        }
        return undefined;
    }

    /** The tiles to draw for an eye, and the requests for the tiles that it wants. */
    select(eye: Place): Draw[] {
        this.selection++;
        this.evict();
        this.wanted = [];
        this.missing = 0;
        const draws: Draw[] = [];
        this.cover(0, 0, 0, eye, draws);
        this.wanted.sort((a, b) => a.z - b.z || a.distance - b.distance);
        for (const want of this.wanted) {
            if (this.loading.size >= LOADS) break;
            this.load(want.key, want.z, want.x, want.y);
        }
        return draws.slice(0, MAX_DRAWS);
    }

    draw(pass: GPURenderPassEncoder, draws: Draw[], eye: Eye): void {
        const { gpu, pipeline, bindGroup, indices, uniforms } = this;
        if (!gpu || !pipeline || !bindGroup || !indices || !uniforms || !draws.length) return;
        const data = new Float32Array(4 * draws.length);
        draws.forEach(({ tile }, i) => data.set([tile.centre[0] - eye.east, tile.centre[1] - eye.north, -eye.z, tile.z], 4 * i));
        gpu.device.queue.writeBuffer(uniforms, 0, data);
        pass.setPipeline(pipeline);
        pass.setBindGroup(0, bindGroup);
        pass.setIndexBuffer(indices, 'uint16');
        draws.forEach(({ tile, quadrants }, i) => {
            if (!tile.buffer) {
                tile.buffer = gpu.buffer('terrain', tile.vertices.byteLength, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST);
                gpu.device.queue.writeBuffer(tile.buffer, 0, tile.vertices);
            }
            pass.setVertexBuffer(0, tile.buffer);
            if (quadrants === 15) pass.drawIndexed(4 * QUADRANT_INDICES, 1, 0, 0, i);
            else for (let q = 0; q < 4; q++) if (quadrants & (1 << q)) pass.drawIndexed(QUADRANT_INDICES, 1, q * QUADRANT_INDICES, 0, i);
        });
    }

    /** Appends the draws that cover a tile, and returns whether they cover all of it. */
    private cover(z: number, x: number, y: number, eye: Place, draws: Draw[]): boolean {
        const box = this.box(z, x, y);
        if (!box) return true;
        const key = `${z}/${x}/${y}`, tile = this.tiles.get(key);
        const distance = Math.hypot(Math.max(box[0] - eye.east, 0, eye.east - box[2]), Math.max(box[1] - eye.north, 0, eye.north - box[3]), eye.above);
        if (tile) tile.used = this.selection;
        else if (!this.absent.has(key)) {
            this.missing++;
            if (!this.loading.has(key)) this.wanted.push({ key, z, x, y, distance });
        }
        let quadrants = 15;
        const start = draws.length;
        if (z < MAX_ZOOM && distance < SPLIT * Math.max(box[2] - box[0], box[3] - box[1])) {
            quadrants = 0;
            for (let q = 0; q < 4; q++) if (!this.cover(z + 1, 2 * x + (q & 1), 2 * y + (q >> 1), eye, draws)) quadrants |= 1 << q;
            if (!quadrants) return true;
        }
        if (tile) {
            draws.push({ tile, quadrants });
            return true;
        }
        // The parent draws this whole quadrant, so the children may not draw there too.
        draws.length = start;
        return false;
    }

    /** The tile's extent in the frame, or undefined when it lies outside the bounds. */
    private box(z: number, x: number, y: number): [number, number, number, number] | undefined {
        const [west, north] = tileLonLat(x, y, z), [east, south] = tileLonLat(x + 1, y + 1, z);
        const [w, s, e, n] = this.bounds;
        if (west >= e || east <= w || south >= n || north <= s) return undefined;
        const key = `${z}/${x}/${y}`;
        let box = this.boxes.get(key);
        if (!box) {
            box = [Infinity, Infinity, -Infinity, -Infinity];
            for (const [u, v] of [[0, 0], [0.5, 0], [1, 0], [0, 0.5], [1, 0.5], [0, 1], [0.5, 1], [1, 1]]) {
                const [px, py] = this.frame.local(...tileLonLat(x + u, y + v, z));
                box = [Math.min(box[0], px), Math.min(box[1], py), Math.max(box[2], px), Math.max(box[3], py)];
            }
            this.boxes.set(key, box);
        }
        return box;
    }

    private async load(key: string, z: number, x: number, y: number): Promise<void> {
        this.loading.add(key);
        this.requests++;
        try {
            const tile = await this.heightTile(z, x, y);
            if (!tile) {
                this.absent.add(key);
                return;
            }
            this.bytes += tile.bytes;
            this.tiles.set(key, { ...tileMesh(this.frame, z, x, y, tile.raster, tile.size), used: this.selection });
        } catch (error) {
            // A tile that fails stays out for this page, as an absent one.
            console.warn(`Terrain tile ${key}: ${error instanceof Error ? error.message : error}`);
            this.failed++;
            this.absent.add(key);
        } finally {
            this.loading.delete(key);
            this.changed();
        }
    }

    /**
     * Keeps every tile that the last selection used, so a tile leaves before the selection and never
     * loads again at once. A view that wants more than the pool holds keeps them all.
     */
    private evict(): void {
        let held = this.tiles.size * VERTICES * VERTEX_BYTES;
        if (held <= MESH_BUDGET) return;
        for (const [key, tile] of [...this.tiles].sort((a, b) => a[1].used - b[1].used)) {
            if (held <= MESH_BUDGET || tile.used >= this.selection - 1) break;
            if (tile.buffer) this.gpu?.destroy('terrain', tile.buffer);
            this.tiles.delete(key);
            held -= VERTICES * VERTEX_BYTES;
        }
    }
}

/** The grid vertex (i, j): column i from the west edge, row j from the north edge. */
const vertex = (i: number, j: number) => j * (GRID + 1) + i;
/** The vertex below grid vertex k of skirt line s: rows first, then columns. */
const skirt = (s: number, k: number) => (GRID + 1) ** 2 + s * (GRID + 1) + k;

/** The indices of the four quadrants, one after the other: their cells, then the skirts of their edges. */
function gridIndices(): Uint16Array<ArrayBuffer> {
    const out = new Uint16Array(4 * QUADRANT_INDICES);
    let n = 0;
    const quad = (a: number, b: number, c: number, d: number) => { out.set([a, c, b, b, c, d], n); n += 6; };
    for (let q = 0; q < 4; q++) {
        const i0 = (q & 1) * HALF, j0 = (q >> 1) * HALF;
        for (let j = j0; j < j0 + HALF; j++) for (let i = i0; i < i0 + HALF; i++) quad(vertex(i, j), vertex(i + 1, j), vertex(i, j + 1), vertex(i + 1, j + 1));
        for (const [line, j] of [[SKIRTS.indexOf(j0), j0], [SKIRTS.indexOf(j0 + HALF), j0 + HALF]]) {
            for (let i = i0; i < i0 + HALF; i++) quad(vertex(i, j), vertex(i + 1, j), skirt(line, i), skirt(line, i + 1));
        }
        for (const [line, i] of [[SKIRTS.indexOf(i0), i0], [SKIRTS.indexOf(i0 + HALF), i0 + HALF]]) {
            for (let j = j0; j < j0 + HALF; j++) quad(vertex(i, j), vertex(i, j + 1), skirt(SKIRTS.length + line, j), skirt(SKIRTS.length + line, j + 1));
        }
    }
    return out;
}

/** The height at grid position (gx, gy) on the triangles that the indices draw, NaN where unknown. */
export function surfaceAt(grid: Float32Array, gx: number, gy: number): number | undefined {
    const i = Math.min(GRID - 1, Math.max(0, Math.floor(gx))), j = Math.min(GRID - 1, Math.max(0, Math.floor(gy)));
    const fx = gx - i, fy = gy - j;
    const a = grid[vertex(i, j)], b = grid[vertex(i + 1, j)], c = grid[vertex(i, j + 1)], d = grid[vertex(i + 1, j + 1)];
    const h = fx + fy <= 1 ? a + fx * (b - a) + fy * (c - a) : d + (1 - fx) * (c - d) + (1 - fy) * (b - d);
    return Number.isNaN(h) ? undefined : h;
}

/** A tile's mesh: grid vertices relative to its centre, with normals, then the skirts that hang two cells below the edges. */
export function tileMesh(frame: Frame, z: number, x: number, y: number, raster: Int16Array, size: number): Tile {
    const n = GRID + 1, centre = frame.local(...tileLonLat(x + 0.5, y + 0.5, z));
    const grid = new Float32Array(n * n), positions = new Float64Array(3 * n * n);
    const sample = (px: number, py: number) => raster[Math.min(size - 1, Math.max(0, py)) * size + Math.min(size - 1, Math.max(0, px))];
    for (let j = 0; j <= GRID; j++) for (let i = 0; i <= GRID; i++) {
        const k = vertex(i, j), [east, north] = frame.local(...tileLonLat(x + i / GRID, y + j / GRID, z));
        // Pixel centres: the edge of the tile lies half a pixel beyond its outer samples.
        const px = i / GRID * size - 0.5, py = j / GRID * size - 0.5;
        const x0 = Math.floor(px), y0 = Math.floor(py), fx = px - x0, fy = py - y0;
        const a = sample(x0, y0), b = sample(x0 + 1, y0), c = sample(x0, y0 + 1), d = sample(x0 + 1, y0 + 1);
        const known = a !== UNKNOWN_HEIGHT && b !== UNKNOWN_HEIGHT && c !== UNKNOWN_HEIGHT && d !== UNKNOWN_HEIGHT;
        grid[k] = known ? (a * (1 - fx) + b * fx) * (1 - fy) + (c * (1 - fx) + d * fx) * fy : NaN;
        positions[3 * k] = east - centre[0];
        positions[3 * k + 1] = north - centre[1];
        positions[3 * k + 2] = known ? grid[k] : 0;
    }
    const vertices = new ArrayBuffer(VERTICES * VERTEX_BYTES);
    const floats = new Float32Array(vertices), bytes = new Int8Array(vertices);
    const at = (i: number, j: number, c: number) => positions[3 * vertex(Math.min(GRID, Math.max(0, i)), Math.min(GRID, Math.max(0, j))) + c];
    for (let j = 0; j <= GRID; j++) for (let i = 0; i <= GRID; i++) {
        const k = vertex(i, j), o = 16 * k;
        floats.set(positions.subarray(3 * k, 3 * k + 3), 4 * k);
        // The cross product of the tangents to the south and to the east.
        const ux = at(i + 1, j, 0) - at(i - 1, j, 0), uy = at(i + 1, j, 1) - at(i - 1, j, 1), uz = at(i + 1, j, 2) - at(i - 1, j, 2);
        const vx = at(i, j + 1, 0) - at(i, j - 1, 0), vy = at(i, j + 1, 1) - at(i, j - 1, 1), vz = at(i, j + 1, 2) - at(i, j - 1, 2);
        const nx = vy * uz - vz * uy, ny = vz * ux - vx * uz, nz = vx * uy - vy * ux, scale = 127 / (Math.hypot(nx, ny, nz) || 1);
        bytes[o + 12] = Math.round(nx * scale);
        bytes[o + 13] = Math.round(ny * scale);
        bytes[o + 14] = Math.round(nz * scale);
        bytes[o + 15] = Number.isNaN(grid[k]) ? 0 : 127;
    }
    const [west] = frame.local(...tileLonLat(x, y, z)), [east] = frame.local(...tileLonLat(x + 1, y, z));
    const drop = 2 * (east - west) / GRID;
    const hang = (index: number, k: number) => {
        bytes.copyWithin(16 * index, 16 * k, 16 * k + 16);
        floats[4 * index + 2] -= drop;
    };
    SKIRTS.forEach((line, s) => {
        for (let k = 0; k <= GRID; k++) {
            hang(skirt(s, k), vertex(k, line));
            hang(skirt(SKIRTS.length + s, k), vertex(line, k));
        }
    });
    return { z, x, y, centre, grid, vertices, used: 0 };
}
