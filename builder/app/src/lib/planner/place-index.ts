import { VectorTile } from '@mapbox/vector-tile';
import { PbfReader } from 'pbf';
import { anchorProgress, type Place } from './editor';
import { kmPerDegree, routeDistance, type Coordinate } from './geo';
import { openTileArchive, type TileArchive } from './layers/tile-archive';
import { poiKinds } from './poi-kinds';

/** Protomaps feature IDs put the OSM element type in the high bits above its 44-bit ID. */
export function osmSource(id: string | number | undefined): string | undefined {
    if (typeof id === 'string' && /^[nwr][1-9]\d*$/.test(id)) return id;
    const value = Number(id), unit = 2 ** 44, type = Math.floor(value / unit), identity = value % unit;
    return Number.isSafeInteger(value) && type >= 1 && type <= 3 && identity > 0 ? `${'nwr'[type - 1]}${identity}` : undefined;
}

/** A basemap place as a planner place, or null when the planner does not show its kind. */
export function poiPlace(id: string | number | undefined, kind: string, name: unknown, coordinate: Coordinate): Place | null {
    const known = poiKinds[kind];
    if (!known) return null;
    return {
        id: osmSource(id) ?? `poi-${id}`, kind: 'place', placeKind: kind, label: String(name ?? known.label), coordinate,
        progress: anchorProgress(coordinate), category: known.category, description: known.label,
    };
}

/** Keys `z/x/y` of the tiles at `zoom` that a square buffer of `bufferKm` around the route touches. */
export function corridorTiles(coordinates: Coordinate[], bufferKm: number, zoom: number): string[] {
    const n = 2 ** zoom;
    const column = (lon: number) => Math.floor((lon + 180) / 360 * n);
    const row = (lat: number) => {
        const rad = lat * Math.PI / 180;
        return Math.floor((1 - Math.log(Math.tan(rad) + 1 / Math.cos(rad)) / Math.PI) / 2 * n);
    };
    const keys = new Set<string>();
    const dLat = bufferKm / kmPerDegree;
    // Samples no further apart than half a box or a quarter tile leave no gap between boxes.
    const step = Math.max(1e-6, Math.min(dLat, 90 / n));
    coordinates.forEach((b, i) => {
        const a = coordinates[Math.max(0, i - 1)];
        const count = Math.max(1, Math.ceil(Math.hypot(b[0] - a[0], b[1] - a[1]) / step));
        for (let s = 0; s <= count; s++) {
            const lon = a[0] + (b[0] - a[0]) * s / count;
            const lat = a[1] + (b[1] - a[1]) * s / count;
            const dLon = bufferKm / (kmPerDegree * Math.cos(lat * Math.PI / 180));
            for (let x = column(lon - dLon); x <= column(lon + dLon); x++) {
                for (let y = row(lat + dLat); y <= row(lat - dLat); y++) keys.add(`${zoom}/${x}/${y}`);
            }
        }
    });
    return [...keys];
}

let source: Promise<TileArchive> | undefined;
const tiles = new Map<string, Promise<Place[]>>();

/** Rider places within `bufferKm` of the route, from the archive's most detailed tiles. Each tile loads once per session. */
export async function corridorPlaces(url: string, coordinates: Coordinate[], bufferKm = 5): Promise<Place[]> {
    source ??= openTileArchive(url, 'Place').catch(error => { source = undefined; throw error; });
    const archive = await source;
    // The maximum zoom sets how many tiles a corridor loads; place archives go to zoom 14 at most.
    if (!Number.isInteger(archive.maxZoom) || archive.maxZoom < 0 || archive.maxZoom > 14) throw new Error('Invalid place tile source.');
    const loaded = await Promise.all(corridorTiles(coordinates, bufferKm, archive.maxZoom).map(key => {
        // A tile that fails loads again on the next call.
        if (!tiles.has(key)) tiles.set(key, loadTile(archive, key).catch(error => { tiles.delete(key); throw error; }));
        return tiles.get(key)!;
    }));
    const distance = routeDistance(coordinates, bufferKm);
    return [...new Map(loaded.flat().map(place => [place.id, place])).values()].filter(place => Number.isFinite(distance(place.coordinate)));
}

async function loadTile(source: TileArchive, key: string): Promise<Place[]> {
    const [z, x, y] = key.split('/').map(Number);
    const tile = await source.get(z, x, y);
    const layer = tile && new VectorTile(new PbfReader(new Uint8Array(tile))).layers.pois;
    if (!layer) return [];
    const found: Place[] = [];
    for (let i = 0; i < layer.length; i++) {
        const feature = layer.feature(i);
        if (!poiKinds[String(feature.properties.kind)]) continue;
        const { geometry } = feature.toGeoJSON(x, y, z);
        if (geometry.type !== 'Point') continue;
        const place = poiPlace(feature.id, String(feature.properties.kind), feature.properties['name:en'] ?? feature.properties.name, geometry.coordinates as Coordinate);
        if (place) found.push(place);
    }
    return found;
}
