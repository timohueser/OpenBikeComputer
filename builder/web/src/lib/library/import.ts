/** Finds the rides and the planned routes in GPX and FIT files, in folders and in the ZIP exports of services. */
import { settings } from '../settings/settings.svelte';
import type { Bike, RideRecord } from './records';
import type { Library } from './store';
import { rideSummary, type RideSample } from './summary';
import { entries, open, UnsupportedPacking } from './zip';

/** A point in microdegrees, with the time in Unix seconds. */
export interface TrackPoint { lat: number; lon: number; ele?: number; time?: number; segmentStart?: true }
/** A GPX file or a FIT course, as the readers give it. */
export interface Track {
    name: string;
    points: TrackPoint[];
    waypoints: { lat: number; lon: number; name?: string; note?: string }[];
    kind?: string;
}
export interface Recording { sport: string; samples: RideSample[] }

/** The file readers of `lib/core`, which the caller gives so that the library depends on no other system. They throw
 * an error with the name `GpxError`, or with the name `FitError` and a `code`. */
export interface Readers {
    gpx(text: string, fallback: string): Track;
    fit(bytes: Uint8Array, fallback: string): Recording | Track;
}

/** A dropped or chosen file, or a dropped folder. */
export type Item = File | FileSystemEntry;

/** One file that was read. `name` is the name of the record, or else the name of the file. `open` reads the file
 * again: a ride gives its source file, a route its track. */
export type Found = { file: string; name: string } & (
    | { kind: 'ride'; ride: RideRecord; open: () => Promise<Blob> }
    | { kind: 'route'; id: string; lengthM: number; bike: Bike; open: () => Promise<Track> }
    | { kind: 'known' }
    | { kind: 'unreadable'; reason: string });

export const reasons = {
    tcx: 'TCX files cannot be read',
    track: 'the file has no track',
    sport: 'not a ride by bike or on foot',
    packing: 'this packing is not supported',
    damaged: 'the file is damaged',
};

interface Source { path: string; get: () => Promise<Blob> }

const baseName = (path: string) => path.slice(path.lastIndexOf('/') + 1);
const counted = /\.(gpx|fit|tcx)(\.gz)?$/i;

/** An error whose message is the reason that the rider sees. */
class Unreadable extends Error {}

/** The reason of an error while a file, a ZIP entry or a `.gz` file opens: such an error means a damaged file. */
const reasonOf = (error: unknown) => error instanceof Unreadable ? error.message : error instanceof UnsupportedPacking ? reasons.packing : reasons.damaged;
const failure = (path: string, error: unknown): Found => ({ file: path, name: baseName(path), kind: 'unreadable', reason: reasonOf(error) });

const gunzip = (blob: Blob) => new Response(blob.stream().pipeThrough(new DecompressionStream('gzip'))).blob();

// A macOS copy keeps the attributes of each file in a file whose name starts with `._`.
const wanted = (name: string) => !name.startsWith('._') && (counted.test(name) || /\.(csv|zip)$/i.test(name));
const isZip = (name: string) => /\.zip$/i.test(name);

/** Lists the files to read and the CSV files of `item`. It inflates each ZIP in a ZIP, one level deep, and keeps it. */
async function list(item: Item | Blob, path: string, sources: Source[], failed: Found[], signal: AbortSignal, inZip = false): Promise<void> {
    signal.throwIfAborted();
    const name = baseName(path);
    if (!(item instanceof Blob) && item.isDirectory) {
        const reader = (item as FileSystemDirectoryEntry).createReader();
        for (let batch; (batch = await new Promise<FileSystemEntry[]>((resolve, reject) => reader.readEntries(resolve, reject))).length;) {
            for (const child of batch) await list(child, `${path}/${child.name}`, sources, failed, signal);
        }
        return;
    }
    if (!wanted(name)) return;
    const file = () => item instanceof Blob ? Promise.resolve(item) : new Promise<File>((resolve, reject) => (item as FileSystemFileEntry).file(resolve, reject));
    if (!isZip(name)) return void sources.push({ path, get: file });
    let zipped;
    try { zipped = await entries(await file()); } catch (error) { return void failed.push(failure(path, error)); }
    for (const entry of zipped) {
        const inner = `${path}/${entry.name}`;
        if (entry.name.endsWith('/') || !wanted(baseName(entry.name))) continue;
        if (!isZip(entry.name)) sources.push({ path: inner, get: () => open(entry) });
        else if (!inZip) {
            signal.throwIfAborted();
            let content;
            try { content = await open(entry); } catch (error) { failed.push(failure(inner, error)); continue; }
            await list(content, inner, sources, failed, signal, true);
        }
    }
}

