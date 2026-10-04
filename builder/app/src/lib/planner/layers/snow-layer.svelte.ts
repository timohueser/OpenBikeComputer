import type { Map } from 'maplibre-gl';
import { reliefPaint } from '../map-style';
import type { Coordinate } from '../map-types';
import { eachTile, openArchive, type Decoding } from './archive';
import { abgr, noData } from './colour';
import type { DataLayer, Line, Theme, View } from './data-layer';
import { worldPixel } from './mercator';
import { Raster } from './raster';
import { colors, paintTile, seasonDay, snowChart, snowClass, snowClasses, snowMeta, snowYear, type Planar, type SnowMeta } from './snow';

const TILE = 256;
const PIXELS = TILE * TILE;
// Beyond the archive's zoom, tiles blend their ancestor's pixels, so class borders stay smooth and the no-data hatch stays fine.
const MAX_ZOOM = 14;
const FAILED = 'Snow data could not load for this region.';

/** Decoded tiles stay cached, so a date change only recolours them and a route edit requests nothing new. */
const SNOW: Decoding<SnowMeta, Uint8Array> = {
    meta: snowMeta,
    decode(body, meta, z, x, y) {
        if (body.length !== 2 * meta.seasons * PIXELS) throw new Error(`Snow tile ${z}/${x}/${y} has ${body.length} bytes.`);
        return body;
    },
    bytes: tile => tile.byteLength,
    budgetBytes: 128 * 2 ** 20,
};

/** ImageData words by class value; index 4 is the no-data hatch. */
function palette(theme: Theme): Uint32Array {
    const c = colors[theme];
    return Uint32Array.from([0, abgr(c.mostlyFree), abgr(c.mostlySnow), abgr(c.snow), abgr(noData(theme).color)]);
}
const palettes = { light: palette('light'), dark: palette('dark') };

/** The decoded history at each coordinate of a sampled line. */
interface Samples { p: Planar; coordinates: Coordinate[] }

class SnowLayer implements DataLayer<Samples> {
    id = 'snow';
    label = 'Snow';
    icon = 'snow';
    description = 'How often past years had snow on your date.';
    caveat = 'Satellites see less snow under trees.';
    meta = $state.raw<SnowMeta | null>(null);
    error = $state('');
    private map?: Map;
    private raster = new Raster({
        id: this.id,
        extent: () => this.open().then(({ minZoom, maxZoom, bounds }) => ({ minzoom: minZoom, maxzoom: Math.max(maxZoom, MAX_ZOOM), bounds, attribution: this.source })),
        draw: (look, z, x, y, signal) => this.draw(look, z, x, y, signal),
        paint: { 'raster-resampling': 'nearest' },
        report: error => { this.error = error ? FAILED : ''; },
    });

    constructor(private url: string) {}

    get source() {
        const meta = this.meta;
        return meta ? `${meta.attribution} · ${meta.resolution} m · ${meta.firstSeason}–${meta.firstSeason + meta.seasons}` : '';
    }

    legend(theme: Theme) {
        return { swatches: snowClasses(theme).slice(1) };
    }

    private open() {
        return openArchive(this.url, SNOW).then(archive => {
            if (this.meta !== archive.meta) this.meta = archive.meta;
            return archive;
        }, error => {
            this.error = FAILED;
            throw error;
        });
    }

    /** Colours one map tile for the date and theme of a look. */
    private async draw(look: string, z: number, x: number, y: number, signal: AbortSignal) {
        const [date, theme] = look.split('/') as [string, Theme];
        const { maxZoom, meta, tile } = await this.open();
        const up = Math.max(0, z - maxZoom), scale = 2 ** up;
        const data = await tile(z - up, Math.floor(x / scale), Math.floor(y / scale), signal);
        const image = new ImageData(TILE, TILE);
        // The bake omits tiles without data, so a missing tile is drawn as no data.
        paintTile(new Uint32Array(image.data.buffer), data ? { data, size: PIXELS, seasons: meta.seasons } : undefined, seasonDay(date).index, scale, x % scale, y % scale, palettes[theme]);
        return image;
    }

    sync(map: Map, { shown, date, theme }: View & { shown: boolean }) {
        this.map = map;
        if (map.getLayer('relief')) {
            const paint = reliefPaint(theme === 'dark', shown);
            for (const key of Object.keys(paint) as (keyof typeof paint)[]) map.setPaintProperty('relief', key, paint[key]);
        }
        this.raster.sync(map, shown, `${date}/${theme}`);
    }

    async sample({ coordinates: line }: Line, signal: AbortSignal): Promise<Samples> {
        const { meta: { seasons }, maxZoom, tile } = await this.open();
        const size = line.length, data = new Uint8Array(2 * seasons * size).fill(255);
        const spots = line.map(coordinate => {
            const [px, py] = worldPixel(coordinate, TILE * 2 ** maxZoom).map(Math.floor);
            return { x: Math.floor(px / TILE), y: Math.floor(py / TILE), offset: (py % TILE) * TILE + (px % TILE) };
        });
        await eachTile(spots, (x, y) => tile(maxZoom, x, y, signal), (found, indexes) => {
            if (!found) return;
            for (const i of indexes) for (let s = 0; s < 2 * seasons; s++) data[s * size + i] = found[s * PIXELS + spots[i].offset];
        }, signal);
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
