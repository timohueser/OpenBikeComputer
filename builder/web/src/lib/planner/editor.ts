import { movingSecondsAt, pacedLine, progressAtSeconds, type RoutingLine } from './routing';
import { profileAscent, profileDescent } from './profile-data';
import { ridingProfiles } from './riding-profiles';
import { cumulative, firstIndex, nearestOnLine, nearestProgress, routeSlice, segmentDistances, type Coordinate } from './geo';
import type { MapSegment } from './map-types';
import type { PlaceCategory } from './poi-kinds';
import { formatMinutes } from '../settings/format';

/** A vertex of a drawn leg; a third number is its elevation in metres. */
export type DrawnCoordinate = Coordinate | [number, number, number];
/** The device reads 32 days of a trip object (`MAX_TRIP_DAYS` of `obc-route`). */
export const partDays = 32;
/** A plan goes to the device as two trip objects of `partDays` days at most. */
export const maxRidingDays = 2 * partDays;
export type LegMode = 'routed' | 'straight' | 'drawn';
/** A kind of place to sleep. */
export type Stay = 'camp' | 'hut' | 'hostel' | 'hotel' | 'shelter';
export const allStay: Stay[] = ['camp', 'hut', 'hostel', 'hotel', 'shelter'];
export type PointKind = 'start' | 'finish' | 'pass' | 'via' | 'waypoint' | 'detour' | 'night' | 'marker' | 'place';
export interface RoutePoint {
    placeKind?: string;
    /** The search id of the mapped place the point was made from, such as `n123`; the point shows that place's details. */
    placeId?: string;
    /** The town of the mapped place of a night; the day list names the day end by it. */
    locality?: string;
    id: string;
    coordinate: Coordinate;
    label: string;
    /** The planner made the label and may replace it: a visit's address or coordinate, or an endpoint's "Start", "Finish" or
     * nearest town. The label follows the point when it moves. */
    autoLabel?: boolean;
    kind: PointKind;
    night?: number;
    /** Mode of the leg that ends at this point; absent means routed. In a loop the start also ends the closing leg. */
    leg?: LegMode;
    /** Inner path of a drawn leg. The leg always joins its two points; moving a point routes its drawn legs again (`routeLegsAround`).
     * An imported line also repeats both points with their elevations, so the leg has a height at each end. */
    drawn?: DrawnCoordinate[];
    /** A visit-and-return rejoins this exact position on the planned line. */
    anchor?: Coordinate;
    /** The route turns back at this point. On the start of a loop, it counts once the start moves (`startLoopHere`). */
    turnaround?: true;
    /** A line about the place, such as a route file's waypoint description. */
    note?: string;
    /** The route point that ends the leg a marker belongs to. Where the route passes the marker more than once, the pass
     * nearest that leg shows it. */
    legEnd?: string;
}
export type Place = RoutePoint & {
    category: PlaceCategory;
    description: string;
    openingHours?: string;
    website?: string;
    phone?: string;
    content?: import('./place-content').PlaceContent;
    hoursStatus?: import('./search/types').HoursStatus;
};
export interface Trip {
    /** The name of a planned signed route; the title shows it. */
    name?: string;
    startDate?: string;
    bike?: import('./riding-profiles').BikeType;
    preset?: string;
    /** A stored plan can hold a pace that this reader does not know: `planPace` reads it as Moderate. */
    pace?: import('./riding-profiles').Pace;
    /** Every route point between the start and the finish, in route order. Markers are not route points. */
    routeOrder: string[];
    mode?: 'route' | 'trip';
    /** The finish is the start: the route order ends with the start again, and a loop has no finish point. */
    loop?: boolean;
    points: RoutePoint[];
    days: number;
    /** The rider set the day count; without it, the planner guessed it. */
    daysSet?: true;
    /** Night number → progress along the current route of a provisional day end set by drag. */
    splits?: Record<number, number>;
    /** The kinds of place the suggested nights look for; absent means all. */
    stay?: Stay[];
    /** Night number → the kinds of place for that night only. */
    nightStay?: Record<number, Stay[]>;
}
/** Figures of a day or of the whole route. Ascent and descent count known heights only. Without a routed line, the
 * climb is 0 and the hours follow the activity's pace. */
export interface Figures {
    distance: number;
    hours: number;
    ascent: number;
    descent: number;
}
/** "5h 20m" */
export function duration(hours: number): string {
    return formatMinutes(Math.round(hours * 60));
}
export interface Day extends Figures {
    number: number;
    from: number;
    to: number;
    pinned: RoutePoint | undefined;
    split?: boolean;
}

/** A new plan has no route until the rider chooses both endpoints. */
export function emptyTrip(mode: Trip['mode'] = 'route'): Trip {
    return { points: [], routeOrder: [], mode, bike: 'touring', days: 3 };
}

const startLabel = 'Start';
const shapeLabel = 'Shaping point';

/** The label of a start or a finish that has no name yet. */
export const endpointLabel = (kind: 'start' | 'finish') => kind === 'start' ? startLabel : 'Finish';

/** The title of a plan: the name of its signed route, else its start and finish. */
export function planTitle(trip: Trip): string {
    const start = trip.points.find(p => p.kind === 'start'), finish = trip.points.find(p => p.kind === 'finish');
    return trip.name ? trip.name : start && trip.loop ? `Loop from ${start.label}` : start && finish ? `${start.label} → ${finish.label}`
        : start ? `From ${start.label}` : finish ? `To ${finish.label}` : 'New route';
}

