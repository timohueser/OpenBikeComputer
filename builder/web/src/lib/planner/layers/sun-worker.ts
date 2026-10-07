import type { Coordinate } from '../map-types';
import { SunSurface } from './sun-source';
import { coordinateAt, instantAt, solarPosition, solarTerms, visibility, mapVisibility, UNKNOWN, type SunDay } from './sun';

export type SunRequest = { id: number; url: string; dem: string } & (
    { kind: 'tile'; z: number; x: number; y: number; size: number; date: string; minute: number }
    | { kind: 'sample'; coordinates: Coordinate[]; date: string; minute: number }
    | { kind: 'day'; coordinate: Coordinate; date: string }
    | { kind: 'meta' });

let opened: Promise<SunSurface> | undefined;
const jobs = new Map<number, AbortController>();
const raster = new Map<string, Uint8Array>();
const queue: SunRequest[] = [];
let running = false;
const days = new Map<string, SunDay>();
let yielded = performance.now();

async function execute(request: SunRequest, signal: AbortSignal) {
    opened ??= SunSurface.open(request.url, request.dem).catch(error => { opened = undefined; throw error; });
    const surface = await opened;
    signal.throwIfAborted();
    if (request.kind === 'meta') return surface.meta;
    const zoom = request.kind === 'tile' ? Math.min(12, Math.max(8, request.z + 1)) : 12;
    const at = async (coordinate: Coordinate, terms: ReturnType<typeof solarTerms>, detailed = false) =>
        surface.run(() => detailed ? visibility(surface, coordinate, solarPosition(coordinate, terms), surface.meta.distance_m, surface.stats) : surface.sunlight(coordinate, solarPosition(coordinate, terms), zoom), signal);
    const checkpoint = async () => {
        if (performance.now() - yielded < 8) return;
        await new Promise(resolve => setTimeout(resolve, 0)); signal.throwIfAborted(); yielded = performance.now();
    };
    if (request.kind === 'day') {
        const key = `${request.coordinate}/${request.date}`;
        const cached = days.get(key);
        if (cached) { surface.stats.cacheHits++; return cached; }
        // Five-minute wall-clock bins; a missing spring DST bin stays unknown.
        const states = new Uint8Array(288).fill(UNKNOWN);
        let first = -1, last = -1;
        for (let i = 0; i < states.length; i++) {
            let instant: number;
            try { instant = instantAt(request.date, i * 5 + 2, surface.meta.timezone); }
            catch { continue; }
            const terms = solarTerms(instant);
            const sun = solarPosition(request.coordinate, terms);
            if (sun.altitude > -.833 * Math.PI / 180) { if (first < 0) first = i * 5; last = (i + 1) * 5; }
            states[i] = await at(request.coordinate, terms, true);
            if (i % 24 === 0) await checkpoint();
        }
        const day: SunDay = { states, daylight: first < 0 ? null : [first, last], timezone: surface.meta.timezone };
        days.set(key, day);
        while (days.size > 64) days.delete(days.keys().next().value!);
        return day;
    }
    const terms = solarTerms(instantAt(request.date, request.minute, surface.meta.timezone));
    if (request.kind === 'sample') {
        const values = new Uint8Array(request.coordinates.length);
        for (let i = 0; i < values.length; i++) {
            values[i] = await at(request.coordinates[i], terms, request.coordinates.length === 1);
            if (i % 64 === 0) await checkpoint();
        }
        return values;
    }
    const key = `${request.z}/${request.x}/${request.y}/${request.size}/${request.date}/${request.minute}`;
    const cached = raster.get(key);
    if (cached) { raster.delete(key); raster.set(key, cached); surface.stats.cacheHits++; return cached; }
    const values = new Uint8Array(request.size ** 2);
    for (let row = 0; row < request.size; row++) {
        await surface.run(() => {
            for (let col = 0; col < request.size; col++) {
                const coordinate = coordinateAt(request.x * request.size + col + .5, request.y * request.size + row + .5, request.z, request.size);
                const sun = solarPosition(coordinate, terms);
                values[row * request.size + col] = mapVisibility(coordinate, sun, request.z, surface.meta.bounds, () => surface.sunlight(coordinate, sun, zoom));
            }
        }, signal);
        await checkpoint();
    }
    raster.set(key, values);
    while (raster.size > 128) raster.delete(raster.keys().next().value!);
    return values;
}

async function drain() {
    if (running) return;
    running = true;
    while (queue.length) {
        // Inspections go ahead of tiles waiting to draw; an active tile yields for cancellation.
        const priority = queue.findIndex(request => request.kind !== 'tile');
        const data = queue.splice(priority < 0 ? 0 : priority, 1)[0];
        const abort = jobs.get(data.id)!;
        try {
            abort.signal.throwIfAborted();
            const start = performance.now();
            const result = await execute(data, abort.signal);
            const surface = await opened!;
            self.postMessage({ id: data.id, result, elapsedMs: performance.now() - start, stats: { ...surface.stats } });
        } catch (error) {
            if (!abort.signal.aborted) self.postMessage({ id: data.id, error: error instanceof Error ? error.message : String(error) });
        } finally { jobs.delete(data.id); }
    }
    running = false;
}

self.onmessage = ({ data }: MessageEvent<SunRequest | { cancel: number }>) => {
    if ('cancel' in data) { jobs.get(data.cancel)?.abort(); return; }
    jobs.set(data.id, new AbortController());
    queue.push(data);
    void drain();
};
