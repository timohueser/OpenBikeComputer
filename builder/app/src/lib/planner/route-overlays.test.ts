import { afterEach, expect, it, vi } from 'vitest';
import type { Map as MapLibreMap, MapMouseEvent } from 'maplibre-gl';
import { RouteOverlays, routeWebsite } from './route-overlays';
import { trailMarker } from './trail-markers';

afterEach(() => { vi.unstubAllGlobals(); });

const tilejson = { tiles: ['https://tiles.test/overlays/{z}/{x}/{y}.mvt'], minzoom: 6, maxzoom: 14, bounds: [7.45, 47.5, 10.5, 49.85],
    attribution: 'OSM', routing_package: 'package-a' };

function overlays() {
    let zoom = 12;
    const definitions = new Map<string, { layout: Record<string, unknown>; filter?: unknown; [key: string]: unknown }>();
    const sources = new Map<string, unknown>();
    const map = {
        on: vi.fn(), off: vi.fn(), getZoom: () => zoom,
        getBounds: () => ({ getWest: () => 7.9, getSouth: () => 48, getEast: () => 8, getNorth: () => 48.1 }),
        getSource: (id: string) => sources.get(id), addSource: vi.fn((id: string, spec: unknown) => sources.set(id, spec)),
        getStyle: () => ({ layers: [] }), getLayer: (id: string) => definitions.get(id),
        addLayer: (layer: { id: string; layout?: Record<string, unknown> }) => definitions.set(layer.id, { ...layer, layout: layer.layout ?? {} }),
        setLayoutProperty: (id: string, key: string, value: unknown) => { definitions.get(id)!.layout[key] = value; },
        getLayoutProperty: (id: string, key: string) => definitions.get(id)?.layout[key],
        setFilter: (id: string, filter: unknown) => { definitions.get(id)!.filter = filter; }, setPaintProperty: vi.fn(),
        queryRenderedFeatures: vi.fn(), querySourceFeatures: vi.fn(), setFeatureState: vi.fn(),
    };
    const status = vi.fn();
    const layer = new RouteOverlays(map as unknown as MapLibreMap, 'https://tiles.test/overlays.json', status);
    const visible = () => [...definitions].filter(([, d]) => d.layout.visibility === 'visible').map(([id]) => id);
    return { layer, map, definitions, sources, status, visible, zoomTo: (z: number) => { zoom = z; } };
}

it('draws the chosen network and the access of the travel mode from one vector source', async () => {
    const fetcher = vi.fn().mockResolvedValue({ ok: true, json: async () => tilejson });
    vi.stubGlobal('fetch', fetcher);
    const view = overlays();
    view.layer.set({ network: 'cycling', access: true });
    await view.layer.install('light');
    expect(view.sources.get('route-overlays')).toEqual({ type: 'vector', tiles: tilejson.tiles, minzoom: 6, maxzoom: 14, bounds: tilejson.bounds, attribution: 'OSM' });
    expect(view.visible()).toEqual(['network-cycling', 'network-cycling-labels', 'access-lines', 'access-symbols']);
    expect(JSON.stringify(view.definitions.get('access-symbols')!.filter)).toContain('cycling_status');
    view.layer.set({ network: 'hiking', access: true }, 'walking');
    expect(view.visible()).toEqual(['network-hiking', 'network-hiking-labels', 'hiking-markers', 'access-lines', 'access-symbols']);
    expect(JSON.stringify(view.definitions.get('access-symbols')!.filter)).toContain('walking_status');
    view.layer.set({ network: 'none', access: false });
    expect(view.visible()).toEqual([]);
    // A new style after a theme change reuses the archive's TileJSON.
    view.sources.clear(); view.definitions.clear();
    await view.layer.install('dark');
    expect(view.definitions.size).toBe(7);
    expect(fetcher).toHaveBeenCalledTimes(1);
    view.layer.destroy();
});

it('hides every overlay when the route comes from another routing package', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, json: async () => tilejson }));
    const view = overlays();
    view.layer.set({ network: 'cycling', access: true });
    await view.layer.install('light');
    view.layer.verify('package-a');
    expect(view.visible()).toHaveLength(4);
    view.layer.verify('package-b');
    expect(view.visible()).toEqual([]);
    expect(view.status).toHaveBeenLastCalledWith(expect.stringContaining('other map data than the router'));
    view.zoomTo(5);
    view.layer.verify(undefined);
    expect(view.status).toHaveBeenLastCalledWith('Zoom in to see route networks and access restrictions.');
    view.layer.destroy();
});