/** A start, and a finish or a loop: enough points for a route. */
export function hasEndpoints(trip: Trip): boolean {
    return trip.points.some(p => p.kind === 'start') && (!!trip.loop || trip.points.some(p => p.kind === 'finish'));
}

/** A finish opens a loop: the route ends at the new finish instead of returning to the start. */
export function setEndpoint(trip: Trip, kind: 'start' | 'finish', coordinate: Coordinate, label?: string): Trip {
    const name = label === undefined ? { label: endpointLabel(kind), autoLabel: true } : { label };
    if (kind === 'finish' && trip.loop) {
        const point: RoutePoint = { id: crypto.randomUUID(), kind, coordinate: [...coordinate], ...name };
        return { ...trip, loop: undefined, splits: undefined, routeOrder: orderedRoutePoints(trip).slice(1, -1).map(p => p.id),
            points: [...trip.points.map(p => p.kind === 'start' ? { ...p, leg: undefined, drawn: undefined } : p), point] };
    }
    const previous = trip.points.find(p => p.kind === kind);
    if (kind === 'finish' && previous) {
        if (previous.coordinate[0] === coordinate[0] && previous.coordinate[1] === coordinate[1]) return trip;
        const point: RoutePoint = { id: crypto.randomUUID(), kind, coordinate: [...coordinate], ...name };
        const order = orderedRoutePoints(trip).slice(1).map(p => p.id);
        return { ...trip, points: [...trip.points.map(p => p.id === previous.id ? { ...p, kind: 'waypoint' as const } : p), point], routeOrder: order };
    }
    const point: RoutePoint = { id: previous?.id ?? crypto.randomUUID(), kind, coordinate: [...coordinate], ...name };
    return { ...trip, splits: undefined,
        points: previous ? trip.points.map(p => p.id === previous.id ? point : p) : [...trip.points, point] };
}

/**
 * Removing an endpoint promotes its neighbour in route order. A promoted shaping point takes the name of the nearest
 * place from `placeName`, else the endpoint name. Markers never become endpoints.
 */
export function removeRoutePoint(trip: Trip, id: string, placeName?: (coordinate: Coordinate) => string | undefined): Trip {
    const removed = trip.points.find(p => p.id === id);
    if (!removed) return trip;
    if (removed.kind === 'marker') return { ...trip, points: trip.points.filter(p => p.id !== id) };
    const route = orderedRoutePoints(trip).filter(p => p.id !== id);
    const neighbour = removed.kind === 'start' ? route[0] : removed.kind === 'finish' ? route.at(-1) : undefined;
    const promoted = neighbour && neighbour.kind !== 'start' && neighbour.kind !== 'finish' ? neighbour : undefined;
    const points = trip.points.filter(p => p.id !== id).map(p => p !== promoted ? p : {
        ...p, id: p.kind === 'night' ? crypto.randomUUID() : p.id, kind: removed.kind,
        label: p.kind === 'via' ? placeName?.(p.coordinate) ?? (removed.kind === 'start' ? startLabel : 'Finish') : p.label,
        // An endpoint with a name of its own, such as a visit's address, is no longer the planner's to replace.
        autoLabel: p.kind === 'via' || p.label === (removed.kind === 'start' ? startLabel : 'Finish') ? p.autoLabel : undefined,
        night: undefined, anchor: undefined,
        leg: removed.kind === 'start' ? undefined : p.leg, drawn: removed.kind === 'start' ? undefined : p.drawn,
    });
    const next: Trip = { ...trip, points, splits: undefined,
        routeOrder: route.filter(p => p !== promoted && p.kind !== 'start' && p.kind !== 'finish').map(p => p.id) };
    // A loop needs a point to ride to before it returns.
    if (!next.loop || orderedRoutePoints(next).length >= 3) return next;
    return { ...next, loop: undefined, points: points.map(p => p.kind === 'start' ? { ...p, leg: undefined, drawn: undefined } : p) };
}

/** Reverses the route and its itinerary, preserving each leg's drawing and each marker's pass of the line. */
export function reverseTrip(trip: Trip): Trip {
    // A point holds the leg that ends at it. A loop keeps its start and moves its closing leg there.
    const before = orderedRoutePoints(trip), order = before.slice().reverse();
    const kept = trip.loop ? order.slice(0, -1) : order;
    const renamed = (p: RoutePoint) => p.night ? `night-${trip.days - p.night}` : p.id;
    const legEnd = (marker: RoutePoint) => {
        const end = before.findIndex((p, i) => i > 0 && p.id === marker.legEnd);
        return end > 0 ? renamed(before[end - 1]) : marker.legEnd;
    };
    const legInto = (i: number) => ({ leg: order[i - 1].leg, drawn: order[i - 1].drawn?.slice().reverse() });
    const points = kept.map((p, i): RoutePoint => ({
        ...p,
        ...(i ? legInto(i) : trip.loop ? legInto(order.length - 1) : { leg: undefined, drawn: undefined }),
        kind: i === 0 ? 'start' : !trip.loop && i === kept.length - 1 ? 'finish' : p.kind,
        ...(p.night ? { id: renamed(p), night: trip.days - p.night } : {}),
    }));
    return {
        ...trip,
        points: [...points, ...trip.points.filter(p => p.kind === 'marker').map(p => ({ ...p, legEnd: legEnd(p) }))],
        routeOrder: points.slice(1, trip.loop ? undefined : -1).map(p => p.id),
        splits: Object.fromEntries(Object.entries(trip.splits ?? {}).map(([n, p]) => [trip.days - Number(n), 1 - p])),
        ...trip.nightStay && { nightStay: Object.fromEntries(Object.entries(trip.nightStay).map(([n, kinds]) => [trip.days - Number(n), kinds])) },
    };
}

