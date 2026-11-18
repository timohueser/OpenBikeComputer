/** The covers of signed routes of specs/route-pictures.md: the live set, the cell files, and the cover of a record. */
import type { CoverPoints } from '../library/records';
import { PLANNER_CATALOG } from '../planner/release';
import type { CatalogRecord } from '../planner/signed-routes';

/** The live set: what a reader needs of its manifest. `light` is the light of the month that the page shows. */
interface Live { set: string; look: string; light: string; credits: string[] }

export interface RouteCover { bitmap: ImageBitmap; points: CoverPoints; credits: string[] }

const address = (path: string) => new URL(`/route-pictures/${path}`, PLANNER_CATALOG).href;
export const MANIFEST = address('manifest.json');
/** The manifest is read again at most once in this time: after a cover file answers 404, as the set can have changed,
 * and after a read that failed for another reason, such as no signal. */
const REREAD_MS = 60_000;
/** A small image is a quarter of the full image in width and height, and the points are in the frame of the full image. */
const SMALL = 4;

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** The JSON of a file; undefined when it answers 404. Any other failure throws: the answer can change soon. */
async function json(url: string, init?: RequestInit): Promise<unknown> {
    const response = await fetch(url, init);
    if (response.status === 404) return undefined;
    if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
    return response.json();
}

/** The set of a manifest of format 1, with the light of the current month on the device's clock, else of `default_month`. */
function readLive(value: unknown, now: Date): Live | undefined {
    if (!isRecord(value) || value.format !== 1 || typeof value.set !== 'string' || !/^[0-9a-f]{16}$/.test(value.set)) return undefined;
    const { set, look, months, default_month: fallback, credits } = value;
    if (typeof look !== 'string' || !isRecord(months) || !Array.isArray(credits) || !credits.every(credit => typeof credit === 'string')) return undefined;
    const light = months[String(now.getMonth() + 1).padStart(2, '0')] ?? months[String(fallback)];
    return typeof light === 'string' ? { set, look, light, credits } : undefined;
}

let live: Promise<Live | undefined> | undefined, readAt = -Infinity, failed = false;
/** The promise of each cell file of each set, so rows that ask at the same time make one request. */
const cells = new Map<string, Promise<Record<string, string> | undefined>>();

/** The live set; nothing when there is none or its format is not 1. `reload` skips the browser's copy. */
export function routePictures(reload = false): Promise<Live | undefined> {
    if (!live || reload || (failed && Date.now() - readAt >= REREAD_MS)) {
        readAt = Date.now();
        failed = false;
        live = json(MANIFEST, reload ? { cache: 'reload' } : undefined).then(value => readLive(value, new Date()), () => {
            failed = true;
            return undefined;
        });
    }
    return live;
}

/** The key of the cover of `id` in the cell file of `cell`. A cell whose file answers 404 has no cover; another failure
 * is not kept, so a later row asks again. */
export function coverKey(cell: string, id: number): Promise<string | undefined> {
    return routePictures().then(set => {
        if (!set) return undefined;
        const path = `sets/${set.set}/cells/${cell}.json`;
        let file = cells.get(path);
        if (!file) {
            cells.set(path, file = json(address(path)).then(value => isRecord(value) && value.format === 1 && isRecord(value.routes) ? value.routes as Record<string, string> : undefined));
            file.catch(() => { if (cells.get(path) === file) cells.delete(path); });
        }
        return file.then(routes => routes?.[id], () => undefined);
    });
}

/** The key of a cover: the start of the SHA-256 of `look` and the numbers of the line. */
async function keyOf(look: string, line: number[]): Promise<string> {
    const hash = new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(`${look}|${line.join(',')}`)));
    return Array.from(hash.subarray(0, 8), byte => byte.toString(16).padStart(2, '0')).join('');
}

/** A cover file; a 404 means the set may not be live any more. */
async function file(path: string, signal?: AbortSignal): Promise<Response> {
    const response = await fetch(address(path), { signal });
    if (response.status === 404 && Date.now() - readAt >= REREAD_MS) void routePictures(true);
    if (!response.ok) throw new Error(`${path}: HTTP ${response.status}`);
    return response;
}

/** A `null` place becomes NaN, with eye 0, as `pack` of line.ts writes a vertex with no place; each place is multiplied by `scale`. */
function readPoints(value: unknown, vertices: number, scale: number): CoverPoints | undefined {
    if (!isRecord(value)) return undefined;
    const { x, y, eye } = value;
    if (![x, y, eye].every(list => Array.isArray(list) && list.length === vertices)) return undefined;
    const place = (list: unknown[]) => Float32Array.from(list, at => typeof at === 'number' ? at * scale : NaN);
    return { x: place(x as unknown[]), y: place(y as unknown[]), eye: Uint8Array.from(eye as number[]) };
}

/** The cover of a record in the live set, with the small image for a list row, or undefined, also after `signal` aborts.
 * The caller owns the bitmap. */
export async function coverOf(record: CatalogRecord, small = false, signal?: AbortSignal): Promise<RouteCover | undefined> {
    const line = record.line_udeg, cell = record.cells[0];
    try {
        const set = await routePictures();
        if (!set || !line || !cell) return undefined;
        const [held, key] = await Promise.all([coverKey(cell, record.id), keyOf(set.look, line)]);
        if (held !== key || signal?.aborted) return undefined;
        const [image, points] = await Promise.all([
            file(`covers/${record.id}/${key}-${set.light}${small ? '-small' : ''}.webp`, signal).then(response => response.blob()),
            file(`covers/${record.id}/${key}.json`, signal).then(response => response.json()).then(value => readPoints(value, line.length / 2, small ? 1 / SMALL : 1)),
        ]);
        if (!points || signal?.aborted) return undefined;
        const bitmap = await createImageBitmap(image);
        if (signal?.aborted) return void bitmap.close();
        return { bitmap, points, credits: set.credits };
    } catch {
        // Also where `crypto.subtle` is missing: a page on plain http at a LAN address.
        return undefined;
    }
}

type Hold = { cover: Promise<RouteCover | undefined>; users: number; stop: AbortController };
/** The covers that pictures hold now, by size and record: two pictures of one route and one size share one load and one bitmap. */
const holds = { small: new Map<CatalogRecord, Hold>(), full: new Map<CatalogRecord, Hold>() };

/** Holds the cover of a record, small for a list row, until `release`. The last release stops the load and closes the bitmap. */
export function holdCover(record: CatalogRecord, small = false): { cover: Promise<RouteCover | undefined>; release: () => void } {
    const held = holds[small ? 'small' : 'full'];
    let entry = held.get(record);
    if (!entry) {
        const stop = new AbortController();
        held.set(record, entry = { cover: coverOf(record, small, stop.signal), users: 0, stop });
    }
    entry.users++;
    const mine = entry;
    let released = false;
    return {
        cover: mine.cover,
        release: () => {
            if (released) return;
            released = true;
            if (--mine.users > 0) return;
            held.delete(record);
            mine.stop.abort();
            void mine.cover.then(cover => cover?.bitmap.close());
        },
    };
}
