import type { GeoJSONSource, Map } from 'maplibre-gl';
import type { FeatureCollection } from 'geojson';
import WindRose from '../../../components/planner/WindRose.svelte';
import { cumulative } from '../geo';
import { OVERVIEW, weekMonth, type ClimateMeta, type ClimateTile } from './climate';
import { lineBearings, speedRow, windRow } from './climate-route';
import { openClimate, sampleLine, type CellRef, type ClimateSource } from './climate-source';
import { weekOf, type DataLayer, type Line, type Theme, type View } from './data-layer';
import { ARROW_ICONS, WIND_NOTE, arrowStride, headClasses, headFills, mapLegend, speedPaint, viewTiles, windChart, windMap, windYear } from './wind';

const CELLS = 'wind', ARROWS = 'wind-arrows';
const EMPTY: FeatureCollection = { type: 'FeatureCollection', features: [] };
/** The travel direction of a sample spans this many km each way, so hairpins do not break the strip into specks. */
const STRETCH_KM = 0.5;

const ink = { light: { fill: '#ffffff', halo: '#2b2a22' }, dark: { fill: '#f2efe3', halo: '#14130e' } };
const label = { light: { text: '#2b2a22', halo: '#ffffff' }, dark: { text: '#f2efe3', halo: '#14130e' } };
const RATIO = 2, SIZE = 28;

/**
 * A slim north-pointing arrow, white on a thin dark halo: the shaft tapers from a swept head to the
 * tail. A double arrow is one straight shaft with a head at each end. `weight` 0–2 widens it.
 */
function arrowImage(icon: string, theme: Theme): ImageData {
    const [, kind, weight] = icon.split('-'), w = Number(weight);
    const canvas = document.createElement('canvas');
    canvas.width = canvas.height = SIZE * RATIO;
    const context = canvas.getContext('2d')!;
    context.scale(RATIO, RATIO);
    context.lineJoin = 'round';
    const mid = SIZE / 2, top = 2.5, bottom = SIZE - 2.5, half = 3.4 + 0.8 * w, head = 7 + 0.5 * w, neck = 0.9 + 0.5 * w, tail = 0.35 + 0.25 * w;
    // The right half from the top tip down; the left half mirrors it.
    const right: [number, number][] = kind === 'double'
        ? [[half, top + head], [neck, top + head - 1.5], [neck, bottom - head + 1.5], [half, bottom - head], [0, bottom]]
        : [[half, top + head], [neck, top + head - 1.5], [tail, bottom]];
    const shape = new Path2D();
    shape.moveTo(mid, top);
    for (const [x, y] of right) shape.lineTo(mid + x, y);
    for (const [x, y] of [...right].reverse()) shape.lineTo(mid - x, y);
    shape.closePath();
    context.strokeStyle = ink[theme].halo;
    context.lineWidth = 2.4;
    context.stroke(shape);
    context.fillStyle = ink[theme].fill;
    context.fill(shape);
    return context.getImageData(0, 0, canvas.width, canvas.height);
}

/** The overview cell of each coordinate, and of a route the travel bearings, the headwind row and the speed row; a map point has none. */
interface Samples {
    overview: (CellRef | undefined)[];
    bearings: Float32Array | null;
    route: { row: Float32Array; speeds: Float32Array } | null;
}

class WindLayer implements DataLayer<Samples> {
    id = 'wind';
    label = 'Wind';
    icon = 'wind';
    description = 'Where the daytime wind blows most often, and how strong it is.';
    caveat = `${WIND_NOTE}.`;
    meta = $state.raw<ClimateMeta | null>(null);
    error = $state('');
    private map?: Map;
    private listening?: Map;
    private shown = false;
    private date = '';
    private theme: Theme = 'light';
    /** The tiles, week and arrow stride of the drawn data. */
    private drawn = '';
    private runs = 0;
    private frame = 0;

    constructor(private url: string) {}

    get source() {
        const meta = this.meta;
        return meta ? `${meta.attribution} · ${meta.firstYear}–${meta.firstYear + meta.years - 1}` : '';
    }

    legend(theme: Theme) {
        return mapLegend(theme);
    }

    private open(): Promise<ClimateSource> {
        return openClimate(this.url).then(climate => {
            if (this.meta !== climate.meta) this.meta = climate.meta;
            this.error = '';
            return climate;
        }, error => {
            this.error = 'Wind data could not load for this region.';
            throw error;
        });
    }

    sync(map: Map, { shown, date, theme }: View & { shown: boolean }) {
        this.map = map;
        this.shown = shown;
        this.date = date;
        this.theme = theme;
        if (map.getLayer(CELLS)) {
            for (const id of [CELLS, ARROWS]) map.setLayoutProperty(id, 'visibility', shown ? 'visible' : 'none');
            this.schedule();
            return;
        }
        if (!shown) return;
        void this.open().then(() => {
            // A theme change during the request installs into the new style instead.
            if (this.map !== map || this.theme !== theme || map.getSource(CELLS)) return;
            this.install(map, theme);
            this.schedule();
        }, () => {});
    }

