import { expect, it, vi } from 'vitest';
import { addProtocol, type Map, type RequestParameters } from 'maplibre-gl';
import { Raster } from './raster';

vi.mock('maplibre-gl', () => ({ addProtocol: vi.fn() }));

it('draws a failed tile empty and a tile of an older look in the current look, so a data layer never fails the map', async () => {
    const sources: Record<string, { tiles: string[]; setTiles: (tiles: string[]) => void }> = {};
    const map = {
        getSource: (id: string) => sources[id],
        getLayer: (id: string) => sources[id],
        addSource: (id: string, { tiles }: { tiles: string[] }) => { sources[id] = { tiles, setTiles(next) { this.tiles = next; } }; },
        addLayer: vi.fn(), setLayoutProperty: vi.fn(),
    } as unknown as Map;
    const report = vi.fn(), drawn: string[] = [];
    let failing = true;
    const raster = new Raster({
        id: 'snow', extent: async () => ({ maxzoom: 12, attribution: '' }), report,
        draw: async look => { drawn.push(look); if (failing) throw new Error('503'); return undefined; },
    });
    raster.sync(map, true, '2026-01-15/light');
    await vi.waitFor(() => expect(sources.snow?.tiles).toEqual(['obc-snow://2026-01-15/light/{z}/{x}/{y}']));
    const tile = vi.mocked(addProtocol).mock.calls[0][1] as (params: RequestParameters, abort: AbortController) => Promise<{ data: ArrayBuffer }>;

    expect((await tile({ url: 'obc-snow://2026-01-15/light/9/266/177' }, new AbortController())).data.byteLength).toBe(0);
    expect(report).toHaveBeenLastCalledWith(new Error('503'));

    failing = false;
    raster.sync(map, true, '2026-01-16/light');
    expect(sources.snow.tiles).toEqual(['obc-snow://2026-01-16/light/{z}/{x}/{y}']);
    expect(report).toHaveBeenLastCalledWith();
    await tile({ url: 'obc-snow://2026-01-15/light/9/266/177' }, new AbortController());
    expect(drawn).toEqual(['2026-01-15/light', '2026-01-16/light']);

    raster.sync(map, false, '2026-01-16/light');
    await expect(tile({ url: 'obc-snow://2026-01-16/light/9/266/177' }, new AbortController())).rejects.toMatchObject({ name: 'AbortError' });
});
