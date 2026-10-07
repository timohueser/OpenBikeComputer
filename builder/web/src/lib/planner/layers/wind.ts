// The Wind layer from the climate archive: strength colours, the arrows of the map, the headwind
// chance along a route and the rose at a point. Everywhere, a direction is where the wind blows towards.
import type { ExpressionSpecification } from 'maplibre-gl';
import type { FeatureCollection, Point, Polygon } from 'geojson';
import { OVERVIEW, SECTORS, WEEKS, cellAt, cellCentre, cellOf, locate, read, tileCells, weekMonth, windChance, windMode, windRose, windows, type ClimateTile } from './climate';
import type { CellRef } from './climate-source';
import { mix, noData } from './colour';
import { dateLabel, weekOf, type Chart, type Grid, type Legend, type Swatch, type Theme } from './data-layer';

/**
 * The strength colours at 1 to 5 m/s; slower winds take the first colour and faster winds the last.
 * The ten-year weekly means of Baden-Württemberg span 1 to 4 m/s, so the steps there stay apart.
 */
const SPEEDS = [1, 2, 3, 4, 5];
const RAMP = {
    light: ['#eef0f3', '#c9d1de', '#97a3bb', '#66738f', '#3f4964'],
    dark: ['#262a32', '#38404f', '#56607a', '#8590ac', '#c3cbe0'],
};

export function speedColor(speed: number, theme: Theme): string {
    const ramp = RAMP[theme], at = Math.min(SPEEDS.length - 1, Math.max(0, speed - SPEEDS[0])), i = Math.min(SPEEDS.length - 2, Math.floor(at));
    return mix(ramp[i], ramp[i + 1], at - i);
}

/** The map colour of a cell from its `speed` property. */
export function speedPaint(theme: Theme): ExpressionSpecification {
    return ['interpolate', ['linear'], ['get', 'speed'], ...SPEEDS.flatMap((speed, i) => [speed, RAMP[theme][i]])] as ExpressionSpecification;
}

/** The map paints a cell from this mean speed (owner decision): calm weeks keep the map unpainted, with arrows only. */
export const PAINT_FROM = 2.5;

/** km/h per m/s. */
const KMH = 3.6;

/**
 * A speed in m/s as its 5 km/h range, "10–15": the value is the mean of a ~9 km cell, so the UI
 * never shows a single precise speed. Map labels pass a hyphen, which the first glyph range holds.
 */
export function kmhRange(speed: number, dash = '–'): string {
    const low = 5 * Math.floor(speed * KMH / 5 + 1e-9);
    return `${low}${dash}${low + 5}`;
}

/** The map legend: the paint scale from `PAINT_FROM` to the fastest ramp speed in 0.5 m/s steps, labelled at both ends in km/h, and the arrow key. */
export function mapLegend(theme: Theme): Legend {
    const top = SPEEDS.at(-1)!, steps = 2 * (top - PAINT_FROM);
    return {
        scale: Array.from({ length: steps + 1 }, (_, i) => {
            const speed = PAINT_FROM + i / 2, kmh = Math.round(speed * KMH);
            return { color: speedColor(speed, theme), label: i === steps ? `${kmh}+ km/h` : i ? '' : String(kmh) };
        }),
        marks: [{ label: 'Light wind: no colour', path: 'M5 8h14v8H5Z', width: 1 }, ...ARROW_MARKS],
    };
}

/** Arrows shown at most this close, in screen pixels, so they never clutter at low zoom. */
const ARROW_GAP = 44;

/** Arrows sit on every `stride`th cell column and row: a cell is 0.1° of longitude, 512 × 2^zoom ÷ 3600 px wide. */
export function arrowStride(zoom: number): number {
    return 2 ** Math.max(0, Math.ceil(Math.log2(ARROW_GAP * 3600 / (512 * 2 ** zoom))));
}

/**
 * Arrow weights by the steadiness of `windMode`. Chosen on the Baden-Württemberg archive: a third of
 * the cell-months are thin, below 0.37, and a quarter are thick, from 0.44.
 */
export const STEADY = [0.37, 0.44];
export const ARROW_ICONS = ['arrow', 'double'].flatMap(kind => [0, 1, 2].map(weight => `wind-${kind}-${weight}`));

export function arrowIcon({ steadiness, opposite }: { steadiness: number; opposite?: number }): string {
    return `wind-${opposite === undefined ? 'arrow' : 'double'}-${STEADY.filter(edge => steadiness >= edge).length}`;
}

/**
 * The rotation of an arrow in degrees: the main direction, or for two opposite winds the mean axis of
 * both, so a double arrow is one straight line between the two directions.
 */