    private install(map: Map, theme: Theme) {
        for (const icon of ARROW_ICONS) {
            if (!map.hasImage(`${icon}-${theme}`)) map.addImage(`${icon}-${theme}`, arrowImage(icon, theme), { pixelRatio: RATIO });
        }
        const visibility = this.shown ? 'visible' : 'none';
        map.addSource(CELLS, { type: 'geojson', data: EMPTY, attribution: this.source });
        map.addSource(ARROWS, { type: 'geojson', data: EMPTY });
        // Under the relief, so the hillshade shades the colours; without antialiasing, neighbour cells have no seams.
        map.addLayer({ id: CELLS, type: 'fill', source: CELLS, layout: { visibility }, paint: { 'fill-color': speedPaint(theme), 'fill-antialias': false } }, map.getLayer('relief') ? 'relief' : undefined);
        // Over roads and under labels. The speed label sits below the arrow, clear of its turning radius; the
        // basemap labels are higher, so they are placed first and a colliding speed label is dropped.
        map.addLayer({
            id: ARROWS, type: 'symbol', source: ARROWS,
            layout: {
                visibility, 'icon-image': ['concat', ['get', 'icon'], `-${theme}`], 'icon-rotate': ['get', 'rotate'], 'icon-rotation-alignment': 'map',
                'icon-allow-overlap': true, 'icon-ignore-placement': true, 'icon-size': ['interpolate', ['linear'], ['zoom'], 6, 0.8, 9, 1, 12, 1.2],
                'text-field': ['get', 'label'], 'text-font': ['Noto Sans Regular'], 'text-size': 10, 'text-anchor': 'top', 'text-offset': [0, 1.8], 'text-optional': true,
            },
            paint: { 'text-color': label[theme].text, 'text-halo-color': label[theme].halo, 'text-halo-width': 1.2 },
        }, map.getStyle().layers.find(layer => layer.type === 'symbol')?.id);
        this.drawn = '';
        if (this.listening !== map) {
            this.listening = map;
            map.on('moveend', this.schedule);
        }
    }

    /** One redraw per frame while the date is dragged or the map moves. */
    private schedule = () => {
        if (!this.shown) return;
        cancelAnimationFrame(this.frame);
        this.frame = requestAnimationFrame(() => void this.draw());
    };

    /** Builds the cells and arrows of the overview tiles in view; the data changes with the week, the month and the stride. */
    private async draw() {
        const map = this.map, run = ++this.runs;
        if (!map?.getSource(CELLS)) return;
        try {
            const climate = await this.open();
            const view = map.getBounds(), keys = viewTiles([view.getWest(), view.getSouth(), view.getEast(), view.getNorth()], climate.bounds);
            const week = weekOf(this.date), stride = arrowStride(map.getZoom());
            const key = `${keys.join(' ')} ${week} ${stride}`;
            if (key === this.drawn) return;
            const tiles = (await Promise.all(keys.map(([x, y]) => climate.tile(OVERVIEW, x, y)))).filter((tile): tile is ClimateTile => !!tile);
            if (run !== this.runs || map !== this.map || !map.getSource(CELLS)) return;
            this.drawn = key;
            const { cells, arrows } = windMap(tiles, week, weekMonth(week), stride);
            (map.getSource(CELLS) as GeoJSONSource).setData(cells);
            (map.getSource(ARROWS) as GeoJSONSource).setData(arrows);
        } catch {
            // The view stays undrawn, so the next move or date change retries.
            this.error = 'Wind data could not load for this region.';
        }
    }

    async sample({ coordinates }: Line, signal: AbortSignal): Promise<Samples> {
        const climate = await this.open();
        const overview = await sampleLine(coordinates, climate.tile);
        signal.throwIfAborted();
        if (coordinates.length < 2) return { overview, bearings: null, route: null };
        const km = cumulative(coordinates), bearings = lineBearings(coordinates, km, STRETCH_KM);
        return { overview, bearings, route: { row: windRow(overview, km, bearings), speeds: speedRow(overview, km) } };
    }

    strip = {
        legend: (theme: Theme) => ({ swatches: headFills(theme) }),
        fills: headFills,
        values: ({ overview, bearings }: Samples, date: string) => headClasses(overview, bearings ?? [], weekMonth(weekOf(date))),
    };

    /** The point chart reads only the overview tiles the samples already hold, so it requests nothing. */
    chart({ overview, bearings }: Samples, i: number, { date, theme }: View) {
        const { chart, rose } = windChart({ overview: overview[i], bearing: bearings?.[i] }, date);
        return rose ? { ...chart, extra: { component: WindRose, props: { ...rose, theme } } } : chart;
    }

    year(samples: Samples | null, { date, theme }: View) {
        return windYear(samples?.route ?? null, date, theme);
    }
}

export const windLayer = (url: string): DataLayer => new WindLayer(url);
