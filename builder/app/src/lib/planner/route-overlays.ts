import type { ExpressionSpecification, GeoJSONSource, Map, MapGeoJSONFeature, MapMouseEvent } from 'maplibre-gl';
import type { FeatureCollection } from 'geojson';
import type { Coordinate } from './map-types';
import { trailMarker } from './trail-markers';

export interface OverlayOptions { network: 'cycling' | 'hiking' | 'none'; access: boolean }
export type AccessMode = 'cycling' | 'walking';
export interface NetworkRoute { id: number; kind: string; network: string; rank: number; name: string; ref: string; website?: string | null; symbol?: string; symbol_text?: string }
export interface OverlaySelection {
    coordinate: Coordinate;
    way: number;
    kind: string;
    name?: string;
    ref?: string;
    status?: string;
    riding?: boolean[];
    walking?: boolean[];
    pushing?: boolean[];
    conditional?: number;
    tags?: Record<string, string>;
    routes?: NetworkRoute[];
}

export const networkLevels = [
    { label: 'International / national', color: '#7c519c', dark: '#c49de0', rank: 3 },
    { label: 'Regional', color: '#2368b5', dark: '#79b7f1', rank: 2 },
    { label: 'Local', color: '#4f8b24', dark: '#a4cf67', rank: 1 },
    { label: 'Unspecified network', color: '#626a70', dark: '#b0b8be', rank: 0 },
];
export const networkNames: Record<string, string> = {
    icn: 'International cycling route', ncn: 'National cycling route', rcn: 'Regional cycling route', lcn: 'Local cycling route',
    iwn: 'International hiking route', nwn: 'National hiking route', rwn: 'Regional hiking route', lwn: 'Local hiking route',
};
const source = 'route-overlays';
const interactiveLayers = ['network-cycling', 'network-hiking', 'hiking-markers', 'access-symbols'];
const empty: FeatureCollection = { type: 'FeatureCollection', features: [] };
const endpoint = import.meta.env.VITE_PLANNER_ROUTING_URL ?? '/routing';
type OverlayCollection = FeatureCollection & { coverage: [number, number, number, number]; routes?: Record<string, NetworkRoute> };
type CachedOverlay = { bounds: number[]; zoom: number; key: string; data: OverlayCollection };
const contains = (outer: number[], inner: number[]) => inner[0] >= outer[0] && inner[1] >= outer[1] && inner[2] <= outer[2] && inner[3] <= outer[3];

export function overlaySelection(feature: MapGeoJSONFeature, coordinate: Coordinate, catalog?: Record<string, NetworkRoute>): OverlaySelection {
    const p = feature.properties;
    // Vector workers encode nested GeoJSON properties as JSON strings.
    const nested = <T>(value: T | string | undefined): T | undefined => typeof value === 'string' ? JSON.parse(value) : value;
    const routes = nested<(NetworkRoute | number)[]>(p.routes)?.map(route => typeof route === 'number' ? catalog?.[route] : route).filter((route): route is NetworkRoute => !!route);
    return { ...p, way: Number(p.way), kind: p.kind, coordinate, tags: nested(p.tags), routes,
        riding: nested(p.riding), walking: nested(p.walking), pushing: nested(p.pushing) };
}

export function routeWebsite(value?: string | null): string | null {
    if (!value) return null;
    try {
        const text = value.trim();
        const url = new URL(text.startsWith('www.') ? `https://${text}` : text);
        return ['https:', 'http:'].includes(url.protocol) && !url.username && !url.password ? url.href : null;
    } catch { return null; }
}

/** An independent viewport source; route edits never wait for this layer. */
export class RouteOverlays {
    private options: OverlayOptions = { network: 'none', access: false };
    private accessMode: AccessMode = 'cycling';
    private abort?: AbortController;
    private cache: CachedOverlay[] = [];
    private displayed?: OverlayCollection;
    private pending?: { bounds: number[]; zoom: number; key: string; task: Promise<void> };
    private disposed = false;
    private hovered?: string | number;

    constructor(private map: Map, private status: (message: string, retry?: boolean) => void) {
        map.on('moveend', this.refresh);
    }

