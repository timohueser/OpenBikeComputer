import { afterEach, expect, it, vi } from 'vitest';
import type { Map as MapLibreMap } from 'maplibre-gl';
import { RouteOverlays, routeWebsite, overlaySelection } from './route-overlays';
import type { MapGeoJSONFeature, MapMouseEvent } from 'maplibre-gl';
import { trailMarker } from './trail-markers';

afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });

function viewport() {
    const data = vi.fn();
    const update = vi.fn();
    let zoom = 12;
    let bounds = [7.9, 48, 8, 48.1];
    let target: { setData: typeof data; updateData: typeof update } | undefined = { setData: data, updateData: update };
    const definitions = new Map();
    const map = {
        on: vi.fn(), off: vi.fn(), getZoom: () => zoom,
        getBounds: () => ({ getWest: () => bounds[0], getSouth: () => bounds[1], getEast: () => bounds[2], getNorth: () => bounds[3] }),
        getSource: () => target, getLayer: (id: string) => definitions.get(id),
        addSource: () => target = { setData: data, updateData: update }, getStyle: () => ({ layers: [] }),
        addLayer: (layer: { id: string; layout?: object }) => definitions.set(layer.id, { ...layer, layout: layer.layout ?? {} }),
        setLayoutProperty: vi.fn((id: string, key: string, value: string) => { definitions.get(id).layout[key] = value; }),
        getLayoutProperty: (id: string, key: string) => definitions.get(id)?.layout[key],
        queryRenderedFeatures: vi.fn(), setFeatureState: vi.fn(),
    };
    const status = vi.fn();
    const overlays = new RouteOverlays(map as unknown as MapLibreMap, status);
    return { overlays, map, definitions, data, update, status, clearStyle: () => { target = undefined; definitions.clear(); }, move: (b: number[], z = zoom) => { bounds = b; zoom = z; } };
}

const rendered = (data: ReturnType<typeof collection>) => ({ type: data.type, features: data.features.map(({ id }) => ({ id, type: 'Feature', geometry: null, properties: { kind: 'cycling', rank: 1, ref: 'R', marker: undefined, status: undefined } })) });
const collection = (name: string) => ({ type: 'FeatureCollection', package: 'release-a', features: [{ type: 'Feature', id: name, geometry: null, properties: { name, kind: 'cycling', rank: 1, ref: 'R' } }], coverage: [7.65, 47.85, 8.25, 48.18] });

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
    expect(view.data).toHaveBeenLastCalledWith(rendered(collection('current')));
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
    await vi.waitFor(() => expect(view.data).toHaveBeenLastCalledWith(rendered(collection('network'))));
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
    expect(view.data).toHaveBeenLastCalledWith(rendered(data));
    view.overlays.destroy();
});

it('resolves shared route metadata when a map feature is inspected', () => {
    const route = { id: 42, kind: 'cycling', network: 'rcn', rank: 2, name: 'Regional route', ref: 'R' };
    const feature = { properties: { kind: 'cycling', way: 1, routes: '[42]' } } as unknown as MapGeoJSONFeature;
    expect(overlaySelection(feature, [8, 48], { 42: route }).routes).toEqual([route]);
});