/** The start, the points of `routeOrder`, then the finish; a loop ends at its start again. */
export function orderedRoutePoints(trip: Trip): RoutePoint[] {
    const byId = new Map(trip.points.map(p => [p.id, p]));
    const start = trip.points.filter(p => p.kind === 'start');
    return [...start, ...trip.routeOrder.flatMap(id => byId.get(id) ?? []), ...trip.loop ? start : trip.points.filter(p => p.kind === 'finish')];
}

export function reorderPoint(trip: Trip, id: string, offset: number): Trip {
    const route = orderedRoutePoints(trip);
    const points = route.filter(point => point.kind !== 'via');
    const index = points.findIndex(p => p.id === id);
    const target = index + offset;
    if (!Number.isInteger(offset) || !offset || index <= 0 || index >= points.length - 1 || target <= 0 || target >= points.length - 1) return trip;
    points.splice(target, 0, ...points.splice(index, 1));
    const before = new Map<string, string>();
    const shapes = new Map<string, RoutePoint[]>();
    let previous = route[0];
    let pending: RoutePoint[] = [];
    for (const point of route.slice(1)) {
        if (point.kind === 'via') { pending.push(point); continue; }
        before.set(point.id, previous.id);
        shapes.set(point.id, pending);
        pending = [];
        previous = point;
    }
    const changed = new Set(points.slice(1).filter((point, i) => before.get(point.id) !== points[i].id).map(point => point.id));
    // In a loop the start's shapes belong to the closing leg, at the end.
    const reordered = points.flatMap((point, i) => [...(!i || changed.has(point.id) ? [] : shapes.get(point.id) ?? []), point]);
    const retained = new Set(reordered.map(point => point.id));
    return { ...trip, routeOrder: reordered.slice(1, -1).map(point => point.id), splits: undefined,
        points: trip.points.filter(point => point.kind !== 'via' || retained.has(point.id)).map(point =>
            changed.has(point.id) && (point.leg || point.drawn) ? { ...point, leg: undefined, drawn: undefined } : point) };
}

type Stop = { point: RoutePoint; distance: number };

/** What the panel, the map and the profile show of a plan. */
export interface PlanView {
    /** The routed line of the current points at the pace of the plan, when it is calculated. */
    line?: RoutingLine;
    coordinates: Coordinate[];
    /** Each route point by position, with its kilometres along `coordinates`. A loop lists its start twice. */
    stops: Stop[];
    /** Kilometres along `coordinates`. */
    total: number;
    /** The figures of the whole route. */
    summary: Figures;
    days: Day[];
}
type Layout = Pick<PlanView, 'line' | 'coordinates' | 'stops' | 'total'>;

// A trip and a line are never changed after they are built, so each pair's view is computed once.
// Development and test builds freeze a viewed trip, its points and its view, so an in-place edit throws.
const views = new WeakMap<Trip, WeakMap<object, PlanView>>();
const unrouted = {};
function freeze(...values: object[]): void {
    if (import.meta.env.DEV) for (const value of values) Object.freeze(value);
}

/** The view of `trip` along `line`, which must be the line calculated for the trip's points (`routingKey`). Without a
 * line, a route has no length. */
export function planView(trip: Trip, routed: RoutingLine | undefined): PlanView {
    let byLine = views.get(trip);
    if (!byLine) views.set(trip, byLine = new WeakMap());
    const known = byLine.get(routed ?? unrouted);
    if (known) return known;
    freeze(trip, trip.points, ...trip.points);
    const line = routed && pacedLine(routed, trip);
    const points = orderedRoutePoints(trip);
    // By position: a loop lists its start twice.
    const coordinates = points.length < 2 ? [] : line?.coordinates ?? [points[0].coordinate];
    const stops = points.map((point, i) => ({ point, distance: line?.stops[i]?.distance ?? 0 }));
    const layout: Layout = { line, coordinates, stops, total: cumulative(coordinates).at(-1) ?? 0 };
    const days = points.length < 2 ? [] : splitDays(trip, layout);
    const view: PlanView = { ...layout, summary: figures(trip, layout, 0, 1), days };
    freeze(view, view.stops, view.days);
    byLine.set(routed ?? unrouted, view);
    return view;
}

/** One stretch of the route for each leg and riding day, so a map can tell legs apart and colour days. */
export function daySegments({ stops, days, coordinates, line }: PlanView): Omit<MapSegment, 'color'>[] {
    const length = stops.at(-1)?.distance || 1;
    return stops.slice(1).flatMap((stop, i) => days.flatMap(day => {
        const from = Math.max(day.from, stops[i].distance / length), to = Math.min(day.to, stop.distance / length);
        const leg = line?.gapLegs.includes(stop.point.id) ? 'gap' : stop.point.leg ?? 'routed';
        return to <= from ? [] : [{ day: day.number, coordinates: routeSlice(coordinates, from, to), legEndId: stop.point.id, leg }];
    }));
}

function stopProgress(stops: Stop[], point: RoutePoint): number {
    return (stops.find(stop => stop.point.id === point.id)?.distance ?? 0) / (stops.at(-1)?.distance || 1);
}

const minDayKm = 1;