    install(theme: 'light' | 'dark') {
        if (this.map.getSource(source)) return;
        this.abort?.abort();
        this.pending = undefined;
        this.displayed = undefined;
        this.map.addSource(source, { type: 'geojson', data: empty, attribution: '<a href="https://www.openstreetmap.org/copyright">Route networks & access © OpenStreetMap</a>' });
        const before = this.map.getStyle().layers?.find(layer => layer.type === 'symbol')?.id;
        const color: ExpressionSpecification = ['step', ['get', 'rank'], networkLevels[3][theme === 'dark' ? 'dark' : 'color'],
            1, networkLevels[2][theme === 'dark' ? 'dark' : 'color'], 2, networkLevels[1][theme === 'dark' ? 'dark' : 'color'],
            3, networkLevels[0][theme === 'dark' ? 'dark' : 'color']];
        for (const activity of ['cycling', 'hiking']) {
            this.map.addLayer({
                id: `network-${activity}`, type: 'line', source, filter: ['==', ['get', 'kind'], activity],
                layout: { 'line-join': 'round', 'line-sort-key': ['get', 'rank'] },
                paint: { 'line-color': color,
                    'line-opacity': ['case', ['boolean', ['feature-state', 'hover'], false], 1, 0.8],
                    'line-width': ['interpolate', ['linear'], ['zoom'],
                        6, ['case', ['boolean', ['feature-state', 'hover'], false], 2.4, 1.2],
                        10, ['case', ['boolean', ['feature-state', 'hover'], false], 3, 1.8],
                        13, ['case', ['boolean', ['feature-state', 'hover'], false], 4.4, 3.2],
                        17, ['case', ['boolean', ['feature-state', 'hover'], false], 6.2, 5]] },
            }, before);
            this.map.addLayer({
                id: `network-${activity}-labels`, type: 'symbol', source, minzoom: 11,
                filter: ['all', ['==', ['get', 'kind'], activity], ['!=', ['get', 'ref'], '']],
                layout: { 'symbol-placement': 'line', 'symbol-spacing': 350, 'text-field': ['get', 'ref'],
                    'text-font': ['Noto Sans Medium'], 'text-size': 11, 'text-offset': [0, 0.8], 'text-max-width': 8 },
                paint: { 'text-color': color, 'text-halo-color': theme === 'dark' ? '#181d19' : '#ffffff', 'text-halo-width': 2 },
            }, before);
        }
        this.map.addLayer({ id: 'hiking-markers', type: 'symbol', source, minzoom: 14,
            filter: ['all', ['==', ['get', 'kind'], 'hiking'], ['!=', ['get', 'marker'], '']],
            layout: { 'symbol-placement': 'line', 'symbol-spacing': 500, 'icon-image': ['get', 'marker'],
                'icon-size': 0.72, 'icon-rotation-alignment': 'viewport', 'icon-padding': 24 } }, before);
        this.map.addLayer({ id: 'access-lines', type: 'line', source, filter: ['==', ['get', 'kind'], 'access'],
            paint: { 'line-color': theme === 'dark' ? '#727367' : '#aaa99e', 'line-width': 0.8,
                'line-opacity': ['match', ['get', 'status'], ['push', 'directional'], 0, 0.25] } }, before);
        this.map.addLayer({ id: 'access-symbols', type: 'symbol', source, minzoom: 12, filter: ['==', ['get', 'kind'], 'access'],
            layout: { 'symbol-placement': 'line', 'symbol-spacing': 450, 'icon-size': 0.7, 'icon-padding': 8,
                'icon-rotation-alignment': 'viewport',
                'icon-image': ['match', ['get', 'status'], 'push', `push-${theme}`, 'no_bikes', `no-bikes-${theme}`,
                    ['conditional', 'directional', 'limited'], `conditional-${theme}`, `access-${theme}`] } }, before);
        this.sync();
        void this.refresh();
    }

    set(options: OverlayOptions, mode: AccessMode = 'cycling') {
        this.hover();
        this.options = { ...options };
        this.accessMode = mode;
        this.sync();
        void this.refresh();
    }

    private sync() {
        for (const [kind, ids] of [
            ['cycling', ['network-cycling', 'network-cycling-labels']],
            ['hiking', ['network-hiking', 'network-hiking-labels', 'hiking-markers']],
            ['access', ['access-lines', 'access-symbols']],
        ] as const) {
            const shown = kind === 'access' ? this.options.access : this.options.network === kind;
            for (const id of ids) if (this.map.getLayer(id)) this.map.setLayoutProperty(id, 'visibility', shown ? 'visible' : 'none');
        }
    }

    private featureAt(event: MapMouseEvent): MapGeoJSONFeature | undefined {
        const visible = interactiveLayers.filter(id => this.map.getLayer(id) && this.map.getLayoutProperty(id, 'visibility') !== 'none');
        if (!visible.length) return;
        const { x, y } = event.point;
        const features = this.map.queryRenderedFeatures([[x - 5, y - 5], [x + 5, y + 5]], { layers: visible });
        return features.find(f => f.layer.id === 'access-symbols') ?? features.find(f => f.layer.id === 'hiking-markers') ?? features[0];
    }

    hit(event: MapMouseEvent): OverlaySelection | null {
        const feature = this.featureAt(event);
        return feature ? overlaySelection(feature, [event.lngLat.lng, event.lngLat.lat], this.displayed?.routes) : null;
    }

