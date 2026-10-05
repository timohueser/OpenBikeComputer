import { archiveOf, fetchTile, openSource, type TileArchive, type TileGetter, type Tiles } from './archive';
import type { Coordinate } from '../map-types';
import { DEM_ZOOM, INDEX_ZOOM, TILE, nativePoint, horizonVisibility, type SunPosition, type SunMeta, type SunStats, type Surface } from './sun';
import { heights, imagePixels, terrainHeights, UNKNOWN_HEIGHT } from './terrain';

/** Height bounds at zooms 0 to 10 and horizon profiles at zooms 8 to 12. */
interface SunTile { bounds: Int16Array | null; horizons: Uint8Array | null }

class Missing extends Error { constructor(readonly load: (signal: AbortSignal) => Promise<unknown>) { super('Missing tile'); } }

function sunMeta(json: Record<string, unknown>): SunMeta {
    const meta = json as unknown as SunMeta;
    if (meta.sun_format !== 3 || meta.dem_zoom !== DEM_ZOOM || meta.index_zoom !== INDEX_ZOOM || meta.horizon_zoom !== DEM_ZOOM || ![32, 64].includes(meta.horizon_samples) || !(meta.horizon_directions > 0 && meta.horizon_directions % 3 === 0) || !(meta.horizon_step > 0) || !(meta.distance_m > 0 && meta.distance_m <= 30000) || !meta.timezone) throw new Error('Unsupported sunlight index');
    return meta;
}

async function decodeSun(body: Uint8Array<ArrayBuffer>, meta: SunMeta, z: number): Promise<SunTile> {
    const size = meta.horizon_samples, directions = meta.horizon_directions;
    const bounded = z <= INDEX_ZOOM, profiled = z >= 8, width = bounded ? TILE : size;
    const rgba = await imagePixels(body);
    if (rgba.width !== width || rgba.height !== (bounded ? TILE : 0) + (profiled ? size * size * directions / (3 * width) : 0)) throw new Error('Invalid sunlight terrain tile');
    let horizons: Uint8Array | null = null;
    if (profiled) {
        const cells = size * size, offset = bounded ? TILE * TILE : 0;
        horizons = new Uint8Array(cells * directions);
        for (let i = 0; i < horizons.length; i++) horizons[i] = rgba.data[4 * (offset + Math.floor(i / directions) + Math.floor(i % directions / 3) * cells) + i % directions % 3];
        for (let i = 0; i < horizons.length; i++) if (i % directions) horizons[i] = (horizons[i - 1] + horizons[i]) & 255;
    }
    return { bounds: bounded ? heights(rgba.data, TILE * TILE) : null, horizons };
}

/** One worker's sun archive and terrain heights, each in a bounded cache. */
export class SunSurface implements Surface {
    /** The last tile read at each level, so a ray reads its tiles without a cache lookup. */
    private recent: ({ key: number; tile: Int16Array | Uint8Array | null } | undefined)[] = [];
    private constructor(private archive: TileArchive<SunMeta, SunTile>, private terrain: Tiles<Int16Array>, private counts: SunStats) {}

    get meta() { return this.archive.meta; }
    get stats() {
        this.counts.decodedBytes = this.archive.held + this.terrain.held;
        return this.counts;
    }

    static async open(url: string, dem: string) {
        const counts: SunStats = { cpuMs: 0, requests: 0, bytes: 0, decodedBytes: 0, rays: 0, nodes: 0, leaves: 0, cacheHits: 0 };
        const counted = (get: TileGetter): TileGetter => async (z, x, y, signal) => {
            counts.requests++;
            const body = await get(z, x, y, signal);
            counts.bytes += body?.byteLength ?? 0;
            return body;
        };
        const source = await openSource(url);
        const archive = archiveOf({ ...source, get: counted(source.get) }, {
            meta: sunMeta, decode: (body, meta, z) => decodeSun(body, meta, z),
            bytes: tile => (tile.bounds?.byteLength ?? 0) + (tile.horizons?.byteLength ?? 0), budgetBytes: 32 * 2 ** 20,
        });
        return new SunSurface(archive, terrainHeights(counted((z, x, y, signal) => fetchTile(dem, z, x, y, signal))), counts);
    }

    /** Level 0 is the terrain, level 2 to 12 the height bounds of zoom 12 − level, and level 13 + zoom the horizons of a zoom. */
    private read(level: number, z: number, x: number, y: number, size: number) {
        const tx = Math.floor(x / size), ty = Math.floor(y / size);
        if (tx < 0 || ty < 0 || tx >= 2 ** z || ty >= 2 ** z) return { tile: null, pixel: 0 };
        const key = tx * 2 ** 20 + ty, pixel = (y - ty * size) * size + x - tx * size;
        if (this.recent[level]?.key === key) return { tile: this.recent[level].tile, pixel };
        let tile: Int16Array | Uint8Array | null | undefined;
        if (level === 0) tile = this.terrain.loaded(z, tx, ty);
        else {
            const found = this.archive.loaded(z, tx, ty);
            tile = found && (level > DEM_ZOOM ? found.horizons : found.bounds);
        }
        if (tile === undefined) throw new Missing(signal => (level === 0 ? this.terrain : this.archive).tile(z, tx, ty, signal));
        this.recent[level] = { key, tile };
        return { tile, pixel };
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

    /** Suspend only on a missing tile; the ray runs synchronously once its touched tiles are cached. */
    async run<T>(compute: () => T, signal: AbortSignal): Promise<T> {
        while (true) {
            signal.throwIfAborted();
            const start = performance.now();
            try { const result = compute(); this.counts.cpuMs += performance.now() - start; return result; }
            catch (error) {
                this.counts.cpuMs += performance.now() - start;
                if (!(error instanceof Missing)) throw error;
                await error.load(signal);
            }
        }
    }
}
