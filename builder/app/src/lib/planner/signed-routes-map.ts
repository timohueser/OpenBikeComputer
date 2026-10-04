import * as maplibregl from 'maplibre-gl';
import type { ExpressionSpecification, GeoJSONSource, Map, MapMouseEvent } from 'maplibre-gl';
import type { Feature, FeatureCollection, Geometry } from 'geojson';
import type { Coordinate } from './editor';
import { networkLevels } from './route-overlays';

/** What the Routes view draws: the search circle, the listed routes as numbered lines, start dots for the other matches, and the selection. */
export interface SignedRoutesView {
    center: Coordinate;
    radiusKm: number;
    lines: { id: number; number: number; rank: number; line: Coordinate[]; label: string }[];
    dots: { id: number; rank: number; at: Coordinate }[];
    selected: { id: number; line: Coordinate[] } | null;
}

const layers = ['signed-circle', 'signed-casing', 'signed-lines', 'signed-selected-casing', 'signed-selected', 'signed-dots', 'signed-markers', 'signed-numbers'];
const interactive = ['signed-markers', 'signed-dots', 'signed-selected', 'signed-lines'];
const empty: FeatureCollection = { type: 'FeatureCollection', features: [] };
const feature = (geometry: Geometry, properties: Record<string, unknown> = {}): Feature => ({ type: 'Feature', properties, geometry });
const plain = (line: Coordinate[]) => line.map(([lon, lat]) => [lon, lat]);

function circle([lon, lat]: Coordinate, km: number): Coordinate[] {
    const dLat = km / 111.32, dLon = dLat / Math.cos(lat * Math.PI / 180);
    return Array.from({ length: 97 }, (_, i) => [lon + dLon * Math.cos(i / 48 * Math.PI), lat + dLat * Math.sin(i / 48 * Math.PI)]);
}

/** The Routes view on the map. Only matching routes draw, and only the selected route is magenta. */
export class SignedRoutesLayer {
    private view: SignedRoutesView | null = null;
    private hovered: number | null = null;
    private label?: maplibregl.Marker;

    constructor(private map: Map) {}

    /** Adds the sources and layers to the current style, on top of everything else. */
    install(theme: 'light' | 'dark') {
        if (this.map.getSource('signed-circle')) return;
        const dark = theme === 'dark';
        const shade = (level: number) => networkLevels[level][dark ? 'dark' : 'color'];
        const color: ExpressionSpecification = ['step', ['get', 'rank'], shade(3), 1, shade(2), 2, shade(1), 3, shade(0)];
        const hovered = (on: number, off: number): ExpressionSpecification => ['case', ['==', ['get', 'id'], ['global-state', 'signedHover']], on, off];
        const panel = dark ? '#201f17' : '#ffffff';
        // The page sets the route token for its theme before the map style loads.
        const magenta = getComputedStyle(this.map.getContainer()).getPropertyValue('--route').trim() || (dark ? '#f175c5' : '#cc2a93');
        const ink = dark ? '#f2efe3' : '#1c1b14';
        for (const id of ['signed-circle', 'signed-lines', 'signed-selected', 'signed-points']) this.map.addSource(id, { type: 'geojson', data: empty });
        this.map.addLayer({ id: 'signed-circle', type: 'line', source: 'signed-circle', paint: { 'line-color': ink, 'line-width': 1, 'line-opacity': .7 } });
        this.map.addLayer({ id: 'signed-casing', type: 'line', source: 'signed-lines', layout: { 'line-join': 'round', 'line-cap': 'round' },
            paint: { 'line-color': panel, 'line-width': hovered(9, 6) } });
        this.map.addLayer({ id: 'signed-lines', type: 'line', source: 'signed-lines', layout: { 'line-join': 'round', 'line-cap': 'round' },
            paint: { 'line-color': color, 'line-width': hovered(5, 3) } });
        this.map.addLayer({ id: 'signed-selected-casing', type: 'line', source: 'signed-selected', layout: { 'line-join': 'round', 'line-cap': 'round' },
            paint: { 'line-color': panel, 'line-width': 9 } });
        this.map.addLayer({ id: 'signed-selected', type: 'line', source: 'signed-selected', layout: { 'line-join': 'round', 'line-cap': 'round' },
            paint: { 'line-color': magenta, 'line-width': 5 } });
        this.map.addLayer({ id: 'signed-dots', type: 'circle', source: 'signed-points', filter: ['!', ['has', 'number']],
            paint: { 'circle-radius': hovered(6, 4), 'circle-color': color, 'circle-stroke-color': panel, 'circle-stroke-width': 1.5 } });
        const lit: ExpressionSpecification = ['any', ['boolean', ['get', 'selected'], false], ['==', ['get', 'id'], ['global-state', 'signedHover']]];
        this.map.addLayer({ id: 'signed-markers', type: 'circle', source: 'signed-points', filter: ['has', 'number'],
            paint: { 'circle-radius': 10, 'circle-stroke-width': 2,
                'circle-color': ['case', ['boolean', ['get', 'selected'], false], magenta, lit, color, panel],
                'circle-stroke-color': ['case', ['boolean', ['get', 'selected'], false], magenta, color] } });
        this.map.addLayer({ id: 'signed-numbers', type: 'symbol', source: 'signed-points', filter: ['has', 'number'],
            layout: { 'text-field': ['to-string', ['get', 'number']], 'text-font': ['Noto Sans Medium'], 'text-size': 11, 'text-allow-overlap': true, 'text-ignore-placement': true },
            paint: { 'text-color': ['case', lit, '#ffffff', ink] } });
        this.sync();
    }

