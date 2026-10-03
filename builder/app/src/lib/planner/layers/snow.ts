// Snow history decoded per specs/planner-snow-tiles.md: every season from 1 September has an onset
// and a melt-out byte per pixel, as a day index in 2-day steps.
import { columnDays, dateColumn, dateLabel, reach, type SeasonGrid, type Swatch, type Theme } from './data-layer';

/** Per season, `size` onset bytes, then `size` melt-out bytes. A tile has 65,536 items; a line has one per sample. */
export interface Planar { data: Uint8Array; size: number; seasons: number }

const DAYS = 183;
const NO_SNOW = 253, WHOLE_SEASON = 254, NO_DATA = 255;
/** Classes by share of seasons with snow on a date. */
export const FREE = 0, MOSTLY_FREE = 1, MOSTLY_SNOW = 2, SNOW = 3, UNKNOWN = 4;
const DAY_MS = 86_400_000;
const SEPTEMBER = Date.UTC(2001, 8, 1);

export interface SnowMeta { firstSeason: number; seasons: number; resolution: number; attribution: string }

export function snowMeta(json: Record<string, unknown>): SnowMeta {
    const meta = { firstSeason: Number(json.first_season), seasons: Number(json.seasons), resolution: Number(json.resolution_m), attribution: String(json.attribution ?? '') };
    if (!Number.isInteger(meta.firstSeason) || !(meta.seasons > 0) || Number(json.step_days) !== 2) throw new Error('The snow archive has no valid season metadata.');
    return meta;
}

/** The season and day index of an ISO date. A leap day counts as 28 February, so each season has 183 indices. */
export function seasonDay(date: string): { season: number; index: number } {
    const [year, month, day] = date.split('-').map(Number);
    const days = (Date.UTC(month >= 9 ? 2001 : 2002, month - 1, month === 2 ? Math.min(day, 28) : day) - SEPTEMBER) / DAY_MS;
    return { season: month >= 9 ? year : year - 1, index: Math.min(DAYS - 1, Math.floor(days / 2)) };
}

/** "15 Jun" for the first day of a day index, or its last day with `end`. */
export function indexLabel(index: number, end = false): string {
    return dateLabel(new Date(SEPTEMBER + (2 * index + (end ? 1 : 0)) * DAY_MS));
}

// Calendar columns in season terms: Jan–Aug belong to the season before, here 2000.
const columns = columnDays.map(day => seasonDay(day.toISOString().slice(0, 10)));

function shareClass(snow: number, known: number): number {
    if (!known) return UNKNOWN;
    if (!snow) return FREE;
    return 3 * snow < known ? MOSTLY_FREE : 3 * snow <= 2 * known ? MOSTLY_SNOW : SNOW;
}

/** Seasons with snow on day `index` at item `i`, and seasons with data there. */
function snowSeasons(p: Planar, i: number, index: number): [number, number] {
    let snow = 0, known = 0;
    for (let s = 0; s < p.seasons; s++) {
        const onset = p.data[2 * s * p.size + i], melt = p.data[(2 * s + 1) * p.size + i];
        if (onset === NO_DATA || melt === NO_DATA) continue;
        known++;
        if (onset === WHOLE_SEASON || (onset <= index && index <= melt)) snow++;
    }
    return [snow, known];
}

/** The class of item `i` on day `index`: the share of seasons with data that had snow on that day. */
export function snowClass(p: Planar, i: number, index: number): number {
    return shareClass(...snowSeasons(p, i, index));
}

/**
 * Per season, the route melts out on the latest melt-out of its points and snows in on the earliest
 * onset. The route is clear on day d ⇔ d > melt or d < onset. Null marks a season without data.
 */
function routeSeasons(p: Planar): ({ melt: number; onset: number } | null)[] {
    return Array.from({ length: p.seasons }, (_, s) => {
        let melt = -1, onset = DAYS, known = false;
        for (let i = 0; i < p.size; i++) {
            const a = p.data[2 * s * p.size + i], b = p.data[(2 * s + 1) * p.size + i];
            if (a === NO_DATA || b === NO_DATA) continue;
            known = true;
            if (a === NO_SNOW) continue;
            if (a === WHOLE_SEASON) return { melt: DAYS, onset: -1 };
            melt = Math.max(melt, b);
            onset = Math.min(onset, a);
        }
        return known ? { melt, onset } : null;
    });
}
const routeCache = new WeakMap<Planar, ReturnType<typeof routeSeasons>>();
const seasonsOf = (p: Planar) => routeCache.get(p) ?? routeCache.set(p, routeSeasons(p)).get(p)!;

const years = (n: number) => `${n} ${n === 1 ? 'year' : 'years'}`;
const median = (values: number[]) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)];

