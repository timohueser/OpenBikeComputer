import type { Map } from 'maplibre-gl';
import { reliefPaint } from '../map-style';
import type { Coordinate } from '../map-types';
import { eachTile, openArchive, type Decoding } from './archive';
import { abgr } from './colour';
import type { DataLayer, Line, Theme, View } from './data-layer';
import { worldPixel } from './mercator';
import { Raster } from './raster';
import {
    covering, paintTile, routeZoom, RUN_TILE, runImages, seasonDay, seasonsTile, snowBand, snowChart, snowClass, snowClasses, snowMeta, snowYear,
    type Planar, type Runs, type SnowMeta,
} from './snow';
import { imagePixels } from './terrain';

const TILE = 256;
const RUN_PIXELS = RUN_TILE * RUN_TILE;
const SEASON_TILE = 64, SEASON_PIXELS = SEASON_TILE * SEASON_TILE;
// Above the data zoom, a map tile draws its part of a run tile by the bilinear rule of `paintTile`, so class borders stay smooth and the hatch stays fine.
const MAX_ZOOM = 14;
const FAILED = 'Snow data could not load for this region.';

/**
 * The runs of a `snow` tile (1 image) or a `snow-levels` tile (4 images): red is the first band, green
 * the last. Decoded tiles stay cached, so a date change only recolours them.
 */
function runs(images: number, budgetBytes: number): Decoding<SnowMeta, Runs[]> {
    return {
        meta: snowMeta,
        async decode(body, _meta, z, x, y) {
            const parts = runImages(body);
            if (parts.length !== images) throw new Error(`Snow tile ${z}/${x}/${y} has ${parts.length} images.`);
            return Promise.all(parts.map(async part => {
                const { data, width, height } = await imagePixels(part);
                if (width !== RUN_TILE || height !== RUN_TILE) throw new Error(`Snow tile ${z}/${x}/${y} is ${width} × ${height} px.`);
                const first = new Uint8Array(RUN_PIXELS), last = new Uint8Array(RUN_PIXELS);
                for (let i = 0; i < RUN_PIXELS; i++) {
                    first[i] = data[4 * i];
                    last[i] = data[4 * i + 1];
                }
                return { first, last };
            }));
        },
        bytes: levels => 2 * RUN_PIXELS * levels.length,
        budgetBytes,
    };
}
const SNOW = runs(1, 32 * 2 ** 20), LEVELS = runs(4, 64 * 2 ** 20);

const SEASONS: Decoding<SnowMeta, Uint8Array> = {
    meta: snowMeta,
    decode(body, meta, z, x, y) {
        if (body.length !== 2 * meta.seasons * SEASON_PIXELS) throw new Error(`Snow tile ${z}/${x}/${y} has ${body.length} bytes.`);
        return body;
    },
    bytes: tile => tile.byteLength,
    budgetBytes: 16 * 2 ** 20,
};

/** ImageData words by class value: the first, no snow in any year, stays clear, and the last is the no-data hatch. */
const palette = (theme: Theme) => Uint32Array.from(snowClasses(theme), ({ color }, value) => value ? abgr(color) : 0);
const palettes = { light: palette('light'), dark: palette('dark') };

/** The seasons at each coordinate of a sampled line, from the first season of `snow-seasons`. */
interface Samples { p: Planar; coordinates: Coordinate[]; firstSeason: number }

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

    constructor(private snowUrl: string, private levelsUrl: string, private seasonsUrl: string) {}

    get source() {
        const meta = this.meta;
        return meta ? `${meta.attribution} · ${meta.resolution} m · ${meta.firstSeason}–${meta.firstSeason + meta.seasons}` : '';
    }

    legend(theme: Theme) {
        return { swatches: snowClasses(theme).slice(1) };
    }

    private archive<M, T>(url: string, decoding: Decoding<M, T>) {
        return openArchive(url, decoding).catch(error => {
            this.error = FAILED;
            throw error;
        });
    }

    /** The `snow` archive, whose metadata is the layer's. */
    private open() {
        return this.archive(this.snowUrl, SNOW).then(archive => {
            if (this.meta !== archive.meta) this.meta = archive.meta;
            return archive;
        });
    }

    /** Colours one map tile for the date and theme of a look. */
    private async draw(look: string, z: number, x: number, y: number, signal: AbortSignal) {
        const [date, theme] = look.split('/') as [string, Theme];
        const [snow, levels] = await Promise.all([this.open(), this.archive(this.levelsUrl, LEVELS)]);
        // A 512 px run tile holds the four map tiles of the zoom below it.
        const { data: half, zoom } = await covering((z, x, y) => snow.tile(z, x, y, signal), Math.min(Math.max(z - 1, 0), snow.maxZoom), z, x, y);
        const parts = 2 ** (z - zoom);
        const others = half && await levels.tile(zoom, Math.floor(x / parts), Math.floor(y / parts), signal);
        const image = new ImageData(TILE, TILE);
        paintTile(new Uint32Array(image.data.buffer), half && others ? [others[0], others[1], half[0], others[2], others[3]] : undefined, snowBand(seasonDay(date).index), parts, x % parts, y % parts, palettes[theme]);
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

    /** The samples read the finest zoom at which they fall in at most 25 tiles, so a single point reads the max zoom. */
    async sample({ coordinates: line }: Line, signal: AbortSignal): Promise<Samples> {
        const { meta: { seasons, firstSeason }, minZoom, maxZoom, tile } = await this.archive(this.seasonsUrl, SEASONS);
        const zoom = routeZoom(line, minZoom, maxZoom), size = line.length, data = new Uint8Array(2 * seasons * size).fill(255);
        const spots = line.map(coordinate => {
            const [px, py] = worldPixel(coordinate, SEASON_TILE * 2 ** zoom).map(Math.floor);
            return { x: Math.floor(px / SEASON_TILE), y: Math.floor(py / SEASON_TILE), px, py };
        });
        await eachTile(spots, (x, y) => seasonsTile((z, x, y) => tile(z, x, y, signal), zoom, x, y), (found, indexes) => {
            if (!found.data) return;
            const scale = 2 ** (zoom - found.zoom);
            for (const i of indexes) {
                const px = Math.floor(spots[i].px / scale), py = Math.floor(spots[i].py / scale), offset = (py % SEASON_TILE) * SEASON_TILE + (px % SEASON_TILE);
                for (let s = 0; s < 2 * seasons; s++) data[s * size + i] = found.data[s * SEASON_PIXELS + offset];
            }
        }, signal);
        return { p: { data, size, seasons }, coordinates: line, firstSeason };
    }

    strip = {
        legend: (theme: Theme) => ({ swatches: snowClasses(theme) }),
        fills: snowClasses,
        values({ p }: Samples, date: string) {
            const band = snowBand(seasonDay(date).index);
            return Uint8Array.from({ length: p.size }, (_, i) => snowClass(p, i, band));
        },
    };

    chart({ p, coordinates, firstSeason }: Samples, i: number, { date, theme }: View) {
        const chart = snowChart(p, i, firstSeason, date, theme);
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

export const snowLayer = (snow: string, levels: string, seasons: string): DataLayer => new SnowLayer(snow, levels, seasons);
