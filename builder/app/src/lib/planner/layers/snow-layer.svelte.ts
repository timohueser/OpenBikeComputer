import { addProtocol, type Map, type RequestParameters } from 'maplibre-gl';
import { PMTiles } from 'pmtiles';
import { SNOW_URL } from '../map-data';
import { reliefPaint } from '../map-style';
import type { Coordinate } from '../map-types';
import { nearestStored, type DataLayer, type Theme } from './data-layer';
import { colors, paintTile, seasonDay, snowClass, snowClasses, snowGrid, snowMeta, snowStats, type Planar, type SnowMeta } from './snow';

const PROTOCOL = 'obc-snow';
const TILE = 256;
const PIXELS = TILE * TILE;
type Found = { data: Uint8Array; z: number };
// Decoded tiles stay cached, so a date change only recolours them.
const CACHE_BYTES = 128 * 2 ** 20;
// Beyond the archive's zoom, tiles blend their ancestor's pixels, so class borders stay smooth and the no-data hatch stays fine.
const MAX_ZOOM = 14;

interface Archive {
    meta: SnowMeta; minZoom: number; maxZoom: number; bounds: [number, number, number, number];
    /** A stored tile and its zoom; the tile service answers an omitted tile with its nearest stored ancestor. */
    get(z: number, x: number, y: number): Promise<{ data: ArrayBuffer; z: number } | undefined>;
}

/** A hosted region serves TileJSON and tiles from the tile service; a local one reads the PMTiles archive. */
async function openArchive(url: string): Promise<Archive> {
    if (new URL(url).pathname.endsWith('.json')) {
        const response = await fetch(url);
        if (!response.ok) throw new Error(`Snow TileJSON answered ${response.status}.`);
        const json = await response.json();
        const template: string = json.tiles[0];
        return {
            meta: snowMeta(json), minZoom: json.minzoom, maxZoom: json.maxzoom, bounds: json.bounds,
            async get(z, x, y) {
                const tile = await fetch(template.replace('{z}', String(z)).replace('{x}', String(x)).replace('{y}', String(y)));
                if (tile.status === 204) return undefined;
                if (!tile.ok) throw new Error(`Snow tile ${z}/${x}/${y} answered ${tile.status}.`);
                return { data: await tile.arrayBuffer(), z: Number(tile.headers.get('OBC-Tile-Zoom') ?? z) };
            },
        };
    }
    const tiles = new PMTiles(url);
    const [header, metadata] = await Promise.all([tiles.getHeader(), tiles.getMetadata() as Promise<Record<string, unknown>>]);
    return {
        meta: snowMeta(metadata), minZoom: header.minZoom, maxZoom: header.maxZoom, bounds: [header.minLon, header.minLat, header.maxLon, header.maxLat],
        get: async (z, x, y) => {
            const tile = await tiles.getZxy(z, x, y);
            return tile && { data: tile.data, z };
        },
    };
}

/** ABGR words for an ImageData view; index 4 is the no-data hatch. */
function palette(theme: Theme): Uint32Array {
    const word = (hex: string) => {
        const [r, g, b] = [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16));
        return ((255 << 24) | (b << 16) | (g << 8) | r) >>> 0;
    };
    const c = colors[theme];
    return Uint32Array.from([0, word(c.mostlyFree), word(c.mostlySnow), word(c.snow), word(c.unknown)]);
}
const palettes = { light: palette('light'), dark: palette('dark') };

/** Global pixel of a coordinate at a zoom level. */
function pixel([longitude, latitude]: Coordinate, zoom: number): [number, number] {
    const scale = TILE * 2 ** zoom;
    const sin = Math.sin(latitude * Math.PI / 180);
    return [Math.floor((longitude + 180) / 360 * scale), Math.floor((0.5 - Math.log((1 + sin) / (1 - sin)) / (4 * Math.PI)) * scale)];
}