// Night → progress of every fixed day end. A pinned overnight wins over a dragged split, and a split is
// provisional: one that leaves no day of at least `minDayKm` beside the fixed ends around it is dropped.
function fixedNights(trip: Trip, stops: Stop[]): Map<number, number> {
    const fixed = new Map<number, number>();
    for (const pin of trip.points) if (pin.kind === 'night') fixed.set(pin.night!, stopProgress(stops, pin));
    const gap = minDayKm / (stops.at(-1)?.distance || 1);
    for (const [key, progress] of Object.entries(trip.splits ?? {})) {
        const night = Number(key);
        if (fixed.has(night)) continue;
        const lo = Math.max(0, ...[...fixed].filter(([n]) => n < night).map(([, p]) => p));
        const hi = Math.min(1, ...[...fixed].filter(([n]) => n > night).map(([, p]) => p));
        if (progress >= lo + gap && progress <= hi - gap) fixed.set(night, progress);
    }
    return fixed;
}

// Riding time measures an even day, because it includes the climbing. Without a timed line, distance does.
function effortScale(line: RoutingLine | undefined): { at: (progress: number) => number; progress: (effort: number) => number } {
    if (!line || !(line.seconds > 0)) return { at: progress => progress, progress: effort => effort };
    return { at: progress => movingSecondsAt(line, progress) / line.seconds, progress: effort => progressAtSeconds(line, effort * line.seconds) };
}

function nightCount(trip: Trip): number {
    return Math.max(trip.days, ...trip.points.filter(p => p.kind === 'night').map(p => p.night! + 1));
}

/** The stretch around the even end of day `night`, `width` of a day on either side, within the fixed nights around it. */
export function overnightWindow(trip: Trip, line: RoutingLine | undefined, night: number, width = .2): { from: number; to: number; center: number; blocked: boolean } {
    const { stops, line: paced } = planView(trip, line);
    const count = nightCount(trip);
    const fixed = fixedNights(trip, stops);
    fixed.delete(night);
    const anchors = [...fixed.keys()].filter(n => n < count).sort((a, b) => a - b);
    const before = anchors.filter(n => n < night).at(-1);
    const after = anchors.find(n => n > night);
    const lo = before === undefined ? 0 : fixed.get(before)!;
    const hi = after === undefined ? 1 : fixed.get(after)!;
    const daysBefore = night - (before ?? 0);
    const daysAfter = (after ?? count) - night;
    const effort = effortScale(paced), start = effort.at(lo);
    const dayEffort = (effort.at(hi) - start) / (daysBefore + daysAfter);
    const center = Math.max(0, Math.min(1, effort.progress(start + dayEffort * daysBefore)));
    if (night <= 0 || night >= count || hi <= lo) return { from: center, to: center, center, blocked: true };
    const from = Math.max(lo, effort.progress(start + dayEffort * (daysBefore - width)));
    const to = Math.min(hi, effort.progress(start + dayEffort * (daysBefore + width)));
    return { from, to: Math.max(from, to), center, blocked: false };
}

// The figures between two progress points of the route.
function figures(trip: Trip, { line, total }: Layout, from: number, to: number): Figures {
    const distance = Math.max(0, to - from) * total;
    return {
        distance,
        hours: line ? (movingSecondsAt(line, to) - movingSecondsAt(line, from)) / 3600 : distance / ridingProfiles[trip.bike ?? 'touring'].kmh,
        ascent: profileAscent(from, to, line),
        descent: profileDescent(from, to, line),
    };
}

/** A trip with 2 or more riding days. A route, and a trip with 1 day, look and act as one-day routes. */
export const multiDay = (trip: Trip) => trip.mode !== 'route' && trip.days > 1;

function splitDays(trip: Trip, layout: Layout, provisionalEnd?: { night: number; progress: number }): Day[] {
    if (!multiDay(trip)) return [{ number: 1, from: 0, to: 1, pinned: undefined, ...figures(trip, layout, 0, 1) }];
    const pinned = trip.points.filter(p => p.kind === 'night');
    const count = nightCount(trip);
    const fixed = fixedNights(trip, layout.stops);
    if (provisionalEnd) fixed.set(provisionalEnd.night, provisionalEnd.progress);
    const effort = effortScale(layout.line);
    const boundaries = [0];
    for (let i = 1; i < count; i++) {
        if (fixed.has(i)) {
            boundaries.push(fixed.get(i)!);
            continue;
        }
        let next = i + 1;
        while (next < count && !fixed.has(next)) next++;
        const start = effort.at(boundaries[i - 1]), end = effort.at(next < count ? fixed.get(next)! : 1);
        // A stretch without riding time, such as a leg with no legal path, can put the even point before the previous end.
        boundaries.push(Math.max(boundaries[i - 1], effort.progress(start + (end - start) / (next - i + 1))));
    }
    boundaries.push(1);
    return boundaries.slice(1).map((to, i) => {
        const pin = pinned.find(p => p.night === i + 1);
        return { number: i + 1, from: boundaries[i], to, pinned: pin, split: !pin && fixed.has(i + 1), ...figures(trip, layout, boundaries[i], to) };
    });
}

/** The days when night `night` falls at `progress`, such as at a candidate overnight. */
export function provisionalDays(trip: Trip, line: RoutingLine | undefined, night: number, progress: number): Day[] {
    const view = planView(trip, line);
    return view.days.length ? splitDays(trip, view, { night, progress }) : [];
}

/** The kinds of the points that `dayStops` lists. */
export const stopKinds: PointKind[] = ['pass', 'waypoint', 'detour', 'marker'];

