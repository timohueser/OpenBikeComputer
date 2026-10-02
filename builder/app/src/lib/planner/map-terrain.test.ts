import { expect, it, vi } from 'vitest';
import type * as maplibre from 'maplibre-gl';
import contour from 'maplibre-contour';
import { terrainSource } from './map-terrain';

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
    expect(contour.DemSource).toHaveBeenCalledWith({ url: 'https://example.org/terrain/{z}/{x}/{y}.webp', maxzoom: 12, worker: true, cacheSize: 64, encoding: 'terrarium' });
});
