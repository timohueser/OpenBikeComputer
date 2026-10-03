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

/** "15 Jun" for the first day of a day index, or its last day with `end`. The last index ends on 31 August. */
export function indexLabel(index: number, end = false): string {
    return dateLabel(new Date(SEPTEMBER + Math.min(2 * index + (end ? 1 : 0), 364) * DAY_MS));
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

// The four pixels around a point and their bilinear weights.
const near = new Int32Array(4), weight = new Float64Array(4);

/**
 * Snow in season `s` on day `index` by the spec's blend rule over the four pixels in `near`: 1 snow,
 * 0 clear, -1 no data. Days blend only between dated pixels, so a sentinel never mixes with a date.
 */
function blendSeason(p: Planar, s: number, index: number): number {
    const base = 2 * s * p.size;
    let dated = 0, full = 0, none = 0, missing = 0, onset = 0, melt = 0;
    for (let k = 0; k < 4; k++) {
        const w = weight[k], value = p.data[base + near[k]];
        if (value < NO_SNOW) {
            dated += w;
            onset += w * value;
            melt += w * p.data[base + p.size + near[k]];
        } else if (value === WHOLE_SEASON) full += w;
        else if (value === NO_SNOW) none += w;
        else missing += w;
    }
    if (missing > 0.5) return -1;
    return (dated >= full && dated >= none ? onset <= index * dated && index * dated <= melt : full >= none) ? 1 : 0;
}

/**
 * The four pixels in `near` have the same snow state in season `s` on day `index`, so every blend
 * between them has it too: a blended day is a weighted mean, so it stays on the same side of `index`.
 */
function steady(p: Planar, s: number, index: number): boolean {
    const base = 2 * s * p.size, first = p.data[base + near[0]];
    const dated = first < NO_SNOW, after = first > index, before = p.data[base + p.size + near[0]] < index;
    for (let k = 1; k < 4; k++) {
        const onset = p.data[base + near[k]];
        if (dated ? onset >= NO_SNOW || onset > index !== after || p.data[base + p.size + near[k]] < index !== before : onset !== first) return false;
    }
    return true;
}

/**
 * Writes the classes of a 256 px tile for day `index` as `palette` words; a missing tile is no data.
 * Above the data zoom, the tile is the (`x`, `y`) part of `scale` × `scale` parts of `p`, and each
 * screen pixel blends the day values of the four nearest data pixels, so class borders are smooth.
 */
export function paintTile(words: Uint32Array, p: Planar | undefined, index: number, scale: number, x: number, y: number, palette: Uint32Array) {
    const draw = (px: number, py: number, value: number) => {
        // Diagonal lines every 8 px line up across tile edges.
        if (value && (value !== UNKNOWN || ((px + py) & 7) < 2)) words[py * 256 + px] = palette[value];
    };
    if (!p || scale === 1) {
        for (let i = 0; i < 256 * 256; i++) draw(i & 255, i >> 8, p ? snowClass(p, i, index) : UNKNOWN);
        return;
    }
    // Runs of screen rows or columns whose centres lie between the same two data pixels; the tile edge repeats its pixels.
    const runs = (start: number) => {
        const list: { from: number; to: number; low: number; high: number }[] = [], part = new Float64Array(256);
        for (let s = 0; s < 256; s++) {
            const at = Math.min(255, Math.max(0, start + (s + 0.5) / scale - 0.5)), low = Math.floor(at), last = list.at(-1);
            part[s] = at - low;
            if (last?.low === low) last.to = s + 1;
            else list.push({ from: s, to: s + 1, low, high: Math.min(255, low + 1) });
        }
        return { list, part };
    };
    const columns = runs(x * 256 / scale), rows = runs(y * 256 / scale);
    const crossing = new Int32Array(p.seasons);
    for (const row of rows.list) {
        for (const column of columns.list) {
            near[0] = row.low * 256 + column.low; near[1] = row.low * 256 + column.high;
            near[2] = row.high * 256 + column.low; near[3] = row.high * 256 + column.high;
            // A steady season counts once for the whole cell; only the others blend at each screen pixel.
            let snow = 0, known = 0, count = 0;
            for (let s = 0; s < p.seasons; s++) {
                if (!steady(p, s, index)) crossing[count++] = s;
                else {
                    const onset = p.data[2 * s * p.size + near[0]], melt = p.data[(2 * s + 1) * p.size + near[0]];
                    if (onset !== NO_DATA) {
                        known++;
                        if (onset === WHOLE_SEASON || (onset <= index && index <= melt)) snow++;
                    }
                }
            }
            for (let py = row.from; py < row.to; py++) {
                const fr = rows.part[py];
                for (let px = column.from; px < column.to; px++) {
                    let pixelSnow = snow, pixelKnown = known;
                    if (count) {
                        const fc = columns.part[px];
                        weight[0] = (1 - fr) * (1 - fc); weight[1] = (1 - fr) * fc; weight[2] = fr * (1 - fc); weight[3] = fr * fc;
                        for (let c = 0; c < count; c++) {
                            const state = blendSeason(p, crossing[c], index);
                            if (state >= 0) { pixelKnown++; pixelSnow += state; }
                        }
                    }
                    draw(px, py, shareClass(pixelSnow, pixelKnown));
                }
            }
        }
    }
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
    if (!autumn && 2 * clearOn(index) > seasons.length) return { headline, detail: 'Usually clear on this date', year };
    if (autumn && 2 * snowy.length < seasons.length) return { headline, detail: `Snow in only ${snowy.length} of ${years(seasons.length)}`, year };
    // Seasons without snow have no change, so only snowy seasons give dates.
    // The first clear day after melt-out, or the last clear day before the onset; DAYS or -1 when the route never clears.
    const days = snowy.map(({ melt, onset }) => autumn ? onset - 1 : melt + 1);
    const never = days.filter(day => day < 0 || day >= DAYS).length;
    const middle = median(days);
    const label = (day: number) => indexLabel(day, autumn);
    if (middle < 0 || middle >= DAYS) return { headline, detail: `Not clear in ${never} of ${years(seasons.length)}`, year };
    const range = never ? `not clear in ${never} of ${years(seasons.length)}` : `${label(Math.min(...days))} – ${label(Math.max(...days))}`;
    return { headline, detail: `${autumn ? 'Clear until' : 'Clear from'} ${label(middle)} · ${range}`, year };
}

// One row per season; its columns are the season's day indices, from 1 September.
const gridRows = (firstSeason: number, seasons: number) => Array.from({ length: seasons }, (_, s) => `${firstSeason + s}/${String((firstSeason + s + 1) % 100).padStart(2, '0')}`);

/** Seasons (rows, newest last) × days at item `i`: 0 clear, 1 snow, 2 no data. */
export function snowGrid(p: Planar, i: number, firstSeason: number, date: string, theme: Theme): { headline: string; grid: SeasonGrid } {
    const [snow, known] = snowSeasons(p, i, seasonDay(date).index);
    const rows = gridRows(firstSeason, p.seasons).map((label, s) => {
        const cells = Uint8Array.from({ length: DAYS }, (_, day) => {
            const onset = p.data[2 * s * p.size + i], melt = p.data[(2 * s + 1) * p.size + i];
            return onset === NO_DATA || melt === NO_DATA ? 2 : onset === WHOLE_SEASON || (onset <= day && day <= melt) ? 1 : 0;
        });
        return { label, cells };
    });
    const palette = colors[theme];
    return {
        headline: known ? `Snow on ${dateLabel(date)} in ${snow} of ${years(known)}` : 'No snow data here',
        grid: { rows, seasonal: true, marker: seasonDay(date).index, swatches: [
            { label: 'Clear', color: palette.clear }, { label: 'Snow on the ground', color: palette.history }, { label: 'No data', color: palette.unknown, hatch: true },
        ] },
    };
}

export const colors = {
    light: { free: '#e6e0cc', mostlyFree: '#d3dcdc', mostlySnow: '#a9c1d8', snow: '#e8f1fa', unknown: '#b8b5ac', clear: '#efede4', history: '#5f7d91' },
    dark: { free: '#3b382b', mostlyFree: '#33434e', mostlySnow: '#6a88a3', snow: '#e8f0f8', unknown: '#6b685c', clear: '#2b2a21', history: '#9fb7c6' },
} as const;

/** Swatches by class value. */
export function snowClasses(theme: Theme): Swatch[] {
    const c = colors[theme];
    return [
        { label: 'Snow-free', color: c.free, lineOnly: true }, { label: 'Mostly snow-free', color: c.mostlyFree },
        { label: 'Mostly still snow', color: c.mostlySnow }, { label: 'Snow', color: c.snow }, { label: 'No data', color: c.unknown, hatch: true },
    ];
}