/** Stops inside a day in route order, with kilometres from the day start. */
export function dayStops(trip: Trip, line: RoutingLine | undefined, day: Pick<Day, 'from' | 'to'>): { point: RoutePoint; km: number }[] {
    const layout = planView(trip, line), { stops } = layout;
    const total = stops.at(-1)?.distance ?? 0;
    const markers = trip.points.filter(p => p.kind === 'marker').map(point => ({ point, distance: markerKm(layout, point) }));
    return [...stops.filter(stop => stopKinds.includes(stop.point.kind)), ...markers]
        // The first day also holds what lies at the start.
        .filter(stop => (stop.distance > day.from * total || !day.from) && stop.distance <= day.to * total)
        .sort((a, b) => a.distance - b.distance)
        .map(stop => ({ point: stop.point, km: stop.distance - day.from * total }));
}

/** A pass of the line at most this much farther from a marker than the nearest pass is a near pass (`specs/planner-plan.md`).
 * Chosen: an out-and-back can ride the two sides of a river or of a dual carriageway. */
const passKm = .1;

// Kilometres along the line to a marker. Of the passes about as near as the nearest one, the pass whose leg is nearest in
// route order to the marker's own leg wins, so a marker stays in its own day where the route passes it twice.
function markerKm({ coordinates, stops }: Layout, marker: RoutePoint): number {
    const passes = segmentDistances(coordinates, marker.coordinate), lengths = cumulative(coordinates);
    if (!passes.length) return 0;
    const own = stops.findIndex((stop, i) => i > 0 && stop.point.id === marker.legEnd);
    const nearest = passes.reduce((km, pass) => Math.min(km, pass.km), Infinity);
    const leg = (i: number) => Math.max(1, firstIndex(stops.length, k => stops[k].distance >= (lengths[i] + lengths[i + 1]) / 2));
    let best = 0, rank = Infinity;
    passes.forEach((pass, i) => {
        if (pass.km > nearest + passKm) return;
        const r = own > 0 ? Math.abs(leg(i) - own) : 0;
        if (r < rank || (r === rank && pass.km < passes[best].km)) { best = i; rank = r; }
    });
    return lengths[best] + (lengths[best + 1] - lengths[best]) * passes[best].t;
}

/** A marker with the leg nearest to it on the line; without the line, a marker has no leg. */
export function anchorMarker(trip: Trip, line: RoutingLine | undefined, marker: RoutePoint): RoutePoint {
    return { ...marker, legEnd: line && orderedRoutePoints(trip).length > 1 ? nearestLegEnd(trip, line, marker.coordinate) : undefined };
}

/** Moves a provisional day end along the route, keeping a day of at least 1 km on both sides. */
export function setSplit(trip: Trip, line: RoutingLine | undefined, night: number, progress: number): Trip {
    const { days, total } = planView(trip, line);
    if (!multiDay(trip) || night < 1 || night >= days.length || days[night - 1].pinned) return trip;
    const gap = minDayKm / (total || 1);
    const clamped = Math.max(days[night - 1].from + gap, Math.min(days[night].to - gap, progress));
    return { ...trip, splits: { ...trip.splits, [night]: clamped } };
}

/** The fewest riding days the trip can have: one more than its highest kept night. */
export const fewestDays = (trip: Trip) => Math.max(1, ...trip.points.filter(p => p.kind === 'night').map(p => p.night! + 1));

/** The trip with `days` riding days, at least `fewestDays()` and at most `maxRidingDays`. Kept nights keep their
 * numbers. A moved day end or a night-only stay of a night that the trip no longer has goes. */
export function withDays(trip: Trip, days: number): Trip {
    const count = Math.max(fewestDays(trip), Math.min(maxRidingDays, days));
    const kept = <T>(byNight: Record<number, T> | undefined) => {
        const entries = Object.entries(byNight ?? {}).filter(([night]) => Number(night) < count);
        return entries.length ? Object.fromEntries(entries) as Record<number, T> : undefined;
    };
    return { ...trip, days: count, splits: kept(trip.splits), nightStay: kept(trip.nightStay) };
}

/** The rider sets the day count, so the planner no longer guesses it. */
export function setDays(trip: Trip, days: number): Trip {
    return { ...withDays(trip, days), daysSet: true };
}

/** Replaces the day ends strictly inside a route range with `boundaries`, all in kilometres. */
export function repartitionDays(trip: Trip, line: RoutingLine | undefined, [from, to]: [number, number], boundaries: number[]): Trip {
    if (trip.points.some(p => p.kind === 'night')) throw new Error('Unpin the overnight places before changing the day count.');
    const { days, total } = planView(trip, line);
    const ends = [...days.slice(0, -1).map(d => d.to * total).filter(k => k <= from + 1e-6 || k >= to - 1e-6), ...boundaries];
    ends.sort((a, b) => a - b);
    if (ends.length >= maxRidingDays) throw new Error(`A trip can have up to ${maxRidingDays} riding days.`);
    return { ...setDays(trip, ends.length + 1), mode: 'trip',
        splits: Object.fromEntries(ends.map((km, i) => [i + 1, km / total])) };
}

/** Riding hours of a guessed day. */
const guessedDayHours = 6.5;

/** The day count of the riding time of the line. Without a riding time, the trip's count. */
export function guessDays(trip: Trip, line: RoutingLine | undefined): number {
    const { hours } = planView(trip, line).summary;
    return hours > 0 ? withDays(trip, Math.round(hours / guessedDayHours)).days : trip.days;
}