class SnowLayer implements DataLayer<Planar> {
    id = 'snow';
    label = 'Snow';
    description = 'How often past years had snow on your date.';
    caveat = 'Satellites see less snow under trees.';
    treeNote = 'Under trees: snow often stays a little longer than shown.';
    meta = $state<SnowMeta | null>(null);
    error = $state('');
    private archive?: Promise<Archive>;
    private cache = new globalThis.Map<string, Promise<Found | undefined>>();
    /** Tiles that the archive omits. */
    private missing = new Set<string>();
    private map?: Map;
    private date = '';
    private shown = false;
    /** The date and theme of the drawn tiles. */
    private drawn = '';
    private colors = palettes.light;
    private frame = 0;

    get source() {
        const meta = this.meta;
        return meta ? `${meta.attribution} · ${meta.resolution} m · ${meta.firstSeason}–${meta.firstSeason + meta.seasons}` : '';
    }

    swatches = snowClasses;

    private open(): Promise<Archive> {
        this.archive ??= openArchive(SNOW_URL).then(archive => {
            this.meta = archive.meta;
            this.error = '';
            return archive;
        }).catch(error => {
            this.archive = undefined;
            this.error = 'Snow data could not load for this region.';
            throw error;
        });
        return this.archive;
    }

    private async fetch(z: number, x: number, y: number): Promise<Found | undefined> {
        const archive = await this.open(), meta = archive.meta;
        const tile = await archive.get(z, x, y);
        if (!tile) return undefined;
        const data = new Uint8Array(tile.data);
        if (data.length !== 2 * meta.seasons * PIXELS) throw new Error(`Snow tile ${z}/${x}/${y} has ${data.length} bytes.`);
        return { data, z: tile.z };
    }

    private async tile(z: number, x: number, y: number): Promise<Found | undefined> {
        const key = `${z}/${x}/${y}`;
        if (this.missing.has(key)) return undefined;
        const entry = this.cache.get(key) ?? this.fetch(z, x, y);
        this.cache.delete(key);
        this.cache.set(key, entry);
        const limit = Math.max(8, Math.floor(CACHE_BYTES / (2 * (this.meta?.seasons ?? 1) * PIXELS)));
        for (const old of this.cache.keys()) { if (this.cache.size <= limit) break; this.cache.delete(old); }
        try {
            const found = await entry;
            if (!found && this.cache.get(key) === entry) { this.cache.delete(key); this.missing.add(key); }
            return found;
        } catch (error) {
            if (this.cache.get(key) === entry) this.cache.delete(key);
            throw error;
        }
    }

    /**
     * The nearest stored tile at or above z/x/y and its zoom, as specs/planner-snow-tiles.md defines.
     * The PMTiles directory cache knows the omitted tiles, so the walk fetches only the tile it finds;
     * the tile service finds it on its side.
     */
    private async stored(z: number, x: number, y: number) {
        return nearestStored((z, x, y) => this.tile(z, x, y), z, x, y, (await this.open()).minZoom);
    }

    /** Colours one map tile for the layer date. The source bounds keep MapLibre from asking for tiles outside the archive. */
    private render = async (params: RequestParameters) => {
        const [z, x, y] = params.url.slice(PROTOCOL.length + 3).split('/').map(Number);
        const { maxZoom, meta } = await this.open();
        const start = Math.min(z, maxZoom);
        const tile = await this.stored(start, x >> (z - start), y >> (z - start));
        const scale = 2 ** (z - (tile?.z ?? z));
        const image = new ImageData(TILE, TILE);
        paintTile(new Uint32Array(image.data.buffer), tile && { data: tile.data, size: PIXELS, seasons: meta.seasons }, seasonDay(this.date).index, scale, x % scale, y % scale, this.colors);
        return { data: await createImageBitmap(image) };
    };

