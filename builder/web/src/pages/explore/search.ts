/** The day-hike search of the Explore page: a query, the ways to reach each route of the loaded cells, the rows of each
 * shape, the limits, the wishes, the order, the counts and the suggestions for an empty list. */
import { kilometres, kmPerDegree, nearestOnLine, type Coordinate } from '../../lib/planner/geo';
import { tileRow } from '../../lib/planner/place-index';
import { decodeCoordinates } from '../../lib/planner/route-answer';
import { MAX_CELLS, type Box } from '../../lib/planner/route-catalog';
import type { CatalogFacts, CatalogRecord, FactsPlace } from '../../lib/planner/signed-routes';
import type { Range } from '../routes/filter';

export type Shape = 'loop' | 'back' | 'liftDown' | 'liftUp';
/** Loop, there and back, hike up and lift down, lift up and hike down: the order of a tie. */
export const SHAPES: Shape[] = ['loop', 'back', 'liftDown', 'liftUp'];
const LIFT_SHAPES: Shape[] = ['liftDown', 'liftUp'];

/** The wishes, in the order of the filter column; `via:<place id>` is "Passes <name>". */
export const WISHES = ['peak', 'lake', 'hut', 'waterfall', 'official', 'snow', 'quiet', 'forest', 'open', 'water'] as const;
export type Wish = typeof WISHES[number] | `via:${string}`;
/** The wish that a rider trusts for safety: no suggestion loosens it. */
const SAFE: Wish = 'snow';

/** The walk limit in metres; the bake stores the places within 3 km. */
export const WALK_M = { min: 200, max: 3000, default: 1000 } as const;
/** The drive limit in km, in steps of 5 km; the cap of a start point is at most `max`. */
export const DRIVE_KM = { min: 5, max: 50, step: 5, default: 30 } as const;
/** The farthest top station from its valley station that the search uses, and the last walk of the box of the reach. */
const LIFT_KM = 5, LAST_WALK_KM = 3;
/** The hardest grade index: T6. */
export const GRADE_TOP = 5;

export interface Query {
    /** The day of the hike, `YYYY-MM-DD`. */
    date: string;
    /** `[longitude, latitude]`. With no start point and no pin, every route of the map view counts. */
    start?: Coordinate;
    /** The walk limit in metres: to the route, to a valley station, and from a car park or a top station. */
    walk: number;
    car: boolean;
    /** The drive limit in km. A start point uses at most its cap. */
    drive: number;
    /** Lifts off, a lift when it helps, or always a lift. */
    lift?: 'helps' | 'always';
    /** The place id of the pinned car park or lift (its top station): the only start. */
    pin?: string;
    /** In metres, from `[0]` to below `[1]`; `[1]` is Infinity for no upper end. */
    length?: Range;
    climb?: Range;
    /** The accepted shapes. The lift shapes count only while lifts are on. */
    shapes: Shape[];
    /** Indices of `grades_m`: up to T(max + 1), and, when min > 0, a part of at least T(min + 1). */
    grade: [min: number, max: number];
    /** Keep the routes with no grade. Without it, a grade limit removes them. */
    ungraded: boolean;
    wishes: Partial<Record<Wish, 'nice' | 'must'>>;
}

export const defaultQuery = (today: string): Query =>
    ({ date: today, walk: WALK_M.default, car: false, drive: DRIVE_KM.default, shapes: [...SHAPES], grade: [0, GRADE_TOP], ungraded: true, wishes: {} });

/** The loaded files: the records of the map view, and the facts and places of their cells. */
export interface Loaded {
    records: CatalogRecord[];
    factsOf(id: number): CatalogFacts | undefined;
    place(id: string): FactsPlace | undefined;
}

/** A way to reach a row. Distances are straight lines in metres; the cost is the drive in km plus four times the walks in
 * km, plus 1 for a lift. `before` is the walk to the valley station of a lift: from the start point, or from the car park
 * of the lift after `drive`. `valley` reaches a lift shape at its valley station. */
export type Way = { cost: number } & (
    | { kind: 'any' }
    | { kind: 'foot'; walk: number }
    | { kind: 'car'; park: string; drive: number; walk: number }
    | { kind: 'pin'; place: string; walk: number }
    | { kind: 'lift'; lift: string; before: number; drive?: number; walk: number }
    | { kind: 'valley'; lift: string; before: number; drive?: number });

