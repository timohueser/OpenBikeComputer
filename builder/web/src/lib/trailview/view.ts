import { MAX_ALTITUDE, WALK_EYE, type Mode } from './frame/camera';

export const WEATHERS = ['sunny', 'cloudy', 'rainy'] as const;
/**
 * The false-colour views of the terrain shader, in the order of its `debug` switch. `cracks` clears to
 * red; `surface` paints red where drawnHeight leaves the mesh. The ground views key the material,
 * lithology and region codes, and show the snow on the date; `mask` draws the features of the mask flat.
 */
export const DEBUG_VIEWS = ['off', 'normals', 'lod', 'cracks', 'surface', 'material', 'lithology', 'region', 'snow', 'albedo', 'mask'] as const;
/** The clock of a view is the wall clock of the Alps, the only region that TrailView covers. */
export const TIME_ZONE = 'Europe/Zurich';

export type Weather = (typeof WEATHERS)[number];
export type DebugView = (typeof DEBUG_VIEWS)[number];

/** What the address holds. `height` is `eye` (metres above the ground) when walking, `alt` (metres above sea level) when flying. */
export interface View {
    lat: number;
    lon: number;
    heading: number;
    pitch: number;
    mode: Mode;
    height: number;
    /** YYYY-MM-DD */
    date: string;
    /** HH:MM or HH:MM:SS */
    time: string;
    weather: Weather;
    debug: DebugView;
}

/** The date of today in the Alps, YYYY-MM-DD: en-CA formats a date so. */
export const today = (now = new Date()) => new Intl.DateTimeFormat('en-CA', { timeZone: TIME_ZONE }).format(now);

/** A walk at the centre of the bounds, at noon of today in the Alps. */
export function defaultView([west, south, east, north]: [number, number, number, number], now = new Date()): View {
    return { lat: (south + north) / 2, lon: (west + east) / 2, heading: 0, pitch: 0, mode: 'walk', height: WALK_EYE, date: today(now), time: '12:00', weather: 'sunny', debug: 'off' };
}

const number = (text: string | null, min: number, max: number) => {
    const value = text === null || text.trim() === '' ? NaN : Number(text);
    return Number.isFinite(value) && value >= min && value <= max ? value : undefined;
};
const choice = <T extends string>(text: string | null, values: readonly T[]) => values.find(value => value === text);
/** A day that exists: `Date.parse` takes 2026-02-30 as 2 March. */
const isDay = (text: string) => /^\d{4}-\d{2}-\d{2}$/.test(text) && new Date(Date.parse(text) || 0).toISOString().startsWith(text);

/** The view of a query string; a missing or invalid value keeps the fallback's. */
export function parseView(search: string, fallback: View): View {
    const q = new URLSearchParams(search);
    const alt = number(q.get('alt'), -500, MAX_ALTITUDE), eye = number(q.get('eye'), 0, MAX_ALTITUDE);
    const date = q.get('date') ?? '', time = q.get('time') ?? '';
    return {
        lat: number(q.get('lat'), -85, 85) ?? fallback.lat,
        lon: number(q.get('lon'), -180, 180) ?? fallback.lon,
        heading: number(q.get('heading'), -360, 360) ?? fallback.heading,
        pitch: number(q.get('pitch'), -89, 89) ?? fallback.pitch,
        ...(alt !== undefined ? { mode: 'fly', height: alt } : eye !== undefined ? { mode: 'walk', height: eye } : { mode: fallback.mode, height: fallback.height }),
        date: isDay(date) ? date : fallback.date,
        time: /^([01]\d|2[0-3]):[0-5]\d(:[0-5]\d)?$/.test(time) ? time : fallback.time,
        weather: choice(q.get('weather'), WEATHERS) ?? fallback.weather,
        debug: choice(q.get('debug'), DEBUG_VIEWS) ?? fallback.debug,
    };
}

/** The query string of a view. Parameters that are not part of a view stay as they are in `search`. */
export function formatView(view: View, search = ''): string {
    const q = new URLSearchParams(search);
    for (const key of ['lat', 'lon', 'heading', 'pitch', 'eye', 'alt', 'date', 'time', 'weather', 'debug']) q.delete(key);
    q.set('lat', view.lat.toFixed(6));
    q.set('lon', view.lon.toFixed(6));
    q.set('heading', view.heading.toFixed(1));
    q.set('pitch', view.pitch.toFixed(1));
    q.set(view.mode === 'walk' ? 'eye' : 'alt', view.height.toFixed(1));
    q.set('date', view.date);
    q.set('time', view.time);
    q.set('weather', view.weather);
    if (view.debug !== 'off') q.set('debug', view.debug);
    // A copied address stays readable: `time=15:00`, not `time=15%3A00`.
    return `?${q}`.replace(/%3A/g, ':');
}
