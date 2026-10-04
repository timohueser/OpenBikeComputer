import { openTileArchive, type TileArchive } from './tile-archive';
import type { Coordinate } from '../map-types';
import { DEM_ZOOM, INDEX_ZOOM, TILE, UNKNOWN_HEIGHT, nativePoint, horizonVisibility, type SunPosition, type SunMeta, type SunStats, type Surface } from './sun';

class Missing extends Error { constructor(readonly key: string) { super(key); } }

/** One worker's shared LRU, capped at 48 MiB of decoded terrain and horizons. */
export class SunSurface implements Surface {
    private tiles = new Map<number, Int16Array | Uint8Array | null>();
    private recent: { key: number; tile: Int16Array | Uint8Array | null }[] = [];
    private pending = new Map<string, Promise<void>>();
    readonly stats: SunStats = { cpuMs: 0, requests: 0, bytes: 0, decodedBytes: 0, rays: 0, nodes: 0, leaves: 0, cacheHits: 0 };
    private constructor(readonly archive: TileArchive, readonly meta: SunMeta, private dem: string) {}

    static async open(url: string, dem: string) {
        const archive = await openTileArchive(url, 'Sunlight');
        const meta = archive.metadata as unknown as SunMeta;
        if (meta.sun_format !== 3 || meta.dem_zoom !== DEM_ZOOM || meta.index_zoom !== INDEX_ZOOM || meta.horizon_zoom !== DEM_ZOOM || ![32, 64].includes(meta.horizon_samples) || !(meta.horizon_directions > 0 && meta.horizon_directions % 3 === 0) || !(meta.horizon_step > 0) || !(meta.distance_m > 0 && meta.distance_m <= 30000) || !meta.timezone) throw new Error('Unsupported sunlight index');
        return new SunSurface(archive, meta, dem);
    }

    private read(level: number, z: number, x: number, y: number, size: number) {
        const tx = Math.floor(x / size), ty = Math.floor(y / size);
        if (tx < 0 || ty < 0 || tx >= 2 ** z || ty >= 2 ** z) return { tile: null, pixel: 0 };
        const key = (level >= 13 ? z + 13 : z) * 2 ** 40 + tx * 2 ** 20 + ty;
        const tile = this.recent[level]?.key === key ? this.recent[level].tile : this.tiles.get(key);
        if (tile === undefined) throw new Missing(`${level >= 13 ? 'h/' : ''}${z}/${tx}/${ty}`);
        if (this.recent[level]?.key !== key) {
            this.recent[level] = { key, tile };
            this.tiles.delete(key); this.tiles.set(key, tile);
        }
        return { tile, pixel: (y - ty * size) * size + x - tx * size };
    }

    height(level: number, x: number, y: number): number {
        const { tile, pixel } = this.read(level, DEM_ZOOM - level, x, y, TILE);
        const value = tile?.[pixel];
        return value === undefined || value === UNKNOWN_HEIGHT ? Infinity : value;
    }

    sunlight(coordinate: Coordinate, sun: SunPosition, zoom = DEM_ZOOM): number {
        const size = this.meta.horizon_samples, directions = this.meta.horizon_directions;
        const [px, py] = nativePoint(coordinate);
        return horizonVisibility((x, y, direction) => {
            const { tile, pixel } = this.read(zoom + 13, zoom, x, y, size);
            return tile?.[pixel * directions + direction] ?? 255;
        }, (px + .5) * size / TILE / 2 ** (DEM_ZOOM - zoom) - .5, (py + .5) * size / TILE / 2 ** (DEM_ZOOM - zoom) - .5, sun, directions, this.meta.horizon_step);
    }

    private load(key: string, signal: AbortSignal): Promise<void> {
        const pending = this.pending.get(key);
        if (pending) return pending;
        const request = (async () => {
            const horizon = key.startsWith('h/');
            const [z, x, y] = (horizon ? key.slice(2) : key).split('/').map(Number);
            this.stats.requests++;
            let body: ArrayBuffer | undefined;
            if (!horizon && z === DEM_ZOOM) {
                const response = await fetch(this.dem.replace('{z}', String(z)).replace('{x}', String(x)).replace('{y}', String(y)), { signal });
                if (response.status !== 204 && !response.ok) throw new Error(`Terrain answered ${response.status}`);
                if (response.status !== 204) body = await response.arrayBuffer();
            } else body = await this.archive.get(z, x, y, signal);
            this.stats.bytes += body?.byteLength ?? 0;
            let tile: Int16Array | Uint8Array | null = null;
            if (body) {
                const bitmap = await createImageBitmap(new Blob([body]), { colorSpaceConversion: 'none', premultiplyAlpha: 'none' });
                try {
                    const size = this.meta.horizon_samples;
                    const combined = z >= 8 && z <= 10;
                    const width = combined || !horizon ? TILE : size;
                    const height = combined ? TILE + size * size * this.meta.horizon_directions / (3 * TILE) : horizon ? size * this.meta.horizon_directions / 3 : TILE;
                    if (bitmap.width !== width || bitmap.height !== height) throw new Error('Invalid sunlight terrain tile');
                    const context = new OffscreenCanvas(width, height).getContext('2d')!;
                    context.drawImage(bitmap, 0, 0);
                    const rgba = context.getImageData(0, 0, width, height).data;
                    if (horizon) {
                        const cells = size * size, directions = this.meta.horizon_directions;
                        const offset = combined ? TILE * TILE : 0;
                        tile = new Uint8Array(cells * directions);
                        for (let i = 0; i < tile.length; i++) tile[i] = rgba[4 * (offset + Math.floor(i / directions) + Math.floor(i % directions / 3) * cells) + i % directions % 3];
                        for (let i = 0; i < tile.length; i++) if (i % directions) tile[i] = (tile[i - 1] + tile[i]) & 255;
                    } else {
                        tile = new Int16Array(TILE * TILE);
                        for (let i = 0; i < tile.length; i++) tile[i] = rgba[4 * i + 3] === 255 ? rgba[4 * i] * 256 + rgba[4 * i + 1] - 32768 : UNKNOWN_HEIGHT;
                    }
                } finally { bitmap.close(); }
            }
            signal.throwIfAborted();
            const numeric = (horizon ? z + 13 : z) * 2 ** 40 + x * 2 ** 20 + y;
            this.tiles.set(numeric, tile);
            this.stats.decodedBytes += tile?.byteLength ?? 0;
            while (this.stats.decodedBytes > 48 * 1024 ** 2 || this.tiles.size > 256) {
                const old = this.tiles.keys().next().value!;
                this.stats.decodedBytes -= this.tiles.get(old)?.byteLength ?? 0;
                this.tiles.delete(old);
                this.recent = this.recent.map(item => item?.key === old ? undefined! : item);
            }
        })().finally(() => this.pending.delete(key));
        this.pending.set(key, request);
        return request;
    }

    /** Suspend only on a missing tile; the ray runs synchronously once its touched tiles are cached. */
    async run<T>(compute: () => T, signal: AbortSignal): Promise<T> {
        while (true) {
            signal.throwIfAborted();
            const start = performance.now();
            try { const result = compute(); this.stats.cpuMs += performance.now() - start; return result; }
            catch (error) {
                this.stats.cpuMs += performance.now() - start;
                if (!(error instanceof Missing)) throw error;
                await this.load(error.key, signal);
            }
        }
    }
}