it('loads a replacement source after the theme changes during a pending request', async () => {
    const requests: { signal: AbortSignal; resolve: (data: unknown) => void }[] = [];
    vi.stubGlobal('fetch', vi.fn((_url: string, init: RequestInit) => new Promise(resolve => {
        requests.push({ signal: init.signal as AbortSignal, resolve: data => resolve({ ok: true, json: async () => data }) });
    })));
    const view = viewport();
    view.overlays.set({ network: 'cycling', access: false });
    view.clearStyle();
    view.overlays.install('dark');
    const current = view.overlays.refresh();
    expect(requests).toHaveLength(2);
    expect(requests[0].signal.aborted).toBe(true);
    requests[0].resolve(collection('removed source'));
    requests[1].resolve(collection('replacement source'));
    await current;
    expect(view.data).toHaveBeenCalledTimes(1);
    expect(view.data).toHaveBeenLastCalledWith(rendered(collection('replacement source')));
    await view.overlays.refresh();
    expect(view.data).toHaveBeenCalledTimes(1);
    view.overlays.destroy();
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
    expect(view.data).toHaveBeenLastCalledWith(rendered(collection('network')));
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

it('updates entering and leaving unlabeled features and resets geometry at labeled zooms', async () => {
    const line = (id: number) => ({ ...collection('line').features[0], id });
    const first = { ...collection('first'), features: [line(1), line(2)] };
    const second = { ...collection('second'), features: [line(2), line(3)] };
    const fetcher = vi.fn().mockResolvedValueOnce({ ok: true, json: async () => first })
        .mockResolvedValue({ ok: true, json: async () => second });
    vi.stubGlobal('fetch', fetcher);
    const view = viewport();
    view.move([7.9, 48, 8, 48.1], 8);
    view.overlays.set({ network: 'cycling', access: false });
    await view.overlays.refresh();
    view.move([8.1, 48, 8.2, 48.1]);
    await view.overlays.refresh();
    expect(view.update).toHaveBeenLastCalledWith({ remove: [1], add: [{ id: 3, type: 'Feature', geometry: null, properties: { kind: 'cycling', rank: 1, ref: 'R', marker: undefined, status: undefined } }] });
    expect(second.features[0]).toBe(first.features[1]);
    expect(view.data).toHaveBeenCalledTimes(1);
    view.move([7.9, 48, 8, 48.1]);
    await view.overlays.refresh();
    expect(view.update).toHaveBeenLastCalledWith({ remove: [3], add: [{ id: 1, type: 'Feature', geometry: null, properties: { kind: 'cycling', rank: 1, ref: 'R', marker: undefined, status: undefined } }] });
    expect(fetcher).toHaveBeenCalledTimes(2);
    view.move([7.9, 48, 8, 48.1], 13);
    await view.overlays.refresh();
    expect(view.data).toHaveBeenCalledTimes(2);
    fetcher.mockResolvedValue({ ok: true, json: async () => ({ ...second }) });
    view.move([8.1, 48, 8.2, 48.1]);
    await view.overlays.refresh();
    expect(view.data).toHaveBeenCalledTimes(3);
    expect(view.update).toHaveBeenCalledTimes(2);
    view.overlays.destroy();
});

it('keeps complete popup metadata while workers receive only geometry and render fields', async () => {
    const route = { id: 42, kind: 'cycling', network: 'rcn', rank: 2, name: 'Regional route', ref: 'R', website: 'https://example.org' };
    const properties = { kind: 'cycling', rank: 2, ref: 'R', way: 100, name: 'Named road', routes: [42], tags: { surface: 'gravel' }, riding: [true, false], walking: [true, true], pushing: [true, true] };
    const feature = { id: 9, type: 'Feature', geometry: { type: 'LineString', coordinates: [[8, 48], [8.1, 48.1]] }, properties };
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, json: async () => ({ ...collection('network'), features: [feature], routes: { 42: route } }) }));
    const view = viewport();
    view.clearStyle();
    view.overlays.install('light');
    view.overlays.set({ network: 'cycling', access: false });
    await view.overlays.refresh();
    const projected = view.data.mock.calls.at(-1)![0].features[0];
    const fields = (value: unknown): string[] => Array.isArray(value)
        ? value[0] === 'get' ? [value[1]] : value.flatMap(fields)
        : value && typeof value === 'object' ? Object.values(value).flatMap(fields) : [];
    for (const layer of view.definitions.values()) {
        for (const field of fields(layer)) expect(Object.hasOwn(projected.properties, field)).toBe(true);
    }
    expect(projected.geometry).toBe(feature.geometry);
    expect(projected.properties).toEqual({ kind: 'cycling', rank: 2, ref: 'R', marker: undefined, status: undefined });
    expect(view.overlays.selection(projected, [8, 48])).toEqual({ ...properties, coordinate: [8, 48], routes: [route] });
    view.overlays.destroy();
});

it('replaces reused feature IDs and popup metadata when a fresh reply changes release', async () => {
    const response = (name: string, longitude: number) => ({
        ...collection('network'), package: name,
        features: [{ id: 9, type: 'Feature', geometry: { type: 'LineString', coordinates: [[longitude, 48], [longitude + 0.1, 48.1]] },
            properties: { way: 100, kind: 'cycling', rank: 2, ref: name, name, routes: [42] } }],
        routes: { 42: { id: 42, kind: 'cycling', network: 'rcn', rank: 2, name, ref: name } },
    });
    const first = response('release-a', 8), second = response('release-b', 8.2);
    const fetcher = vi.fn().mockResolvedValueOnce({ ok: true, json: async () => first })
        .mockResolvedValue({ ok: true, json: async () => second });
    vi.stubGlobal('fetch', fetcher);
    const view = viewport();
    view.move([7.9, 48, 8, 48.1], 8);
    view.overlays.set({ network: 'cycling', access: false });
    await view.overlays.refresh();
    view.move([8.1, 48, 8.2, 48.1]);
    await view.overlays.refresh();
    expect(view.update).not.toHaveBeenCalled();
    expect(view.data).toHaveBeenCalledTimes(2);
    const projected = view.data.mock.calls.at(-1)![0].features[0];
    expect(projected.geometry).toEqual(second.features[0].geometry);
    expect(projected.properties.ref).toBe('release-b');
    expect(view.overlays.selection(projected, [8.2, 48])).toMatchObject({ name: 'release-b', routes: [second.routes[42]] });
    view.move([7.9, 48, 8, 48.1]);
    await view.overlays.refresh();
    expect(fetcher).toHaveBeenCalledTimes(3);
    view.overlays.destroy();
});
