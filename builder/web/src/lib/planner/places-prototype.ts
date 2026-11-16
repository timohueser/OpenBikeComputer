// PROTOTYPE for issue 3556 (places a rider can predict). Throwaway: it never merges.
//
//   ?places=today|groups|badges   today = the map of develop (default)
//   ?kinds=default|all            default = water, shop, camp, food, bike, station, peak
//   ?radius=26                    groups: two places of one kind nearer than this (px at the whole zoom) are one group
//   ?badgeradius=64               badges: places of all kinds nearer than this are one badge
//   ?split=16                     badges: from this zoom the badge separates into the groups of each kind
//   ?mix                         badges: from the split zoom, places of ALL kinds that would touch (?mixradius=30) stay one
//                                small badge, in place of the groups of each kind. Nothing then lies on another icon.
//   ?minzoom=11 ?names=16 ?margin=1 ?badgecount
import { VectorTile } from '@mapbox/vector-tile';
import type maplibregl from 'maplibre-gl';
import type { ExpressionSpecification, GeoJSONSource } from 'maplibre-gl';
import { PbfReader } from 'pbf';
import { openSource, requestSignal, type Source } from './layers/archive';
import { tileColumn, tileRow } from './place-index';
import { categoryIds, placeCategories, poiKinds, type PlaceCategory } from './poi-kinds';

const query = new URLSearchParams(typeof location === 'undefined' ? '' : location.search);
const number = (key: string, fallback: number) => (query.has(key) && Number.isFinite(Number(query.get(key))) ? Number(query.get(key)) : fallback);

export type PlacesMode = 'today' | 'groups' | 'badges';
export const placesMode: PlacesMode = query.get('places') === 'groups' ? 'groups' : query.get('places') === 'badges' ? 'badges' : 'today';
const DEFAULT_ON: PlaceCategory[] = ['water', 'shop', 'camp', 'food', 'bike', 'station', 'peak'];
/** The kinds that are on when the page opens. */
export const prototypeKinds: PlaceCategory[] = query.get('kinds') === 'all' ? categoryIds : DEFAULT_ON;
/** Which icons a badge shows first. */
const ORDER: PlaceCategory[] = [...DEFAULT_ON, ...categoryIds.filter(id => !DEFAULT_ON.includes(id))];

const RADIUS = number('radius', 26), BADGE_RADIUS = number('badgeradius', 64), SPLIT = number('split', 16);
const MIX = query.has('mix'), MIX_RADIUS = number('mixradius', 30);
const MIN_ZOOM = number('minzoom', 11), NAMES = number('names', 16), MARGIN = number('margin', 1), BADGE_COUNT = query.has('badgecount');
const MAX_ZOOM = 20;

type Properties = { kind: string; category: PlaceCategory; name: string; lon: number; lat: number };
type Feature = GeoJSON.Feature<GeoJSON.Point, Properties>;

const colors = {
    light: { panel: '#ffffff', ring: '#676443', ink: '#1c1b14' },
    dark: { panel: '#201f17', ring: '#aaa383', ink: '#f2efe3' },
};
const ratio = 2;

function pill(width: number, height: number, color: typeof colors.light) {
    const canvas = document.createElement('canvas');
    canvas.width = Math.ceil(width) * ratio;
    canvas.height = height * ratio;
    const context = canvas.getContext('2d')!;
    context.scale(ratio, ratio);
    context.lineCap = context.lineJoin = 'round';
    context.beginPath();
    context.roundRect(1, 1, Math.ceil(width) - 2, height - 2, (height - 2) / 2);
    context.fillStyle = color.panel;
    context.fill();
    context.lineWidth = 1.2;
    context.strokeStyle = color.ring;
    context.stroke();
    context.fillStyle = color.ink;
    context.textBaseline = 'middle';
    return { canvas, context };
}

function glyph(context: CanvasRenderingContext2D, category: PlaceCategory, x: number, y: number, size: number, color: string) {
    context.save();
    context.translate(x, y);
    context.scale(size / 24, size / 24);
    context.lineWidth = 1.1 * 24 / size;
    context.strokeStyle = color;
    context.stroke(new Path2D(placeCategories[category].icon));
    context.restore();
}

const font = (weight: number, size: number) => `${weight} ${size}px ${getComputedStyle(document.body).fontFamily || 'sans-serif'}`;