export function arrowAxis({ towards, opposite }: { towards: number; opposite?: number }): number {
    if (opposite === undefined) return 22.5 * towards;
    const offset = ((opposite - towards) % SECTORS + SECTORS) % SECTORS - SECTORS / 2;
    return (22.5 * (towards + offset / 2) + 360) % 360;
}

/** The arrow key of the map legend, drawn in a 24 px box. */
const ARROW_MARKS = [
    { label: 'Most often blows towards', path: 'M4 12h15m-4-4 4 4-4 4', width: 1.5 },
    { label: 'Steady', path: 'M4 12h14m-4-4 4 4-4 4', width: 3 },
    { label: 'Two opposite winds', path: 'M5 12h14M9 8l-4 4 4 4m6-8 4 4-4 4', width: 2 },
];

/** The overview tiles in view: the tiles that hold the cells of the corners of view ∩ archive. */
export function viewTiles([west, south, east, north]: number[], bounds: number[]): [number, number][] {
    const w = Math.max(west, bounds[0]), s = Math.max(south, bounds[1]), e = Math.min(east, bounds[2]), n = Math.min(north, bounds[3]);
    if (w > e || s > n) return [];
    const from = locate(OVERVIEW, cellAt([w, n])), to = locate(OVERVIEW, cellAt([e, s]));
    const tiles: [number, number][] = [];
    for (let y = from.y; y <= to.y; y++) for (let x = from.x; x <= to.x; x++) tiles.push([x, y]);
    return tiles;
}

/** Cell edges from the edge index, so neighbours share exact corners and the fill has no seams. */
const edgeLon = (col: number) => -180 + 0.1 * (col - 0.5), edgeLat = (row: number) => 90 - 0.1 * (row - 0.5);

export interface WindMap {
    cells: FeatureCollection<Polygon, { speed: number }>;
    arrows: FeatureCollection<Point, { icon: string; rotate: number; label: string }>;
}

/**
 * The cells of the tiles from `PAINT_FROM` with their mean speed of `week`, and on every `stride`th
 * cell with data an arrow of `month` with the km/h range of the week.
 */
export function windMap(tiles: ClimateTile[], week: number, month: number, stride: number): WindMap {
    const cells: WindMap['cells']['features'] = [], arrows: WindMap['arrows']['features'] = [];
    for (const tile of tiles) {
        for (let index = 0; index < tileCells(OVERVIEW); index++) {
            const speed = read(tile, 'wind', week, index);
            if (Number.isNaN(speed)) continue;
            const cell = cellOf(tile, index);
            const [w, e, n, s] = [edgeLon(cell.col), edgeLon(cell.col + 1), edgeLat(cell.row), edgeLat(cell.row + 1)];
            if (speed >= PAINT_FROM) cells.push({ type: 'Feature', properties: { speed }, geometry: { type: 'Polygon', coordinates: [[[w, s], [e, s], [e, n], [w, n], [w, s]]] } });
            if (cell.col % stride || cell.row % stride) continue;
            const mode = windMode(windRose(tile, index, month));
            if (mode) arrows.push({ type: 'Feature', properties: { icon: arrowIcon(mode), rotate: arrowAxis(mode), label: kmhRange(speed, '-') }, geometry: { type: 'Point', coordinates: cellCentre(cell) } });
        }
    }
    return { cells: { type: 'FeatureCollection', features: cells }, arrows: { type: 'FeatureCollection', features: arrows } };
}

/**
 * Classes of the headwind chance. Chosen on the Baden-Württemberg archive: an even rose gives 25 %;
 * over the cell-months and the main bearings, the median is 26 % and 95 % stay below 49 %.
 */
export const HEADWIND_EDGES = [0.15, 0.3, 0.45];
export const NO_WIND = HEADWIND_EDGES.length + 1;

export function headClass(chance: number): number {
    return Number.isNaN(chance) ? NO_WIND : HEADWIND_EDGES.filter(edge => chance >= edge).length;
}

const HEAD = {
    light: ['#e6e9ee', '#c3cad6', '#8792ab', '#414a66'],
    dark: ['#2c3040', '#4a5268', '#8a93ad', '#c3cbe0'],
};

/** Swatches by headwind class; the last is no data. */
export function headFills(theme: Theme): Swatch[] {
    const c = HEAD[theme];
    return [
        { label: 'Headwind under 15 %', color: c[0] }, { label: '15–30 %', color: c[1] }, { label: '30–45 %', color: c[2] },
        { label: '45 % or more', color: c[3] }, noData(theme),
    ];
}

/** Rose petals: headwind and tailwind for a travel direction, the rest crosswind; `even` without a direction. */
export function roseColors(theme: Theme) {
    const c = HEAD[theme];
    return { head: c[3], tail: c[1], cross: theme === 'dark' ? '#8a8670' : '#bdb7a0', even: c[2] };
}

