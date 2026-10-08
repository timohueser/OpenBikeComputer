import { describe, expect, it } from 'vitest';
import { decodedTiles } from './archive';

/** Tiles of `size` bytes that count their requests; tile 9/9/9 is absent and tile 6/6/6 fails once. */
function source(size: number) {
    const requests: string[] = [];
    let failed = false;
    const tiles = decodedTiles(async (z, x, y) => {
        requests.push(`${z}/${x}/${y}`);
        if (z === 6 && !failed) { failed = true; throw new Error('503'); }
        return z === 9 ? undefined : new ArrayBuffer(size);
    }, body => body, 3 * size, tile => tile.byteLength);
    return { tiles, requests };
}

describe('decoded tiles', () => {
    it('requests each tile once while it stays within the byte budget, least recently used out first', async () => {
        const { tiles, requests } = source(100);
        await Promise.all([tiles.tile(1, 0, 0), tiles.tile(1, 0, 0), tiles.tile(1, 1, 0), tiles.tile(1, 2, 0)]);
        expect(tiles.loaded(1, 0, 0)?.byteLength).toBe(100);
        await tiles.tile(1, 3, 0);
        expect([tiles.loaded(1, 0, 0) !== undefined, tiles.loaded(1, 1, 0), tiles.held]).toEqual([true, undefined, 300]);
        expect(await tiles.tile(9, 9, 9)).toBeNull();
        expect(tiles.loaded(9, 9, 9)).toBeNull();
        await expect(tiles.tile(6, 6, 6)).rejects.toThrow('503');
        expect(tiles.loaded(6, 6, 6)).toBeUndefined();
        await tiles.tile(6, 6, 6);
        expect(requests).toEqual(['1/0/0', '1/1/0', '1/2/0', '1/3/0', '9/9/9', '6/6/6', '6/6/6']);
    });

    it('stops only the wait of an aborted caller; the tile still arrives for the next one', async () => {
        const { tiles, requests } = source(10);
        const abort = new AbortController();
        const waiting = tiles.tile(2, 0, 0, abort.signal);
        abort.abort();
        await expect(waiting).rejects.toThrow();
        expect((await tiles.tile(2, 0, 0))?.byteLength).toBe(10);
        expect(requests).toEqual(['2/0/0']);
    });
});
