import type { ExpressionSpecification, GeoJSONSource, Map, SymbolLayerSpecification } from 'maplibre-gl';
import { cumulative } from '../geo';
import { TERRAIN_URL } from '../map-data';
import { DEM_MAX_ZOOM } from '../map-style';
import type { Coordinate } from '../map-types';
import { fetchTile } from './archive';
import { OVERVIEW, locate, type ClimateMeta, type ClimateTile } from './climate';
import { detailCell, openClimate, sampleLine, type CellRef, type ClimateSource } from './climate-source';
import { cropHeights, demPixel, demTile, metres, reliefZoom } from './climate-terrain';
import { weekOf, type DataLayer, type Line, type Theme, type View } from './data-layer';
import { Raster } from './raster';
import { terrainHeights } from './terrain';
import {
    cellBlock, highAt, mapDegrees, mapLegend, mapPlaces, nightLabels, paintWeather, placeLabels, rainAt, rainRange, rampWords, sampleHighs,
    stripClasses, stripFills, stripScale, tileCells, weatherChart, weatherYear, type BasemapFeature, type Samples, type Variable,
} from './weather';

const TILE = 256;
// One DEM pixel per map pixel ends here; MapLibre scales the tiles beyond it.
const MAX_ZOOM = DEM_MAX_ZOOM + 1;
/** The labels of the layer replace the basemap's town labels while they show. */
const LABELS = 'weather-labels', TOWNS = 'places_locality';
/** From this zoom the planner marks peaks with its own symbol and name. */
const PEAK_SYMBOLS = 13;
const FAILED = 'Weather data could not load for this region.';

/** The map's terrain tiles; the relief has loaded most of them, so the browser cache serves them. */
const terrain = terrainHeights((z, x, y) => fetchTile(TERRAIN_URL, z, x, y));

const palettes = Object.fromEntries((['temperature', 'rain'] as const).map(variable =>
    [variable, { light: rampWords(variable, 'light'), dark: rampWords(variable, 'dark') }])) as Record<Variable, Record<Theme, Uint32Array>>;

/** The town label layer of the basemap with the layer's values beside the names, and named peaks with the planner's peak symbol. */
function labelLayer(map: Map, theme: Theme): SymbolLayerSpecification {
    const town = <K extends 'text-size' | 'text-font' | 'text-padding'>(name: K) => map.getLayoutProperty(TOWNS, name) as NonNullable<SymbolLayerSpecification['layout']>[K];
    const peak: ExpressionSpecification = ['get', 'peak'];
    return {
        id: LABELS, type: 'symbol', source: LABELS,
        filter: ['any', ['!', peak], ['<', ['zoom'], PEAK_SYMBOLS]],
        layout: {
            'text-field': ['case', ['has', 'value'], ['format', ['get', 'name'], {}, '  ', {}, ['get', 'value'], { 'font-scale': 1.1 }], ['get', 'name']],
            'text-size': town('text-size'), 'text-font': town('text-font'), 'text-padding': town('text-padding'), 'text-max-width': 14,
            'text-anchor': ['case', peak, 'left', 'center'], 'text-offset': ['case', peak, ['literal', [0.8, 0]], ['literal', [0, 0]]],
            'icon-image': ['case', peak, `poi-peak-${theme}`, ''], 'icon-size': 0.8,
            'symbol-sort-key': ['get', 'sort'],
        },
        paint: { 'text-color': map.getPaintProperty(TOWNS, 'text-color'), 'text-halo-color': map.getPaintProperty(TOWNS, 'text-halo-color'), 'text-halo-width': 1.2 },
    };
}

/**
 * The height at a coordinate from the most detailed decoded terrain tile that holds it. With `load`, a
 * coordinate outside them reads the terrain tile of the view zoom, which the relief has loaded; NaN
 * without terrain.
 */
async function heightAt(coordinate: Coordinate, zoom: number, load: boolean): Promise<number> {
    for (let z = DEM_MAX_ZOOM; z >= 0; z--) {
        const { x, y, pixel } = demPixel(coordinate, z), tile = terrain.loaded(z, x, y);
        if (tile) return metres(tile[pixel]);
    }
    if (!load) return NaN;
    const z = reliefZoom(zoom), { x, y, pixel } = demPixel(coordinate, z);
    const tile = await terrain.tile(z, x, y).catch(() => null);
    return tile ? metres(tile[pixel]) : NaN;
}