/** A route in one shape, with the numbers of that shape. */
export interface Row {
    record: CatalogRecord;
    facts?: CatalogFacts;
    shape: Shape;
    length_m: number;
    ascent_m: number;
    descent_m: number;
    moving_s?: number;
    /** The cheapest first. */
    ways: Way[];
    /** The nice wishes that the row meets, misses, and has no data for. */
    met: Wish[];
    missed: Wish[];
    unknown: Wish[];
}

// ── Reach ───────────────────────────────────────────────────────────────────

/** The box of the points within `km` of `at`. */
function around([lon, lat]: Coordinate, km: number): Box {
    const dy = km / kmPerDegree, dx = km / (kmPerDegree * Math.cos(lat * Math.PI / 180));
    return [lon - dx, lat - dy, lon + dx, lat + dy];
}

/**
 * The largest drive limit, a multiple of 5 km and at most 50 km, whose box of the reach with lifts on meets at most
 * `MAX_CELLS` cells of zoom 9 wherever a start point at `lat` lies. It depends on the latitude only: it stays when the
 * start point moves along a parallel or the lifts change, and never jumps for a small move.
 */
export function driveCap(lat: number): number {
    for (let drive = DRIVE_KM.max; drive > DRIVE_KM.min; drive -= DRIVE_KM.step) {
        const [west, south, east, north] = around([0, lat], drive + LIFT_KM + LAST_WALK_KM);
        const columns = (east - west) / 360 * 2 ** 9, rows = tileRow(south, 9) - tileRow(north, 9);
        // A box w cells wide meets at most floor(w) + 2 of them, edges included.
        if ((Math.floor(columns) + 2) * (Math.floor(rows) + 2) <= MAX_CELLS) return drive;
    }
    return DRIVE_KM.min;
}

/** The drive limit in km that the search uses. */
const driveOf = (query: Query) => query.start ? Math.min(query.drive, driveCap(query.start[1])) : query.drive;

/** The box of the reach: the start point widened by the first leg (the drive with the car, else the walk limit), 5 km with
 * lifts, and 3 km for the last walk. None with no start point. The map fits it when the way of getting there changes. */
export function reachBox(query: Query): Box | undefined {
    if (!query.start) return undefined;
    const first = query.car ? driveOf(query) : query.walk / 1000;
    return around(query.start, first + (query.lift ? LIFT_KM : 0) + LAST_WALK_KM);
}

/**
 * The box whose cells the page loads: the map view, or with a start point the part of the view inside the widest reach of
 * that start point (the cap, lifts on). So each suggestion and count of the view has its routes loaded, and the box meets
 * at most `MAX_CELLS` cells. Null when the view and the reach do not meet.
 */
export function loadBox(query: Query, view: Box): Box | null {
    if (!query.start) return view;
    const reach = around(query.start, driveCap(query.start[1]) + LIFT_KM + LAST_WALK_KM);
    const box: Box = [Math.max(view[0], reach[0]), Math.max(view[1], reach[1]), Math.min(view[2], reach[2]), Math.min(view[3], reach[3])];
    return box[0] <= box[2] && box[1] <= box[3] ? box : null;
}

// ── Ways ────────────────────────────────────────────────────────────────────

const lines = new WeakMap<CatalogRecord, Coordinate[]>();
function lineOf(record: CatalogRecord): Coordinate[] {
    let line = lines.get(record);
    if (!line) lines.set(record, line = decodeCoordinates(record.line_udeg ?? []));
    return line;
}

const point = ([lon, lat]: [number, number]): Coordinate => [lon / 1e6, lat / 1e6];

/** What one search reads for each route. */
interface Search { query: Query; loaded: Loaded; driveKm: number; step: number }

/** The lift `id` with its valley station, when the search uses it: its top station is at most 5 km from its valley. */
function liftOf(id: string, loaded: Loaded): FactsPlace | undefined {
    const place = loaded.place(id), valley = place?.valley_udeg;
    return place?.kind === 'lift' && valley && kilometres(point(place.at_udeg), point(valley)) <= LIFT_KM ? place : undefined;
}