    sync(map: Map, { shown, date, theme }: { shown: boolean; date: string; theme: Theme }) {
        this.colors = palettes[theme];
        this.date = date;
        this.map = map;
        this.shown = shown;
        if (map.getLayer('relief')) {
            const paint = reliefPaint(theme === 'dark', shown);
            for (const key of Object.keys(paint) as (keyof typeof paint)[]) map.setPaintProperty('relief', key, paint[key]);
        }
        if (map.getLayer(this.id)) {
            map.setLayoutProperty(this.id, 'visibility', shown ? 'visible' : 'none');
            // A hidden layer keeps its old tiles, so a date or theme change while hidden refreshes on show.
            const look = `${date} ${theme}`;
            if (shown && look !== this.drawn) {
                this.drawn = look;
                // One refresh per frame while the date is dragged; refreshed tiles stay drawn until they are replaced.
                cancelAnimationFrame(this.frame);
                this.frame = requestAnimationFrame(() => map.getSource(this.id) && map.refreshTiles(this.id));
            }
            return;
        }
        if (!shown) return;
        addProtocol(PROTOCOL, this.render);
        const colors = this.colors;
        void this.open().then(({ minZoom, maxZoom, bounds }) => {
            // A theme change during the request installs into the new style instead.
            if (this.map !== map || this.colors !== colors || map.getSource(this.id)) return;
            this.drawn = `${this.date} ${theme}`;
            map.addSource(this.id, { type: 'raster', tiles: [`${PROTOCOL}://{z}/{x}/{y}`], tileSize: TILE, minzoom: minZoom, maxzoom: Math.max(maxZoom, MAX_ZOOM), bounds, attribution: this.source });
            // Under the relief, so the hillshade shades the snow. The layer may be hidden again while the header loads.
            map.addLayer({ id: this.id, type: 'raster', source: this.id, layout: { visibility: this.shown ? 'visible' : 'none' }, paint: { 'raster-resampling': 'nearest', 'raster-fade-duration': 0 } }, map.getLayer('relief') ? 'relief' : undefined);
        }, () => {});
    }

    async sample(line: Coordinate[], signal: AbortSignal): Promise<Planar> {
        const { meta, maxZoom } = await this.open();
        const size = line.length, seasons = meta.seasons;
        const data = new Uint8Array(2 * seasons * size).fill(255);
        const byTile = new globalThis.Map<string, number[]>();
        const pixels = line.map(coordinate => pixel(coordinate, maxZoom));
        pixels.forEach(([px, py], i) => {
            const key = `${Math.floor(px / TILE)}/${Math.floor(py / TILE)}`;
            byTile.get(key)?.push(i) ?? byTile.set(key, [i]);
        });
        // A route crosses tens of tiles at the highest zoom; fetch a few at a time. Omitted tiles share their ancestor.
        const keys = [...byTile.keys()];
        await Promise.all(Array.from({ length: 4 }, async () => {
            for (let key = keys.pop(); key; key = keys.pop()) {
                signal.throwIfAborted();
                const [x, y] = key.split('/').map(Number);
                const tile = await this.stored(maxZoom, x, y);
                if (!tile) continue;
                const up = maxZoom - tile.z;
                for (const i of byTile.get(key)!) {
                    const offset = ((pixels[i][1] >> up) % TILE) * TILE + ((pixels[i][0] >> up) % TILE);
                    for (let s = 0; s < 2 * seasons; s++) data[s * size + i] = tile.data[s * PIXELS + offset];
                }
            }
        }));
        signal.throwIfAborted();
        return { data, size, seasons };
    }

    classes(p: Planar, date: string) {
        const { index } = seasonDay(date);
        return Uint8Array.from({ length: p.size }, (_, i) => snowClass(p, i, index));
    }

    inspect(p: Planar, i: number, date: string, theme: Theme) {
        return snowGrid(p, i, this.meta?.firstSeason ?? 0, date, theme);
    }

    stats = snowStats;
}

export const snow: DataLayer = new SnowLayer();
