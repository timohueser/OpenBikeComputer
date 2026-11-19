import { afterEach, expect, it, vi } from 'vitest';
import type { Map as MapLibreMap, MapMouseEvent } from 'maplibre-gl';
import { networkScope, RouteOverlays, routeWebsite } from './route-overlays';
import { trailMarker } from './trail-markers';
import { mapStyle } from './map-style';
import { createPropertyExpression, v8, type StylePropertySpecification } from '@maplibre/maplibre-gl-style-spec';
import { testConfig } from '../../../test-support/planner/config';

afterEach(() => { vi.unstubAllGlobals(); });

const tilejson = { tiles: ['https://tiles.test/overlays/{z}/{x}/{y}.mvt'], minzoom: 6, maxzoom: 14, bounds: [7.45, 47.5, 10.5, 49.85],
    attribution: 'OSM', routing_package: 'package-a' };

function overlays(style: { id: string; type: string }[] = []) {
    let zoom = 12;
    const definitions = new Map<string, { layout: Record<string, unknown>; filter?: unknown; [key: string]: unknown }>();
    const sources = new Map<string, unknown>();
    const map = {
        on: vi.fn(), off: vi.fn(), getZoom: () => zoom,
        getBounds: () => ({ getWest: () => 7.9, getSouth: () => 48, getEast: () => 8, getNorth: () => 48.1 }),
        getSource: (id: string) => sources.get(id), addSource: vi.fn((id: string, spec: unknown) => sources.set(id, spec)),
        getStyle: () => ({ layers: style }), getLayer: (id: string) => definitions.get(id),
        addLayer: (layer: { id: string; layout?: Record<string, unknown> }, before?: string) => definitions.set(layer.id, { ...layer, layout: { ...layer.layout }, before }),
        setLayoutProperty: (id: string, key: string, value: unknown) => { definitions.get(id)!.layout[key] = value; },
        getLayoutProperty: (id: string, key: string) => definitions.get(id)?.layout[key],
        setFilter: (id: string, filter: unknown) => { definitions.get(id)!.filter = filter; }, setPaintProperty: vi.fn(), getPaintProperty: () => '#f4f2eb',
        queryRenderedFeatures: vi.fn(), querySourceFeatures: vi.fn(),
    };
    const status = vi.fn(), hint = vi.fn();
    const layer = new RouteOverlays(map as unknown as MapLibreMap, 'https://tiles.test/overlays.json', status, hint);
    const visible = () => [...definitions].filter(([, d]) => d.layout.visibility === 'visible').map(([id]) => id);
    return { layer, map, definitions, sources, status, hint, visible, zoomTo: (z: number) => { zoom = z; } };
}

it('loads the archive when an overlay is first shown, then draws the network and the access of the travel mode', async () => {
    const fetcher = vi.fn().mockResolvedValue({ ok: true, json: async () => tilejson });
    vi.stubGlobal('fetch', fetcher);
    const view = overlays();
    await view.layer.install('light');
    expect(fetcher).not.toHaveBeenCalled();
    view.layer.set({ network: 'cycling', access: true });
    await vi.waitFor(() => expect(view.visible()).toEqual(['network-cycling', 'network-cycling-band', 'network-cycling-labels', 'access-lines', 'access-symbols']));
    expect(view.sources.get('route-overlays')).toEqual({ type: 'vector', tiles: tilejson.tiles, minzoom: 6, maxzoom: 14, bounds: tilejson.bounds, attribution: 'OSM' });
    expect(JSON.stringify(view.definitions.get('access-symbols')!.filter)).toContain('cycling_minzoom');
    view.layer.set({ network: 'hiking', access: true }, 'walking');
    expect(view.visible()).toEqual(['network-hiking', 'network-hiking-band', 'network-hiking-labels', 'hiking-markers', 'access-lines', 'access-symbols']);
    expect(JSON.stringify(view.definitions.get('access-symbols')!.filter)).toContain('walking_minzoom');
    view.layer.set({ network: 'mtb', access: false });
    expect(view.visible()).toEqual(['network-mtb', 'network-mtb-band', 'network-mtb-labels']);
    // A new style after a theme change reuses the archive's TileJSON.
    view.sources.clear(); view.definitions.clear();
    await view.layer.install('dark');
    expect(view.definitions.size).toBe(12);
    view.layer.set({ network: 'none', access: false });
    expect(view.visible()).toEqual([]);
    expect(fetcher).toHaveBeenCalledTimes(1);
    view.layer.destroy();
});

