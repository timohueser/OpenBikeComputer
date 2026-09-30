import { afterEach, expect, it, vi } from 'vitest';
import type { Map as MapLibreMap } from 'maplibre-gl';
import { RouteOverlays, routeWebsite, overlaySelection } from './route-overlays';
import type { MapGeoJSONFeature, MapMouseEvent } from 'maplibre-gl';
import { trailMarker } from './trail-markers';

afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });

function viewport() {
    const data = vi.fn();
    let zoom = 12;
    let bounds = [7.9, 48, 8, 48.1];
    const definitions = new Map();
    const map = {
        on: vi.fn(), off: vi.fn(), getZoom: () => zoom,
        getBounds: () => ({ getWest: () => bounds[0], getSouth: () => bounds[1], getEast: () => bounds[2], getNorth: () => bounds[3] }),
        getSource: () => ({ setData: data }), getLayer: (id: string) => definitions.get(id),
        setLayoutProperty: vi.fn((id: string, key: string, value: string) => { definitions.get(id).layout[key] = value; }),
        getLayoutProperty: (id: string, key: string) => definitions.get(id)?.layout[key],
        queryRenderedFeatures: vi.fn(), setFeatureState: vi.fn(),
    };
    const status = vi.fn();
    const overlays = new RouteOverlays(map as unknown as MapLibreMap, status);
    return { overlays, map, definitions, data, status, move: (b: number[], z = zoom) => { bounds = b; zoom = z; } };
}

const collection = (name: string) => ({ type: 'FeatureCollection', features: [{ name }], coverage: [7.65, 47.85, 8.25, 48.18] });

it('cancels stale viewport replies, reuses a padded viewport, and removes hidden overlays', async () => {
    const requests: { signal: AbortSignal; resolve: (data: unknown) => void }[] = [];
    const fetcher = vi.fn((_url: string, init: RequestInit) => new Promise(resolve => {
        requests.push({ signal: init.signal as AbortSignal, resolve: data => resolve({ ok: true, json: async () => data }) });
    }));
    vi.stubGlobal('fetch', fetcher);
    const view = viewport();
    view.overlays.set({ network: 'cycling', access: true });
    view.move([8.1, 48, 8.2, 48.1]);
    const moved = view.overlays.refresh();
    expect(requests[0].signal.aborted).toBe(true);
    requests[1].resolve(collection('current'));
    await moved;
    requests[0].resolve(collection('stale'));
    await Promise.resolve(); await Promise.resolve();
    expect(view.data).toHaveBeenLastCalledWith(collection('current'));
    view.move([8.105, 48.005, 8.205, 48.105]);
    await view.overlays.refresh();
    expect(fetcher).toHaveBeenCalledTimes(2);
    view.overlays.set({ network: 'none', access: false });
    expect(view.data).toHaveBeenLastCalledWith({ type: 'FeatureCollection', features: [] });
    expect(fetcher).toHaveBeenCalledTimes(2);
    view.overlays.destroy();
});

it('reports missing coverage and failures instead of leaving misleading old lines', async () => {
    const fetcher = vi.fn().mockResolvedValue({ ok: true, json: async () => collection('regional') });
    vi.stubGlobal('fetch', fetcher);
    const view = viewport();
    view.move([9, 48, 9.1, 48.1]);
    view.overlays.set({ network: 'cycling', access: true });
    await vi.waitFor(() => expect(view.status).toHaveBeenLastCalledWith(expect.stringContaining('Outside the routing region')));
    fetcher.mockResolvedValue({ ok: false, json: async () => ({ message: 'Zoom in to show route networks and access restrictions.' }) });
    view.move([7.9, 48, 8, 48.1]);
    await view.overlays.refresh();
    expect(view.status).toHaveBeenLastCalledWith('Zoom in to show route networks and access restrictions.', true);
    expect(view.data).toHaveBeenLastCalledWith({ type: 'FeatureCollection', features: [] });
    view.move([7, 47, 9, 49], 5);
    await view.overlays.refresh();
    expect(fetcher).toHaveBeenCalledTimes(2);
    view.overlays.destroy();
});

it('requests one network at a time, including at wider zoom levels', async () => {
    const fetcher = vi.fn().mockResolvedValue({ ok: true, json: async () => collection('network') });
    vi.stubGlobal('fetch', fetcher);
    const view = viewport();
    view.move([7, 47, 9, 49], 6);
    view.overlays.set({ network: 'cycling', access: false });
    await vi.waitFor(() => expect(view.data).toHaveBeenLastCalledWith(collection('network')));
    expect(fetcher.mock.calls[0][0]).toContain('layers=cycling');
    view.overlays.set({ network: 'hiking', access: false });
    expect(fetcher.mock.calls[1][0]).toContain('layers=hiking');
    const hiking = new URL(fetcher.mock.calls[1][0], 'http://localhost');
    expect(hiking.searchParams.get('layers')).toBe('hiking');
    expect(hiking.searchParams.get('mode')).toBe('cycling');
    view.overlays.set({ network: 'hiking', access: true }, 'walking');
    expect(fetcher.mock.calls[2][0]).toContain('mode=walking');
    view.overlays.destroy();
});

it('only links valid route websites and preserves worker-encoded mode permissions', () => {
    expect(routeWebsite('https://example.org/route')).toBe('https://example.org/route');
    expect(routeWebsite('www.example.org')).toBe('https://www.example.org/');
    for (const value of [null, '', 'javascript:alert(1)', 'data:text/html,hello', 'https://user:pass@example.org']) expect(routeWebsite(value)).toBeNull();
    const selected = overlaySelection({ properties: { kind: 'access', way: 1, riding: '[false,false]', walking: '[true,true]', pushing: '[true,false]' } } as unknown as MapGeoJSONFeature, [8,48]);
    expect(selected.pushing).toEqual([true,false]);
    expect(selected.walking).toEqual([true,true]);
});