class WeatherLayer implements DataLayer<Samples> {
    id = 'weather';
    label = 'Weather';
    icon = 'weather';
    description = 'Daytime highs, night lows and rain from ten past years.';
    caveat = 'Valleys can be colder on clear nights.';
    meta = $state<ClimateMeta | null>(null);
    error = $state('');
    variable = $state({ label: 'Map shows', options: [{ value: 'temperature', label: 'Temperature' }, { value: 'rain', label: 'Rain' }], value: 'temperature' });
    private climate?: ClimateSource;
    private map?: Map;
    private date = '';
    private theme: Theme = 'light';
    private shown = false;
    /** The view, week, variable and decoded tiles of the labels on the map; an unchanged view rebuilds nothing. */
    private labelled = '';
    /** The basemap town labels are hidden in the current style. */
    private townsHidden = false;
    private raster = new Raster({
        id: this.id,
        extent: () => this.open().then(({ bounds, meta }) => ({ maxzoom: MAX_ZOOM, bounds, attribution: meta.attribution })),
        draw: (look, z, x, y, signal) => this.draw(look, z, x, y, signal),
        report: error => { this.error = error ? FAILED : ''; },
    });

    constructor(private url: string) {}

    get source() {
        const meta = this.meta;
        return meta ? `${meta.attribution} · ~9 km grid · ${meta.firstYear}–${meta.firstYear + meta.years - 1}` : '';
    }

    legend(theme: Theme) {
        return mapLegend(this.variable.value as Variable, theme);
    }

    private open() {
        return openClimate(this.url).then(source => {
            this.meta = source.meta;
            this.climate = source;
            return source;
        }, error => {
            this.error = FAILED;
            throw error;
        });
    }

    /** Colours one map tile for the week, variable and theme of a look. */
    private async draw(look: string, z: number, x: number, y: number, signal: AbortSignal) {
        const [week, variable, theme] = look.split('/') as [string, Variable, Theme];
        const source = await this.open();
        const cells = tileCells(z, x, y);
        // The cells whose centres surround the tile's pixels.
        const col = Math.floor(cells.cols[0]), row = Math.floor(cells.rows[0]);
        const cols = Math.floor(cells.cols[TILE - 1]) - col + 2, rows = Math.floor(cells.rows[TILE - 1]) - row + 2;
        const tiles = new globalThis.Map<string, ClimateTile | null>();
        for (let r = 0; r < rows; r++) {
            for (let c = 0; c < cols; c++) {
                const { x: tx, y: ty } = locate(OVERVIEW, { col: col + c, row: row + r });
                const key = `${tx}/${ty}`;
                if (!tiles.has(key)) tiles.set(key, await source.tile(OVERVIEW, tx, ty, signal));
            }
        }
        const cell = (c: number, r: number): CellRef | undefined => {
            const { x: tx, y: ty, index } = locate(OVERVIEW, { col: c, row: r });
            const tile = tiles.get(`${tx}/${ty}`);
            return tile ? { tile, index } : undefined;
        };
        const part = demTile(z, x, y);
        const heights = variable === 'temperature' ? await terrain.tile(part.z, part.x, part.y, signal).catch(error => { if (signal.aborted) throw error; return null; }) : null;
        const image = new ImageData(TILE, TILE);
        paintWeather(new Uint32Array(image.data.buffer), cells, cellBlock(col, row, cols, rows, cell, variable, Number(week)), heights ? cropHeights(heights, part) : undefined, variable, palettes[variable][theme]);
        return image;
    }

    /** The decoded overview cell of a global cell; undefined while its tile loads. */
    private cell = (col: number, row: number): CellRef | undefined => {
        const { x, y, index } = locate(OVERVIEW, { col, row });
        const tile = this.climate?.loaded(OVERVIEW, x, y);
        return tile ? { tile, index } : undefined;
    };

    /**
     * Labels the places of the loaded basemap tiles with the value of the layer week: the high at the
     * place's height, or the typical rain. It reads only decoded tiles, so it requests nothing; a town
     * without a decoded terrain tile keeps its bare name until the map renders that tile. It runs on each
     * `idle`, so it sets nothing that renders again unless the view, week or decoded tiles changed.
     */
    private relabel = async () => {
        const map = this.map, climate = this.climate, variable = this.variable.value as Variable, date = this.date;
        if (!map?.getSource(LABELS) || !climate || !this.shown) return;
        const view = `${map.getBounds().toArray()} ${map.getZoom()} ${weekOf(date)} ${variable} ${terrain.version} ${climate.version}`;
        if (view === this.labelled) return;
        this.labelled = view;
        const places = mapPlaces([
            ...map.querySourceFeatures('basemap', { sourceLayer: 'places', filter: ['==', ['get', 'kind'], 'locality'] }),
            ...map.querySourceFeatures('basemap', { sourceLayer: 'pois', filter: ['==', ['get', 'kind'], 'peak'] }),
        ] as BasemapFeature[]);
        const week = weekOf(date);
        const values = variable === 'rain'
            ? places.map(({ coordinate }) => rainAt(coordinate, this.cell, week, climate.meta.firstYear))
            : await Promise.all(places.map(async place => {
                const height = place.elevation ?? await heightAt(place.coordinate, map.getZoom(), false);
                return Number.isNaN(height) ? NaN : highAt(place.coordinate, height, this.cell, week);
            }));
        // A newer view, week or variable relabels on its own.
        if (map !== this.map || view !== this.labelled || !this.shown || !map.getSource(LABELS)) return;
        const labels = placeLabels(places, values.map(value => Number.isNaN(value) ? undefined : variable === 'rain' ? `${rainRange(value, '-')} mm` : mapDegrees(value)));
        (map.getSource(LABELS) as GeoJSONSource).setData(labels);
    };

