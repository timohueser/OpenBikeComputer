import { addProtocol, type Map, type RequestParameters } from 'maplibre-gl';
import { kilometres } from '../editor';
import { TERRAIN_URL } from '../map-data';
import { DEM_MAX_ZOOM, DEM_TILE } from '../map-style';
import type { Coordinate } from '../map-types';
import { OVERVIEW, locate, type ClimateMeta } from './climate';
import { detailCell, openClimate, sampleLine, type CellRef, type ClimateSource } from './climate-source';
import { cropHeights, demPixels, demTile, pixelHeight, reliefZoom } from './climate-terrain';
import { weekOf, type DataLayer, type Line, type Theme, type View } from './data-layer';
import {
    cellBlock, mapLegend, nightLabels, paintWeather, rampWords, sampleHighs, temperatureClass, temperatureFills, tileCells,
    weatherChart, weatherYear, type Samples, type Variable,
} from './weather';

const PROTOCOL = 'obc-weather';
const TILE = 256;
// One DEM pixel per map pixel ends here; MapLibre scales the tiles beyond it.
const MAX_ZOOM = DEM_MAX_ZOOM + 1;
// A decoded DEM tile takes 1 MB; four map tiles share one.
const DEM_TILES = 16;

const palettes = Object.fromEntries((['temperature', 'rain'] as const).map(variable =>
    [variable, { light: rampWords(variable, 'light'), dark: rampWords(variable, 'dark') }])) as Record<Variable, Record<Theme, Uint32Array>>;

class WeatherLayer implements DataLayer<Samples> {
    id = 'weather';
    label = 'Weather';
    icon = 'weather';
    description = 'Daytime highs, night lows and rain from ten past years.';
    caveat = 'Valleys can be colder on clear nights.';
    meta = $state<ClimateMeta | null>(null);
    error = $state('');
    variable = $state({ label: 'Map shows', options: [{ value: 'temperature', label: 'Temperature' }, { value: 'rain', label: 'Rain' }], value: 'temperature' });
    private archive?: Promise<ClimateSource>;
    /** Decoded DEM tiles of rendered map tiles, newest last. */
    private dems = new globalThis.Map<string, Promise<Uint8ClampedArray | undefined>>();
    private map?: Map;
    private date = '';
    private theme: Theme = 'light';
    private shown = false;
    /** The week, variable and theme of the drawn tiles. */
    private drawn = '';
    private frame = 0;

    constructor(private url: string) {}

    get source() {
        const meta = this.meta;
        return meta ? `${meta.attribution} · ~9 km grid · ${meta.firstYear}–${meta.firstYear + meta.years - 1}` : '';
    }

    legend(theme: Theme) {
        return mapLegend(this.variable.value as Variable, theme);
    }

    private open(): Promise<ClimateSource> {
        this.archive ??= openClimate(this.url).then(source => {
            this.meta = source.meta;
            this.error = '';
            return source;
        }).catch(error => {
            this.archive = undefined;
            this.error = 'Weather data could not load for this region.';
            throw error;
        });
        return this.archive;
    }

    /** The pixels of a DEM tile that the map's relief has loaded, so the browser cache serves them. */
    private dem(z: number, x: number, y: number): Promise<Uint8ClampedArray | undefined> {
        const key = `${z}/${x}/${y}`;
        const entry = this.dems.get(key) ?? demPixels(TERRAIN_URL, z, x, y).catch(() => undefined);
        this.dems.delete(key);
        this.dems.set(key, entry);
        for (const old of this.dems.keys()) { if (this.dems.size <= DEM_TILES) break; this.dems.delete(old); }
        return entry;
    }

    /**
     * The height at a coordinate from the most detailed decoded DEM tile that holds it, or else from
     * the DEM tile of the view zoom, which the relief has loaded; NaN without terrain.
     */
    private async heightAt([longitude, latitude]: Coordinate): Promise<number> {
        const sin = Math.sin(latitude * Math.PI / 180);
        const pixel = (z: number) => {
            const scale = DEM_TILE * 2 ** z;
            const px = Math.floor((longitude + 180) / 360 * scale), py = Math.floor((0.5 - Math.log((1 + sin) / (1 - sin)) / (4 * Math.PI)) * scale);
            return { key: [z, Math.floor(px / DEM_TILE), Math.floor(py / DEM_TILE)] as const, column: px % DEM_TILE, row: py % DEM_TILE };
        };
        for (let z = DEM_MAX_ZOOM; z >= 0; z--) {
            const { key, column, row } = pixel(z);
            const rgba = await this.dems.get(key.join('/'));
            if (rgba) return pixelHeight(rgba, column, row);
        }
        const { key, column, row } = pixel(reliefZoom(this.map?.getZoom() ?? 0));
        const rgba = await this.dem(...key);
        return rgba ? pixelHeight(rgba, column, row) : NaN;
    }