/** A multi-day trip whose day count the rider did not set has the guessed count of the line. The guess never
 * changes the type: it gives at least 2 days, and a route keeps its stored count. */
export function guessedTrip(trip: Trip, line: RoutingLine | undefined): Trip {
    if (trip.daysSet || !multiDay(trip)) return trip;
    const days = Math.max(2, guessDays(trip, line));
    return days === trip.days ? trip : withDays(trip, days);
}

/** A route as a multi-day trip. A set count and its kept nights come back. */
export function routeToTrip(trip: Trip, line: RoutingLine | undefined): Trip {
    return guessedTrip({ ...trip, mode: 'trip', days: Math.max(2, trip.days) }, line);
}

/** One riding day fewer. At the lowest count the trip becomes a route that keeps its days and nights. */
export function fewerDays(trip: Trip): Trip {
    return trip.days > Math.max(2, fewestDays(trip)) ? setDays(trip, trip.days - 1) : { ...trip, mode: 'route' };
}

/** Pins several nights on one line, the latest first: each new point then lies after the nights still to pin, so they
 * find their legs among the points before it, which still match the line. */
export function pinNights(trip: Trip, line: RoutingLine | undefined, nights: { night: number; coordinate: Coordinate; label: string; place: PlaceRef }[]): Trip {
    return [...nights].sort((a, b) => b.night - a.night).reduce((next, { night, coordinate, label, place }) => pinNight(next, line, night, coordinate, label, undefined, place), trip);
}

/** The mapped place a point stands for. */
export type PlaceRef = Pick<RoutePoint, 'placeKind' | 'placeId' | 'locality'>;

/** Pins night `night`. Without `place`, the night keeps the place of the point it replaces. A kept night moved past a route
 * point finds its leg again. */
export function pinNight(trip: Trip, line: RoutingLine | undefined, night: number, coordinate: Coordinate, label: string, sourceId?: string, place?: PlaceRef): Trip {
    if (!Number.isInteger(night) || night < 1 || night >= maxRidingDays) return trip;
    const id = `night-${night}`;
    const source = trip.points.find(p => p.id === (sourceId ?? id));
    if (source?.kind === 'start' || source?.kind === 'finish') return trip;
    const days = Math.max(trip.days, night + 1);
    const others: Trip = {
        ...trip, days,
        points: trip.points.filter(p => p.id !== id && p.id !== sourceId),
        routeOrder: sourceId ? trip.routeOrder.filter(pointId => pointId !== id || pointId === sourceId).map(pointId => pointId === sourceId ? id : pointId) : trip.routeOrder,
    };
    const point: RoutePoint = { ...source, id, kind: 'night', night, coordinate, label, autoLabel: undefined, ...place && { placeKind: place.placeKind, placeId: place.placeId, locality: place.locality } };
    const route = orderedRoutePoints(trip), at = route.findIndex(p => p.id === id), legEnd = nearestLegEnd(trip, line, coordinate);
    const inPlace = others.routeOrder.includes(id) && (sourceId || !line || legEnd === id || legEnd === route[at + 1]?.id);
    // A moved night takes the mode of the leg it joins.
    const next = inPlace ? { ...others, points: [...others.points, point] }
        : intoLeg({ ...others, routeOrder: others.routeOrder.filter(other => other !== id) }, { ...point, drawn: undefined }, legEnd);
    if (trip.splits?.[night] === undefined) return next;
    const { [night]: _, ...splits } = trip.splits;
    return { ...next, splits };
}

/** Night `night` becomes a suggestion again: its kept point and its moved day end go. A marker of the leg that ended at the
 * night joins the next leg. */
export function releaseNight(trip: Trip, night: number): Trip {
    const id = `night-${night}`, route = orderedRoutePoints(trip), at = route.findIndex(p => p.id === id);
    const { [night]: _, ...splits } = trip.splits ?? {};
    return { ...trip, splits, routeOrder: trip.routeOrder.filter(other => other !== id),
        points: trip.points.filter(p => p.id !== id).map(p => at > 0 && p.legEnd === id ? { ...p, legEnd: route[at + 1]?.id } : p) };
}

/** A clicked point extends a single route at its end. On a trip it joins its nearest leg, so it stays in the day it lies in. */
export function addClickedPoint(trip: Trip, line: RoutingLine | undefined, point: RoutePoint): Trip {
    if (multiDay(trip)) return addPointNear(trip, line, point);
    return point.kind === 'marker' ? addPointNear(trip, line, point) : intoLeg(trip, point, orderedRoutePoints(trip).at(-1)!.id);
}

/** Adds a point in the leg nearest to it; the other points keep their order. */
export function addPointNear(trip: Trip, line: RoutingLine | undefined, point: RoutePoint): Trip {
    if (point.kind === 'marker') return { ...trip, points: [...trip.points, anchorMarker(trip, line, point)] };
    return intoLeg(trip, point, nearestLegEnd(trip, line, point.coordinate));
}

/** Replaces point `id` with `point`, which can have another kind and ID. A marker that becomes a route point joins its
 * nearest leg; a route point that becomes a marker leaves the route, and it and the markers of its leg join the next leg. */
