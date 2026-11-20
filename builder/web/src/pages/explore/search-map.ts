/** The map of the day-hike search: which car parks, lifts and places it marks, the start point and its reach, the pin and
 * the place click, the box of an open row, and the start point from the browser. */
import type { Feature, FeatureCollection } from 'geojson';
import type * as maplibregl from 'maplibre-gl';
import { kmPerDegree, type Coordinate } from '../../lib/planner/geo';
import { decodeCoordinates } from '../../lib/planner/route-answer';
import type { Box } from '../../lib/planner/route-catalog';
import type { FactsPlace } from '../../lib/planner/signed-routes';
import { formatHeight } from '../../lib/settings/format';
import { ICON_PATHS } from '../../lib/ui/Icon.svelte';
import { driveOf, find, point, type Loaded, type Query, type Row, type Wish } from './search';
import { hikes, liftAt, parkAt, placeName, shortDistance } from './words';

type Place = (id: string) => FactsPlace | undefined;

/** How a marker shows: `pin` for the pinned start and a "Passes" place, `hot` on the row under the pointer or open,
 * `quiet` for a car park or lift that a row uses and a place of a wished kind that a row passes. */
export type Level = 'quiet' | 'hot' | 'pin';

/** The car park or lift of the cheapest way of a row: none for a walk from the start point. With no start point, the one
 * that the row names in "Getting there". */
function usedBy(row: Row, place: Place): string | undefined {
    const way = row.ways[0];
    switch (way.kind) {
        case 'car': return way.park;
        case 'pin': return way.place;
        case 'lift': case 'valley': return way.lift;
        case 'any': return row.shape === 'liftDown' || row.shape === 'liftUp' ? row.facts?.lift_ends?.[0] : parkAt(row.facts, place) ?? liftAt(row.facts);
        default: return undefined;
    }
}

/** The marked places and their levels. A place has the highest level that a rule gives it. */
export function markers(rows: Row[], query: Query, hot: Row | undefined, place: Place): Map<string, Level> {
    const levels = new Map<string, Level>(), wished = new Set<string>(Object.keys(query.wishes));
    const set = (id: string | undefined, level: Level) => {
        if (id && place(id)) levels.set(id, level);
    };
    for (const row of rows) {
        set(usedBy(row, place), 'quiet');
        for (const [id] of row.facts?.places ?? []) if (wished.has(place(id)?.kind ?? '')) set(id, 'quiet');
    }
    if (hot) {
        set(usedBy(hot, place), 'hot');
        for (const [id] of hot.facts?.places ?? []) set(id, 'hot');
    }
    for (const wish of Object.keys(query.wishes)) if (wish.startsWith('via:')) set(wish.slice(4), 'pin');
    set(query.pin, 'pin');
    return levels;
}

const label = (at: FactsPlace) => at.kind === 'peak' && at.ele_m !== undefined ? `${placeName(at)} ${formatHeight(at.ele_m)}` : placeName(at);

/** The ring of the points `km` from `centre`. */
function ring([lon, lat]: Coordinate, km: number): Coordinate[] {
    const dy = km / kmPerDegree, dx = dy / Math.cos(lat * Math.PI / 180);
    return Array.from({ length: 65 }, (_, i) => [lon + dx * Math.cos(i * Math.PI / 32), lat + dy * Math.sin(i * Math.PI / 32)]);
}

/** The features of the map: the markers, the cable of each marked lift, the start point and the ring of its first leg. */
export function mapData(rows: Row[], query: Query, hot: Row | undefined, place: Place): FeatureCollection {
    const features: Feature[] = [];
    const add = (role: string, geometry: Feature['geometry'], properties: Record<string, unknown> = {}) => features.push({ type: 'Feature', properties: { role, ...properties }, geometry });
    for (const [id, level] of markers(rows, query, hot, place)) {
        const at = place(id)!, top = point(at.at_udeg);
        // With no start point each row names a car park or a lift, so a wide view holds too many.
        const role = level === 'quiet' && !query.start && (at.kind === 'parking' || at.kind === 'lift') ? 'trailhead' : level;
        add(role, { type: 'Point', coordinates: top }, { id, icon: `explore-${at.kind}${level === 'pin' ? '-pin' : ''}`, level, label: level === 'quiet' ? '' : label(at) });
        if (at.kind === 'lift' && at.valley_udeg) {
            add('cable', { type: 'LineString', coordinates: [point(at.valley_udeg), top] });
            add('valley', { type: 'Point', coordinates: point(at.valley_udeg) });
        }
    }
    const pinned = query.pin ? place(query.pin) : undefined;
    if (query.start && !pinned) add('pin', { type: 'Point', coordinates: query.start }, { icon: 'explore-start', level: 'pin', label: 'Start' });
    const centre = pinned ? point(pinned.at_udeg) : query.start;
    if (centre) add('reach', { type: 'LineString', coordinates: ring(centre, pinned || !query.car ? query.walk / 1000 : driveOf(query)) });
    return { type: 'FeatureCollection', features };
}

