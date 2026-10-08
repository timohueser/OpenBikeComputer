import { movingSecondsAt, type RoutingLine } from './routing';
import { profileAscent, profileDescent } from './profile-data';
import { ridingProfiles } from './riding-profiles';
import { cumulative, firstIndex, nearestOnLine, nearestProgress, segmentDistances, type Coordinate } from './geo';
import type { PlaceCategory } from './poi-kinds';

/** A vertex of a drawn leg; a third number is its elevation in metres. */
export type DrawnCoordinate = Coordinate | [number, number, number];
export const maxRidingDays = 14;
/** A transfer is a straight leg that the rider does not ride, such as a train; it adds no ridden distance or time. */
export type LegMode = 'routed' | 'straight' | 'drawn' | 'transfer';
export type PointKind = 'start' | 'finish' | 'pass' | 'via' | 'waypoint' | 'detour' | 'night' | 'marker' | 'place';
export interface RoutePoint {
    placeKind?: string;
    id: string;
    coordinate: Coordinate;
    label: string;
    /** An address or coordinate label follows the point when it moves. */
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
    /** The route turns back at this shaping point. */
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
    locality?: string;
    openingHours?: string;
    website?: string;
    phone?: string;
    content?: import('./place-content').PlaceContent;
    detailsLoaded?: boolean;
    hoursStatus?: import('./search/types').HoursStatus;
};
export interface Trip {
    /** The name of a planned signed route; the title shows it. */
    name?: string;
    startDate?: string;
    bike?: import('./riding-profiles').BikeType;
    preset?: string;
    /** Every route point between the start and the finish, in route order. Markers are not route points. */
    routeOrder: string[];
    restNames?: string[];
    mode?: 'route' | 'trip';
    /** The finish is the start: the route order ends with the start again, and a loop has no finish point. */
    loop?: boolean;
    points: RoutePoint[];
    days: number;
    budget: 'days' | 'distance' | 'hours';
    target: number;
    limit: number;
    restAfter?: number[];
    /** Night number → progress along the current route of a provisional day end set by drag. */
    splits?: Record<number, number>;
    /** Climb per day in metres; 0 or absent means no target. */
    climbTarget?: number;
}
/** Figures of a day or of the whole route. Ascent and descent count known heights only. Without a routed line, the
 * climb is 0 and the hours follow the activity's pace. */
export interface Figures {
    distance: number;
    hours: number;
    ascent: number;
    descent: number;
}
export interface Day extends Figures {
    number: number;
    from: number;
    to: number;
    pinned: RoutePoint | undefined;
    split?: boolean;
}
export interface ItineraryDay extends Day {
    ridingNumber: number;
    rest: boolean;
    restIndex?: number;
}

/** A new plan has no route until the rider chooses both endpoints. */
export function emptyTrip(mode: Trip['mode'] = 'route'): Trip {
    return { points: [], routeOrder: [], mode, bike: 'touring', days: 3, budget: 'days', target: 3, limit: 50, restAfter: [] };
}

const startLabel = 'Start';
const shapeLabel = 'Shaping point';

/** The title of a plan: the name of its signed route, else its start and finish. */
export function planTitle(trip: Trip): string {
    const start = trip.points.find(p => p.kind === 'start'), finish = trip.points.find(p => p.kind === 'finish');
    return trip.name ? trip.name : start && trip.loop ? `Loop from ${start.label}` : start && finish ? `${start.label} → ${finish.label}`
        : start ? `From ${start.label}` : finish ? `To ${finish.label}` : 'New plan';
}

/** A start, and a finish or a loop: enough points for a route. */
export function hasEndpoints(trip: Trip): boolean {
    return trip.points.some(p => p.kind === 'start') && (!!trip.loop || trip.points.some(p => p.kind === 'finish'));
}