it('offers a retry when the overlay archive cannot load', async () => {
    const fetcher = vi.fn().mockResolvedValueOnce({ ok: false }).mockResolvedValue({ ok: true, json: async () => tilejson });
    vi.stubGlobal('fetch', fetcher);
    const view = overlays();
    view.layer.set({ network: 'cycling', access: false });
    await view.layer.install('light');
    expect(view.status).toHaveBeenLastCalledWith('Route networks and access could not load.', true);
    view.layer.retry();
    await vi.waitFor(() => expect(view.visible()).toEqual(['network-cycling', 'network-cycling-labels']));
    expect(view.status).toHaveBeenLastCalledWith('');
    view.layer.destroy();
});

it('opens route details from the tile catalogue, and access rules of the travel mode at their symbol', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, json: async () => tilejson }));
    const view = overlays();
    view.layer.set({ network: 'hiking', access: true }, 'walking');
    await view.layer.install('light');
    const trail = { id: 12, sourceLayer: 'hiking', layer: { id: 'network-hiking' }, properties: { rank: 2, ref: 'W', routes: '[42]' } };
    const access = { id: 13, sourceLayer: 'access', properties: { cycling_status: 'push', walking_status: 'closed', name: 'Steg', riding: '[false,false]',
        walking: '[true,false]', pushing: '[true,true]', conditional: 0, tags: '{"foot":"no"}' } };
    const restriction = { ...access, layer: { id: 'access-lines' } }, symbol = { ...access, layer: { id: 'access-symbols' } };
    const route = { id: 42, kind: 'hiking', network: 'rwn', rank: 2, name: 'Westweg', ref: 'W', website: 'https://example.org' };
    view.map.querySourceFeatures.mockReturnValue([{ id: 42, properties: { kind: 'hiking', network: 'rwn', rank: 2, name: 'Westweg', ref: 'W', website: 'https://example.org' } }]);
    let underPointer: object[] = [restriction, trail];
    view.map.queryRenderedFeatures.mockImplementation((_box, { layers }) => underPointer.filter(f => layers.includes((f as typeof trail).layer.id)));
    const event = { point: { x: 40, y: 40 }, lngLat: { lng: 8, lat: 48 } } as MapMouseEvent;
    expect(view.layer.hit(event)).toMatchObject({ kind: 'hiking', routes: [route] });
    expect(view.map.querySourceFeatures).toHaveBeenLastCalledWith('route-overlays', { sourceLayer: 'routes', filter: ['in', ['id'], ['literal', [42]]] });
    underPointer = [restriction, trail, symbol];
    expect(view.layer.hit(event)).toEqual({ coordinate: [8, 48], way: 13, kind: 'access', name: 'Steg', ref: undefined, status: 'closed', conditional: 0,
        tags: { foot: 'no' }, riding: [false, false], walking: [true, false], pushing: [true, true], routes: undefined });
    underPointer = [restriction];
    expect(view.layer.hit(event)).toBeNull();
    view.layer.destroy();
});

it('highlights only the hovered network line and clears it when leaving or switching layers', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, json: async () => tilejson }));
    const view = overlays();
    view.layer.set({ network: 'hiking', access: false });
    await view.layer.install('light');
    const trail = { id: 12, sourceLayer: 'hiking', layer: { id: 'network-hiking' }, properties: {} };
    view.map.queryRenderedFeatures.mockReturnValue([trail]);
    const event = { point: { x: 40, y: 40 } } as MapMouseEvent;
    const state = { source: 'route-overlays', sourceLayer: 'hiking', id: 12 };
    expect(view.layer.hover(event)).toBe(true);
    expect(view.map.setFeatureState).toHaveBeenLastCalledWith(state, { hover: true });
    view.layer.hover(event);
    expect(view.map.setFeatureState).toHaveBeenCalledTimes(1);
    view.map.queryRenderedFeatures.mockReturnValue([]);
    expect(view.layer.hover(event)).toBe(false);
    expect(view.map.setFeatureState).toHaveBeenLastCalledWith(state, { hover: false });
    view.map.queryRenderedFeatures.mockReturnValue([trail]);
    view.layer.hover(event);
    view.layer.set({ network: 'none', access: false });
    expect(view.map.setFeatureState).toHaveBeenLastCalledWith(state, { hover: false });
    view.layer.destroy();
});

it('only links valid route websites', () => {
    expect(routeWebsite('https://example.org/route')).toBe('https://example.org/route');
    expect(routeWebsite('www.example.org')).toBe('https://www.example.org/');
    for (const value of [null, '', 'javascript:alert(1)', 'data:text/html,hello', 'https://user:pass@example.org']) expect(routeWebsite(value)).toBeNull();
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
