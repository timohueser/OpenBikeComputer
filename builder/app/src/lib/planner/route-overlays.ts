import type { ExpressionSpecification, FilterSpecification, Map, MapGeoJSONFeature, MapMouseEvent, VectorSourceSpecification } from 'maplibre-gl';
import { PMTiles } from 'pmtiles';
import type { Coordinate } from './map-types';

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
const labelZoom = 11;
const interactiveLayers = ['network-cycling', 'network-hiking', 'hiking-markers', 'access-symbols'];
const layers = {
    cycling: ['network-cycling', 'network-cycling-labels'],
    hiking: ['network-hiking', 'network-hiking-labels', 'hiking-markers'],
    access: ['access-lines', 'access-symbols'],
} as const;

/** The vector source of an overlay archive, and the routing package that the archive was baked from. */
interface Archive { source: VectorSourceSpecification; minzoom: number; bounds: number[]; routingPackage: string }

/** Reads a TileJSON URL ending in `.json`, or a PMTiles archive. */
export async function overlayArchive(url: string): Promise<Archive> {
    if (!url.endsWith('.json')) {
        const archive = new PMTiles(url);
        const [header, metadata] = await Promise.all([archive.getHeader(), archive.getMetadata() as Promise<Record<string, string>>]);
        return { source: { type: 'vector', url: `pmtiles://${url}`, attribution: metadata.attribution }, minzoom: header.minZoom,
            bounds: [header.minLon, header.minLat, header.maxLon, header.maxLat], routingPackage: metadata.routing_package };
    }
    const response = await fetch(url);
    if (!response.ok) throw new Error('Route networks and access could not load.');
    const { tiles, minzoom, maxzoom, bounds, attribution, routing_package } = await response.json();
    return { source: { type: 'vector', tiles, minzoom, maxzoom, bounds, attribution }, minzoom, bounds, routingPackage: routing_package };
}

/** Details of an overlay feature. Vector tiles carry lists as JSON text; a route line names its routes by ID. */
export function overlaySelection(feature: Pick<MapGeoJSONFeature, 'id' | 'sourceLayer' | 'properties'>, coordinate: Coordinate,
    catalog: Record<string, NetworkRoute>, mode: AccessMode): OverlaySelection {
    const p = feature.properties;
    const json = <T>(value: unknown): T | undefined => typeof value === 'string' ? JSON.parse(value) : undefined;
    return { coordinate, way: Number(feature.id), kind: feature.sourceLayer ?? '', name: p.name, ref: p.ref, status: p[`${mode}_status`],
        conditional: p.conditional, tags: json(p.tags), riding: json(p.riding), walking: json(p.walking), pushing: json(p.pushing),
        routes: json<number[]>(p.routes)?.map(id => catalog[id]).filter((route): route is NetworkRoute => !!route) };
}

export function routeWebsite(value?: string | null): string | null {
    if (!value) return null;
    try {
        const text = value.trim();
        const url = new URL(text.startsWith('www.') ? `https://${text}` : text);
        return ['https:', 'http:'].includes(url.protocol) && !url.username && !url.password ? url.href : null;
    } catch { return null; }
}

const accessStatus = (mode: AccessMode): ExpressionSpecification => ['get', `${mode}_status`];

/** Route networks and access restrictions from the overlay tiles; route edits never wait for this layer. */
export class RouteOverlays {
    private options: OverlayOptions = { network: 'none', access: false };
    private accessMode: AccessMode = 'cycling';
    private theme: 'light' | 'dark' = 'light';
    private archive?: Promise<Archive>;
    private loaded?: Archive;
    private failure = '';
    private routingPackage?: string;
    private hovered?: { sourceLayer: string; id: string | number };

    constructor(private map: Map, private url: string, private status: (message: string, retry?: boolean) => void) {
        map.on('moveend', this.report);
    }