/** The two stats lines and the year strip of a route for a date. `km` holds the distance of each item. */
export function snowStats(p: Planar, km: ArrayLike<number>, date: string, theme: Theme) {
    const { index } = seasonDay(date);
    const on = dateLabel(date);
    const seasons = seasonsOf(p).filter(season => season !== null);
    const clearOn = (day: number) => seasons.filter(({ melt, onset }) => day > melt || day < onset).length;
    const cells = Uint8Array.from(columns, ({ index }) => shareClass(seasons.length - clearOn(index), seasons.length));
    const year = { label: seasons.length ? `Whole route snow-free on ${on}: ${clearOn(index)} of ${years(seasons.length)}` : 'No snow data along the route',
        grid: { rows: [{ label: '', cells }], marker: dateColumn(date), swatches: snowClasses(theme) } satisfies SeasonGrid };
    if (!seasons.length) return { headline: 'No snow data along the route', detail: '', year };
    let snowKm = 0;
    for (let i = 0; i < p.size; i++) if (snowClass(p, i, index) === SNOW) snowKm += reach(km, i);
    const headline = snowKm >= 0.05 ? `${snowKm.toFixed(1)} km snowed in on ${on}` : `Not snowed in on ${on}`;
    const snowy = seasons.filter(({ melt }) => melt >= 0);
    if (!snowy.length) return { headline, detail: `Snow-free in all ${years(seasons.length)}`, year };
    // The next change after the date: the melt-out inside the typical snow period, else the next onset.
    const autumn = !(median(snowy.map(s => s.onset)) <= index && index <= median(snowy.map(s => s.melt)));
    // The first clear day after melt-out, or the last clear day before the onset; DAYS or -1 when the route never clears.
    const days = seasons.map(({ melt, onset }) => autumn ? onset - 1 : melt + 1);
    const never = days.filter(day => day < 0 || day >= DAYS).length;
    const middle = median(days);
    const label = (day: number) => indexLabel(day, autumn);
    if (middle < 0 || middle >= DAYS) return { headline, detail: `Not clear in ${never} of ${years(seasons.length)}`, year };
    const range = never ? `not clear in ${never} of ${years(seasons.length)}` : `${label(Math.min(...days))} – ${label(Math.max(...days))}`;
    return { headline, detail: `${autumn ? 'Clear until' : 'Clear from'} ${label(middle)} · ${range}`, year };
}

/** Calendar years (rows, newest last) × days at item `i`: 0 clear, 1 snow, 2 no data, 255 outside the record. */
export function snowGrid(p: Planar, i: number, firstSeason: number, date: string, theme: Theme): { headline: string; grid: SeasonGrid } {
    const [snow, known] = snowSeasons(p, i, seasonDay(date).index);
    const rows = Array.from({ length: p.seasons }, (_, row) => {
        const year = firstSeason + 1 + row;
        const cells = Uint8Array.from(columns, ({ season, index: day }) => {
            const s = season - 2001 + year - firstSeason;
            if (s < 0 || s >= p.seasons) return 255;
            const onset = p.data[2 * s * p.size + i], melt = p.data[(2 * s + 1) * p.size + i];
            return onset === NO_DATA || melt === NO_DATA ? 2 : onset === WHOLE_SEASON || (onset <= day && day <= melt) ? 1 : 0;
        });
        return { label: String(year), cells };
    });
    const palette = colors[theme];
    return {
        headline: known ? `Snow on ${dateLabel(date)} in ${snow} of ${years(known)}` : 'No snow data here',
        grid: { rows, marker: dateColumn(date), swatches: [
            { label: 'Clear', color: palette.clear }, { label: 'Snow on the ground', color: palette.history }, { label: 'No data', color: palette.unknown, hatch: true },
        ] },
    };
}

export const colors = {
    light: { free: '#e6e0cc', mostlyFree: '#d8e2e8', mostlySnow: '#a8bfcd', snow: '#ffffff', unknown: '#b8b5ac', clear: '#efede4', history: '#5f7d91' },
    dark: { free: '#3b382b', mostlyFree: '#34434c', mostlySnow: '#5f7a8c', snow: '#dfe7ec', unknown: '#6b685c', clear: '#2b2a21', history: '#9fb7c6' },
} as const;

/** Swatches by class value. */
export function snowClasses(theme: Theme): Swatch[] {
    const c = colors[theme];
    return [
        { label: 'Snow-free', color: c.free, lineOnly: true }, { label: 'Mostly snow-free', color: c.mostlyFree },
        { label: 'Mostly still snow', color: c.mostlySnow }, { label: 'Snow', color: c.snow }, { label: 'No data', color: c.unknown, hatch: true },
    ];
}