export function replacePoint(trip: Trip, line: RoutingLine | undefined, id: string, point: RoutePoint): Trip {
    const old = trip.points.find(p => p.id === id);
    if (old?.kind === 'marker') return point.kind === 'marker' ? { ...trip, points: trip.points.map(p => p === old ? point : p) }
        : addPointNear({ ...trip, points: trip.points.filter(p => p !== old) }, line, { ...point, legEnd: undefined });
    const route = orderedRoutePoints(trip), order = trip.routeOrder.map(other => other === id ? point.id : other);
    const legEnd = point.kind === 'marker' ? route[route.findIndex(p => p.id === id) + 1]?.id : point.id;
    const placed = point.kind === 'marker' ? { ...point, legEnd } : point;
    return { ...trip, points: trip.points.map(p => p.id === id ? placed : p.legEnd === id ? { ...p, legEnd } : p),
        routeOrder: point.kind === 'marker' ? order.filter(other => other !== point.id) : order };
}

/** Inserts a shaping point into the leg that ends at `legEndId`. */
export function insertPoint(trip: Trip, legEndId: string, coordinate: Coordinate): Trip {
    const end = trip.points.find(p => p.id === legEndId);
    if (!end || (end.kind === 'start' && !trip.loop) || end.kind === 'marker') return trip;
    return intoLeg(trip, { id: crypto.randomUUID(), kind: 'via', label: shapeLabel, coordinate: [...coordinate] }, legEndId);
}

/** A point dragged out of a leg: both legs beside it follow roads. */
export function dragPointOut(trip: Trip, legEndId: string, coordinate: Coordinate): Trip {
    const next = insertPoint(trip, legEndId, coordinate);
    const ids = new Set(trip.points.map(p => p.id));
    const added = next.points.find(p => !ids.has(p.id));
    return added ? routeLegsAround(next, added.id) : next;
}

export function setLegMode(trip: Trip, id: string, mode: LegMode): Trip {
    return withLeg(trip, id, { leg: mode === 'routed' ? undefined : mode });
}

export function setDrawnLeg(trip: Trip, id: string, coordinates: DrawnCoordinate[]): Trip {
    return withLeg(trip, id, { leg: 'drawn', drawn: coordinates.map(c => [...c] as DrawnCoordinate) });
}

function withLeg(trip: Trip, id: string, change: Partial<RoutePoint>): Trip {
    return { ...trip, points: trip.points.map(p => p.id === id ? { ...p, ...change } : p) };
}

// Places `point` in the leg that ends at `legEndId`. Both halves keep that leg's mode. A drawing splits on its segment
// nearest the point and keeps every vertex, so a point on the drawn line leaves the line unchanged. When both ends of that
// segment have an elevation, both halves get a vertex at the point with the interpolated elevation.
function intoLeg(trip: Trip, point: RoutePoint, legEndId: string): Trip {
    const end = trip.points.find(p => p.id === legEndId)!;
    const placed: RoutePoint = { ...point, leg: end.leg };
    let points = trip.points;
    if (end.leg === 'drawn') {
        const route = orderedRoutePoints(trip);
        const line: DrawnCoordinate[] = [route[route.map(p => p.id).lastIndexOf(legEndId) - 1].coordinate, ...end.drawn ?? [], end.coordinate];
        const { index: cut, t } = nearestOnLine(line, point.coordinate);
        const [a, b] = [line[cut][2], line[cut + 1][2]];
        const at: DrawnCoordinate[] = a === undefined || b === undefined ? [] : [[...point.coordinate, a + (b - a) * t]];
        placed.drawn = [...line.slice(1, cut + 1), ...at];
        points = points.map(p => p.id === end.id ? { ...p, drawn: [...at, ...line.slice(cut + 1, -1)] } : p);
    }
    return { ...trip, points: [...points, placed], routeOrder: orderBefore(trip, point.id, legEndId) };
}

/** Dragging a point plans its drawn neighbouring legs on roads again; the other legs stay as they are. */
export function routeLegsAround(trip: Trip, id: string): Trip {
    const route = orderedRoutePoints(trip);
    const ends = new Set(route.flatMap((p, i) => p.id === id ? [p.id, route[i + 1]?.id] : []));
    return { ...trip, points: trip.points.map(p => ends.has(p.id) && p.leg === 'drawn' ? { ...p, leg: undefined, drawn: undefined } : p) };
}

function nearestLegEnd(trip: Trip, line: RoutingLine | undefined, coordinate: Coordinate): string {
    const { coordinates, stops, total } = planView(trip, line);
    const distance = nearestProgress(coordinates, coordinate) * total;
    // The start ends no leg, so a point at or before it joins the first leg.
    return (stops.slice(1).find(stop => stop.distance >= distance) ?? stops.at(-1)!).point.id;
}

// The route order of the middle points with `id` placed just before `legEndId`; before the finish means last.
function orderBefore(trip: Trip, id: string, legEndId: string): string[] {
    const middle = orderedRoutePoints(trip).slice(1, -1).map(p => p.id).filter(other => other !== id);
    const index = middle.indexOf(legEndId);
    middle.splice(index < 0 ? middle.length : index, 0, id);
    return middle;
}

/** "Back to start": the finish becomes the last stop, and a routed leg returns to the start. */
export function closeLoop(trip: Trip): Trip {
    const finish = trip.points.find(p => p.kind === 'finish');
    if (trip.loop || !finish || !hasEndpoints(trip)) return trip;
    return { ...trip, loop: true, splits: undefined, routeOrder: orderedRoutePoints(trip).slice(1).map(p => p.id),
        points: trip.points.map(p => p.id === finish.id ? { ...p, kind: 'waypoint' as const } : p) };
}

/** A loop that turns back at a point between its start and its return to the start. */
export function isThereAndBack(trip: Trip): boolean {
    return !!trip.loop && orderedRoutePoints(trip).slice(1, -1).some(p => p.turnaround);
}

