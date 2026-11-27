import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Source } from '../../planner/layers/archive';
import { FORMAT } from './features';
import { zxyToTileId } from 'pmtiles';
import { featureTiles, fineTerrain, releaseTiles } from './terrain';

const gets = vi.hoisted(() => ({ count: 0 }));
vi.mock('../../planner/layers/archive', async original => ({
    ...(await original<typeof import('../../planner/layers/archive')>()),
    openSource: async () => ({ metadata: { features_format: FORMAT }, get: async () => (gets.count++, new Uint8Array([1, 2, 3])) }) as unknown as Source,
}));

const RELEASE = 'https://maps.test/releases/abc';
const config = { terrain: `${RELEASE}/terrain/{z}/{x}/{y}.webp`, manifest: `${RELEASE}/release.json` } as Parameters<typeof releaseTiles>[0];

/**
 * A PMTiles archive of version 3 with no compression: a root directory that names one leaf directory for each list of
 * tile IDs, and a tile of one byte for each ID. `leaves` are the offsets where the leaf directories start and end.
 */
function archive(leafIds: number[][]): { bytes: Uint8Array; leaves: number[] } {
    const varint = (value: number) => { const out = []; for (; value > 127; value = Math.floor(value / 128)) out.push((value % 128) | 128); out.push(value); return out; };
    // A directory: the count of its entries, then the step of each tile ID, each run length, each length and each offset plus one.
    const directory = (entries: { id: number; run: number; length: number; offset: number }[]) => [varint(entries.length), ...entries.map((entry, i) => varint(entry.id - (i ? entries[i - 1].id : 0))),
        ...entries.map(entry => varint(entry.run)), ...entries.map(entry => varint(entry.length)), ...entries.map(entry => varint(entry.offset + 1))].flat();
    let tiles = 0;
    const leaves = leafIds.map(ids => directory(ids.map(id => ({ id, run: 1, length: 1, offset: tiles++ }))));
    const starts = leaves.map((_, i) => leaves.slice(0, i).reduce((sum, leaf) => sum + leaf.length, 0));
    const root = directory(leafIds.map((ids, i) => ({ id: ids[0], run: 0, length: leaves[i].length, offset: starts[i] })));
    const leafStart = 127 + root.length, tileStart = leafStart + leaves.flat().length;
    const bytes = new Uint8Array(tileStart + tiles), view = new DataView(bytes.buffer);
    bytes.set([...new TextEncoder().encode('PMTiles'), 3]);
    // The offset and the length of the root directory, the metadata, the leaf directories and the tiles, as 64 bit numbers.
    [127, root.length, 0, 0, leafStart, tileStart - leafStart, tileStart, tiles].forEach((value, i) => view.setUint32(8 + 8 * i, value, true));
    bytes.set([1, 1], 97);
    bytes[101] = 13;
    bytes.set([...root, ...leaves.flat()], 127);
    return { bytes, leaves: [...starts, tileStart - leafStart].map(start => leafStart + start) };
}

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

    it('reads zoom 13 of grid format 4 from the deepest pack of the block index of its zoom 9 square', async () => {
        const fetched: string[] = [];
        const index = { '12-2143-1444.pmtiles': { sha256: 'c'.repeat(64), encoding: 'identity' }, '9-267-180.pmtiles': { sha256: 'd'.repeat(64), encoding: 'identity' } };
        vi.stubGlobal('fetch', async (url: URL | string) => {
            fetched.push(String(url));
            if (String(url).endsWith('/grid.json')) return new Response('{"format":4}');
            return String(url).endsWith('/7-66-45.json') ? new Response(JSON.stringify(index)) : new Response(null, { status: 404 });
        });
        const tiles = releaseTiles(config);
        // The object store of this stub has no packs, so each read of a pack fails after its first request.
        await expect(tiles(13, 4287, 2889)).rejects.toThrow();
        await expect(tiles(13, 4280, 2880)).rejects.toThrow();
        expect(await tiles(13, 4224, 2880)).toBeUndefined();
        expect(fetched).toEqual([`${RELEASE}/public/grid.json`, `${RELEASE}/public/maps/tiles/terrain/13/7-66-45.json`,
            `https://maps.test/objects/${'c'.repeat(64)}`, `https://maps.test/objects/${'d'.repeat(64)}`]);
    });

    it('answers whether a national model holds the ground at a coordinate from the directories of the pack, with no tile', async () => {
        // The pack holds tile 13/4283/2882, around Pilatus, and not the tile to its east.
        const pack = archive([[1, 2], [zxyToTileId(13, 4283, 2882)]]), fetched: string[] = [];
        vi.stubGlobal('fetch', async (url: URL | string, init?: RequestInit) => {
            const range = /bytes=(\d+)-(\d+)/.exec(new Headers(init?.headers).get('range') ?? '');
            fetched.push(String(url).slice(String(url).lastIndexOf('/') + 1, String(url).lastIndexOf('/') + 14) + (range ? ` ${range[1]}-${range[2]}` : ''));
            if (range) return new Response(pack.bytes.slice(+range[1], +range[2] + 1), { status: 206 });
            if (String(url).endsWith('/grid.json')) return new Response('{"format":4}');
            return String(url).endsWith('/7-66-45.json') ? new Response(JSON.stringify({ '9-267-180.pmtiles': { sha256: 'd'.repeat(64), encoding: 'identity' } })) : new Response(null, { status: 404 });
        });
        const fine = fineTerrain(config);
        expect([await fine([8.255, 46.98]), await fine([8.3, 46.98]), await fine([5.2, 46.98])]).toEqual([true, false, false]);
        // One read of the header with the root directory and one of the second leaf directory: no tile.
        expect(fetched).toEqual(['grid.json', '7-66-45.json', 'ddddddddddddd 0-16383', `ddddddddddddd ${pack.leaves[1]}-${pack.leaves[2] - 1}`, '7-65-45.json']);
        expect(await fineTerrain({ ...config, manifest: undefined })([5.2, 46.98])).toBe(true);
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
