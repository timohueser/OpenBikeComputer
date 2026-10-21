import { PMTiles } from 'pmtiles';
import type { PlannerConfig } from '../../planner/config';
import { fetchTile, openSource, requestSignal, type Source } from '../../planner/layers/archive';
import type { PageData } from './heights';

/** The deepest stored zoom: the planner's zooms 0 to 12, and zoom 13 where a national model holds the ground. */
export const TILE_ZOOM = 13;
const WORKERS = 2;

/** Where the stored terrain comes from: the body of tile z/x/y (undefined where it is absent) and its pages. */
export interface TerrainSource {
    fetch(z: number, x: number, y: number): Promise<ArrayBuffer | undefined>;
    decode(body: ArrayBuffer): Promise<(PageData | null)[]>;
}

/**
 * The stored tiles of a release. The tile service serves zooms 0 to 12. Zoom 13 is in a PMTiles pack
 * per zoom 11 tile (specs/planner-release.md), read with range requests through its public pointer
 * beside the manifest. A config without a manifest reads zoom 13 from the tile template too.
 */
export function releaseTiles(config: PlannerConfig): TerrainSource['fetch'] {
    const packs = new Map<string, Promise<PMTiles | undefined>>();
    const pack = (manifest: string, x: number, y: number) => {
        const name = `11-${x >> 2}-${y >> 2}`;
        let entry = packs.get(name);
        if (!entry) {
            // A plain fetch: an area without zoom 13 has no pointer, and that 404 is no reason to read the catalogue again.
            entry = fetch(new URL(`public/maps/tiles/terrain/13/${name}.pmtiles.json`, manifest), { signal: requestSignal() }).then(async response => {
                if (response.status === 404) return undefined;
                if (!response.ok) throw new Error(`The pointer of terrain pack ${name} answered ${response.status}.`);
                const pointer = await response.json();
                if (pointer.encoding !== 'identity') throw new Error(`Terrain pack ${name} is not stored for range requests.`);
                return new PMTiles(new URL(`../../objects/${pointer.sha256}`, manifest).href);
            });
            packs.set(name, entry);
            entry.catch(() => packs.delete(name));
        }
        return entry;
    };
    return async (z, x, y) => {
        if (z === TILE_ZOOM && config.manifest) return (await (await pack(config.manifest, x, y))?.getZxy(z, x, y, requestSignal()))?.data;
        return fetchTile(config.terrain, z, x, y, requestSignal());
    };
}

/** The pages of tile bodies, decoded in workers. */
export function pageDecoder(): TerrainSource['decode'] {
    const workers = Array.from({ length: WORKERS }, () => new Worker(new URL('./decode-worker.ts', import.meta.url), { type: 'module' }));
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
 * The feature tiles of a release: the body of tile z/x/y, undefined where it is absent. Undefined
 * when the release has no feature layer.
 */
export function featureTiles(config: PlannerConfig): ((z: number, x: number, y: number) => Promise<ArrayBuffer | undefined>) | undefined {
    const url = (config.layers as Record<string, string | undefined>)['trailview-features'];
    if (!url) return undefined;
    let source: Promise<Source> | undefined;
    return async (z, x, y) => {
        source ??= openSource(url);
        source.catch(() => (source = undefined));
        return (await source).get(z, x, y, requestSignal());
    };
}
