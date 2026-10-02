import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as maplibre from 'maplibre-gl';
import contour from 'maplibre-contour';
import { terrainRetry, terrainSource } from './map-terrain';

vi.mock('maplibre-contour', () => ({ default: { DemSource: vi.fn(function () {
    return { setupMaplibre: vi.fn(), sharedDemProtocolId: 'dem-shared', contourProtocolId: 'dem-contour' };
}) } }));

beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); });

it('shares one DEM manager across overlapping mounts and repeated remounts', () => {
    const removeProtocol = vi.fn();
    const library = { removeProtocol } as unknown as typeof maplibre;
    const terrain = terrainSource('https://example.org/terrain/{z}/{x}/{y}.webp');
    const first = terrain.acquire(library), second = terrain.acquire(library);
    expect(first.dem).toBe(second.dem);
    expect(first.dem.setupMaplibre).toHaveBeenCalledTimes(1);
    first.release(); first.release();
    expect(removeProtocol).not.toHaveBeenCalled();
    second.release();
    expect(removeProtocol.mock.calls).toEqual([['dem-shared'], ['dem-contour']]);
    for (let index = 0; index < 10; index++) terrain.acquire(library).release();
    expect(contour.DemSource).toHaveBeenCalledTimes(1);
    expect(contour.DemSource).toHaveBeenCalledWith({ url: 'https://example.org/terrain/{z}/{x}/{y}.webp', maxzoom: 12, worker: true, cacheSize: 64, encoding: 'terrarium', timeoutMs: 20_000 });
});

function fakeMap() {
    const handlers: Record<string, (event: object) => void> = {};
    const refreshTiles = vi.fn();
    const on = (type: string, handler: (event: object) => void) => { handlers[type] = handler; };
    const absorb = terrainRetry({ refreshTiles, getSource: () => ({}), on, once: on } as unknown as maplibre.Map);
    const event = (sourceId: string, x: number) => ({ type: 'error', error: new Error('timed out'), sourceId, tile: { tileID: { canonical: { x, y: 5, z: 7 } } } });
    return {
        handlers,
        fail: (sourceId: string, x = 3) => absorb(event(sourceId, x) as Parameters<typeof absorb>[0]),
        load: (x: number) => handlers.sourcedata(event('terrain', x)),
        /** The tiles reloaded since the last call, as `x` of each. */
        refreshed: () => {
            const xs = refreshTiles.mock.calls.map(([, [tile]]) => tile.x);
            refreshTiles.mockClear();
            return xs;
        },
    };
}

it('keeps terrain errors off the map failure and reloads the tile with a growing wait', () => {
    const map = fakeMap();
    expect(map.fail('basemap')).toBe(false);
    for (const wait of [5_000, 10_000, 20_000]) {
        expect(map.fail('terrain')).toBe(true);
        vi.advanceTimersByTime(wait - 1);
        expect(map.refreshed()).toEqual([]);
        vi.advanceTimersByTime(1);
        expect(map.refreshed()).toEqual([3]);
    }
    expect(map.fail('terrain')).toBe(true);
    expect(map.fail('contours')).toBe(true);
    map.handlers.remove({});
    vi.runAllTimers();
    expect(map.refreshed()).toEqual([]);
});

it('runs one retry at a time and gives a tile that loaded fresh retries', () => {
    const map = fakeMap();
    map.fail('terrain', 1);
    map.fail('terrain', 2);
    vi.advanceTimersByTime(5_000);
    expect(map.refreshed()).toEqual([1]);
    map.load(1);
    expect(map.refreshed()).toEqual([2]);
    map.fail('terrain', 1);
    map.fail('terrain', 2);
    vi.advanceTimersByTime(5_000);
    expect(map.refreshed()).toEqual([1]);
    // Tile 2 is due after 10 s but waits until the retry of tile 1 ends or reaches the DEM limit.
    vi.advanceTimersByTime(19_999);
    expect(map.refreshed()).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(map.refreshed()).toEqual([2]);
});