    /** Colours one map tile for the layer week and variable. */
    private render = async (params: RequestParameters) => {
        const [z, x, y] = params.url.slice(PROTOCOL.length + 3).split('/').map(Number);
        const variable = this.variable.value as Variable, week = weekOf(this.date), palette = palettes[variable][this.theme];
        const source = await this.open();
        const cells = tileCells(z, x, y);
        // The cells whose centres surround the tile's pixels.
        const col = Math.floor(cells.cols[0]), row = Math.floor(cells.rows[0]);
        const cols = Math.floor(cells.cols[TILE - 1]) - col + 2, rows = Math.floor(cells.rows[TILE - 1]) - row + 2;
        const tiles = new globalThis.Map<string, Awaited<ReturnType<ClimateSource['tile']>>>();
        for (let r = 0; r < rows; r++) {
            for (let c = 0; c < cols; c++) {
                const { x: tx, y: ty } = locate(OVERVIEW, { col: col + c, row: row + r });
                const key = `${tx}/${ty}`;
                if (!tiles.has(key)) tiles.set(key, await source.tile(OVERVIEW, tx, ty));
            }
        }
        const cell = (c: number, r: number): CellRef | undefined => {
            const { x: tx, y: ty, index } = locate(OVERVIEW, { col: c, row: r });
            const tile = tiles.get(`${tx}/${ty}`);
            return tile && { tile, index };
        };
        const block = cellBlock(col, row, cols, rows, cell, variable, week);
        const part = demTile(z, x, y), rgba = variable === 'temperature' ? await this.dem(part.z, part.x, part.y) : undefined;
        const heights = rgba && cropHeights(rgba, part);
        const image = new ImageData(TILE, TILE);
        paintWeather(new Uint32Array(image.data.buffer), cells, block, heights, variable, palette);
        return { data: await createImageBitmap(image) };
    };

    sync(map: Map, { shown, date, theme }: View & { shown: boolean }) {
        this.date = date;
        this.theme = theme;
        this.map = map;
        this.shown = shown;
        if (map.getLayer(this.id)) {
            map.setLayoutProperty(this.id, 'visibility', shown ? 'visible' : 'none');
            // A hidden layer keeps its old tiles, so a change while hidden refreshes on show.
            const look = `${weekOf(date)} ${this.variable.value} ${theme}`;
            if (shown && look !== this.drawn) {
                this.drawn = look;
                cancelAnimationFrame(this.frame);
                this.frame = requestAnimationFrame(() => map.getSource(this.id) && map.refreshTiles(this.id));
            }
            return;
        }
        if (!shown) return;
        addProtocol(PROTOCOL, this.render);
        void this.open().then(({ bounds }) => {
            // A theme change during the request installs into the new style instead.
            if (this.map !== map || this.theme !== theme || map.getSource(this.id)) return;
            this.drawn = `${weekOf(this.date)} ${this.variable.value} ${theme}`;
            map.addSource(this.id, { type: 'raster', tiles: [`${PROTOCOL}://{z}/{x}/{y}`], tileSize: TILE, maxzoom: MAX_ZOOM, bounds, attribution: this.meta?.attribution });
            // Under the relief, so the hillshade shades the colours.
            map.addLayer({ id: this.id, type: 'raster', source: this.id, layout: { visibility: this.shown ? 'visible' : 'none' }, paint: { 'raster-fade-duration': 0 } }, map.getLayer('relief') ? 'relief' : undefined);
        }, () => {});
    }

    async sample({ coordinates, elevation }: Line, signal: AbortSignal): Promise<Samples> {
        const source = await this.open();
        // A route reads the overview; a chart or a night label reads the detail of its one sample.
        const overview = await sampleLine(coordinates, source.tile, OVERVIEW);
        // A map point has no profile height, so the rendered terrain gives it one. A route keeps its profile, so its values never depend on the view.
        const heights = coordinates.length === 1 && elevation[0] === null ? [await this.heightAt(coordinates[0])] : elevation.map(height => height ?? NaN);
        signal.throwIfAborted();
        // Not editor's cumulative: it freezes the coordinates, and a map point is a state proxy that cannot freeze.
        const km = new Float64Array(coordinates.length);
        for (let i = 1; i < km.length; i++) km[i] = km[i - 1] + kilometres(coordinates[i - 1], coordinates[i]);
        return { overview, detail: i => detailCell(source, coordinates[i]), elevation: Float32Array.from(heights), km };
    }

    strip = {
        label: 'Daytime high',
        legend: (theme: Theme) => mapLegend('temperature', theme),
        fills: temperatureFills,
        values: (samples: Samples, date: string) => Uint8Array.from(sampleHighs(samples, date), temperatureClass),
    };

    chart(samples: Samples, i: number, { date, theme }: View) {
        return weatherChart(samples, i, this.meta?.firstYear ?? 0, date, theme, this.variable.value as Variable);
    }

    year(samples: Samples | null, { date, theme }: View) {
        return weatherYear(samples, date, theme);
    }

    nights(samples: Samples, stops: { index: number; date: string }[]) {
        return nightLabels(samples, stops);
    }
}

export const weatherLayer = (url: string): DataLayer => new WeatherLayer(url);
