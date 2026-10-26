import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Source } from '../../planner/layers/archive';
import { FORMAT } from './features';
import { featureTiles, releaseTiles } from './terrain';

const gets = vi.hoisted(() => ({ count: 0 }));
vi.mock('../../planner/layers/archive', async original => ({
    ...(await original<typeof import('../../planner/layers/archive')>()),
    openSource: async () => ({ metadata: { features_format: FORMAT }, get: async () => (gets.count++, new Uint8Array([1, 2, 3])) }) as unknown as Source,
}));

const RELEASE = 'https://maps.test/releases/abc';
const config = { terrain: `${RELEASE}/terrain/{z}/{x}/{y}.webp`, manifest: `${RELEASE}/release.json` } as Parameters<typeof releaseTiles>[0];

describe('the stored terrain of a release', () => {
    afterEach(() => vi.unstubAllGlobals());

    it('reads zooms up to 12 from the tile service, and zoom 13 through the block index of its zoom 11 pack', async () => {
        const fetched: string[] = [];
        vi.stubGlobal('fetch', async (url: URL | string) => {
            fetched.push(String(url));
            if (String(url).endsWith('.webp')) return new Response(new Uint8Array([1, 2, 3]));
            if (String(url).endsWith('/grid.json')) return new Response('{"format":3,"map_zoom":11}');
            return String(url).endsWith('/7-66-45.json') ? new Response('{}') : new Response(null, { status: 404 });
        });
        const tiles = releaseTiles(config);
        expect(new Uint8Array((await tiles(12, 2143, 1444))!)).toEqual(new Uint8Array([1, 2, 3]));
        // A block without zoom 13 has no index, and a block index without the pack has none of it: the
        // tiles are absent, and one request answers all of a block.
        expect(await tiles(13, 4287, 2889)).toBeUndefined();
        expect(await tiles(13, 4224, 2880)).toBeUndefined();
        expect(await tiles(13, 4160, 2880)).toBeUndefined();
        expect(fetched).toEqual([`${RELEASE}/terrain/12/2143/1444.webp`, `${RELEASE}/public/grid.json`,
            `${RELEASE}/public/maps/tiles/terrain/13/7-66-45.json`, `${RELEASE}/public/maps/tiles/terrain/13/7-65-45.json`]);
    });

    it('reads the pointer of a pack of a release of grid format 2, such as a rollback', async () => {
        const fetched: string[] = [];
        vi.stubGlobal('fetch', async (url: URL | string) => {
            fetched.push(String(url));
            return String(url).endsWith('/grid.json') ? new Response('{"format":2,"map_zoom":11}') : new Response(null, { status: 404 });
        });
        expect(await releaseTiles(config)(13, 4287, 2889)).toBeUndefined();
        expect(fetched).toEqual([`${RELEASE}/public/grid.json`, `${RELEASE}/public/maps/tiles/terrain/13/11-1071-722.pmtiles.json`]);
    });

    it('ends a request when its stop aborts', async () => {
        vi.stubGlobal('fetch', (_: string, { signal }: RequestInit) => new Promise((_, reject) => signal!.addEventListener('abort', () => reject(signal!.reason))));
        const stop = new AbortController(), tile = releaseTiles(config, stop.signal)(12, 2143, 1444);
        stop.abort();
        await expect(tile).rejects.toThrow('aborted');
    });
});

describe('the feature tiles of a release', () => {
    it('ask once for a tile that three readers read, and give each a buffer of its own', async () => {
        const tiles = featureTiles({ layers: { 'trailview-features': 'https://maps.test/features.pmtiles' } } as unknown as Parameters<typeof featureTiles>[0])!;
        const [a, b, c] = await Promise.all([tiles(14, 1, 2), tiles(14, 1, 2), tiles(14, 1, 2)]);
        expect(gets.count).toBe(1);
        structuredClone(a, { transfer: [a!] });
        expect(a!.byteLength).toBe(0);
        expect([new Uint8Array(b!), new Uint8Array(c!)]).toEqual([new Uint8Array([1, 2, 3]), new Uint8Array([1, 2, 3])]);
    });
});