/** "There and back": the route turns at its far point and returns to the start through the points of the way out, as
 * shaping points on legs of the same mode. The far point is the finish, or the last point of a loop before its start. */
export function thereAndBack(trip: Trip): Trip {
    const route = orderedRoutePoints(trip).slice(0, trip.loop ? -1 : undefined), far = route.at(-1)!;
    if (!hasEndpoints(trip) || isThereAndBack(trip) || far.kind === 'start') return trip;
    // A point holds the leg that ends at it, so a point of the way back takes the leg of the point after it on the way out.
    const mirrored = (i: number) => ({ leg: route[i + 1].leg, drawn: route[i + 1].drawn?.slice().reverse() });
    // The way back passes the anchor of a visit-and-return, not its place.
    const back = route.slice(1, -1).map((p, i): RoutePoint => ({ id: crypto.randomUUID(), kind: 'via', label: shapeLabel,
        coordinate: [...p.anchor ?? p.coordinate], ...p.turnaround && { turnaround: true as const }, ...mirrored(i + 1) })).reverse();
    // A loop has no finish, and a visit-and-return has a turn of its own at its place: both become a visit.
    const turn = (p: RoutePoint): RoutePoint => p === far
        ? { ...p, turnaround: true, ...(p.kind === 'finish' || p.kind === 'detour') && { kind: 'waypoint' as const, anchor: undefined } }
        : p.kind === 'start' ? { ...p, turnaround: true, ...mirrored(0) } : p;
    return { ...trip, loop: true, splits: undefined, points: [...trip.points.map(turn), ...back],
        routeOrder: [...route.slice(1), ...back].map(p => p.id) };
}

/** "One way": a there-and-back trip ends at its far point, and the points of the way back go with their nights. */
export function oneWay(trip: Trip): Trip {
    if (!isThereAndBack(trip)) return trip;
    const route = orderedRoutePoints(trip).slice(0, -1);
    // The way back repeats each turn of the way out, so the far point is the middle turn.
    const turns = route.flatMap((p, i) => i && p.turnaround ? [i] : []), at = turns[turns.length >> 1], far = route[at];
    const gone = new Set(route.slice(at + 1).map(p => p.id));
    const finish: RoutePoint = { ...far, kind: 'finish', turnaround: undefined, anchor: undefined,
        // `night-N` names a night, and a shaping point has no name of its own.
        ...far.kind === 'night' && { id: crypto.randomUUID(), night: undefined },
        ...far.kind === 'via' && { label: endpointLabel('finish'), autoLabel: true } };
    return { ...trip, loop: undefined, splits: undefined, routeOrder: route.slice(1, at).map(p => p.id),
        points: trip.points.filter(p => !gone.has(p.id)).map(p => p.id === far.id ? finish
            : p.kind === 'start' ? { ...p, turnaround: undefined, leg: undefined, drawn: undefined } : p) };
}

/** A loop from the first coordinate through the others as shaping points. */
export function loopTrip(trip: Trip, coordinates: Coordinate[], label = startLabel): Trip {
    if (coordinates.length < 2) return trip;
    const [start, ...shape] = coordinates.map((coordinate, i): RoutePoint => ({ id: crypto.randomUUID(), kind: i ? 'via' : 'start',
        label: i ? shapeLabel : label, coordinate: [...coordinate] }));
    return { ...trip, loop: true, splits: undefined, points: [start, ...shape], routeOrder: shape.map(p => p.id) };
}

/** Day ends follow the route order, so a trip with pinned nights keeps its start. */
export function canMoveLoopStart(trip: Trip): boolean {
    return !!trip.loop && !trip.points.some(p => p.kind === 'night');
}

// Makes the route point `id` the start. The points keep their order around the loop, and no leg changes.
// The old start becomes a visit when it has a name of its own, and a shaping point otherwise.
function startLoopAt(trip: Trip, id: string): Trip {
    const route = orderedRoutePoints(trip).slice(0, -1);
    const index = route.findIndex(p => p.id === id);
    if (!canMoveLoopStart(trip) || index < 1) return trip;
    const old = route[0];
    const unnamed = old.autoLabel || old.label === startLabel || old.label === shapeLabel;
    return { ...trip, splits: undefined, routeOrder: [...route.slice(index + 1), old, ...route.slice(1, index)].map(p => p.id),
        points: trip.points.map(p => p.id === id ? { ...p, kind: 'start' as const, anchor: undefined }
            : p.id === old.id ? { ...p, kind: unnamed ? 'via' as const : 'waypoint' as const } : p) };
}

/** "Start the loop here": a new start at `coordinate` on the leg that ends at `legEndId`. */
export function startLoopHere(trip: Trip, legEndId: string, coordinate: Coordinate, label = startLabel): Trip {
    const point: RoutePoint = { id: crypto.randomUUID(), kind: 'via', label, coordinate: [...coordinate] };
    return canMoveLoopStart(trip) && trip.points.some(p => p.id === legEndId) ? startLoopAt(intoLeg(trip, point, legEndId), point.id) : trip;
}

export function nightOrderConflicts(trip: Trip, line: RoutingLine | undefined): [RoutePoint, RoutePoint][] {
    const nights = trip.points.filter(p => p.kind === 'night').sort((a, b) => a.night! - b.night!);
    const { stops } = planView(trip, line);
    return nights.slice(1).flatMap((point, i) => stopProgress(stops, point) <= stopProgress(stops, nights[i]) ? [[nights[i], point] as [RoutePoint, RoutePoint]] : []);
}
