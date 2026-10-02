import { expect, it, vi } from 'vitest';
import type * as maplibre from 'maplibre-gl';
import contour from 'maplibre-contour';
import { terrainRetry, terrainSource } from './map-terrain';

vi.mock('maplibre-contour', () => ({ default: { DemSource: vi.fn(function () {
    return { setupMaplibre: vi.fn(), sharedDemProtocolId: 'dem-shared', contourProtocolId: 'dem-contour' };
}) } }));

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

it('keeps terrain errors off the map failure and reloads the tile with a growing wait', () => {
    vi.useFakeTimers();
    const refreshTiles = vi.fn();
    const handlers: Record<string, () => void> = {};
    const map = { refreshTiles, getSource: () => ({}), once: (type: string, handler: () => void) => { handlers[type] = handler; } };
    const absorb = terrainRetry(map as unknown as maplibre.Map);
    const tile = { tileID: { canonical: { x: 3, y: 5, z: 7 } } };
    const error = (sourceId: string) => ({ type: 'error', error: new Error('timed out'), sourceId, tile }) as Parameters<typeof absorb>[0];

    expect(absorb(error('basemap'))).toBe(false);
    for (const wait of [5_000, 10_000, 20_000]) {
        expect(absorb(error('terrain'))).toBe(true);
        vi.advanceTimersByTime(wait - 1);
        expect(refreshTiles).not.toHaveBeenCalled();
        vi.advanceTimersByTime(1);
        expect(refreshTiles).toHaveBeenCalledWith('terrain', [{ x: 3, y: 5, z: 7 }]);
        refreshTiles.mockClear();
    }
    expect(absorb(error('terrain'))).toBe(true);
    expect(absorb(error('contours'))).toBe(true);
    handlers.remove();
    vi.runAllTimers();
    expect(refreshTiles).not.toHaveBeenCalled();
    vi.useRealTimers();
});