/** The walk in km from the car park of a lift to its valley station; Infinity for a lift with no car park. */
function parkWalk(place: FactsPlace, loaded: Loaded): number {
    const park = place.parking ? loaded.place(place.parking) : undefined;
    return park ? kilometres(point(park.at_udeg), point(place.valley_udeg!)) : Infinity;
}

/** The ways from the start point to the valley station of `lift`: a walk, or a drive to its car park and the walk from
 * there. The cost holds the 1 of the lift. */
function toValley(lift: string, { query, loaded, driveKm }: Search): { before: number; drive?: number; cost: number }[] {
    const place = liftOf(lift, loaded), start = query.start;
    if (!place || !start) return [];
    const park = place.parking ? loaded.place(place.parking) : undefined, ways = [];
    const walk = kilometres(start, point(place.valley_udeg!)), fromPark = parkWalk(place, loaded);
    if (walk * 1000 <= query.walk) ways.push({ before: walk * 1000, cost: 4 * walk + 1 });
    const drive = park ? kilometres(start, point(park.at_udeg)) : Infinity;
    if (query.car && drive <= driveKm && fromPark * 1000 <= query.walk) ways.push({ drive: drive * 1000, before: fromPark * 1000, cost: drive + 4 * fromPark + 1 });
    return ways;
}

const byCost = (ways: Way[]) => ways.sort((a, b) => a.cost - b.cost);

/** The ways to reach a loop or a there-and-back route: a walk, a drive to a car park and its walk, a lift and the walk from
 * its top. A pinned car park or top station is the only start. */
function routeWays(record: CatalogRecord, facts: CatalogFacts | undefined, search: Search): Way[] {
    const { query, loaded, driveKm } = search, walk = query.walk;
    if (query.pin) {
        const list = loaded.place(query.pin)?.kind === 'lift' ? facts?.lifts : facts?.parking;
        const metres = list?.find(([place]) => place === query.pin)?.[1];
        return metres !== undefined && metres <= walk ? [{ kind: 'pin', place: query.pin, walk: metres, cost: 4 * metres / 1000 }] : [];
    }
    if (!query.start) return [{ kind: 'any', cost: 0 }];
    const start = query.start, ways: Way[] = [];
    if (query.lift !== 'always') {
        const line = lineOf(record), metres = line.length ? kilometres(start, nearestOnLine(line, start).at) * 1000 : Infinity;
        if (metres <= walk) ways.push({ kind: 'foot', walk: metres, cost: 4 * metres / 1000 });
        if (query.car) for (const [park, metres] of facts?.parking ?? []) {
            const at = loaded.place(park), drive = at ? kilometres(start, point(at.at_udeg)) : Infinity;
            if (metres <= walk && drive <= driveKm) ways.push({ kind: 'car', park, drive: drive * 1000, walk: metres, cost: drive + 4 * metres / 1000 });
        }
    }
    if (query.lift) for (const [lift, metres] of facts?.lifts ?? []) {
        if (metres > walk) continue;
        for (const way of toValley(lift, search)) ways.push({ kind: 'lift', lift, ...way, walk: metres, cost: way.cost + 4 * metres / 1000 });
    }
    return byCost(ways);
}

/** The ways to reach a lift shape: only at the valley station of its lift. A pinned car park reaches it only when it is the
 * car park of the lift; a pinned top station never does. */
function valleyWays(lift: string, search: Search): Way[] {
    const { query } = search, place = liftOf(lift, search.loaded);
    if (!place) return [];
    if (query.pin) {
        const walk = parkWalk(place, search.loaded);
        return place.parking === query.pin && walk * 1000 <= query.walk ? [{ kind: 'pin', place: query.pin, walk: walk * 1000, cost: 4 * walk + 1 }] : [];
    }
    if (!query.start) return [{ kind: 'any', cost: 0 }];
    return byCost(toValley(lift, search).map(way => ({ kind: 'valley', lift, ...way })));
}

// ── Wishes ──────────────────────────────────────────────────────────────────