    set(view: SignedRoutesView | null) {
        this.view = view;
        this.sync();
    }

    /** Highlights a route of the view: a wider line, a filled marker and a label with its name and figures. */
    hover(id: number | null) {
        this.hovered = id;
        this.showHover();
    }

    /** The route under the pointer: a marker or dot first, then a line. */
    hit(event: MapMouseEvent): number | null {
        if (!this.view) return null;
        const { x, y } = event.point;
        const shown = interactive.filter(id => this.map.getLayer(id));
        const features = this.map.queryRenderedFeatures([[x - 5, y - 5], [x + 5, y + 5]], { layers: shown });
        const found = shown.flatMap(id => features.filter(f => f.layer.id === id))[0];
        return found ? Number(found.properties.id) : null;
    }

    private sync() {
        const view = this.view;
        if (!this.map.getSource('signed-circle')) return;
        for (const id of layers) this.map.setLayoutProperty(id, 'visibility', view ? 'visible' : 'none');
        const data = (id: string, features: Feature[]) => (this.map.getSource(id) as GeoJSONSource).setData({ type: 'FeatureCollection', features });
        data('signed-circle', view ? [feature({ type: 'LineString', coordinates: circle(view.center, view.radiusKm) })] : []);
        data('signed-lines', (view?.lines ?? []).filter(route => route.id !== view?.selected?.id && route.line.length > 1)
            .map(route => feature({ type: 'LineString', coordinates: plain(route.line) }, { id: route.id, rank: route.rank })));
        data('signed-selected', view?.selected && view.selected.line.length > 1 ? [feature({ type: 'LineString', coordinates: plain(view.selected.line) }, { id: view.selected.id })] : []);
        data('signed-points', [
            ...(view?.dots ?? []).map(dot => feature({ type: 'Point', coordinates: [...dot.at] }, { id: dot.id, rank: dot.rank })),
            ...(view?.lines ?? []).map(route => feature({ type: 'Point', coordinates: [...route.line[0]] },
                { id: route.id, rank: route.rank, number: route.number, selected: route.id === view?.selected?.id })),
        ]);
        this.showHover();
    }

    private showHover() {
        if (!this.map.getSource('signed-circle')) return;
        this.map.setGlobalStateProperty('signedHover', this.hovered ?? -1);
        this.label?.remove();
        const hovered = this.view?.lines.find(route => route.id === this.hovered);
        if (!hovered) return;
        const element = Object.assign(document.createElement('div'), { className: 'planner-map-note', textContent: hovered.label });
        this.label = new maplibregl.Marker({ element, anchor: 'left', offset: [16, 0] }).setLngLat(hovered.line[0]).addTo(this.map);
    }

    destroy() {
        this.label?.remove();
    }
}