/** The image of a group (`proto-group-KIND-COUNT-THEME`) or a badge (`proto-badge-KIND.KIND.-COUNT-THEME`), drawn on demand. */
export function prototypeIcon(id: string): { image: ImageData; pixelRatio: number } | null {
    const match = id.match(/^proto-(group|badge)-([\w.]+)-(\d+)-(light|dark)$/);
    if (!match) return null;
    const color = colors[match[4] as 'light' | 'dark'];
    const kinds = match[2].split('.').filter(Boolean) as PlaceCategory[];
    if (kinds.some(kind => !placeCategories[kind])) return null;
    const count = match[3];
    const measure = document.createElement('canvas').getContext('2d')!;
    if (kinds.length === 1) {
        // A group: the icon of the kind and its count.
        measure.font = font(700, 11);
        const width = 21 + measure.measureText(count).width + 7;
        const { canvas, context } = pill(width, 20, color);
        glyph(context, kinds[0], 5, 4, 12, color.ink);
        context.font = font(700, 11);
        context.fillText(count, 20, 10.5);
        return { image: context.getImageData(0, 0, canvas.width, canvas.height), pixelRatio: ratio };
    }
    // A badge: the icons of the kinds of the spot, four at most, then "+N" for the other kinds.
    const shown = kinds.slice(0, 4), more = kinds.length - shown.length;
    const tail = [more ? `+${more}` : '', BADGE_COUNT ? `· ${count}` : ''].filter(Boolean).join(' ');
    measure.font = font(600, 10);
    const width = 7 + shown.length * 16 + (tail ? measure.measureText(tail).width + 5 : 0) + 4;
    const { canvas, context } = pill(width, 22, color);
    shown.forEach((kind, index) => glyph(context, kind, 7 + index * 16, 4.5, 13, color.ink));
    if (tail) {
        context.font = font(600, 10);
        context.fillText(tail, 7 + shown.length * 16 + 1, 11.5);
    }
    return { image: context.getImageData(0, 0, canvas.width, canvas.height), pixelRatio: ratio };
}

const groupSource = (category: PlaceCategory) => `proto-group-${category}`;
const BADGES = 'proto-badges', MIXED = 'proto-mixed';

export class PlacesPrototype {
    readonly mode = placesMode;
    readonly settings = { RADIUS, BADGE_RADIUS, SPLIT, MIN_ZOOM, NAMES, MARGIN };
    /** One entry for each time new place tiles arrived: how long the tiles took and how long until the map was idle. */
    readonly timings: { tiles: number; places: number; fetchMs: number; idleMs: number }[] = [];
    private source: Promise<Source> | undefined;
    private readonly tiles = new Map<string, Feature[] | null>();
    private features: Feature[] = [];
    private shown: PlaceCategory[] = prototypeKinds;
    private theme: 'light' | 'dark' = 'light';
    private installed = false;
    private readonly mixed = placesMode === 'badges' && MIX;
    /** Loads that have not yet reached the map. */
    busy = 0;

    constructor(private readonly map: maplibregl.Map, private readonly url: string) {
        Object.assign(window, { __places: this });
        if (this.mode === 'today') return;
        map.on('moveend', () => void this.load());
    }

    get layers(): string[] {
        if (this.mode === 'today' || !this.installed) return [];
        return [...(this.mode === 'badges' ? [BADGES] : []), ...(this.mixed ? [MIXED] : categoryIds.map(groupSource))];
    }