it('puts the bands over the water and its structures, and under the lines of the planner style', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, json: async () => tilejson }));
    const style = mapStyle('light', testConfig, { dem: '' }).layers;
    const view = overlays(style);
    view.layer.set({ network: 'cycling', access: false });
    await view.layer.install('light');
    const at = (id: string) => { const index = style.findIndex(layer => layer.id === id); expect(index, id).toBeGreaterThanOrEqual(0); return index; };
    const band = at(view.definitions.get('network-cycling-band')!.before as string);
    for (const under of ['water', 'land-structure']) expect(band).toBeGreaterThan(at(under));
    for (const over of ['land-protected', 'water_stream', 'water_river', 'cliffs', 'ferries', 'roads_other', 'roads_minor', 'roads_bridges_other']) expect(band).toBeLessThanOrEqual(at(over));
    expect(at(view.definitions.get('network-cycling')!.before as string)).toBeGreaterThan(at('roads_bridges_highway'));
    view.layer.destroy();
});

it('draws a wider band for a wider road, and the width of a path for a tile that names no road', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, json: async () => tilejson }));
    const view = overlays();
    view.layer.set({ network: 'cycling', access: false });
    await view.layer.install('light');
    const band = view.definitions.get('network-cycling-band') as unknown as { paint: Record<string, unknown> };
    const width = createPropertyExpression(band.paint['line-width'], 'line-width', v8.paint_line['line-width'] as StylePropertySpecification);
    if (width.result === 'error') throw new Error(width.value[0].message);
    const at = (zoom: number, properties: Record<string, string>) => width.value.evaluate({ zoom }, { type: 2, properties });
    expect(['', 'minor', 'major'].map(road => at(15, road ? { road } : {}))).toEqual([10, 13, 14]);
    expect(at(18, {})).toBe(22);
    view.layer.destroy();
});

it('hides every overlay when the route comes from another routing package', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, json: async () => tilejson }));
    const view = overlays();
    view.layer.set({ network: 'cycling', access: true });
    await view.layer.install('light');
    view.layer.verify('package-a');
    expect(view.visible()).toHaveLength(5);
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
    await vi.waitFor(() => expect(view.visible()).toEqual(['network-cycling', 'network-cycling-band', 'network-cycling-labels']));
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

it('hints at the route under a resting pointer, and takes clicks only on access symbols', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, json: async () => tilejson }));
    vi.useFakeTimers();
    const view = overlays();
    view.layer.set({ network: 'hiking', access: true });
    await view.layer.install('light');
    const trail = { id: 12, sourceLayer: 'hiking', layer: { id: 'network-hiking' }, properties: { routes: '[42,43]' } };
    view.map.queryRenderedFeatures.mockReturnValue([trail]);
    view.map.querySourceFeatures.mockReturnValue([{ id: 42, properties: { name: 'Westweg' } }, { id: 43, properties: { ref: 'HW1' } }]);
    const event = { point: { x: 40, y: 40 }, lngLat: { lng: 8, lat: 48 } } as MapMouseEvent;
    expect(view.layer.hover(event)).toBe(false);
    vi.advanceTimersByTime(300);
    // A pointer that moves on starts the rest again.
    view.layer.hover(event);
    vi.advanceTimersByTime(300);
    expect(view.hint).not.toHaveBeenCalledWith(expect.anything());
    vi.advanceTimersByTime(300);
    expect(view.hint).toHaveBeenLastCalledWith({ point: event.point, name: 'Westweg +1' });
    view.layer.hover();
    expect(view.hint).toHaveBeenLastCalledWith(null);
    view.map.queryRenderedFeatures.mockReturnValue([{ id: 13, sourceLayer: 'access', layer: { id: 'access-symbols' }, properties: {} }]);
    expect(view.layer.hover(event)).toBe(true);
    vi.runAllTimers();
    expect(view.hint).toHaveBeenLastCalledWith(null);
    view.layer.destroy();
    vi.useRealTimers();
});

it('names the reach of a cycling or a walking network', () => {
    expect(networkScope({ network: 'rcn' })).toBe('Regional');
    expect(networkScope({ network: 'iwn' })).toBe('International');
    expect(networkScope({ network: '' })).toBe('');
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