    async install(theme: 'light' | 'dark') {
        this.theme = theme;
        if (this.map.getSource(source)) return;
        this.archive ??= overlayArchive(this.url);
        try {
            this.loaded = await this.archive;
            this.failure = '';
        } catch (error) {
            this.archive = undefined;
            this.failure = error instanceof Error ? error.message : 'Route networks and access could not load.';
            this.report();
            return;
        }
        // A theme change during the request installs into the new style instead.
        if (theme !== this.theme || this.map.getSource(source)) return;
        this.hovered = undefined;
        this.map.addSource(source, this.loaded.source);
        const before = this.map.getStyle().layers?.find(layer => layer.type === 'symbol')?.id;
        const shade = (level: number) => networkLevels[level][theme === 'dark' ? 'dark' : 'color'];
        const color: ExpressionSpecification = ['step', ['get', 'rank'], shade(3), 1, shade(2), 2, shade(1), 3, shade(0)];
        const hovered = (on: number, off: number): ExpressionSpecification => ['case', ['boolean', ['feature-state', 'hover'], false], on, off];
        for (const activity of ['cycling', 'hiking']) {
            this.map.addLayer({
                id: `network-${activity}`, type: 'line', source, 'source-layer': activity,
                layout: { 'line-join': 'round', 'line-sort-key': ['get', 'rank'] },
                paint: { 'line-color': color, 'line-opacity': hovered(1, 0.8),
                    'line-width': ['interpolate', ['linear'], ['zoom'], 6, hovered(2.4, 1.2), 10, hovered(3, 1.8), 13, hovered(4.4, 3.2), 17, hovered(6.2, 5)] },
            }, before);
            this.map.addLayer({
                id: `network-${activity}-labels`, type: 'symbol', source, 'source-layer': activity, minzoom: labelZoom,
                filter: ['!=', ['get', 'ref'], ''],
                layout: { 'symbol-placement': 'line', 'symbol-spacing': 350, 'text-field': ['get', 'ref'],
                    'text-font': ['Noto Sans Medium'], 'text-size': 11, 'text-offset': [0, 0.8], 'text-max-width': 8 },
                paint: { 'text-color': color, 'text-halo-color': theme === 'dark' ? '#181d19' : '#ffffff', 'text-halo-width': 2 },
            }, before);
        }
        this.map.addLayer({ id: 'hiking-markers', type: 'symbol', source, 'source-layer': 'hiking', minzoom: 14, filter: ['has', 'marker'],
            layout: { 'symbol-placement': 'line', 'symbol-spacing': 500, 'icon-image': ['concat', 'trail:', ['get', 'marker']],
                'icon-size': 0.72, 'icon-rotation-alignment': 'viewport', 'icon-padding': 24 } }, before);
        this.map.addLayer({ id: 'access-lines', type: 'line', source, 'source-layer': 'access',
            paint: { 'line-color': theme === 'dark' ? '#727367' : '#aaa99e', 'line-width': 0.8 } }, before);
        this.map.addLayer({ id: 'access-symbols', type: 'symbol', source, 'source-layer': 'access', minzoom: 12,
            layout: { 'symbol-placement': 'line', 'symbol-spacing': 450, 'icon-size': 0.7, 'icon-padding': 8,
                'icon-rotation-alignment': 'viewport' } }, before);
        this.sync();
    }

    retry() {
        void this.install(this.theme);
    }

    set(options: OverlayOptions, mode: AccessMode = 'cycling') {
        this.hover();
        this.options = { ...options };
        this.accessMode = mode;
        this.sync();
    }

    /** The routing package of the latest route. The router excludes what the overlay shows as closed, so both must match. */
    verify(routingPackage?: string) {
        this.routingPackage = routingPackage;
        this.sync();
    }

    private get mismatch() {
        return !!this.routingPackage && !!this.loaded && this.loaded.routingPackage !== this.routingPackage;
    }

