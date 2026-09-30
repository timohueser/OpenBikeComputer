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

/** Keys `z/x/y` of the tiles at `zoom` that a square buffer of `bufferKm` around the route touches. */
export function corridorTiles(coordinates: Coordinate[], bufferKm: number, zoom: number): string[] {
    const n = 2 ** zoom;
    const column = (lon: number) => Math.floor((lon + 180) / 360 * n);
    const row = (lat: number) => {
        const rad = lat * Math.PI / 180;
        return Math.floor((1 - Math.log(Math.tan(rad) + 1 / Math.cos(rad)) / Math.PI) / 2 * n);
    };
    const keys = new Set<string>();
    const dLat = bufferKm / 110.574;
    // Samples no further apart than half a box or a quarter tile leave no gap between boxes.
    const step = Math.max(1e-6, Math.min(dLat, 90 / n));
    coordinates.forEach((b, i) => {
        const a = coordinates[Math.max(0, i - 1)];
        const count = Math.max(1, Math.ceil(Math.hypot(b[0] - a[0], b[1] - a[1]) / step));
        for (let s = 0; s <= count; s++) {
            const lon = a[0] + (b[0] - a[0]) * s / count;
            const lat = a[1] + (b[1] - a[1]) * s / count;
            const dLon = bufferKm / (111.32 * Math.cos(lat * Math.PI / 180));
            for (let x = column(lon - dLon); x <= column(lon + dLon); x++) {
                for (let y = row(lat + dLat); y <= row(lat - dLat); y++) keys.add(`${zoom}/${x}/${y}`);
            }
        }
    });
    return [...keys];
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
// Hundreds of parallel range requests fail in the browser, so tiles load a few at a time.
const parallel = 8;
let active = 0;
const waiting: (() => void)[] = [];

async function queued<T>(task: () => Promise<T>): Promise<T> {
    if (active >= parallel) await new Promise<void>(resolve => waiting.push(resolve));
    active++;
    try {
        return await task();
    } finally {
        active--;
        waiting.shift()?.();
    }
}

/** Rider places in the archive's most detailed tiles around the route. Each tile loads once per session. */
export async function corridorPlaces(url: string, coordinates: Coordinate[], bufferKm = 5): Promise<Place[]> {
    source ??= tileSource(url).catch(error => { source = undefined; throw error; });
    const archive = await source;
    const { maxZoom } = archive;
    const loaded = await Promise.all(corridorTiles(coordinates, bufferKm, maxZoom).map(key => {
        // A dropped range request is retried once; a tile that still fails loads again on the next call.
        const load = () => loadTile(archive, key);
        if (!tiles.has(key)) tiles.set(key, queued(() => load().catch(load)).catch(error => { tiles.delete(key); throw error; }));
        return tiles.get(key)!;
    }));
    return [...new Map(loaded.flat().map(place => [place.id, place])).values()];
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
