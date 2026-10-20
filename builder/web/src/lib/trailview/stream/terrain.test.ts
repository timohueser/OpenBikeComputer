import { afterEach, describe, expect, it, vi } from 'vitest';
import { releaseTiles } from './terrain';

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