const PLACE_KINDS: Partial<Record<Wish, FactsPlace['kind']>> = { peak: 'peak', lake: 'lake', hut: 'hut', waterfall: 'waterfall' };

/** The day step of `snow-seasons` for a day `YYYY-MM-DD`: half the days since the last 1 September, from 0 to 182. */
export function dayStep(date: string): number {
    const [year, month, day] = date.split('-').map(Number);
    const september = Date.UTC(month >= 9 ? year : year - 1, 8, 1);
    return Math.floor((Date.UTC(year, month - 1, day) - september) / 86_400_000 / 2);
}

const isMade = (record: CatalogRecord) => 'made' in record;

/** Whether a route meets a wish, or null with no data for it. */
function meets(record: CatalogRecord, facts: CatalogFacts | undefined, wish: Wish, step: number, loaded: Pick<Loaded, 'place'>): boolean | null {
    if (wish === 'official') return !isMade(record);
    if (!facts) return null;
    if (wish.startsWith('via:')) return !!facts.places?.some(([place]) => place === wish.slice(4));
    const kind = PLACE_KINDS[wish];
    if (kind) return !!facts.places?.some(([place]) => loaded.place(place)?.kind === kind);
    const canopy = facts.canopy_pct, free = facts.snow_free;
    switch (wish) {
        case 'snow': return !free ? null : free.length === 2 && (free[0] <= free[1] ? free[0] <= step && step <= free[1] : step >= free[0] || step <= free[1]);
        case 'quiet': return facts.big_road_pct <= 5;
        case 'forest': return canopy === undefined ? null : canopy >= 60;
        case 'open': return canopy === undefined ? null : canopy <= 20;
        default: return facts.water_pct >= 25;
    }
}

const ELEMENTS = 'nwr';
/** The wishes of a query in a fixed order: the table, then each place by element and id. */
export function wishesOf(query: Query, level?: 'nice' | 'must'): Wish[] {
    const place = (wish: string) => [ELEMENTS.indexOf(wish[4]), Number(wish.slice(5))];
    return (Object.keys(query.wishes) as Wish[]).filter(wish => !level || query.wishes[wish] === level).sort((a, b) => {
        const [x, y] = [WISHES.indexOf(a as never), WISHES.indexOf(b as never)];
        if (x >= 0 || y >= 0) return (x < 0 ? Infinity : x) - (y < 0 ? Infinity : y);
        const [p, q] = [place(a), place(b)];
        return p[0] - q[0] || p[1] - q[1];
    });
}

// ── Rows ────────────────────────────────────────────────────────────────────

const within = (value: number, range: Range | undefined) => !range || (value >= range[0] && value < range[1]);

function graded({ hardest }: CatalogRecord, { grade: [min, max], ungraded }: Query): boolean {
    if (hardest === undefined) return ungraded || (min === 0 && max === GRADE_TOP);
    return min <= hardest && hardest <= max;
}

/** The rows of a route that the query keeps, before its wishes. */
function rowsOf(record: CatalogRecord, search: Search): Omit<Row, 'met' | 'missed' | 'unknown'>[] {
    const { query, loaded } = search, facts = loaded.factsOf(record.id), rows: Omit<Row, 'met' | 'missed' | 'unknown'>[] = [];
    if (!graded(record, query)) return rows;
    const add = (shape: Shape, numbers: Pick<Row, 'length_m' | 'ascent_m' | 'descent_m' | 'moving_s'>, ways: () => Way[]) => {
        if (!query.shapes.includes(shape) || !within(numbers.length_m, query.length) || !within(numbers.ascent_m, query.climb)) return;
        const found = ways();
        if (found.length) rows.push({ record, facts, shape, ...numbers, ways: found });
    };
    const { length_m, ascent_m, descent_m, moving_s } = record, back_s = facts?.back_s;
    if (record.loop) {
        add('loop', { length_m, ascent_m, descent_m, moving_s }, () => routeWays(record, facts, search));
        return rows;
    }
    const both = ascent_m + descent_m;
    add('back', { length_m: 2 * length_m, ascent_m: both, descent_m: both, moving_s: moving_s === undefined || back_s === undefined ? undefined : moving_s + back_s },
        () => routeWays(record, facts, search));
    if (query.lift && facts?.lift_ends) {
        const [lift, valley] = facts.lift_ends;
        const forward = { length_m, ascent_m, descent_m, moving_s }, backward = { length_m, ascent_m: descent_m, descent_m: ascent_m, moving_s: back_s };
        const ways = () => valleyWays(lift, search);
        // Hike up from the valley and ride down; ride up and hike down from the top.
        add('liftDown', valley === 'start' ? forward : backward, ways);
        add('liftUp', valley === 'start' ? backward : forward, ways);
    }
    return rows;
}