    /** Adds the sources and layers to a new style. */
    install(theme: 'light' | 'dark', shown: PlaceCategory[]) {
        if (this.mode === 'today') return;
        const { map } = this;
        this.theme = theme;
        this.shown = shown;
        const dark = theme === 'dark', panel = colors[theme].panel;
        for (const id of ['planner-pois', 'planner-poi-icons']) map.setLayoutProperty(id, 'visibility', 'none');
        const before = map.getLayer('contour-labels') ? 'contour-labels' : undefined;
        const text = { 'text-font': ['Noto Sans Regular'], 'text-size': 11, 'text-anchor': 'top', 'text-offset': [0, 0.9], 'text-optional': true } as const;
        const paint = { 'text-color': dark ? '#f2efe3' : '#1c1b14', 'text-halo-color': panel, 'text-halo-width': 1.2 };
        const name: ExpressionSpecification = ['step', ['zoom'], '', NAMES > 0 ? NAMES : 99, ['case', ['has', 'point_count'], '', ['get', 'name']]];
        const groupsFrom = this.mode === 'badges' ? SPLIT : MIN_ZOOM;
        for (const category of this.mixed ? [] : categoryIds) {
            map.addSource(groupSource(category), { type: 'geojson', data: this.collection(category), cluster: true, clusterRadius: RADIUS, clusterMaxZoom: MAX_ZOOM - 1, maxzoom: MAX_ZOOM });
            map.addLayer({
                id: groupSource(category), type: 'symbol', source: groupSource(category), minzoom: groupsFrom,
                layout: {
                    visibility: shown.includes(category) ? 'visible' : 'none',
                    'icon-image': ['case', ['has', 'point_count'], ['concat', `proto-group-${category}-`, ['to-string', ['get', 'point_count']], `-${theme}`], `poi-${category}-${theme}`],
                    'icon-allow-overlap': true, 'text-field': name, ...text,
                },
                paint,
            }, before);
        }
        if (this.mode === 'badges') {
            const has = (category: PlaceCategory): ExpressionSpecification => ['>', ['get', category], 0];
            const kinds = ['concat', ...ORDER.map(category => ['case', has(category), `${category}.`, ''])] as unknown as ExpressionSpecification;
            const add = (id: string, radius: number, clusterMaxZoom: number, zooms: { minzoom: number; maxzoom?: number }) => {
                map.addSource(id, {
                    type: 'geojson', data: this.collection(), cluster: true, clusterRadius: radius, clusterMaxZoom, maxzoom: MAX_ZOOM,
                    clusterProperties: Object.fromEntries(categoryIds.map(category => [category, ['+', ['case', ['==', ['get', 'category'], category], 1, 0]]])),
                });
                map.addLayer({
                    id, type: 'symbol', source: id, ...zooms,
                    layout: {
                        'icon-image': ['case', ['has', 'point_count'],
                            ['concat', 'proto-badge-', kinds, '-', ['to-string', ['get', 'point_count']], `-${theme}`],
                            ['concat', 'poi-', ['get', 'category'], `-${theme}`]],
                        'icon-allow-overlap': true, 'text-field': name, ...text,
                    },
                    paint,
                }, before);
            };
            add(BADGES, BADGE_RADIUS, SPLIT - 1, { minzoom: MIN_ZOOM, maxzoom: SPLIT });
            if (this.mixed) add(MIXED, MIX_RADIUS, MAX_ZOOM - 1, { minzoom: SPLIT });
        }
        this.installed = true;
        void this.load();
    }

    setKinds(shown: PlaceCategory[]) {
        this.shown = shown;
        if (!this.installed) return;
        for (const category of categoryIds) if (this.map.getLayer(groupSource(category))) this.map.setLayoutProperty(groupSource(category), 'visibility', shown.includes(category) ? 'visible' : 'none');
        for (const id of [BADGES, MIXED]) (this.map.getSource(id) as GeoJSONSource | undefined)?.setData(this.collection());
    }

    /** A click on a group or a badge zooms in until it separates. True when the click hit one. */
    click(point: maplibregl.Point): boolean {
        if (!this.layers.length) return false;
        const feature = this.map.queryRenderedFeatures([[point.x - 6, point.y - 6], [point.x + 6, point.y + 6]], { layers: this.layers }).find(feature => feature.properties.cluster);
        if (!feature || feature.geometry.type !== 'Point') return false;
        const center = feature.geometry.coordinates as [number, number];
        void (this.map.getSource(feature.source) as GeoJSONSource).getClusterExpansionZoom(Number(feature.properties.cluster_id))
            .then(zoom => this.map.easeTo({ center, zoom: Math.max(zoom, this.map.getZoom() + 1) }));
        return true;
    }

    private collection(category?: PlaceCategory): GeoJSON.FeatureCollection<GeoJSON.Point, Properties> {
        return { type: 'FeatureCollection', features: this.features.filter(feature => category ? feature.properties.category === category : this.shown.includes(feature.properties.category)) };
    }