    hover(event?: MapMouseEvent): boolean {
        const feature = event ? this.featureAt(event) : undefined;
        const id = feature?.properties.kind === 'access' ? undefined : feature?.id;
        if (id !== this.hovered) {
            if (this.map.getSource(source)) {
                if (this.hovered !== undefined) this.map.setFeatureState({ source, id: this.hovered }, { hover: false });
                if (id !== undefined) this.map.setFeatureState({ source, id }, { hover: true });
            }
            this.hovered = id;
        }
        return !!feature;
    }

    refresh = async () => {
        const target = this.map.getSource(source) as GeoJSONSource | undefined;
        if (!target || this.disposed) return;
        this.hover();
        const selected = [this.options.network === 'none' ? '' : this.options.network, this.options.access ? 'access' : ''].filter(Boolean).join(',');
        const key = `${selected}:${this.accessMode}`;
        const zoom = Math.floor(this.map.getZoom());
        if (!selected || zoom < 6) {
            this.abort?.abort();
            this.pending = undefined;
            this.displayed = undefined;
            target.setData(empty);
            this.status(selected ? 'Zoom in to see route networks and access restrictions.' : '');
            return;
        }
        const view = this.map.getBounds();
        const b = [view.getWest(), view.getSouth(), view.getEast(), view.getNorth()];
        const cached = this.cache.find(item => item.key === key && item.zoom === zoom && contains(item.bounds, b));
        if (cached) {
            this.abort?.abort();
            this.pending = undefined;
            if (this.displayed !== cached.data) target.setData(cached.data);
            this.displayed = cached.data;
            this.cache = [cached, ...this.cache.filter(item => item !== cached)];
            this.coverageStatus(b, cached.data);
            return;
        }
        if (this.pending?.key === key && this.pending.zoom === zoom && contains(this.pending.bounds, b)) return this.pending.task;
        this.abort?.abort();
        // Stable bounds let HTTP caches reuse replies across nearby views and users.
        const step = 360 / (2 ** zoom * 8);
        const dx = (b[2] - b[0]) * 0.15, dy = (b[3] - b[1]) * 0.15;
        const bounds = [Math.max(-180, Math.floor((b[0] - dx) / step) * step), Math.max(-90, Math.floor((b[1] - dy) / step) * step),
            Math.min(180, Math.ceil((b[2] + dx) / step) * step), Math.min(90, Math.ceil((b[3] + dy) / step) * step)];
        const abort = this.abort = new AbortController();
        this.status('Loading route networks and access…');
        const task = this.load(target, bounds, zoom, selected, key, abort);
        this.pending = { bounds, zoom, key, task };
        await task;
        if (this.pending?.task === task) this.pending = undefined;
    };

    private async load(target: GeoJSONSource, bounds: number[], zoom: number, selected: string, key: string, abort: AbortController) {
        try {
            const url = `${endpoint}/v1/overlays?${new URLSearchParams({ bbox: bounds.join(','), zoom: String(zoom), layers: selected, mode: this.accessMode })}`;
            let response = await fetch(url, { signal: abort.signal });
            for (let retry = 0; response.status === 503 && retry < 2; retry++) {
                await new Promise(resolve => setTimeout(resolve, 250 * (retry + 1)));
                if (abort.signal.aborted || this.disposed) return;
                response = await fetch(url, { signal: abort.signal });
            }
            const data = await response.json() as OverlayCollection & { message?: string };
            if (!response.ok) throw new Error(data.message ?? 'Map overlays could not load.');
            if (abort.signal.aborted || this.disposed) return;
            for (const feature of data.features) {
                const p = feature.properties;
                if (p?.kind === 'hiking') {
                    const routes: NetworkRoute[] = data.routes ? p.routes.map((id: number) => data.routes![id]) : p.routes;
                    const symbol = routes?.find(route => trailMarker(route.symbol))?.symbol;
                    p.marker = symbol ? `trail:${symbol}` : '';
                }
            }
            this.cache = [{ bounds, zoom, key, data }, ...this.cache].slice(0, 6);
            this.displayed = data;
            target.setData(data);
            const view = this.map.getBounds();
            this.coverageStatus([view.getWest(), view.getSouth(), view.getEast(), view.getNorth()], data);
        } catch (error) {
            if (abort.signal.aborted || this.disposed) return;
            this.displayed = undefined;
            target.setData(empty);
            this.status(error instanceof Error ? error.message : 'Map overlays could not load.', true);
        }
    }

    private coverageStatus(view: number[], data: OverlayCollection) {
        const b = data.coverage;
        this.status(b && (view[0] > b[2] || view[2] < b[0] || view[1] > b[3] || view[3] < b[1])
            ? 'Outside the routing region. Route networks and access data are unavailable here.' : '');
    }

    destroy() {
        this.hover();
        this.disposed = true;
        this.abort?.abort();
        this.map.off('moveend', this.refresh);
    }
}
