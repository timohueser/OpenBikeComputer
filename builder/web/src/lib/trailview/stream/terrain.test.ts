import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Source } from '../../planner/layers/archive';
import { featureTiles, releaseTiles } from './terrain';

const gets = vi.hoisted(() => ({ count: 0 }));
vi.mock('../../planner/layers/archive', async original => ({
    ...(await original<typeof import('../../planner/layers/archive')>()),
    openSource: async () => ({ get: async () => (gets.count++, new Uint8Array([1, 2, 3])) }) as unknown as Source,
}));

const RELEASE = 'https://maps.test/releases/abc';
const config = { terrain: `${RELEASE}/terrain/{z}/{x}/{y}.webp`, manifest: `${RELEASE}/release.json` } as Parameters<typeof releaseTiles>[0];

describe('the stored terrain of a release', () => {
    afterEach(() => vi.unstubAllGlobals());

    it('reads zooms up to 12 from the tile service, and zoom 13 from the pointer of its zoom 11 pack', async () => {
        const fetched: string[] = [];
        vi.stubGlobal('fetch', async (url: URL | string) => {
            fetched.push(String(url));
            return String(url).endsWith('.webp') ? new Response(new Uint8Array([1, 2, 3])) : new Response(null, { status: 404 });
        });
        const tiles = releaseTiles(config);
        expect(new Uint8Array((await tiles(12, 2143, 1444))!)).toEqual(new Uint8Array([1, 2, 3]));
        // An area without zoom 13 has no pointer: its tiles are absent, and one request answers all of them.
        expect(await tiles(13, 4287, 2889)).toBeUndefined();
        expect(await tiles(13, 4286, 2888)).toBeUndefined();
        expect(fetched).toEqual([`${RELEASE}/terrain/12/2143/1444.webp`, `${RELEASE}/public/maps/tiles/terrain/13/11-1071-722.pmtiles.json`]);
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
