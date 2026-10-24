import type { PlannerConfig } from '../../planner/config';
import { openSource, requestSignal, type Source } from '../../planner/layers/archive';
import { imagePixels } from '../../planner/layers/terrain';
import type { Gpu } from '../gpu/device';

/** The canopy layer's zoom and cells a side (specs/trailview-tiles.md#canopy). */
export const CANOPY_ZOOM = 12, CELLS = 256;
/** Tiles on the GPU, tiles along a side of the window about the eye that the map indexes, and tile loads in flight: the eye's tile and its neighbours at once. */
const SLOTS = 32, WINDOW = 8, LOADS = 9;
/** Mip levels of a tile, 256 cells a side to 1. */
const MIPS = 9;
/** A view wants the tiles within this many tiles of the eye's at most: 5 × 5 fit the slots and the window. */
export const AROUND = 2;

interface Slot { key: string; x: number; y: number; slot: number; used: number }

/**
 * The tiles of the `trailview-canopy` layer about the eye, in a pool of r16uint texture layers: each
 * cell's code as specs/trailview-tiles.md#canopy defines it. A map indexes the window of tiles about
 * the eye: its first tile, then per tile the layer + 1, 0 for none. A tile that the archive does not hold is unknown, and
 * so is a tile that has not arrived: no tree stands there yet.
 */
export class Canopy {
    requests = 0;
    bytes = 0;
    failed = 0;
    /** Whether the last update changed the tiles on the GPU. */
    changed = false;
    private source?: Source;
    private texture?: GPUTexture;
    private mapBuffer?: GPUBuffer;
    private gpu?: Gpu;
    private readonly slots = new Map<string, Slot>();
    private readonly absent = new Set<string>();
    private readonly loading = new Set<string>();
    private arrived: { x: number; y: number; codes: Uint16Array<ArrayBuffer> }[] = [];
    private frame = 0;
    private origin: [number, number] = [0, 0];

    constructor(config: PlannerConfig, private readonly wake: () => void) {
        const url = (config.layers as Partial<Record<string, string>>)['trailview-canopy'];
        if (url) openSource(url).then(source => {
            this.source = source;
            wake();
        }, error => console.warn(`TrailView draws no forest: ${error instanceof Error ? error.message : error}`));
    }

    get idle(): boolean {
        return this.loading.size === 0 && this.arrived.length === 0;
    }

    /** The texture of the pool on a device; the window's map goes to the start of `map`. */
    attach(gpu: Gpu, map: GPUBuffer): GPUTexture {
        this.gpu = gpu;
        this.mapBuffer = map;
        this.slots.clear();
        this.texture = gpu.texture('vegetation', { size: [CELLS, CELLS, SLOTS], mipLevelCount: MIPS, format: 'r16uint', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST }, CELLS * CELLS * SLOTS * 2 * 4 / 3);
        return this.texture;
    }

    /**
     * Asks for the tiles within `around` tiles (AROUND at most) of the eye at world pixel (x, y) of zoom
     * CANOPY_ZOOM and puts the arrived ones on the GPU.
     */
    update(x: number, y: number, around: number): void {
        this.changed = false;
        this.frame++;
        const tx = Math.floor(x), ty = Math.floor(y), wanted: [number, number][] = [];
        for (let j = -around; j <= around; j++) for (let i = -around; i <= around; i++) wanted.push([tx + i, ty + j]);
        wanted.sort((a, b) => Math.hypot(a[0] + 0.5 - x, a[1] + 0.5 - y) - Math.hypot(b[0] + 0.5 - x, b[1] + 0.5 - y));
        for (const [i, j] of wanted) {
            const key = `${i}/${j}`, slot = this.slots.get(key);
            if (slot) slot.used = this.frame;
            else this.load(i, j);
        }
        for (const { x: i, y: j, codes } of this.arrived.splice(0)) {
            const slot = this.allocate(`${i}/${j}`, i, j);
            if (!slot || !this.gpu || !this.texture) continue;
            densest(codes).forEach((level, mip) => {
                const side = CELLS >> mip;
                this.gpu!.device.queue.writeTexture({ texture: this.texture!, origin: [0, 0, slot.slot], mipLevel: mip }, level, { bytesPerRow: 2 * side }, [side, side, 1]);
            });
            this.changed = true;
        }
        const origin: [number, number] = [tx - WINDOW / 2, ty - WINDOW / 2];
        if (!this.changed && origin[0] === this.origin[0] && origin[1] === this.origin[1]) return;
        this.origin = origin;
        const map = new Int32Array(4 + WINDOW * WINDOW);
        map.set(origin);
        for (const { x: i, y: j, slot } of this.slots.values()) {
            const a = i - origin[0], b = j - origin[1];
            if (a >= 0 && b >= 0 && a < WINDOW && b < WINDOW) map[4 + b * WINDOW + a] = slot + 1;
        }
        this.gpu?.device.queue.writeBuffer(this.mapBuffer!, 0, map);
        this.changed = true;
    }