    /**
     * Shows or hides the basemap town labels. Opacity, not visibility: a layout change would make the
     * basemap parse its tiles again. The hidden labels still take their space, but the layer's labels
     * are above them, so MapLibre places those first. MapLibre renders again on each paint change, even
     * to the same value, so this changes the labels only when they are not already as asked.
     */
    private townLabels(map: Map, shown: boolean) {
        if (!map.getLayer(TOWNS) || this.townsHidden === !shown) return;
        this.townsHidden = !shown;
        for (const property of ['text-opacity', 'icon-opacity'] as const) map.setPaintProperty(TOWNS, property, shown ? undefined : 0);
    }

    sync(map: Map, { shown, date, theme }: View & { shown: boolean }) {
        this.date = date;
        this.theme = theme;
        this.map = map;
        this.shown = shown;
        this.raster.sync(map, shown, `${weekOf(date)}/${this.variable.value}/${theme}`);
        if (map.getLayer(LABELS)) {
            map.setLayoutProperty(LABELS, 'visibility', shown ? 'visible' : 'none');
            this.townLabels(map, !shown);
            // A relabel that a hide cut short runs again on show.
            if (shown) void this.relabel();
            else this.labelled = '';
            return;
        }
        if (!shown || !map.getLayer(TOWNS)) return;
        void this.open().then(() => {
            // A theme change during the request installs into the new style instead.
            if (this.map !== map || this.theme !== theme || !this.shown || map.getSource(LABELS)) return;
            this.labelled = '';
            // A new style shows its town labels.
            this.townsHidden = false;
            map.addSource(LABELS, { type: 'geojson', data: { type: 'FeatureCollection', features: [] } });
            const order = map.getLayersOrder();
            map.addLayer(labelLayer(map, theme), order[order.indexOf(TOWNS) + 1]);
            this.townLabels(map, false);
            // Each rendered view relabels: the rendered tiles have decoded the terrain and climate tiles it reads.
            map.off('idle', this.relabel);
            map.on('idle', this.relabel);
        }, () => {});
    }

    async sample({ coordinates, elevation }: Line, signal: AbortSignal): Promise<Samples> {
        const source = await this.open();
        // A route reads the overview; a chart or a night label reads the detail of its one sample.
        const overview = await sampleLine(coordinates, source.tile, OVERVIEW);
        // A map point has no profile height, so the rendered terrain gives it one. A route keeps its profile, so its values never depend on the view.
        const heights = coordinates.length === 1 && elevation[0] === null ? [await heightAt(coordinates[0], this.map?.getZoom() ?? 0, true)] : elevation.map(height => height ?? NaN);
        signal.throwIfAborted();
        // A map point is a state proxy, which `cumulative` cannot freeze.
        const km = coordinates.length < 2 ? [0] : cumulative(coordinates);
        return { overview, detail: i => detailCell(source, coordinates[i]), elevation: Float32Array.from(heights), km };
    }

    strip = {
        label: 'Daytime high',
        fills: stripFills,
        values: (samples: Samples, date: string) => stripClasses(sampleHighs(samples, date)),
        scaled: (samples: Samples, date: string) => stripScale(sampleHighs(samples, date)),
    };

    chart(samples: Samples, i: number, { date, theme }: View) {
        return weatherChart(samples, i, this.meta?.firstYear ?? 0, date, theme, this.variable.value as Variable);
    }

    year(samples: Samples | null, { date, theme }: View) {
        return weatherYear(samples, date, theme, this.meta?.firstYear ?? 0);
    }

    nights(samples: Samples, stops: { index: number; date: string }[]) {
        return nightLabels(samples, stops);
    }
}

export const weatherLayer = (url: string): DataLayer => new WeatherLayer(url);