export type Sort = 'match' | 'name' | 'shape' | 'length' | 'climb' | 'time' | 'grade' | 'access';

/** An absent value is Infinity and sorts last; Infinity minus Infinity is NaN, which `||` passes to the tie. */
const by = (value: (row: Row) => number) => (a: Row, b: Row) => value(a) - value(b);
const ORDERS: Record<Sort, (a: Row, b: Row) => number> = {
    match: (a, b) => b.met.length - a.met.length || (a.facts?.paved_pct ?? Infinity) - (b.facts?.paved_pct ?? Infinity) || a.ways[0].cost - b.ways[0].cost,
    name: (a, b) => (a.record.name ?? a.record.ref ?? '').localeCompare(b.record.name ?? b.record.ref ?? '', 'en'),
    shape: by(row => SHAPES.indexOf(row.shape)),
    length: by(row => row.length_m),
    climb: by(row => row.ascent_m),
    time: by(row => row.moving_s ?? Infinity),
    grade: by(row => row.record.hardest ?? Infinity),
    access: by(row => row.ways[0].cost),
};
const tie = (a: Row, b: Row) => a.record.id - b.record.id || SHAPES.indexOf(a.shape) - SHAPES.indexOf(b.shape);

/**
 * The rows of the loaded routes that the query keeps, in the order `sort`. "Best match": most nice wishes met, then the
 * smallest paved share, then the lowest cost of a way; each order then takes the route id and the shape. With no start
 * point every cost is 0.
 */
export function find(query: Query, loaded: Loaded, sort: Sort = 'match'): Row[] {
    const search: Search = { query, loaded, driveKm: driveOf(query), step: dayStep(query.date) };
    const must = wishesOf(query, 'must'), nice = wishesOf(query, 'nice'), found: Row[] = [];
    for (const record of loaded.records) {
        for (const row of rowsOf(record, search)) {
            const meet = (wish: Wish) => meets(record, row.facts, wish, search.step, loaded);
            if (!must.every(wish => meet(wish) === true)) continue;
            const known = nice.map(wish => [wish, meet(wish)] as const);
            found.push({ ...row, met: known.filter(([, met]) => met).map(([wish]) => wish), missed: known.filter(([, met]) => met === false).map(([wish]) => wish),
                unknown: known.filter(([, met]) => met === null).map(([wish]) => wish) });
        }
    }
    return found.sort((a, b) => ORDERS[sort](a, b) || tie(a, b));
}

/**
 * The count of each wish: the rows of `rows` (the answer of `find`) that meet it, so the rows that the wish as a must would
 * keep. Null when the loaded routes give the wish nothing to tell: no route has its fact, or no route is made for
 * "Official route". `ungraded` counts the rows with no grade that "Include routes with an unknown grade" keeps.
 */
export function counts(query: Query, loaded: Loaded, rows: Row[]): { wishes: Map<Wish, number | null>; ungraded: number } {
    const step = dayStep(query.date), wishes = new Map<Wish, number | null>();
    for (const wish of [...WISHES, ...wishesOf(query).filter(wish => wish.startsWith('via:'))]) {
        const told = wish === 'official' ? loaded.records.some(isMade)
            : loaded.records.some(record => meets(record, loaded.factsOf(record.id), wish, step, loaded) !== null);
        wishes.set(wish, told ? rows.filter(row => meets(row.record, row.facts, wish, step, loaded)).length : null);
    }
    const ungraded = query.ungraded ? rows : find({ ...query, ungraded: true }, loaded);
    return { wishes, ungraded: ungraded.filter(row => row.record.hardest === undefined).length };
}

// ── Suggestions ─────────────────────────────────────────────────────────────

