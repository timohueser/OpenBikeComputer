/** The covers of signed routes of specs/route-pictures.md: the live set, the cell files, and the cover of a record. */
import type { CoverPoints } from '../library/records';
import { PLANNER_CATALOG } from '../planner/release';
import type { CatalogRecord } from '../planner/signed-routes';

/** The live set: what a reader needs of its manifest. `light` is the light of the month that the page shows. */
interface Live { set: string; look: string; light: string; credits: string[] }

export interface RouteCover { bitmap: ImageBitmap; points: CoverPoints; credits: string[] }

const address = (path: string) => new URL(`/route-pictures/${path}`, PLANNER_CATALOG).href;
export const MANIFEST = address('manifest.json');
/** A cover file that answers 404 reads the manifest again, at most once in this time: the set can have changed. */
const REREAD_MS = 60_000;

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const json = (url: string, init?: RequestInit) => fetch(url, init).then(response => response.ok ? response.json() as Promise<unknown> : undefined);

/** The set of a manifest of format 1, with the light of the current month on the device's clock, else of `default_month`. */
function readLive(value: unknown, now: Date): Live | undefined {
    if (!isRecord(value) || value.format !== 1 || typeof value.set !== 'string' || !/^[0-9a-f]{16}$/.test(value.set)) return undefined;
    const { set, look, months, default_month: fallback, credits } = value;
    if (typeof look !== 'string' || !isRecord(months) || !Array.isArray(credits) || !credits.every(credit => typeof credit === 'string')) return undefined;
    const light = months[String(now.getMonth() + 1).padStart(2, '0')] ?? months[String(fallback)];
    return typeof light === 'string' ? { set, look, light, credits } : undefined;
}

let live: Promise<Live | undefined> | undefined, readAt = -Infinity;
/** The promise of each cell file of each set, so rows that ask at the same time make one request. */
const cells = new Map<string, Promise<Record<string, string> | undefined>>();

/** The live set, read once; nothing when the request fails or the format is not 1. `reload` skips the browser's copy. */
export function routePictures(reload = false): Promise<Live | undefined> {
    if (!live || reload) {
        readAt = Date.now();
        live = json(MANIFEST, reload ? { cache: 'reload' } : undefined).then(value => readLive(value, new Date()), () => undefined);
    }
    return live;
}

/** The key of the cover of `id` in the cell file of `cell`; a cell with no file has no cover. */
export function coverKey(cell: string, id: number): Promise<string | undefined> {
    return routePictures().then(set => {
        if (!set) return undefined;
        const path = `sets/${set.set}/cells/${cell}.json`;
        let file = cells.get(path);
        if (!file) cells.set(path, file = json(address(path)).then(
            value => isRecord(value) && value.format === 1 && isRecord(value.routes) ? value.routes as Record<string, string> : undefined, () => undefined));
        return file.then(routes => routes?.[id]);
    });
}

/** The key of a cover: the start of the SHA-256 of `look` and the numbers of the line. */
async function keyOf(look: string, line: number[]): Promise<string> {
    const hash = new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(`${look}|${line.join(',')}`)));
    return Array.from(hash.subarray(0, 8), byte => byte.toString(16).padStart(2, '0')).join('');
}

/** A cover file; a 404 means the set may not be live any more. */
async function file(path: string): Promise<Response> {
    const response = await fetch(address(path));
    if (response.status === 404 && Date.now() - readAt >= REREAD_MS) void routePictures(true);
    if (!response.ok) throw new Error(`${path}: HTTP ${response.status}`);
    return response;
}

/** A `null` place becomes NaN, with eye 0, as `pack` of line.ts writes a vertex with no place. */
function readPoints(value: unknown, vertices: number): CoverPoints | undefined {
    if (!isRecord(value)) return undefined;
    const { x, y, eye } = value;
    if (![x, y, eye].every(list => Array.isArray(list) && list.length === vertices)) return undefined;
    const place = (list: unknown[]) => Float32Array.from(list, at => typeof at === 'number' ? at : NaN);
    return { x: place(x as unknown[]), y: place(y as unknown[]), eye: Uint8Array.from(eye as number[]) };
}

/** The cover of a record in the live set, or undefined. The caller owns the bitmap and closes it. */
export async function coverOf(record: CatalogRecord): Promise<RouteCover | undefined> {
    const line = record.line_udeg, cell = record.cells[0];
    const set = await routePictures();
    if (!set || !line || !cell) return undefined;
    const [held, key] = await Promise.all([coverKey(cell, record.id), keyOf(set.look, line)]);
    if (held !== key) return undefined;
    try {
        const [image, points] = await Promise.all([
            file(`covers/${record.id}/${key}-${set.light}.webp`).then(response => response.blob()),
            file(`covers/${record.id}/${key}.json`).then(response => response.json()).then(value => readPoints(value, line.length / 2)),
        ]);
        if (!points) return undefined;
        return { bitmap: await createImageBitmap(image), points, credits: set.credits };
    } catch {
        return undefined;
    }
}