    /** Loads the place tiles of the view and a margin of whole tiles around it. Loaded tiles stay. */
    private async load() {
        if (!this.installed || this.map.getZoom() < MIN_ZOOM - 1) return;
        const start = performance.now();
        this.source ??= openSource(this.url);
        const source = await this.source, zoom = source.maxZoom, bounds = this.map.getBounds();
        const [west, south, east, north] = source.bounds;
        const clamp = (value: number, low: number, high: number) => Math.max(low, Math.min(high, value));
        const x0 = clamp(Math.floor(tileColumn(bounds.getWest(), zoom)) - MARGIN, Math.floor(tileColumn(west, zoom)), Math.floor(tileColumn(east, zoom)));
        const x1 = clamp(Math.floor(tileColumn(bounds.getEast(), zoom)) + MARGIN, Math.floor(tileColumn(west, zoom)), Math.floor(tileColumn(east, zoom)));
        const y0 = clamp(Math.floor(tileRow(bounds.getNorth(), zoom)) - MARGIN, Math.floor(tileRow(north, zoom)), Math.floor(tileRow(south, zoom)));
        const y1 = clamp(Math.floor(tileRow(bounds.getSouth(), zoom)) + MARGIN, Math.floor(tileRow(north, zoom)), Math.floor(tileRow(south, zoom)));
        const missing: string[] = [];
        for (let x = x0; x <= x1; x++) for (let y = y0; y <= y1; y++) if (!this.tiles.has(`${zoom}/${x}/${y}`)) missing.push(`${zoom}/${x}/${y}`);
        if (!missing.length) return;
        this.busy++;
        for (const key of missing) this.tiles.set(key, null);
        await Promise.all(missing.map(async key => {
            try { this.tiles.set(key, await readTile(source, key)); } catch (error) { this.tiles.delete(key); console.warn('places prototype: tile', key, error); }
        }));
        const fetchMs = performance.now() - start;
        // One list of all loaded places, in an order that does not depend on which tile arrived first.
        const all = new Map<number, Feature>();
        for (const tile of this.tiles.values()) for (const feature of tile ?? []) all.set(feature.id as number, feature);
        this.features = [...all.values()].sort((a, b) => (a.id as number) - (b.id as number));
        if (!this.map.getSource(this.layers[0])) { this.busy--; return; }
        for (const category of categoryIds) (this.map.getSource(groupSource(category)) as GeoJSONSource | undefined)?.setData(this.collection(category));
        for (const id of [BADGES, MIXED]) (this.map.getSource(id) as GeoJSONSource | undefined)?.setData(this.collection());
        this.map.once('idle', () => {
            this.busy--;
            const timing = { tiles: missing.length, places: this.features.length, fetchMs: Math.round(fetchMs), idleMs: Math.round(performance.now() - start) };
            this.timings.push(timing);
            console.info('places prototype:', JSON.stringify(timing));
        });
    }

    // What follows is for the pictures and the measurements.

    get loadedTiles(): string[] { return [...this.tiles.keys()].sort(); }

    /** The places of the loaded area of each kind. */
    stats(): Record<string, number> {
        const counts: Record<string, number> = { total: this.features.length };
        for (const category of categoryIds) counts[category] = this.features.filter(feature => feature.properties.category === category).length;
        return counts;
    }

    /** What the prototype layers draw in the view: each icon, group or badge with its position and the number of its places. */
    drawn(): { layer: string; lon: number; lat: number; count: number; kinds: string; x: number; y: number }[] {
        return this.map.queryRenderedFeatures({ layers: this.layers }).filter(feature => feature.geometry.type === 'Point').map(feature => {
            const [lon, lat] = (feature.geometry as GeoJSON.Point).coordinates, at = this.map.project([lon, lat]), properties = feature.properties;
            return {
                layer: feature.layer.id, lon: Math.round(lon * 1e5) / 1e5, lat: Math.round(lat * 1e5) / 1e5, count: Number(properties.point_count ?? 1),
                kinds: properties.cluster && (feature.layer.id === BADGES || feature.layer.id === MIXED) ? ORDER.filter(category => properties[category] > 0).join('.') : String(properties.category ?? feature.layer.id.slice(12)),
                x: Math.round(at.x), y: Math.round(at.y),
            };
        });
    }
}

async function readTile(source: Source, key: string): Promise<Feature[]> {
    const [z, x, y] = key.split('/').map(Number);
    const tile = await source.get(z, x, y, requestSignal());
    const layer = tile && new VectorTile(new PbfReader(new Uint8Array(tile))).layers.pois;
    if (!layer) return [];
    const found: Feature[] = [];
    for (let i = 0; i < layer.length; i++) {
        const feature = layer.feature(i), properties = feature.properties, kind = String(properties.kind);
        if (!poiKinds[kind]) continue;
        const lon = Number(properties.lon), lat = Number(properties.lat);
        found.push({
            type: 'Feature', id: feature.id, geometry: { type: 'Point', coordinates: [lon, lat] },
            properties: { kind, category: poiKinds[kind].category, name: String(properties['name:en'] ?? properties.name ?? ''), lon, lat },
        });
    }
    return found;
}