/** One change of the query that helps an empty list, the new query and its count of rows. */
export type Suggestion = { query: Query; count: number } & (
    | { kind: 'nice'; wish: Wish } | { kind: 'shape'; shape: Shape }
    | { kind: 'helps' | 'part' | 'walk' | 'drive' | 'car' | 'lifts' | 'unpin' | 'length' });

/**
 * The changes that give an empty list rows, at most four, in this order: each must as a nice wish, a lift when it helps in
 * place of always, no part of a grade needed, a walk 1 km longer, a drive 10 km longer up to the cap, the car on, lifts on,
 * the pin removed, each shape that is off, the length one `lengthStep` (metres) wider at each end. None loosens the grade
 * limit or the snow wish.
 */
export function suggestions(query: Query, loaded: Loaded, lengthStep: number): Suggestion[] {
    const tries: [Suggestion['kind'], Partial<Query>, Partial<Suggestion>?][] = [];
    for (const wish of wishesOf(query, 'must')) if (wish !== SAFE) tries.push(['nice', { wishes: { ...query.wishes, [wish]: 'nice' } }, { wish }]);
    if (query.lift === 'always') tries.push(['helps', { lift: 'helps' }]);
    if (query.grade[0] > 0) tries.push(['part', { grade: [0, query.grade[1]] }]);
    if (query.walk < WALK_M.max) tries.push(['walk', { walk: Math.min(WALK_M.max, query.walk + 1000) }]);
    const cap = query.start ? driveCap(query.start[1]) : DRIVE_KM.max;
    if (query.car && driveOf(query) < cap) tries.push(['drive', { drive: Math.min(cap, driveOf(query) + 10) }]);
    if (!query.car) tries.push(['car', { car: true }]);
    if (!query.lift) tries.push(['lifts', { lift: 'helps' }]);
    if (query.pin) tries.push(['unpin', { pin: undefined }]);
    for (const shape of SHAPES) {
        if (!query.shapes.includes(shape) && (query.lift || !LIFT_SHAPES.includes(shape))) {
            tries.push(['shape', { shapes: SHAPES.filter(one => one === shape || query.shapes.includes(one)) }, { shape }]);
        }
    }
    const [low, high] = query.length ?? [0, Infinity];
    if (low > 0 || high < Infinity) tries.push(['length', { length: [Math.max(0, low - lengthStep), high + lengthStep] }]);
    const found: Suggestion[] = [];
    for (const [kind, change, extra] of tries) {
        if (found.length === 4) break;
        const next = { ...query, ...change }, count = find(next, loaded).length;
        if (count) found.push({ kind, query: next, count, ...extra } as Suggestion);
    }
    return found;
}

// ── The page address ────────────────────────────────────────────────────────

/** The key of the last query in `localStorage`. */
export const KEPT_QUERY = 'explore-day-query';

const decimals = (value: number, digits: number) => String(Number(value.toFixed(digits)));
const rangeParam = ([low, high]: Range, unit: number) => `${decimals(low / unit, 3)}-${high === Infinity ? '' : decimals(high / unit, 3)}`;

/**
 * The query, and the map view, as the readable parameters after `?` of the page address: one for each value that is not
 * the default, in a fixed order, so the same query gives the same address. The same text keeps the query in the history
 * and in the browser.
 */
export function queryParams(query: Query, today: string, view?: Box): string {
    const params: [string, string | false | undefined][] = [
        ['date', query.date !== today && query.date],
        ['start', query.start?.map(value => decimals(value, 5)).join(',')],
        ['walk', query.walk !== WALK_M.default && String(query.walk)],
        ['car', query.car && '1'],
        ['drive', query.drive !== DRIVE_KM.default && String(query.drive)],
        ['lift', query.lift],
        ['pin', query.pin],
        ['length', query.length && rangeParam(query.length, 1000)],
        ['climb', query.climb && rangeParam(query.climb, 1)],
        ['shapes', query.shapes.length < SHAPES.length && SHAPES.filter(shape => query.shapes.includes(shape)).join(',')],
        ['grade', (query.grade[0] > 0 || query.grade[1] < GRADE_TOP) && `T${query.grade[0] + 1}-T${query.grade[1] + 1}`],
        ['ungraded', !query.ungraded && '0'],
        ['nice', wishesOf(query, 'nice').join(',') || undefined],
        ['must', wishesOf(query, 'must').join(',') || undefined],
        ['view', view?.map(value => decimals(value, 5)).join(',')],
    ];
    // The values hold no character that needs escaping. `shapes=` is no accepted shape.
    return params.filter(([, value]) => typeof value === 'string').map(([key, value]) => `${key}=${value}`).join('&');
}