/** The rows of a CSV text: fields in quotes can hold commas, quotes as `""`, and line breaks. */
export function csvRows(text: string): string[][] {
    const rows: string[][] = [];
    let row: string[] = [], field = '', quoted = false;
    for (let i = text.charCodeAt(0) === 0xfeff ? 1 : 0; i < text.length; i++) {
        const c = text[i];
        if (quoted) {
            if (c !== '"') field += c;
            else if (text[i + 1] === '"') { field += '"'; i++; }
            else quoted = false;
        } else if (c === '"') quoted = true;
        else if (c === ',') { row.push(field); field = ''; }
        else if (c === '\n' || c === '\r') {
            if (c === '\r' && text[i + 1] === '\n') i++;
            rows.push([...row, field]);
            row = []; field = '';
        } else field += c;
    }
    if (field || row.length) rows.push([...row, field]);
    return rows;
}

// The columns of Strava's export in English. Another language has the same order with other headers.
const stravaColumns = { name: 2, type: 3, file: 12 };

/** Strava's `activities.csv`: the name and the type of each activity, by the base name of its file. With no English
 * headers, it finds the file column by its values, and uses the CSV only when that column is where the English one is.
 * Then it keeps only a type that names a bike or a walk, so that a type in another language refuses no file. */
async function strava(source: Source, into: Map<string, { name: string; type: string }>): Promise<void> {
    const [header = [], ...rows] = csvRows(await (await source.get()).text());
    let [name, type, file] = ['Activity Name', 'Activity Type', 'Filename'].map(column => header.indexOf(column));
    const english = file >= 0;
    if (!english) {
        file = header.findIndex((_, i) => rows.some(row => row[i]?.startsWith('activities/')));
        if (file !== stravaColumns.file) return;
        ({ name, type } = stravaColumns);
    }
    for (const row of rows) {
        const kind = row[type] ?? '';
        if (row[file]) into.set(baseName(row[file]), { name: row[name] ?? '', type: english || bikeOf(kind) ? kind : '' });
    }
}

/** The bike of an activity type of a file or of Strava, such as `cycling`, `Gravel Ride` or `Hike`. `touring` is the
 * match of a type that names no kind of bike. */
function bikeOf(type = ''): Bike | undefined {
    const t = type.toLowerCase();
    if (/hik|walk|run|trek/.test(t)) return 'hiking';
    if (/mountain|mtb/.test(t)) return 'mtb';
    if (/gravel/.test(t)) return 'gravel';
    if (/road/.test(t)) return 'road';
    return /cycl|bik|ride/.test(t) ? 'touring' : undefined;
}

/** A specific bike of Strava, else the bike of the file, else touring. Null when Strava names a sport that is not by
 * bike or on foot, or when the FIT file does and Strava names none. */
function chooseBike(stravaType: string | undefined, fromFile: Bike | null | undefined): Bike | null {
    const fromStrava = stravaType ? bikeOf(stravaType) ?? null : undefined;
    if (fromStrava === null) return null;
    if (fromStrava && fromStrava !== 'touring') return fromStrava;
    if (fromFile === null) return fromStrava ?? null;
    return fromFile ?? 'touring';
}

const fitBikes: Record<string, Bike | null> = { road: 'road', gravel: 'gravel', mtb: 'mtb', bike: 'touring', unknown: 'touring', foot: 'hiking', other: null };

/** A name that the rider knows: not a number such as `12345` or `12345_ACTIVITY`, and not a file name with an email
 * address. */
const usable = (name: string | undefined): name is string => !!name && !name.includes('@') && /[^\d\s_.-]/.test(name.replace(/activity/gi, ''));
const dateName = (start: number, bike: Bike) =>
    `${bike === 'hiking' ? 'Hike' : 'Ride'}, ${new Date(start * 1000).toLocaleDateString('en-GB', { day: 'numeric', month: 'short', year: 'numeric' })}`;

/** `YYYYMMDDTHHMMSSZ` of a Unix time. */
const rideKey = (seconds: number) => new Date(Math.floor(seconds) * 1000).toISOString().replace(/[-:]|\.\d+/g, '');

async function lineId(line: number[]): Promise<string> {
    const hash = new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(JSON.stringify(line))));
    return `file-${Array.from(hash.slice(0, 16), b => b.toString(16).padStart(2, '0')).join('')}`;
}

