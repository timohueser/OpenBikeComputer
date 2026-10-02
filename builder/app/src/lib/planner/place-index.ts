import { PMTiles } from 'pmtiles';
import { VectorTile } from '@mapbox/vector-tile';
import { PbfReader } from 'pbf';
import { anchorProgress, type Coordinate, type Place } from './editor';
import { poiKinds } from './poi-kinds';

/** A basemap place as a planner place, or null when the planner does not show its kind. */
export function poiPlace(id: string | number | undefined, kind: string, name: unknown, coordinate: Coordinate): Place | null {
    const known = poiKinds[kind];
    if (!known) return null;
    return {
        id: `poi-${id}`, kind: 'place', label: String(name ?? known.label), coordinate,
        progress: anchorProgress(coordinate), category: known.category, description: known.label,
    };
}

// The sphere of `kilometres`, so the corridor and the route distance measure as the rest of the planner does.
const kmPerDegree = 6371 * Math.PI / 180;

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

/**
 * Distance in km from a point to the route line, or Infinity beyond `km`. Each piece of the route has a
 * bounding box, so a point skips far pieces at once. A flat projection around the point is exact enough at this scale.
 */
export function routeDistance(coordinates: Coordinate[], km: number): (point: Coordinate) => number {
    const pieces: { line: Coordinate[]; box: number[] }[] = [];
    for (let i = 0; i < coordinates.length; i += 64) {
        const line = coordinates.slice(Math.max(0, i - 1), i + 64);
        const lons = line.map(c => c[0]), lats = line.map(c => c[1]);
        pieces.push({ line, box: [Math.min(...lons), Math.min(...lats), Math.max(...lons), Math.max(...lats)] });
    }
    return ([lon, lat]) => {
        const kx = kmPerDegree * Math.cos(lat * Math.PI / 180), ky = kmPerDegree;
        const dLon = km / kx, dLat = km / ky;
        let nearest = Infinity;
        for (const { line, box } of pieces) {
            if (lon < box[0] - dLon || box[2] + dLon < lon || lat < box[1] - dLat || box[3] + dLat < lat) continue;
            line.forEach((b, i) => {
                const a = line[Math.max(0, i - 1)];
                const ax = (a[0] - lon) * kx, ay = (a[1] - lat) * ky, dx = (b[0] - a[0]) * kx, dy = (b[1] - a[1]) * ky;
                const t = Math.max(0, Math.min(1, -(ax * dx + ay * dy) / (dx * dx + dy * dy || 1)));
                nearest = Math.min(nearest, (ax + t * dx) ** 2 + (ay + t * dy) ** 2);
            });
        }
        return nearest <= km * km ? Math.sqrt(nearest) : Infinity;
    };
}

type TileSource = { maxZoom: number; getZxy: (z: number, x: number, y: number) => Promise<{ data: ArrayBuffer } | undefined> };
let source: Promise<TileSource> | undefined;
async function tileSource(url: string): Promise<TileSource> {
    if (!url.endsWith('.json')) {
        const archive = new PMTiles(url);
        return { maxZoom: (await archive.getHeader()).maxZoom, getZxy: (z, x, y) => archive.getZxy(z, x, y) };
    }
    const response = await fetch(url);
    if (!response.ok) throw new Error('Place tiles are unavailable.');
    const info = await response.json();
    if (!Number.isInteger(info.maxzoom) || info.maxzoom < 0 || info.maxzoom > 14 || !info.tiles?.[0]) throw new Error('Invalid place tile source.');
    return { maxZoom: info.maxzoom, async getZxy(z, x, y) {
        const response = await fetch(info.tiles[0].replace('{z}', z).replace('{x}', x).replace('{y}', y));
        if (response.status === 204) return undefined;
        if (!response.ok) throw new Error('Place tiles are unavailable.');
        return { data: await response.arrayBuffer() };
    } };
}
const tiles = new Map<string, Promise<Place[]>>();

/** Rider places within `bufferKm` of the route, from the archive's most detailed tiles. Each tile loads once per session. */
export async function corridorPlaces(url: string, coordinates: Coordinate[], bufferKm = 5): Promise<Place[]> {
    source ??= tileSource(url).catch(error => { source = undefined; throw error; });
    const archive = await source;
    const loaded = await Promise.all(corridorTiles(coordinates, bufferKm, archive.maxZoom).map(key => {
        // A tile that fails loads again on the next call.
        if (!tiles.has(key)) tiles.set(key, loadTile(archive, key).catch(error => { tiles.delete(key); throw error; }));
        return tiles.get(key)!;
    }));
    const distance = routeDistance(coordinates, bufferKm);
    return [...new Map(loaded.flat().map(place => [place.id, place])).values()].filter(place => distance(place.coordinate) <= bufferKm);
}

async function loadTile(source: TileSource, key: string): Promise<Place[]> {
    const [z, x, y] = key.split('/').map(Number);
    const tile = await source.getZxy(z, x, y);
    const layer = tile && new VectorTile(new PbfReader(new Uint8Array(tile.data))).layers.pois;
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