    // A hidden layer leaves its source unused, so MapLibre requests none of its tiles.
    private sync() {
        for (const [kind, ids] of Object.entries(layers)) {
            const shown = !this.mismatch && (kind === 'access' ? this.options.access : this.options.network === kind);
            for (const id of ids) if (this.map.getLayer(id)) this.map.setLayoutProperty(id, 'visibility', shown ? 'visible' : 'none');
        }
        if (this.map.getLayer('access-lines')) {
            const status = accessStatus(this.accessMode);
            const filter: FilterSpecification = ['>=', ['zoom'], ['match', status, '', 99, 'push', 15, 'directional', 14, ['construction', 'conditional'], 10, 13]];
            for (const id of layers.access) this.map.setFilter(id, filter);
            this.map.setPaintProperty('access-lines', 'line-opacity', ['match', status, ['push', 'directional'], 0, 0.25]);
            this.map.setLayoutProperty('access-symbols', 'icon-image', ['match', status, 'push', `push-${this.theme}`, 'no_bikes', `no-bikes-${this.theme}`,
                ['conditional', 'directional', 'limited'], `conditional-${this.theme}`, `access-${this.theme}`]);
        }
        this.report();
    }

    private report = () => {
        const view = this.map.getBounds();
        const b = this.loaded?.bounds;
        if (this.options.network === 'none' && !this.options.access) this.status('');
        else if (this.failure) this.status(this.failure, true);
        else if (this.mismatch) this.status('Route networks and access come from other map data than the router.');
        else if (this.loaded && this.map.getZoom() < this.loaded.minzoom) this.status('Zoom in to see route networks and access restrictions.');
        else if (b && (view.getWest() > b[2] || view.getEast() < b[0] || view.getSouth() > b[3] || view.getNorth() < b[1])) {
            this.status('Outside the routing region. Route networks and access data are unavailable here.');
        } else this.status('');
    };

    private featureAt(event: MapMouseEvent): MapGeoJSONFeature | undefined {
        const visible = interactiveLayers.filter(id => this.map.getLayer(id) && this.map.getLayoutProperty(id, 'visibility') !== 'none');
        if (!visible.length) return;
        const { x, y } = event.point;
        const features = this.map.queryRenderedFeatures([[x - 5, y - 5], [x + 5, y + 5]], { layers: visible });
        return features.find(f => f.layer.id === 'access-symbols') ?? features.find(f => f.layer.id === 'hiking-markers') ?? features[0];
    }

    hit(event: MapMouseEvent): OverlaySelection | null {
        const feature = this.featureAt(event);
        if (!feature) return null;
        // The tile of the line holds its routes in its `routes` layer.
        const ids: number[] = typeof feature.properties.routes === 'string' ? JSON.parse(feature.properties.routes) : [];
        const routes = ids.length ? this.map.querySourceFeatures(source, { sourceLayer: 'routes', filter: ['in', ['id'], ['literal', ids]] }) : [];
        const catalog = Object.fromEntries(routes.map(route => [route.id, { ...route.properties, id: Number(route.id) } as NetworkRoute]));
        return overlaySelection(feature, [event.lngLat.lng, event.lngLat.lat], catalog, this.accessMode);
    }

    hover(event?: MapMouseEvent): boolean {
        const feature = event ? this.featureAt(event) : undefined;
        const next = feature && feature.sourceLayer !== 'access' && feature.id !== undefined ? { sourceLayer: feature.sourceLayer!, id: feature.id } : undefined;
        if (next?.id !== this.hovered?.id || next?.sourceLayer !== this.hovered?.sourceLayer) {
            if (this.map.getSource(source)) {
                if (this.hovered) this.map.setFeatureState({ source, ...this.hovered }, { hover: false });
                if (next) this.map.setFeatureState({ source, ...next }, { hover: true });
            }
            this.hovered = next;
        }
        return !!feature;
    }

    destroy() {
        this.hover();
        this.map.off('moveend', this.report);
    }
}
