import { PMTiles } from 'pmtiles';
import type { PlannerConfig } from '../../planner/config';
import { decodedTiles, fetchTile, openSource, requestSignal, type Source } from '../../planner/layers/archive';
import { worldPixel } from '../../planner/layers/mercator';
import { DEEPEST, FORMAT } from './features';
import type { PageData } from './heights';

/** The deepest stored zoom: the planner's zooms 0 to 12, and zoom 13 where a national model holds the ground. */
export const TILE_ZOOM = 13;
const WORKERS = 2;
/** Feature tile bodies kept for the terrain, the houses and the orchards, which read the same tiles. */
const FEATURE_BODIES = 32 * 2 ** 20;

/** Where the stored terrain comes from: the body of tile z/x/y (undefined where it is absent) and its pages. */
export interface TerrainSource {
    fetch(z: number, x: number, y: number): Promise<ArrayBuffer | undefined>;
    decode(body: ArrayBuffer): Promise<(PageData | null)[]>;
}

/** Whether a load ended because its engine stopped: `destroy()` aborts without a reason, a timeout aborts with one. */
export const stopped = (error: unknown) => error instanceof DOMException && error.name === 'AbortError';

/**
 * The stored tiles of a release. The tile service serves zooms 0 to 12. Zoom 13 is in a PMTiles pack
 * per zoom 11 tile (specs/planner-release.md), read with range requests through its pointer beside the
 * manifest: in the block index of grid format 3, else a file of its own. A config without a manifest
 * reads zoom 13 from the tile template too. An abort of `stop` ends every request.
 */
export function releaseTiles(config: PlannerConfig, stop = new AbortController().signal): TerrainSource['fetch'] {
    const signal = () => AbortSignal.any([requestSignal(), stop]);
    type Pointer = { sha256: string; encoding: string };
    const files = new Map<string, Promise<unknown>>();
    // A public file of the release, undefined where it is absent: a plain fetch, as an area without zoom
    // 13 has no pointer, and that 404 is no reason to read the catalogue again.
    const file = (manifest: string, path: string): Promise<unknown> => {
        let entry = files.get(path);
        if (!entry) {
            entry = fetch(new URL(`public/${path}`, manifest), { signal: signal() }).then(async response => {
                if (response.status === 404) return undefined;
                if (!response.ok) throw new Error(`The public file ${path} answered ${response.status}.`);
                return response.json();
            });
            files.set(path, entry);
            entry.catch(() => files.delete(path));
        }
        return entry;
    };
    const packs = new Map<string, Promise<PMTiles | undefined>>();
    const pack = (manifest: string, x: number, y: number) => {
        const name = `11-${x >> 2}-${y >> 2}.pmtiles`;
        let entry = packs.get(name);
        if (!entry) {
            entry = file(manifest, 'grid.json').then(async grid => {
                let pointer: Pointer | undefined;
                if (((grid as { format?: number } | undefined)?.format ?? 0) >= 3) {
                    const index = (await file(manifest, `maps/tiles/terrain/13/7-${x >> 6}-${y >> 6}.json`)) as Record<string, Pointer> | undefined;
                    pointer = index && Object.hasOwn(index, name) ? index[name] : undefined;
                } else {
                    pointer = (await file(manifest, `maps/tiles/terrain/13/${name}.json`)) as Pointer | undefined;
                }
                if (!pointer) return undefined;
                if (pointer.encoding !== 'identity') throw new Error(`Terrain pack ${name} is not stored for range requests.`);
                return new PMTiles(new URL(`../../objects/${pointer.sha256}`, manifest).href);
            });
            packs.set(name, entry);
            entry.catch(() => packs.delete(name));
        }
        return entry;
    };
    return async (z, x, y) => {
        if (z === TILE_ZOOM && config.manifest) return (await (await pack(config.manifest, x, y))?.getZxy(z, x, y, signal()))?.data;
        return fetchTile(config.terrain, z, x, y, signal());
    };
}

/** The stored terrain and the feature tiles of a release. */
export interface EyeTiles {
    terrain: TerrainSource['fetch'];
    features: ReturnType<typeof featureTiles>;
    /** Settles when both tiles under the eye have arrived or failed. */
    arrived: Promise<unknown>;
}

/**
 * The tiles of a release, with the deepest terrain tile and the feature tile under the eye started at
 * once: they load while the engine opens the ground and compiles its pipelines. The first fetch of the
 * terrain tile takes its body. An abort of `stop` ends every request.
 */
export function eyeTiles(config: PlannerConfig, lonLat: [number, number], stop: AbortSignal): EyeTiles {
    const fetch = releaseTiles(config, stop), features = featureTiles(config, stop), at = (z: number) => worldPixel(lonLat, 2 ** z).map(Math.floor);
    const [ex, ey] = at(TILE_ZOOM), [fx, fy] = at(DEEPEST);
    let early: Promise<ArrayBuffer | undefined> | undefined = fetch(TILE_ZOOM, ex, ey);
    // The features keep their body; a view that never asks for the terrain tile leaves its failure unseen.
    const arrived = Promise.allSettled([early, features?.(DEEPEST, fx, fy)]);
    const terrain: TerrainSource['fetch'] = (z, x, y) => {
        const body = z === TILE_ZOOM && x === ex && y === ey ? early : undefined;
        if (body) early = undefined;
        return body ?? fetch(z, x, y);
    };
    return { terrain, features, arrived };
}

/** The pages of tile bodies, decoded in workers until `stop` aborts. */
export function pageDecoder(stop: AbortSignal): TerrainSource['decode'] {
    const workers = Array.from({ length: WORKERS }, () => new Worker(new URL('./decode-worker.ts', import.meta.url), { type: 'module' }));
    stop.addEventListener('abort', () => workers.forEach(worker => worker.terminate()));
    const waiting = new Map<number, { resolve: (pages: (PageData | null)[]) => void; reject: (error: Error) => void }>();
    for (const worker of workers) {
        worker.onmessage = ({ data }) => {
            const wait = waiting.get(data.id);
            waiting.delete(data.id);
            if (data.error) wait?.reject(new Error(data.error));
            else wait?.resolve(data.pages);
        };
    }
    let next = 0;
    return body => {
        const id = next++;
        return new Promise((resolve, reject) => {
            waiting.set(id, { resolve, reject });
            workers[id % WORKERS].postMessage({ id, body });
        });
    };
}

/**
 * The feature tiles of a release: a copy of the body of tile z/x/y, undefined where it is absent.
 * Undefined when the release has no feature layer. An abort of `stop` ends every request.
 */
export function featureTiles(config: PlannerConfig, stop = new AbortController().signal): ((z: number, x: number, y: number) => Promise<ArrayBuffer | undefined>) | undefined {
    const url = (config.layers as Record<string, string | undefined>)['trailview-features'];
    if (!url) return undefined;
    let source: Promise<Source | undefined> | undefined;
    const open = async () => {
        const opened = await openSource(url);
        if (opened.metadata.features_format === FORMAT) return opened;
        // Tiles of another format lie at another scale and zoom: drawn, they would be in the wrong place.
        console.warn(`The TrailView feature archive has features_format ${opened.metadata.features_format}, not ${FORMAT}. TrailView shows no features.`);
        return undefined;
    };
    const tiles = decodedTiles(async (z, x, y, signal) => {
        source ??= open();
        source.catch(() => (source = undefined));
        return (await source)?.get(z, x, y, AbortSignal.any([signal, stop]));
    }, body => body.buffer, FEATURE_BODIES, body => body.byteLength);
    return async (z, x, y) => (await tiles.tile(z, x, y))?.slice(0);
}
