// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import RoutesMap from './RoutesMap.svelte';
import type { Item } from './filter';

/** The layers of each map that the component made, by id, with their zoom ranges. */
const { maps } = vi.hoisted(() => ({ maps: [] as { layers: Map<string, { minzoom?: number; maxzoom?: number }>; load: () => void }[] }));

vi.mock('maplibre-gl', () => ({
    Map: class {
        layers = new Map<string, { minzoom?: number; maxzoom?: number }>();
        load = () => {};
        dragRotate = { disable() {} };
        touchZoomRotate = { disableRotation() {} };
        constructor() { maps.push(this); }
        on(type: string, ...rest: unknown[]) { if (type === 'style.load') this.load = rest[0] as () => void; }
        once() {}
        addControl() {}
        addSource() {}
        addLayer(layer: { id: string; minzoom?: number; maxzoom?: number }) { this.layers.set(layer.id, layer); }
        getLayer(id: string) { return this.layers.get(id); }
        getSource() { return { setData() {} }; }
        setFilter() {}
        setPaintProperty() {}
        remove() {}
        resize() {}
    },
    NavigationControl: class {},
}));
vi.mock('../../lib/planner/map-setup', () => ({
    ClosedAttribution: class {}, styleConfig: {}, useMapProtocols() {},
    terrain: { acquire: () => ({ relief: () => ({}), release() {} }) },
}));
vi.mock('../../lib/planner/map-terrain', () => ({ terrainRetry: () => () => false }));
vi.mock('../../lib/planner/map-style', () => ({ basemapStyle: () => ({}) }));

let map: ReturnType<typeof mount> | undefined;
afterEach(async () => { if (map) await unmount(map); maps.length = 0; document.body.replaceChildren(); });

function layersOf(look: 'route' | 'signed') {
    const item = { kind: look, id: '1', name: 'A', at: 0, summary: { line: [8, 46, 8.1, 46.1], bounds: [8, 46, 8.1, 46.1] } } as unknown as Item;
    map = mount(RoutesMap, { target: document.body, props: { items: [item], shown: [item], selected: [], look, fit: 0, onselect() {}, onview() {} } });
    flushSync();
    maps[0].load();
    return maps[0].layers;
}

describe('routes map', () => {
    it('counts the routes of the rider in bubbles below the zoom of the lines', () => {
        const layers = layersOf('route');
        expect(layers.get('routes')?.minzoom).toBe(8);
        expect(layers.get('bubbles')?.maxzoom).toBe(8);
    });

    // Explore changes its list only when the map is still, so a bubble would count the routes of the last view.
    it('draws signed routes at each zoom, with no bubble', () => {
        const layers = layersOf('signed');
        expect(layers.get('routes')?.minzoom).toBe(0);
        expect([...layers.keys()].filter(id => id.startsWith('bubble'))).toEqual([]);
    });
});