/** The headwind class of each sample of a line for its travel bearing in `month`. */
export function headClasses(cells: (CellRef | undefined)[], bearings: ArrayLike<number>, month: number): Uint8Array {
    let last: CellRef | undefined, rose: ArrayLike<number> = [];
    return Uint8Array.from(cells, (cell, i) => {
        if (!cell) return NO_WIND;
        if (cell.tile !== last?.tile || cell.index !== last.index) rose = windRose(cell.tile, cell.index, month);
        last = cell;
        return headClass(windChance(rose, bearings[i]).head);
    });
}

const monthName = (month: number) => new Date(Date.UTC(2001, month, 15)).toLocaleDateString('en-GB', { month: 'long', timeZone: 'UTC' });
const percent = (share: number) => `${Math.round(100 * share)} %`;
const COMPASS = ['N', 'NNE', 'NE', 'ENE', 'E', 'ESE', 'SE', 'SSE', 'S', 'SSW', 'SW', 'WSW', 'W', 'WNW', 'NW', 'NNW'];

/** " · wind typically 10–15 km/h", or nothing without a speed. */
const typically = (speed: number) => Number.isNaN(speed) ? '' : ` · wind typically ${kmhRange(speed)} km/h`;

/** The year slider: the headwind chance of each month along the route from `windRow`, and the typical speed of the week from `speedRow`. */
export function windYear(route: { row: ArrayLike<number>; speeds: ArrayLike<number> } | null, date: string, theme: Theme): Grid {
    const week = weekOf(date), month = weekMonth(week);
    const label = !route ? 'Plan a route to see the chance of headwind along it.'
        : Number.isNaN(route.row[month]) ? 'No wind data along the route'
        : `Chance of headwind on the route in ${monthName(month)}: ${percent(route.row[month])}${typically(route.speeds[week])}`;
    const cells = route ? Uint8Array.from({ length: WEEKS }, (_, w) => headClass(route.row[weekMonth(w)])) : new Uint8Array(WEEKS).fill(255);
    return { label, columns: WEEKS, rows: [{ label: '', cells }], fills: headFills(theme) };
}

export const WIND_NOTE = '~9 km grid, daytime wind (09–18 h)';

/** The rose of a point chart: the shares by towards-sector, the travel bearing on a route and the facts beside it. */
export interface RoseView { shares: number[]; bearing?: number; month: string; facts: string[] }

/** The three sectors of a `windMode` window, named by its outer sectors: "W–NW" for the window centred on WNW. */
const span = (sector: number) => `${COMPASS[(sector + SECTORS - 1) % SECTORS]}–${COMPASS[(sector + 1) % SECTORS]}`;

/**
 * The plain facts of a point: the main directions of the month of `date`, or on a route the headwind
 * and tailwind chances for the travel bearing, and the typical speed of the week. Shares are whole
 * percent; the speed is the mean over all directions, so it has its own line.
 */
export function windFacts(rose: ArrayLike<number>, speed: number, date: string, bearing?: number): string[] {
    const mode = windMode(rose);
    if (!mode) return [];
    const name = monthName(weekMonth(weekOf(date)));
    const second = mode.opposite === undefined ? '' : ` and ${span(mode.opposite)} on ${percent(windows(rose)[mode.opposite])}`;
    const direction = `Blows towards ${span(mode.towards)} on ${percent(mode.steadiness)}${second} of ${name} daytime hours`;
    const strength = Number.isNaN(speed) ? [] : [`Wind typically ${kmhRange(speed)} km/h around ${dateLabel(date)} (daytime, all directions)`];
    if (bearing === undefined) return [direction, ...strength];
    const { head, tail } = windChance(rose, bearing);
    return [`Headwind on ${percent(head)} of hours for your direction`, `Tailwind on ${percent(tail)} of hours for your direction`, ...strength];
}

/** The point chart: the daytime rose of the month with its facts, from the overview cell. */
export function windChart({ overview, bearing }: { overview?: CellRef; bearing?: number }, date: string): { chart: Chart; rose?: RoseView } {
    const week = weekOf(date), month = weekMonth(week), none = { chart: { headline: 'No wind data here', grids: [], note: WIND_NOTE } };
    if (!overview) return none;
    const shares = windRose(overview.tile, overview.index, month), facts = windFacts(shares, read(overview.tile, 'wind', week, overview.index), date, bearing);
    if (!facts.length) return none;
    const name = monthName(month);
    return { chart: { headline: `Daytime wind in ${name}`, grids: [], note: WIND_NOTE }, rose: { shares: Array.from(shares), bearing, month: name, facts } };
}