    /** A lost device keeps the absent tiles; every tile loads again. */
    lost(): void {
        this.slots.clear();
        this.arrived = [];
    }

    private load(x: number, y: number): void {
        const key = `${x}/${y}`, source = this.source;
        if (!source || this.loading.has(key) || this.absent.has(key) || this.loading.size >= LOADS || this.arrived.some(a => a.x === x && a.y === y)) return;
        this.loading.add(key);
        source.get(CANOPY_ZOOM, x, y, requestSignal()).then(async body => {
            this.requests++;
            if (!body) return void this.absent.add(key);
            this.bytes += body.byteLength;
            const { data, width } = await imagePixels(new Uint8Array(body));
            if (width !== CELLS) throw new Error(`Canopy tile ${key} has ${width} cells a side.`);
            const codes = new Uint16Array(CELLS * CELLS);
            for (let i = 0; i < codes.length; i++) codes[i] = data[4 * i] | (data[4 * i + 1] << 8);
            this.arrived.push({ x, y, codes });
        }).catch(error => {
            // A tile that fails stays out, as an absent one.
            this.failed++;
            this.absent.add(key);
            console.warn(`Canopy tile ${key}: ${error instanceof Error ? error.message : error}`);
        }).finally(() => {
            this.loading.delete(key);
            this.wake();
        });
    }

    /** A free layer, or the one that the frames used longest ago; none while this frame uses them all. */
    private allocate(key: string, x: number, y: number): Slot | undefined {
        let slot = this.slots.size < SLOTS ? this.slots.size : -1;
        if (slot < 0) {
            let oldest: Slot | undefined;
            for (const s of this.slots.values()) if (s.used < this.frame && (!oldest || s.used < oldest.used)) oldest = s;
            if (!oldest) return undefined;
            this.slots.delete(oldest.key);
            slot = oldest.slot;
        }
        const entry = { key, x, y, slot, used: this.frame };
        this.slots.set(key, entry);
        return entry;
    }
}

/**
 * A tile's codes and the levels above them, coarser by halves: a cell of a level holds the most stems
 * and cover and the lowest canopy of the forest cells under it, so its density bounds theirs.
 */
export function densest(codes: Uint16Array): Uint16Array<ArrayBuffer>[] {
    const levels: Uint16Array<ArrayBuffer>[] = [Uint16Array.from(codes)];
    const forest = (code: number) => code !== 0 && code !== 0xffff;
    for (let side = CELLS >> 1; side >= 1; side >>= 1) {
        const below = levels[levels.length - 1], out = new Uint16Array(side * side);
        for (let j = 0; j < side; j++) for (let i = 0; i < side; i++) {
            let stems = 0, cover = 0, height = 127, any = false;
            for (const [di, dj] of [[0, 0], [1, 0], [0, 1], [1, 1]]) {
                const code = below[(2 * j + dj) * 2 * side + 2 * i + di];
                if (!forest(code)) continue;
                any = true;
                height = Math.min(height, code & 127);
                cover = Math.max(cover, (code >> 7) & 15);
                stems = Math.max(stems, (code >> 11) & 7);
            }
            out[j * side + i] = any ? height | (cover << 7) | (stems << 11) : 0;
        }
        levels.push(out);
    }
    return levels;
}