/** The layers that take a click and show a tip: a layer for each level, so that a pin draws over a lit marker, and a lit
 * one over the others; and quiet car parks and lifts with no start point. */
export const MARKER_LAYERS = ['explore-trailhead', 'explore-quiet', 'explore-hot', 'explore-pin'];
/** A quiet car park or lift with no start point shows from this zoom: a wider view holds too many. */
const TRAILHEAD_ZOOM = 11;

/** The images of the markers: the glyph of each kind on a disc, the car park as a P on a square, and the start point. A
 * pinned one is rust. */
function markerImage(kind: FactsPlace['kind'] | 'start', pinned: boolean, colour: (name: string) => string): ImageData {
    const size = 22, ratio = 2, canvas = document.createElement('canvas');
    canvas.width = canvas.height = size * ratio;
    const context = canvas.getContext('2d')!;
    context.scale(ratio, ratio);
    const fill = kind === 'start' || pinned ? colour('--rust') : kind === 'parking' ? colour('--water') : colour('--panel');
    const ink = kind === 'start' || pinned || kind === 'parking' ? '#fff' : colour('--ink');
    context.beginPath();
    if (kind === 'parking') context.roundRect(2, 2, 18, 18, 4);
    else context.arc(11, 11, kind === 'start' ? 7 : 9, 0, Math.PI * 2);
    context.fillStyle = fill;
    context.fill();
    context.lineWidth = kind === 'start' ? 2.5 : 1.5;
    context.strokeStyle = kind === 'start' || pinned || kind === 'parking' ? '#fff' : ink;
    context.stroke();
    if (kind === 'parking') {
        context.fillStyle = ink;
        context.font = '700 11px sans-serif';
        context.textAlign = 'center';
        context.textBaseline = 'middle';
        context.fillText('P', 11, 11.5);
    } else if (kind !== 'start') {
        const ICONS = { lift: 'lift', peak: 'mountain', hut: 'hut', lake: 'lake', waterfall: 'waterfall' } as const;
        context.translate(5, 5);
        context.scale(0.5, 0.5);
        context.lineWidth = 2.6;
        context.lineCap = context.lineJoin = 'round';
        context.strokeStyle = ink;
        context.stroke(new Path2D(ICON_PATHS[ICONS[kind]]));
    }
    return context.getImageData(0, 0, canvas.width, canvas.height);
}

/** Adds the layers of the source `overlay` and their images, over the routes. `colour` reads a colour of the theme. */
export function addMarkerLayers(map: maplibregl.Map, colour: (name: string) => string) {
    for (const kind of ['parking', 'lift', 'peak', 'hut', 'lake', 'waterfall', 'start'] as const) {
        for (const pinned of kind === 'start' ? [false] : [false, true]) {
            const id = `explore-${kind}${pinned ? '-pin' : ''}`;
            if (!map.hasImage(id)) map.addImage(id, markerImage(kind, pinned, colour), { pixelRatio: 2 });
        }
    }
    const role = (name: string): maplibregl.FilterSpecification => ['==', ['get', 'role'], name];
    const ink = colour('--ink');
    map.addLayer({ id: 'explore-reach', type: 'line', source: 'overlay', filter: role('reach'),
        paint: { 'line-color': ink, 'line-width': 1.5, 'line-dasharray': [3, 3], 'line-opacity': 0.55 } });
    map.addLayer({ id: 'explore-cable', type: 'line', source: 'overlay', filter: role('cable'), layout: { 'line-cap': 'round' },
        paint: { 'line-color': ink, 'line-width': 2, 'line-dasharray': [0.1, 2.5] } });
    map.addLayer({ id: 'explore-valley', type: 'circle', source: 'overlay', filter: role('valley'),
        paint: { 'circle-radius': 3, 'circle-color': colour('--panel'), 'circle-stroke-color': ink, 'circle-stroke-width': 1.5 } });
    // A higher layer places its labels first.
    for (const id of MARKER_LAYERS) {
        map.addLayer({
            id, type: 'symbol', source: 'overlay', filter: role(id.slice(8)), minzoom: id === 'explore-trailhead' ? TRAILHEAD_ZOOM : 0,
            layout: {
                'icon-image': ['get', 'icon'], 'icon-size': ['match', ['get', 'level'], 'quiet', 0.8, 1.1], 'icon-allow-overlap': true, 'icon-ignore-placement': true,
                'text-field': ['get', 'label'], 'text-font': ['Noto Sans Medium'], 'text-size': 12, 'text-anchor': 'left', 'text-offset': [1.2, 0], 'text-optional': true,
            },
            paint: { 'text-color': ink, 'text-halo-color': colour('--panel'), 'text-halo-width': 1.5 },
        });
    }
}

