import type { Map } from 'maplibre-gl';
import SunInspect from '../../../components/planner/SunInspect.svelte';
import { TERRAIN_URL } from '../map-data';
import { SunClient } from './sun-client';
import { clock, OUTSIDE, TERRAIN_MAP_ZOOM, type SunMeta } from './sun';
import type { Coordinate } from '../map-types';
import { abgr } from './colour';
import type { DataLayer, Line, Theme, View } from './data-layer';
import { Raster } from './raster';

const SIZE = 128, TILE_SIZE = 256;
const names = ['Direct sun', 'Terrain shade', 'Night', 'Terrain unavailable'];
const colours = { light: ['#e8b352', '#668397', '#405468', '#9b9c8c'], dark: ['#e8b352', '#8b9faa', '#506371', '#a1a18c'] };
interface Samples { coordinates: Coordinate[]; values: Uint8Array }

class SunLayer implements DataLayer<Samples> {
    id = 'sun'; label = 'Sunlight'; icon = 'sun';
    description = 'Terrain shade at a chosen date and time. Inspect a place for its sun windows.';
    caveat = 'Map shading gets coarser as you zoom out. Routes use an overview grid of about 200 m. Inspect a place for detailed sun windows. Clear-sky terrain estimate. Clouds, trees and buildings are excluded.';
    error = $state('');
    time = $state({ value: '09:00', timezone: '', detail: '' });
    private meta = $state.raw<SunMeta>();
    private client: SunClient;
    private opened?: Promise<SunMeta>;
    private overview = $state(false);
    private map?: Map;
    private zoom = () => {
        if (!this.map) return;
        // MapLibre rounds raster zoom after converting its 512 px world to source tiles.
        const zoom = Math.round(this.map.getZoom() + Math.log2(512 / TILE_SIZE));
        this.overview = zoom < TERRAIN_MAP_ZOOM;
        this.time.detail = this.overview ? 'Day / night overview. Zoom in for terrain shade.' : zoom < 10 ? 'Terrain shade overview. Zoom in for local detail.' : '';
    };
    private raster = new Raster({
        id: this.id,
        extent: () => this.open().then(meta => ({ maxzoom: 13, attribution: meta.attribution })),
        draw: async (look, z, x, y, signal) => {
            const [date, minute, theme] = look.split('/');
            const values = await this.client.tile(z, x, y, date, Number(minute), signal, SIZE);
            const image = new ImageData(SIZE, SIZE), words = new Uint32Array(image.data.buffer), palette = colours[theme as Theme];
            for (let i = 0; i < values.length; i++) {
                if (values[i] === OUTSIDE || values[i] === 0 && z < TERRAIN_MAP_ZOOM) continue;
                words[i] = abgr(palette[values[i]], values[i] === 3 ? ((i % SIZE + Math.floor(i / SIZE)) % 8 < 2 ? 135 : 30) : values[i] === 0 ? 48 : 125);
            }
            return image;
        },
        paint: { 'raster-resampling': 'nearest' },
        report: error => { this.error = error ? error instanceof Error ? error.message : 'Sunlight could not load. Toggle the layer to retry.' : ''; },
    });
    constructor(url: string) { this.client = new SunClient(url, TERRAIN_URL); }
    get source() { return this.meta ? `${this.meta.attribution} · terrain shadows within ${this.meta.distance_m / 1000} km` : ''; }
    private minute() { const [h, m] = this.time.value.split(':').map(Number); return h * 60 + m; }
    sampleKey(date: string) { return `${date}/${this.time.value}`; }
    private palette(theme: Theme) { return { swatches: names.map((label, i) => ({ label, color: colours[theme][i], hatch: i === 3 })) }; }
    legend(theme: Theme) { return { swatches: this.overview ? this.palette(theme).swatches.slice(2, 3) : this.palette(theme).swatches }; }
    private open() {
        return this.opened ??= this.client.meta(new AbortController().signal).then(meta => {
            this.meta = meta; this.time.timezone = meta.timezone; return meta;
        }).catch(error => { this.opened = undefined; this.error = 'Sunlight data could not load. Toggle the layer to retry.'; throw error; });
    }
    sync(map: Map, { shown, date, theme }: View & { shown: boolean }) {
        this.map = map;
        this.zoom();
        if (shown) {
            map.off('zoomend', this.zoom); map.on('zoomend', this.zoom);
            map.off('resize', this.zoom); map.on('resize', this.zoom);
        }
        this.raster.sync(map, shown, `${date}/${this.minute()}/${theme}`);
    }
    async sample({ coordinates }: Line, signal: AbortSignal, view?: View): Promise<Samples> {
        const date = view!.date, minute = this.minute();
        await this.open();
        // One selected instant everywhere along the route; no rider schedule enters this layer.
        return { coordinates, values: await this.client.sample(coordinates, date, minute, signal) };
    }
    get strip() { return {
        label: `Sunlight · ${this.time.value}`, hint: 'every place at the selected time · inspect for sun windows',
        fills: (theme: Theme) => this.palette(theme).swatches,
        legend: (theme: Theme) => this.palette(theme),
        values: (samples: Samples) => samples.values,
    }; }
    chart(samples: Samples, i: number, { date }: View) {
        return {
            headline: `${names[samples.values[i]]} · ${this.time.value}`,
            grids: [],
            extra: { component: SunInspect, props: { client: this.client, coordinate: samples.coordinates[i], date, minute: this.minute(), onTime: samples.coordinates.length === 1 ? (minute: number) => this.time.value = clock(minute) : undefined } },
        };
    }
}
export const sunLayer = (url: string): DataLayer => new SunLayer(url);