it('reuses in-flight views and cached network selections without reprocessing GeoJSON', async () => {
    let resolve!: (response: unknown) => void;
    const fetcher = vi.fn(() => new Promise(r => resolve = r));
    vi.stubGlobal('fetch', fetcher);
    const view = viewport();
    view.overlays.set({ network: 'cycling', access: false });
    const pending = view.overlays.refresh();
    expect(fetcher).toHaveBeenCalledTimes(1);
    const data = collection('cycling');
    resolve({ ok: true, json: async () => data });
    await pending;
    await view.overlays.refresh();
    expect(view.data).toHaveBeenCalledTimes(1);
    view.overlays.set({ network: 'none', access: false });
    view.overlays.set({ network: 'cycling', access: false });
    expect(fetcher).toHaveBeenCalledTimes(1);
    expect(view.data).toHaveBeenLastCalledWith(data);
    view.overlays.destroy();
});

it('resolves shared route metadata when a map feature is inspected', () => {
    const route = { id: 42, kind: 'cycling', network: 'rcn', rank: 2, name: 'Regional route', ref: 'R' };
    const feature = { properties: { kind: 'cycling', way: 1, routes: '[42]' } } as unknown as MapGeoJSONFeature;
    expect(overlaySelection(feature, [8, 48], { 42: route }).routes).toEqual([route]);
});

it('retries a busy overlay service, and cancels a retry when the layer is hidden', async () => {
    vi.useFakeTimers();
    const fetcher = vi.fn().mockResolvedValueOnce({ status: 503 })
        .mockResolvedValue({ ok: true, status: 200, json: async () => collection('network') });
    vi.stubGlobal('fetch', fetcher);
    const view = viewport();
    view.overlays.set({ network: 'cycling', access: false });
    await vi.advanceTimersByTimeAsync(250);
    expect(fetcher).toHaveBeenCalledTimes(2);
    expect(view.data).toHaveBeenLastCalledWith(collection('network'));
    fetcher.mockResolvedValue({ status: 503 });
    view.overlays.set({ network: 'hiking', access: false });
    await vi.advanceTimersByTimeAsync(0);
    view.overlays.set({ network: 'none', access: false });
    await vi.advanceTimersByTimeAsync(1000);
    expect(fetcher).toHaveBeenCalledTimes(3);
    view.overlays.destroy();
});

it('renders common OSM blazes without approximating unsupported symbols', () => {
    expect(trailMarker('red:yellow:white_red_diamond')?.foreground[0].shape).toBe('white_red_diamond');
    expect(trailMarker('red:white:red_diamond:K:white')?.text).toBe('K');
    expect(trailMarker('green:white:yellow_bar:green_stripe')?.foreground).toHaveLength(2);
    expect(trailMarker('black:black::X:white')?.text).toBe('X');
    expect(trailMarker('blue:blue:shell_modern')).toBeNull();
    expect(trailMarker('red:white:red_unknown')).toBeNull();
    expect(trailMarker(null)).toBeNull();
});

it('opens trail information along a restricted path and access rules only at its symbol', () => {
    const view = viewport();
    const trail = { id: 12, layer: { id: 'network-hiking' }, properties: { way: 1, kind: 'hiking' } };
    const restriction = { id: 13, layer: { id: 'access-lines' }, properties: { way: 1, kind: 'access', status: 'push' } };
    const walker = { ...restriction, layer: { id: 'access-symbols' } };
    for (const feature of [trail, restriction, walker]) view.definitions.set(feature.layer.id, { layout: {} });
    let underPointer = [restriction, trail];
    view.map.queryRenderedFeatures.mockImplementation((_bounds, { layers }) => underPointer.filter(f => layers.includes(f.layer.id)));
    const event = { point: { x: 40, y: 40 }, lngLat: { lng: 8, lat: 48 } } as MapMouseEvent;
    expect(view.overlays.hit(event)?.kind).toBe('hiking');
    underPointer = [restriction, trail, walker];
    expect(view.overlays.hit(event)?.status).toBe('push');
    underPointer = [restriction];
    expect(view.overlays.hit(event)).toBeNull();
    view.overlays.destroy();
});

it('highlights only the hovered network segment and clears it when leaving or switching layers', () => {
    const view = viewport();
    view.definitions.set('network-hiking', { layout: {} });
    const trail = { id: 12, layer: { id: 'network-hiking' }, properties: { way: 1, kind: 'hiking' } };
    view.map.queryRenderedFeatures.mockReturnValue([trail]);
    const event = { point: { x: 40, y: 40 } } as MapMouseEvent;
    expect(view.overlays.hover(event)).toBe(true);
    expect(view.map.setFeatureState).toHaveBeenLastCalledWith({ source: 'route-overlays', id: 12 }, { hover: true });
    view.overlays.hover(event);
    expect(view.map.setFeatureState).toHaveBeenCalledTimes(1);
    view.map.queryRenderedFeatures.mockReturnValue([]);
    expect(view.overlays.hover(event)).toBe(false);
    expect(view.map.setFeatureState).toHaveBeenLastCalledWith({ source: 'route-overlays', id: 12 }, { hover: false });
    view.map.queryRenderedFeatures.mockReturnValue([trail]);
    view.overlays.hover(event);
    view.overlays.set({ network: 'none', access: false });
    expect(view.map.setFeatureState).toHaveBeenLastCalledWith({ source: 'route-overlays', id: 12 }, { hover: false });
    view.overlays.destroy();
});