/** A finish opens a loop: the route ends at the new finish instead of returning to the start. */
export function setEndpoint(trip: Trip, kind: 'start' | 'finish', coordinate: Coordinate, label?: string): Trip {
    if (kind === 'finish' && trip.loop) {
        const point: RoutePoint = { id: crypto.randomUUID(), kind, coordinate: [...coordinate], label: label ?? 'Finish' };
        return { ...trip, loop: undefined, splits: undefined, routeOrder: orderedRoutePoints(trip).slice(1, -1).map(p => p.id),
            points: [...trip.points.map(p => p.kind === 'start' ? { ...p, leg: undefined, drawn: undefined } : p), point] };
    }
    const previous = trip.points.find(p => p.kind === kind);
    if (kind === 'finish' && previous) {
        if (previous.coordinate[0] === coordinate[0] && previous.coordinate[1] === coordinate[1]) return trip;
        const point: RoutePoint = { id: crypto.randomUUID(), kind, coordinate: [...coordinate], label: label ?? 'Finish' };
        const order = orderedRoutePoints(trip).slice(1).map(p => p.id);
        return { ...trip, points: [...trip.points.map(p => p.id === previous.id ? { ...p, kind: 'waypoint' as const } : p), point], routeOrder: order };
    }
    const point: RoutePoint = { id: previous?.id ?? crypto.randomUUID(), kind, coordinate: [...coordinate],
        label: label ?? (kind === 'start' ? startLabel : 'Finish') };
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
        night: undefined, anchor: undefined,
        leg: removed.kind === 'start' ? undefined : p.leg, drawn: removed.kind === 'start' ? undefined : p.drawn,
    });
    const next: Trip = { ...trip, points, splits: undefined,
        routeOrder: route.filter(p => p !== promoted && p.kind !== 'start' && p.kind !== 'finish').map(p => p.id) };
    // A loop needs a point to ride to before it returns.
    if (!next.loop || orderedRoutePoints(next).length >= 3) return next;
    return { ...next, loop: undefined, points: points.map(p => p.kind === 'start' ? { ...p, leg: undefined, drawn: undefined } : p) };
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
    /** The routed line of the current points, when it is calculated. */
    line?: RoutingLine;
    coordinates: Coordinate[];
    /** Each route point by position, with its kilometres along `coordinates`. A loop lists its start twice. */
    stops: Stop[];
    /** Kilometres along `coordinates`, transfers included. */
    total: number;
    /** The figures of the whole route. */
    summary: Figures;
    days: Day[];
    /** The riding days, each followed by its rest days. */
    itinerary: ItineraryDay[];
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
export function planView(trip: Trip, line: RoutingLine | undefined): PlanView {
    let byLine = views.get(trip);
    if (!byLine) views.set(trip, byLine = new WeakMap());
    const known = byLine.get(line ?? unrouted);
    if (known) return known;
    freeze(trip, trip.points, ...trip.points);
    const points = orderedRoutePoints(trip);
    // By position: a loop lists its start twice.
    const coordinates = points.length < 2 ? [] : line?.coordinates ?? [points[0].coordinate];
    const stops = points.map((point, i) => ({ point, distance: line?.stops[i]?.distance ?? 0 }));
    const layout: Layout = { line, coordinates, stops, total: cumulative(coordinates).at(-1) ?? 0 };
    const days = points.length < 2 ? [] : splitDays(trip, layout);
    const view: PlanView = { ...layout, summary: figures(trip, layout, 0, 1), days, itinerary: itinerary(trip, days) };
    freeze(view, view.stops, view.days, view.itinerary);
    byLine.set(line ?? unrouted, view);
    return view;
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

function nightCount(trip: Trip): number {
    return Math.max(trip.days, ...trip.points.filter(p => p.kind === 'night').map(p => p.night! + 1));
}

export function overnightWindow(trip: Trip, line: RoutingLine | undefined, night: number): { from: number; to: number; center: number; blocked: boolean } {
    const { stops, total } = planView(trip, line);
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
    const dayFraction = (hi - lo) / (daysBefore + daysAfter);
    const center = Math.max(0, Math.min(1, lo + dayFraction * daysBefore));
    if (night <= 0 || night >= count || hi <= lo) return { from: center, to: center, center, blocked: true };
    const allowance = trip.limit > 0 && total > 0 ? trip.limit / total : Infinity;
    const feasibleFrom = Math.max(lo, hi - daysAfter * allowance);
    const feasibleTo = Math.min(hi, lo + daysBefore * allowance);
    const from = Math.max(feasibleFrom, center - .2 * dayFraction);
    const to = Math.min(feasibleTo, center + .2 * dayFraction);
    if (from > to + 1e-9) return { from: center, to: center, center, blocked: true };
    return { from, to: Math.max(from, to), center, blocked: false };
}

// The figures between two progress points of the route. A transfer is not ridden, so it adds no distance or time.
function figures(trip: Trip, { line, stops, total }: Layout, from: number, to: number): Figures {
    const distance = Math.max(0, to - from) * total - transferKm(stops, from * total, to * total);
    return {
        distance,
        hours: line ? (movingSecondsAt(line, to) - movingSecondsAt(line, from)) / 3600 : distance / ridingProfiles[trip.bike ?? 'touring'].kmh,
        ascent: profileAscent(from, to, line),
        descent: profileDescent(from, to, line),
    };
}

function splitDays(trip: Trip, layout: Layout, provisionalEnd?: { night: number; progress: number }): Day[] {
    if (trip.mode === 'route') return [{ number: 1, from: 0, to: 1, pinned: undefined, ...figures(trip, layout, 0, 1) }];
    const pinned = trip.points.filter(p => p.kind === 'night');
    const count = nightCount(trip);
    const fixed = fixedNights(trip, layout.stops);
    if (provisionalEnd) fixed.set(provisionalEnd.night, provisionalEnd.progress);
    const boundaries = [0];
    for (let i = 1; i < count; i++) {
        if (fixed.has(i)) {
            boundaries.push(fixed.get(i)!);
            continue;
        }
        let next = i + 1;
        while (next < count && !fixed.has(next)) next++;
        const end = next < count ? fixed.get(next)! : 1;
        boundaries.push(boundaries[i - 1] + (end - boundaries[i - 1]) / (next - i + 1));
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

// Kilometres of transfer legs between two distances along the route.
function transferKm(stops: Stop[], from: number, to: number): number {
    return stops.slice(1).reduce((km, stop, i) => stop.point.leg !== 'transfer' ? km
        : km + Math.max(0, Math.min(to, stop.distance) - Math.max(from, stops[i].distance)), 0);
}

/** Stops inside a day in route order, with kilometres from the day start. */
export function dayStops(trip: Trip, line: RoutingLine | undefined, day: Pick<Day, 'from' | 'to'>): { point: RoutePoint; km: number }[] {
    const layout = planView(trip, line), { stops } = layout;
    const total = stops.at(-1)?.distance ?? 0;
    const markers = trip.points.filter(p => p.kind === 'marker').map(point => ({ point, distance: markerKm(layout, point) }));
    return [...stops.filter(stop => ['pass', 'waypoint', 'detour'].includes(stop.point.kind)), ...markers]
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
    if (trip.mode === 'route' || night < 1 || night >= days.length || days[night - 1].pinned) return trip;
    const gap = minDayKm / (total || 1);
    const clamped = Math.max(days[night - 1].from + gap, Math.min(days[night].to - gap, progress));
    return { ...trip, splits: { ...trip.splits, [night]: clamped } };
}

/** How far a day goes over the rider's targets; 0 when under or without a target. */
export function dayOverTarget(trip: Trip, day: Pick<Day, 'distance'>, ascent: number): { km: number; climb: number } {
    return {
        km: trip.limit > 0 ? Math.max(0, day.distance - trip.limit) : 0,
        climb: trip.climbTarget ? Math.max(0, ascent - trip.climbTarget) : 0,
    };
}

export interface OvernightCandidate {
    place: Place;
    distance: number;
    ascent: number;
    from: number;
    to: number;
}

/** The three places nearest the suggested day end, each with its predicted day, shortest day first. */
export function overnightCandidates(trip: Trip, line: RoutingLine | undefined, night: number, places: Place[]): OvernightCandidate[] {
    const { coordinates, days } = planView(trip, line);
    if (trip.mode === 'route' || night < 1 || night >= days.length) return [];
    const { center } = overnightWindow(trip, line, night);
    return places
        .filter(place => place.category !== 'water')
        .map(place => ({ place, progress: nearestProgress(coordinates, place.coordinate) }))
        .sort((a, b) => Math.abs(a.progress - center) - Math.abs(b.progress - center))
        .slice(0, 3)
        .map(({ place, progress }) => {
            const { distance, ascent, from, to } = provisionalDays(trip, line, night, progress)[night - 1];
            return { place, distance, ascent, from, to };
        })
        .sort((a, b) => a.distance - b.distance);
}

export function applyBudget(trip: Trip, line: RoutingLine | undefined, budget: Trip['budget'], target: number, limit: number): Trip {
    const { total, summary, itinerary } = planView(trip, line);
    const restAfter = trip.restAfter ?? [];
    const restIndices = budget === 'days'
        ? itinerary.filter(day => day.rest && day.number <= Math.round(target)).map(day => day.restIndex!)
        : restAfter.map((_, index) => index);
    const wanted = budget === 'days' ? Math.round(target) - restIndices.length : Math.ceil(budget === 'distance' ? total / target : summary.hours / target);
    const pinnedMinimum = Math.max(1, ...trip.points.filter(p => p.kind === 'night').map(p => (p.night ?? 0) + 1));
    const days = Math.max(pinnedMinimum, Math.min(maxRidingDays, Math.max(1, wanted)));
    const retained = restIndices.filter(index => restAfter[index] <= days);
    return {
        ...trip, budget, target, limit, days, splits: days === trip.days ? trip.splits : undefined,
        restAfter: retained.map(index => restAfter[index]), restNames: retained.map(index => trip.restNames?.[index] ?? ''),
    };
}

function itinerary(trip: Trip, days: Day[]): ItineraryDay[] {
    const result: ItineraryDay[] = [];
    for (const day of days) {
        result.push({ ...day, number: result.length + 1, ridingNumber: day.number, rest: false });
        (trip.mode === 'route' ? [] : trip.restAfter ?? []).forEach((after, restIndex) => {
            if (after === day.number) result.push({
                ...day, number: result.length + 1, ridingNumber: day.number, rest: true, restIndex,
                from: day.to, to: day.to, distance: 0, hours: 0, ascent: 0, descent: 0,
            });
        });
    }
    return result;
}

export function addRestDay(trip: Trip, after: number): Trip {
    if (!Number.isInteger(after) || after < 1 || after > planView(trip, undefined).days.length) return trip;
    return { ...trip, restAfter: [...(trip.restAfter ?? []), after], restNames: [...(trip.restAfter ?? []).map((_,i)=>trip.restNames?.[i]??''), ''], target: trip.budget === 'days' ? trip.target + 1 : trip.target };
}

export function removeRestDay(trip: Trip, index: number): Trip {
    const restAfter = trip.restAfter ?? [];
    if (!Number.isInteger(index) || index < 0 || index >= restAfter.length) return trip;
    return { ...trip, restAfter: restAfter.filter((_, i) => i !== index), restNames: restAfter.flatMap((_,i)=>i===index?[]:[trip.restNames?.[i]??'']), target: trip.budget === 'days' ? Math.max(1, trip.target - 1) : trip.target };
}

export function pinNight(trip: Trip, line: RoutingLine | undefined, night: number, coordinate: Coordinate, label: string, sourceId?: string): Trip {
    if (!Number.isInteger(night) || night < 1 || night >= maxRidingDays) return trip;
    const id = `night-${night}`;
    const source = trip.points.find(p => p.id === (sourceId ?? id));
    if (source?.kind === 'start' || source?.kind === 'finish') return trip;
    const days = Math.max(trip.days, night + 1);
    const others: Trip = {
        ...trip, days, target: trip.budget === 'days' ? trip.target + days - trip.days : trip.target,
        points: trip.points.filter(p => p.id !== id && p.id !== sourceId),
        routeOrder: sourceId ? trip.routeOrder.filter(pointId => pointId !== id || pointId === sourceId).map(pointId => pointId === sourceId ? id : pointId) : trip.routeOrder,
    };
    const point: RoutePoint = { ...source, id, kind: 'night', night, coordinate, label, autoLabel: undefined };
    const next = others.routeOrder.includes(id) ? { ...others, points: [...others.points, point] } : intoLeg(others, point, nearestLegEnd(others, line, coordinate));
    if (trip.splits?.[night] === undefined) return next;
    const { [night]: _, ...splits } = trip.splits;
    return { ...next, splits };
}

/** A clicked point extends a single route at its end. On a trip it joins its nearest leg, so it stays in the day it lies in. */
export function addClickedPoint(trip: Trip, line: RoutingLine | undefined, point: RoutePoint): Trip {
    if (trip.mode !== 'route') return addPointNear(trip, line, point);
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
    const { coordinates, stops } = planView(trip, line);
    const distance = nearestProgress(coordinates, coordinate) * stops.at(-1)!.distance;
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
    const unnamed = old.label === startLabel || old.label === shapeLabel;
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