/**
 * Reads each GPX and FIT file of `items`, also inside folders, ZIP files, a ZIP in a ZIP and `.gz` files, one file at a
 * time, and gives one `Found` for each. Other files are not counted. A GPX file with a time at each point and a FIT
 * activity are rides; another file is a route. A ride is known when its key is stored or deleted, a route when its id
 * is stored, and both when an earlier file of `items` had the same key or id. The names and types of Strava's
 * `activities.csv` apply to its files.
 */
export async function* readFiles(items: Item[], library: Library, readers: Readers, signal: AbortSignal): AsyncGenerator<Found> {
    const sources: Source[] = [], failed: Found[] = [];
    for (const item of items) {
        signal.throwIfAborted();
        await list(item, item instanceof Blob ? item.name : item.fullPath.replace(/^\//, ''), sources, failed, signal);
    }
    yield* failed;
    const activities = new Map<string, { name: string; type: string }>();
    for (const source of sources) if (baseName(source.path).toLowerCase() === 'activities.csv') await strava(source, activities);
    const deleted = new Set((await library.state('deleted') ?? []).map(d => d.key)), seen = new Set<string>();
    for (const source of sources) {
        signal.throwIfAborted();
        if (!counted.test(source.path)) continue;
        const name = baseName(source.path), found = { file: source.path, name };
        const zipped = /\.gz$/i.test(name), plain = zipped ? name.slice(0, -3) : name, fallback = plain.replace(/\.\w+$/, '');
        const format = plain.slice(-3).toLowerCase() as 'gpx' | 'fit' | 'tcx';
        const strava = activities.get(name);
        const content = () => source.get().then(blob => zipped ? gunzip(blob) : blob);
        // Another error of a reader stops the import.
        const read = async (): Promise<Recording | Track> => {
            const blob = await content().catch(error => { throw new Unreadable(reasonOf(error)); });
            try { return format === 'gpx' ? readers.gpx(await blob.text(), fallback) : readers.fit(new Uint8Array(await blob.arrayBuffer()), fallback); } catch (error) {
                const { name, code } = error as { name?: string; code?: string };
                if (name === 'GpxError') throw new Unreadable(reasons.track);
                if (name === 'FitError') throw new Unreadable(code === 'fit-no-positions' || code === 'fit-unsupported' ? reasons.track : reasons.damaged);
                throw error;
            }
        };
        if (format === 'tcx') { yield { ...found, kind: 'unreadable', reason: reasons.tcx }; continue; }
        let file;
        try { file = await read(); } catch (error) {
            if (!(error instanceof Unreadable)) throw error;
            yield failure(source.path, error);
            continue;
        }
        const timed = 'points' in file && file.points.every(p => p.time !== undefined);
        const samples = 'samples' in file ? file.samples
            : file.points.map(p => ({ timeS: p.time ?? 0, lat: p.lat, lon: p.lon, elevationM: p.ele, segmentStart: !!p.segmentStart }));
        const bike = chooseBike(strava?.type, 'sport' in file ? fitBikes[file.sport] : bikeOf(file.kind));
        if (bike === null) { yield { ...found, kind: 'unreadable', reason: reasons.sport }; continue; }
        if (samples.length < 2) { yield { ...found, kind: 'unreadable', reason: reasons.track }; continue; }
        // A route's summary here gives only its line and its length: the planner makes the stored one.
        const summary = rideSummary(samples, bike);
        if ('samples' in file || timed) {
            const key = rideKey(summary.start);
            const title = [strava?.name, 'name' in file ? file.name : fallback].find(usable) ?? dateName(summary.start, bike);
            const ride: RideRecord = { key, name: title, source: 'file', format: format as 'gpx' | 'fit', summary,
                maxHr: settings.maxHr?.value ?? null, ftp: settings.ftp?.value ?? null };
            const known = seen.has(key) || deleted.has(key) || !!await library.ride(key);
            seen.add(key);
            yield known ? { ...found, kind: 'known' } : { ...found, name: ride.name, kind: 'ride', ride, open: content };
        } else {
            const id = await lineId(summary.line), title = usable(file.name) ? file.name : 'Planned route';
            const known = seen.has(id) || !!await library.item(id);
            seen.add(id);
            yield known ? { ...found, kind: 'known' }
                : { ...found, name: title, kind: 'route', id, lengthM: summary.lengthM, bike, open: async () => ({ ...await read() as Track, name: title }) };
        }
    }
}