/** The change of the query of a click on a marker: a car park or a top station becomes the only start, or stops being it;
 * a place becomes the must wish "Passes <name>", or stops being it. */
export function markerClick(id: string, query: Query, place: Place): Partial<Query> {
    const kind = place(id)?.kind;
    if (kind === 'parking' || kind === 'lift') return { pin: query.pin === id ? undefined : id };
    const wishes = { ...query.wishes }, wish: Wish = `via:${id}`;
    if (wishes[wish]) delete wishes[wish];
    else wishes[wish] = 'must';
    return { wishes };
}

/** The tip of a marker: its name, the hikes that a click keeps, and what the click does. */
export function markerTip(id: string, query: Query, loaded: Loaded): { title: string; text: string; action: string } | undefined {
    const at = loaded.place(id);
    if (!at) return undefined;
    if (at.kind === 'parking' || at.kind === 'lift') {
        const count = find({ ...query, pin: id }, loaded).length;
        const what = at.kind === 'lift' ? 'Top station' : at.size === 'large' ? 'Large car park' : 'Small car park';
        return { title: placeName(at), text: `${what} · ${hikes(count)} within a ${shortDistance(query.walk)} walk`, action: query.pin === id ? 'Click: unpin' : 'Click: only hikes from here' };
    }
    const on = !!query.wishes[`via:${id}`], count = find({ ...query, wishes: { ...query.wishes, [`via:${id}`]: 'must' } }, loaded).length;
    return { title: label(at), text: `${hikes(count)} ${count === 1 ? 'passes' : 'pass'} here`, action: on ? 'Click: show all hikes again' : 'Click: only hikes that pass here' };
}

/** The box of an open row: its line, and the car park, the valley station or the start point of its cheapest way. */
export function rowBox(row: Row, query: Query, place: Place): Box {
    const points: Coordinate[] = decodeCoordinates(row.record.line_udeg ?? []);
    const used = usedBy(row, place), at = used ? place(used) : undefined;
    if (at) points.push(point(at.valley_udeg && row.ways[0].kind !== 'pin' ? at.valley_udeg : at.at_udeg));
    if (row.ways[0].kind === 'foot' && query.start) points.push(query.start);
    const lons = points.map(([lon]) => lon), lats = points.map(([, lat]) => lat);
    return [Math.min(...lons), Math.min(...lats), Math.max(...lons), Math.max(...lats)];
}

/** A start point as the page address keeps it. */
export const startAt = ([lon, lat]: Coordinate): Coordinate => [Number(lon.toFixed(5)), Number(lat.toFixed(5))];

/** The position of the browser, or why it has none: the rider said no, or the browser does not know it. */
export function myLocation(): Promise<Coordinate | 'denied' | 'unknown'> {
    return new Promise(done => {
        if (!('geolocation' in navigator)) return done('unknown');
        navigator.geolocation.getCurrentPosition(
            ({ coords }) => done(startAt([coords.longitude, coords.latitude])),
            error => done(error.code === error.PERMISSION_DENIED ? 'denied' : 'unknown'),
            { timeout: 15_000, maximumAge: 300_000 });
    });
}