function isDay(text: string): boolean {
    const time = Date.parse(`${text}T00:00:00Z`);
    return /^\d{4}-\d{2}-\d{2}$/.test(text) && Number.isFinite(time) && new Date(time).toISOString().startsWith(text);
}

function readRange(text: string | null, unit: number): Range | undefined {
    const match = text && /^(\d+(?:\.\d+)?)-(\d+(?:\.\d+)?)?$/.exec(text);
    if (!match) return undefined;
    const low = Math.round(Number(match[1]) * unit), high = match[2] === undefined ? Infinity : Math.round(Number(match[2]) * unit);
    return low < high && (low > 0 || high < Infinity) ? [low, high] : undefined;
}

/** The query and the map view of `queryParams`. A value that is not valid keeps its default; a day in the past is today. */
export function readQuery(text: string, today: string): { query: Query; view?: Box } {
    const params = new URLSearchParams(text), query = defaultQuery(today);
    const numbers = (key: string, count: number) => {
        const values = params.get(key)?.split(',').map(Number);
        return values?.length === count && values.every(Number.isFinite) ? values : undefined;
    };
    const date = params.get('date');
    if (date && isDay(date) && date > today) query.date = date;
    const start = numbers('start', 2);
    if (start && Math.abs(start[0]) <= 180 && Math.abs(start[1]) <= 85) query.start = start as Coordinate;
    const walk = Number(params.get('walk'));
    if (walk >= WALK_M.min && walk <= WALK_M.max) query.walk = walk;
    query.car = params.get('car') === '1';
    const drive = Number(params.get('drive'));
    if (drive >= DRIVE_KM.min && drive <= DRIVE_KM.max && drive % DRIVE_KM.step === 0) query.drive = drive;
    const lift = params.get('lift');
    if (lift === 'helps' || lift === 'always') query.lift = lift;
    const pin = params.get('pin');
    if (pin && /^[nwr]\d+$/.test(pin)) query.pin = pin;
    query.length = readRange(params.get('length'), 1000);
    query.climb = readRange(params.get('climb'), 1);
    const shapes = params.get('shapes');
    if (shapes !== null) query.shapes = SHAPES.filter(shape => shapes.split(',').includes(shape));
    const grade = /^T([1-6])-T([1-6])$/.exec(params.get('grade') ?? '');
    if (grade && grade[1] <= grade[2]) query.grade = [Number(grade[1]) - 1, Number(grade[2]) - 1];
    query.ungraded = params.get('ungraded') !== '0';
    for (const level of ['nice', 'must'] as const) {
        for (const wish of params.get(level)?.split(',') ?? []) {
            if ((WISHES as readonly string[]).includes(wish) || /^via:[nwr]\d+$/.test(wish)) query.wishes[wish as Wish] = level;
        }
    }
    const view = numbers('view', 4);
    return view && view[0] < view[2] && view[1] < view[3] ? { query, view: view as Box } : { query };
}

/** The query that the page opens with: from its address, else from `history.state.explore`, else the last query kept in
 * the browser. Each is the text of `queryParams`. */
export function openingQuery(address: string, kept: unknown, stored: string | null, today: string): { query: Query; view?: Box } {
    const text = [address, kept, stored].find((one): one is string => typeof one === 'string' && one !== '');
    return readQuery(text ?? '', today);
}

/** The last query kept in the browser; the page works without storage. */
export function storedQuery(): string | null {
    try {
        return localStorage.getItem(KEPT_QUERY);
    } catch {
        return null;
    }
}

export function keepQuery(query: Query, today: string) {
    try {
        localStorage.setItem(KEPT_QUERY, queryParams(query, today));
    } catch {
        // Not kept.
    }
}
