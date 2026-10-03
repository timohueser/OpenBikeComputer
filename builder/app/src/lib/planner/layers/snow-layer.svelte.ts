import { addProtocol, type Map, type RequestParameters } from 'maplibre-gl';
import { reliefPaint } from '../map-style';
import type { Coordinate } from '../map-types';
import type { DataLayer, Line, Theme, View } from './data-layer';
import { openTileArchive, type TileArchive } from './tile-archive';
import { colors, paintTile, seasonDay, snowChart, snowClass, snowClasses, snowMeta, snowYear, type Planar, type SnowMeta } from './snow';

const PROTOCOL = 'obc-snow';
const TILE = 256;
const PIXELS = TILE * TILE;
// Decoded tiles stay cached, so a date change only recolours them.
const CACHE_BYTES = 128 * 2 ** 20;
// Beyond the archive's zoom, tiles blend their ancestor's pixels, so class borders stay smooth and the no-data hatch stays fine.
const MAX_ZOOM = 14;

type Archive = TileArchive & { meta: SnowMeta };

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

/** The decoded history at each coordinate of a sampled line. */
interface Samples { p: Planar; coordinates: Coordinate[] }

class SnowLayer implements DataLayer<Samples> {
    id = 'snow';
    label = 'Snow';
    icon = 'snow';
    description = 'How often past years had snow on your date.';
    caveat = 'Satellites see less snow under trees.';
    meta = $state<SnowMeta | null>(null);
    error = $state('');
    private archive?: Promise<Archive>;
    private cache = new globalThis.Map<string, Promise<Uint8Array | undefined>>();
    private map?: Map;
    private date = '';
    private shown = false;
    /** The date and theme of the drawn tiles. */
    private drawn = '';
    private colors = palettes.light;
    private frame = 0;

    constructor(private url: string) {}

    get source() {
        const meta = this.meta;
        return meta ? `${meta.attribution} · ${meta.resolution} m · ${meta.firstSeason}–${meta.firstSeason + meta.seasons}` : '';
    }

    legend(theme: Theme) {
        return { swatches: snowClasses(theme).slice(1) };
    }

    private open(): Promise<Archive> {
        this.archive ??= openTileArchive(this.url, 'Snow').then(opened => {
            const archive = { ...opened, meta: snowMeta(opened.metadata) };
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

    private async fetch(z: number, x: number, y: number, signal?: AbortSignal): Promise<Uint8Array | undefined> {
        const archive = await this.open(), meta = archive.meta;
        const tile = await archive.get(z, x, y, signal);
        if (!tile) return undefined;
        const data = new Uint8Array(tile);
        if (data.length !== 2 * meta.seasons * PIXELS) throw new Error(`Snow tile ${z}/${x}/${y} has ${data.length} bytes.`);
        return data;
    }

    private tile(z: number, x: number, y: number): Promise<Uint8Array | undefined> {
        const key = `${z}/${x}/${y}`;
        const entry = this.cache.get(key) ?? this.fetch(z, x, y);
        this.cache.delete(key);
        this.cache.set(key, entry);
        entry.catch(() => { if (this.cache.get(key) === entry) this.cache.delete(key); });
        const limit = Math.max(8, Math.floor(CACHE_BYTES / (2 * (this.meta?.seasons ?? 1) * PIXELS)));
        for (const old of this.cache.keys()) { if (this.cache.size <= limit) break; this.cache.delete(old); }
        return entry;
    }

    /** Colours one map tile for the layer date. The source bounds keep MapLibre from asking for tiles outside the archive. */
    private render = async (params: RequestParameters) => {
        const [z, x, y] = params.url.slice(PROTOCOL.length + 3).split('/').map(Number);
        const { maxZoom, meta } = await this.open();
        const up = Math.max(0, z - maxZoom), scale = 2 ** up;
        const data = await this.tile(z - up, Math.floor(x / scale), Math.floor(y / scale));
        const image = new ImageData(TILE, TILE);
        // The bake omits tiles without data, so a missing tile is drawn as no data.
        paintTile(new Uint32Array(image.data.buffer), data && { data, size: PIXELS, seasons: meta.seasons }, seasonDay(this.date).index, scale, x % scale, y % scale, this.colors);
        return { data: await createImageBitmap(image) };
    };

    sync(map: Map, { shown, date, theme }: View & { shown: boolean }) {
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

    async sample({ coordinates: line }: Line, signal: AbortSignal): Promise<Samples> {
        const { meta, maxZoom } = await this.open();
        const size = line.length, seasons = meta.seasons;
        const data = new Uint8Array(2 * seasons * size).fill(255);
        const byTile = new globalThis.Map<string, number[]>();
        const offsets = new Int32Array(size);
        line.forEach((coordinate, i) => {
            const [px, py] = pixel(coordinate, maxZoom);
            offsets[i] = (py % TILE) * TILE + (px % TILE);
            const key = `${Math.floor(px / TILE)}/${Math.floor(py / TILE)}`;
            byTile.get(key)?.push(i) ?? byTile.set(key, [i]);
        });
        // A route crosses tens of tiles at the highest zoom; fetch a few at a time and keep only the samples.
        const keys = [...byTile.keys()];
        await Promise.all(Array.from({ length: 4 }, async () => {
            for (let key = keys.pop(); key; key = keys.pop()) {
                const [x, y] = key.split('/').map(Number);
                const tile = await this.fetch(maxZoom, x, y, signal);
                if (!tile) continue;
                for (const i of byTile.get(key)!) {
                    for (let s = 0; s < 2 * seasons; s++) data[s * size + i] = tile[s * PIXELS + offsets[i]];
                }
            }
        }));
        signal.throwIfAborted();
        return { p: { data, size, seasons }, coordinates: line };
    }

    strip = {
        legend: (theme: Theme) => ({ swatches: snowClasses(theme) }),
        fills: snowClasses,
        values({ p }: Samples, date: string) {
            const { index } = seasonDay(date);
            return Uint8Array.from({ length: p.size }, (_, i) => snowClass(p, i, index));
        },
    };

    chart({ p, coordinates }: Samples, i: number, { date, theme }: View) {
        const chart = snowChart(p, i, this.meta?.firstSeason ?? 0, date, theme);
        return this.underTrees(coordinates[i]) ? { ...chart, note: 'Under trees: snow often stays a little longer than shown.' } : chart;
    }

    year(samples: Samples | null, { date, theme }: View) {
        return snowYear(samples?.p ?? null, date, theme);
    }

    /** Whether the basemap draws forest at a coordinate on screen; false off screen. */
    private underTrees(coordinate: Coordinate): boolean {
        const map = this.map;
        if (!map || !map.getBounds().contains(coordinate)) return false;
        const layers = ['landcover', 'land-ground'].filter(id => map.getLayer(id));
        return map.queryRenderedFeatures(map.project(coordinate), { layers }).some(feature => ['forest', 'wood'].includes(feature.properties.kind));
    }
}

export const snowLayer = (url: string): DataLayer => new SnowLayer(url);
