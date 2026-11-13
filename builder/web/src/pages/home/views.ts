/** The home views of specs/home-views.md: the live manifest, the view and the slot for a time, and the address of an image. */
import { PLANNER_CATALOG } from '../../lib/planner/release';

export type Slot = 'morning' | 'noon' | 'evening' | 'night';

export interface HomeView {
    id: string;
    name: string;
    region: string;
    lat: number;
    lon: number;
    /** `HH:MM`. */
    sunrise: string;
    sunset: string;
    night: boolean;
}

export interface Manifest {
    format: 1;
    set: string;
    credits: string[];
    views: HomeView[];
}

/** The manifest of the live set, on the host of the planner catalogue. */
export const MANIFEST = new URL('/home-views/manifest.json', PLANNER_CATALOG).href;

const CLOCK = /^([01]\d|2[0-3]):[0-5]\d$/;
const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const isText = (value: unknown): value is string => typeof value === 'string' && value.trim() !== '';

function isView(value: unknown): value is HomeView {
    if (!isRecord(value)) return false;
    const { id, name, region, lat, lon, sunrise, sunset, night } = value;
    return typeof id === 'string' && /^[a-z0-9-]+$/.test(id) && isText(name) && isText(region)
        && Number.isFinite(lat) && Number.isFinite(lon) && typeof sunrise === 'string' && CLOCK.test(sunrise)
        && typeof sunset === 'string' && CLOCK.test(sunset) && sunrise < sunset && typeof night === 'boolean';
}

/** The manifest, or undefined when the value is not a manifest of format 1. */
export function readManifest(value: unknown): Manifest | undefined {
    if (!isRecord(value) || value.format !== 1 || typeof value.set !== 'string' || !/^[0-9a-f]{16}$/.test(value.set)) return undefined;
    const { credits, views } = value;
    if (!Array.isArray(credits) || !credits.every(credit => typeof credit === 'string')) return undefined;
    if (!Array.isArray(views) || !views.length || !views.every(isView)) return undefined;
    return value as unknown as Manifest;
}

/** Reads the live manifest; undefined when it is missing or cannot be read. `reload` skips the copy of the browser. */
export function loadManifest(reload = false): Promise<Manifest | undefined> {
    return fetch(MANIFEST, reload ? { cache: 'reload' } : undefined)
        .then(response => response.ok ? response.json() : undefined)
        .then(readManifest, () => undefined);
}

const DAY_MS = 86_400_000;
const minutes = (clock: string) => Number(clock.slice(0, 2)) * 60 + Number(clock.slice(3));

/** The view of the local date of `now`, `step` views later, and its slot by the local time as if the visitor stood there. */
export function viewAt(manifest: Manifest, now: Date, step = 0): { view: HomeView; slot: Slot } {
    const days = Math.floor(Date.UTC(now.getFullYear(), now.getMonth(), now.getDate()) / DAY_MS);
    const count = manifest.views.length, view = manifest.views[((days + step) % count + count) % count];
    const at = now.getHours() * 60 + now.getMinutes(), rise = minutes(view.sunrise), set = minutes(view.sunset), noon = (rise + set) / 2;
    if (at < rise || at >= set) return { view, slot: view.night ? 'night' : 'evening' };
    if (at < (rise + noon) / 2) return { view, slot: 'morning' };
    return { view, slot: at >= (noon + set) / 2 ? 'evening' : 'noon' };
}

export const imageUrl = (manifest: Manifest, view: HomeView, slot: Slot) =>
    new URL(`/home-views/${manifest.set}/${view.id}/${slot}.webp`, PLANNER_CATALOG).href;

/** The time of the visitor. A dev server takes another time from `?now=YYYY-MM-DDTHH:MM`, local time. */
export function homeNow(): Date {
    const set = import.meta.env.DEV ? new URLSearchParams(location.search).get('now') : null;
    const date = set && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/.test(set) ? new Date(set) : undefined;
    return date && !Number.isNaN(date.getTime()) ? date : new Date();
}
