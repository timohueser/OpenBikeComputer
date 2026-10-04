import { movingSecondsAt } from './routing';
import { profileAscent } from './profile-data';
import type { PlaceCategory } from './poi-kinds';

export type Coordinate = [number, number];
export const maxRidingDays = 14;
export type LegMode = 'routed' | 'straight' | 'drawn';
export type PointKind = 'start' | 'finish' | 'pass' | 'via' | 'waypoint' | 'detour' | 'night' | 'marker' | 'place';
export interface RoutePoint {
    placeKind?: string;
    id: string;
    coordinate: Coordinate;
    label: string;
    /** An address or coordinate label follows the point when it moves. */
    autoLabel?: boolean;
    kind: PointKind;
    progress: number;
    night?: number;
    /** Mode of the leg that ends at this point; absent means routed. In a loop the start also ends the closing leg. */
    leg?: LegMode;
    /** Inner path of a drawn leg. The leg always joins its two points, so moving a point keeps the drawing. */
    drawn?: Coordinate[];
    /** A visit-and-return rejoins this exact position on the planned line. */
    anchor?: Coordinate;
    /** A shaping point of a signed route: the map draws no pin for it. */
    hidden?: true;
    /** The route turns back at this shaping point. */
    turnaround?: true;
}
export type Place = RoutePoint & {
    category: PlaceCategory;
    description: string;
    locality?: string;
    openingHours?: string;
    hoursStatus?: import('./search/types').HoursStatus;
};
export interface Trip {
    startDate?: string;
    live?: boolean;
    routing?: import('./routing').RoutingLine;
    bike?: import('./riding-profiles').BikeType;
    preset?: string;
    routeOrder?: string[];
    restNames?: string[];
    mode?: 'route' | 'trip';
    /** The finish is the start: the route order ends with the start again, and a loop has no finish point. */
    loop?: boolean;
    points: RoutePoint[];
    days: number;
    budget: 'days' | 'distance' | 'hours';
    target: number;
    limit: number;
    variant: 'valley' | 'direct';
    restAfter?: number[];
    /** Night number → progress along the current route of a provisional day end set by drag. */
    splits?: Record<number, number>;
    /** Climb per day in metres; 0 or absent means no target. */
    climbTarget?: number;
}
export interface Day {
    number: number;
    from: number;
    to: number;
    distance: number;
    hours: number;
    pinned: RoutePoint | undefined;
    split?: boolean;
}
export interface ItineraryDay extends Day {
    ridingNumber: number;
    rest: boolean;
    restIndex?: number;
}

// Geographic control points only. The mock joins them; it does not find roads.
const valley: Coordinate[] = [
    [7.589, 47.557], [7.53, 47.581], [7.436, 47.577], [7.357, 47.59],
    [7.239, 47.594], [7.152, 47.581], [7.031, 47.574], [6.887, 47.513],
    [6.797, 47.51], [6.691, 47.432], [6.579, 47.392], [6.422, 47.355],
    [6.362, 47.348], [6.232, 47.29], [6.138, 47.263], [6.024, 47.237],
];

export function initialTrip(): Trip {
    return {
        points: [
            { id: 'start', coordinate: [...valley[0]], label: 'Basel', kind: 'start', progress: 0 },
            { id: 'finish', coordinate: [...valley.at(-1)!], label: 'Besançon', kind: 'finish', progress: 1 },
        ],
        days: 3, budget: 'days', target: 3, limit: 50, variant: 'valley', restAfter: [],
    };
}

/** A new plan has no route until the rider chooses both endpoints. */
export function emptyTrip(mode: Trip['mode'] = 'route'): Trip {
    return { points: [], live: true, routeOrder: [], mode, bike: 'touring',
        days: 3, budget: 'days', target: 3, limit: 50, variant: 'valley', restAfter: [] };
}

const startLabel = 'Start';
const shapeLabel = 'Shaping point';

/** A start, and a finish or a loop: enough points for a route. */
export function hasEndpoints(trip: Trip): boolean {
    return trip.points.some(p => p.kind === 'start') && (!!trip.loop || trip.points.some(p => p.kind === 'finish'));
}

/** A finish opens a loop: the route ends at the new finish instead of returning to the start. */
export function setEndpoint(trip: Trip, kind: 'start' | 'finish', coordinate: Coordinate, label?: string): Trip {
    if (kind === 'finish' && trip.loop) {
        const point: RoutePoint = { id: crypto.randomUUID(), kind, coordinate: [...coordinate], label: label ?? 'Finish', progress: 1 };
        return { ...trip, loop: undefined, splits: undefined, routeOrder: orderedRoutePoints(trip).slice(1, -1).map(p => p.id),
            points: [...trip.points.map(p => p.kind === 'start' ? { ...p, leg: undefined, drawn: undefined } : p), point] };
    }
    const previous = trip.points.find(p => p.kind === kind);
    if (kind === 'finish' && previous) {
        if (previous.coordinate[0] === coordinate[0] && previous.coordinate[1] === coordinate[1]) return trip;
        const point: RoutePoint = { id: crypto.randomUUID(), kind, coordinate: [...coordinate], label: label ?? 'Finish', progress: 1 };
        const order = orderedRoutePoints(trip).slice(1).map(p => p.id);
        return { ...trip, points: [...trip.points.map(p => p.id === previous.id ? { ...p, kind: 'waypoint' as const } : p), point], routeOrder: order };
    }
    const point: RoutePoint = { id: previous?.id ?? crypto.randomUUID(), kind, coordinate: [...coordinate],
        label: label ?? (kind === 'start' ? startLabel : 'Finish'), progress: kind === 'start' ? 0 : 1 };
    return { ...trip, splits: undefined,
        points: previous ? trip.points.map(p => p.id === previous.id ? point : p) : [...trip.points, point] };
}

/** Removing an endpoint promotes its neighbour in route order. Markers never become endpoints. */
export function removeRoutePoint(trip: Trip, id: string): Trip {
    const removed = trip.points.find(p => p.id === id);
    if (!removed) return trip;
    if (removed.kind === 'marker') return { ...trip, points: trip.points.filter(p => p.id !== id), routeOrder: trip.routeOrder?.filter(pointId => pointId !== id) };
    const route = orderedRoutePoints(trip).filter(p => p.id !== id);
    const neighbour = removed.kind === 'start' ? route[0] : removed.kind === 'finish' ? route.at(-1) : undefined;
    const promoted = neighbour && neighbour.kind !== 'start' && neighbour.kind !== 'finish' ? neighbour : undefined;
    const points = trip.points.filter(p => p.id !== id).map(p => p !== promoted ? p : {
        ...p, id: p.kind === 'night' ? crypto.randomUUID() : p.id, kind: removed.kind,
        progress: removed.kind === 'start' ? 0 : 1, night: undefined, anchor: undefined,
        leg: removed.kind === 'start' ? undefined : p.leg, drawn: removed.kind === 'start' ? undefined : p.drawn,
    });
    const next: Trip = { ...trip, points, routing: undefined, splits: undefined,
        routeOrder: route.filter(p => p !== promoted && p.kind !== 'start' && p.kind !== 'finish').map(p => p.id) };
    // A loop needs a point to ride to before it returns.
    if (!next.loop || orderedRoutePoints(next).length >= 3) return next;
    return { ...next, loop: undefined, points: points.map(p => p.kind === 'start' ? { ...p, leg: undefined, drawn: undefined } : p) };
}

export function kilometres(a: Coordinate, b: Coordinate): number {
    const rad = Math.PI / 180;
    const x = Math.sin((b[1] - a[1]) * rad / 2) ** 2
        + Math.cos(a[1] * rad) * Math.cos(b[1] * rad) * Math.sin((b[0] - a[0]) * rad / 2) ** 2;
    return 6371 * 2 * Math.atan2(Math.sqrt(x), Math.sqrt(1 - x));
}

function corridorCoordinates(trip: Trip): Coordinate[] {
    const lengths = cumulative(valley);
    const total = lengths.at(-1)!;
    const samples = valley.map((coordinate, i) => ({ coordinate, progress: lengths[i] / total }))
        .filter((_, i) => i > 0 && i < valley.length - 1 && (trip.variant === 'valley' || ![2, 3, 4, 8, 9, 12].includes(i)));
    const throughPoints = trip.points.filter(p => p.kind !== 'detour' && p.kind !== 'marker');
    const through = [
        ...samples.filter(p => !throughPoints.some(other => Math.abs(other.progress - p.progress) < 1e-9)),
        ...throughPoints,
    ].sort((a, b) => a.progress - b.progress);
    const detours = trip.points.filter(p => p.kind === 'detour').sort((a, b) => a.progress - b.progress);
    const excursions: { coordinate: Coordinate; progress: number }[] = [];
    for (const detour of detours) {
        const progress = Math.max(0, Math.min(1, detour.progress));
        const index = Math.max(1, through.findIndex(p => p.progress >= progress));
        const before = through[index - 1];
        const after = through[index];
        const t = (progress - before.progress) / (after.progress - before.progress || 1);
        const anchor = before.coordinate.map((n, axis) => n + (after.coordinate[axis] - n) * t) as Coordinate;
        excursions.push({ progress, coordinate: anchor }, { progress, coordinate: detour.coordinate }, { progress, coordinate: anchor });
    }
    const result = [...through, ...excursions].sort((a, b) => a.progress - b.progress).map(p => p.coordinate);
    return result.filter((p, i) => i === 0 || p[0] !== result[i - 1][0] || p[1] !== result[i - 1][1]);
}

export function orderedRoutePoints(trip: Trip): RoutePoint[] {
    const middle = trip.points.filter(p => !['start', 'finish', 'marker'].includes(p.kind)).sort((a, b) => a.progress - b.progress);
    const order = trip.routeOrder ?? middle.map(p => p.id);
    const ranked = order.flatMap(id => middle.filter(p => p.id === id));
    const start = trip.points.filter(p => p.kind === 'start');
    return [...start, ...ranked, ...middle.filter(p => !order.includes(p.id)), ...trip.loop ? start : trip.points.filter(p => p.kind === 'finish')];
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

function routeLayout(trip: Trip): { coordinates: Coordinate[]; stops: Stop[] } {
    const points = orderedRoutePoints(trip);
    if (points.length < 2) return { coordinates: [], stops: points.map(point => ({ point, distance: 0 })) };
    if (trip.live) {
        const line = trip.routing?.key === routingKey(trip) ? trip.routing : undefined;
        return {
            coordinates: line?.coordinates ?? [points[0].coordinate],
            // By position: a loop lists its start twice.
            stops: points.map((point, i) => ({ point, distance: line?.stops[i]?.distance ?? 0 })),
        };
    }
    const base = corridorCoordinates(trip);
    if (!trip.routeOrder && !trip.loop && points.slice(1).every(p => (p.leg ?? 'routed') === 'routed')) {
        const total = cumulative(base).at(-1)!;
        return {coordinates:base, stops:points.map(point=>({point,distance:point.kind==='start'?0:point.kind==='finish'?total:nearestProgress(base,point.coordinate)*total}))};
    }
    const coordinates: Coordinate[] = [[...points[0].coordinate]];
    const stops: Stop[] = [{ point: points[0], distance: 0 }];
    let distance = 0;
    for (let i = 1; i < points.length; i++) {
        const leg = legCoordinates(base, points[i - 1], points[i]);
        distance += cumulative(leg).at(-1)!;
        coordinates.push(...leg.slice(1));
        stops.push({ point: points[i], distance });
    }
    return { coordinates, stops };
}

function legCoordinates(base: Coordinate[], before: RoutePoint, point: RoutePoint): Coordinate[] {
    if (point.leg === 'straight' || point.leg === 'drawn') {
        return [[...before.coordinate], ...(point.leg === 'drawn' ? point.drawn ?? [] : []).map(c => [...c] as Coordinate), [...point.coordinate]];
    }
    const from = before.kind === 'start' ? 0 : nearestProgress(base, before.coordinate);
    const to = point.kind === 'finish' ? 1 : nearestProgress(base, point.coordinate);
    return routeSlice(base, from, to);
}

export function routeCoordinates(trip: Trip): Coordinate[] { return routeLayout(trip).coordinates; }
export function routeStops(trip: Trip): Stop[] { return routeLayout(trip).stops; }

/** Itinerary edits and labels never invalidate a selected route. */
export function routingKey(trip: Trip): string {
    return JSON.stringify([trip.bike ?? 'touring', trip.preset ?? 'Balanced', orderedRoutePoints(trip).map(p => [p.id, p.coordinate, p.leg ?? 'routed', p.drawn ?? [], p.kind === 'detour', p.anchor, p.turnaround])]);
}

// Stored anchors use one corridor frame; current-route fractions change with every detour.
export function anchorProgress(coordinate: Coordinate): number {
    return nearestProgress(valley, coordinate);
}

// Coordinate arrays are never changed after they are built, so each array's distances are computed once.
// Development and test builds freeze a measured array and its pairs, so an in-place edit throws.
const distances = new WeakMap<Coordinate[], readonly number[]>();

/** Kilometres from the first coordinate to each coordinate. */
export function cumulative(coordinates: Coordinate[]): readonly number[] {
    const known = distances.get(coordinates);
    if (known) return known;
    if (import.meta.env.DEV) {
        for (const pair of coordinates) Object.freeze(pair);
        Object.freeze(coordinates);
    }
    const result = [0];
    for (let i = 1; i < coordinates.length; i++) result.push(result[i - 1] + kilometres(coordinates[i - 1], coordinates[i]));
    distances.set(coordinates, result);
    return result;
}

/** The first index at which `reached` is true, or `length`; once true, `reached` must stay true. */
export function firstIndex(length: number, reached: (index: number) => boolean): number {
    let low = 0, high = length;
    while (low < high) {
        const middle = (low + high) >>> 1;
        if (reached(middle)) high = middle;
        else low = middle + 1;
    }
    return low;
}

export function coordinateAt(coordinates: Coordinate[], progress: number): Coordinate {
    if (coordinates.length === 0) throw new Error('A route needs at least one coordinate.');
    if (coordinates.length === 1) return [...coordinates[0]];
    const distances = cumulative(coordinates);
    const goal = distances.at(-1)! * Math.max(0, Math.min(1, progress));
    const index = Math.max(1, Math.min(distances.length - 1, firstIndex(distances.length, i => distances[i] >= goal)));
    const t = (goal - distances[index - 1]) / (distances[index] - distances[index - 1] || 1);
    return coordinates[index - 1].map((n, axis) => n + (coordinates[index][axis] - n) * t) as Coordinate;
}

export function routeSlice(coordinates: Coordinate[], from: number, to: number): Coordinate[] {
    if (coordinates.length === 0) return [];
    if (from > to) return routeSlice(coordinates, to, from).reverse();
    const start = Math.max(0, Math.min(1, from));
    const end = Math.max(0, Math.min(1, to));
    const first = coordinateAt(coordinates, start);
    if (start === end || coordinates.length === 1) return [first];
    const distances = cumulative(coordinates);
    const total = distances.at(-1)!;
    const inside = coordinates.slice(
        firstIndex(distances.length, i => distances[i] > start * total),
        firstIndex(distances.length, i => distances[i] >= end * total),
    );
    return [first, ...inside, coordinateAt(coordinates, end)];
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

export function overnightWindow(trip: Trip, night: number): { from: number; to: number; center: number; blocked: boolean } {
    const { coordinates, stops } = routeLayout(trip);
    const total = cumulative(coordinates).at(-1)!;
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

export function nearestProgress(coordinates: Coordinate[], point: Coordinate): number {
    const lengths = cumulative(coordinates);
    let nearest = Infinity;
    let progress = .5;
    coordinates.slice(1).forEach((b, i) => {
        const a = coordinates[i];
        const dx = b[0] - a[0];
        const dy = b[1] - a[1];
        const t = Math.max(0, Math.min(1, ((point[0] - a[0]) * dx + (point[1] - a[1]) * dy) / (dx * dx + dy * dy || 1)));
        const distance = kilometres(point, [a[0] + dx * t, a[1] + dy * t]);
        if (distance < nearest) {
            nearest = distance;
            progress = (lengths[i] + (lengths[i + 1] - lengths[i]) * t) / lengths.at(-1)!;
        }
    });
    return Math.max(.005, Math.min(.995, progress));
}

export function tripDays(trip: Trip, provisionalEnd?: { night: number; progress: number }): Day[] {
    if (orderedRoutePoints(trip).length < 2) return [];
    const line = trip.routing?.key === routingKey(trip) ? trip.routing : undefined;
    const hours = (from: number, to: number, distance: number) => line ? (movingSecondsAt(line, to) - movingSecondsAt(line, from)) / 3600 : distance / 15;
    const { coordinates, stops } = routeLayout(trip);
    const total = cumulative(coordinates).at(-1)!;
    if (trip.mode === 'route') {
        return [{ number: 1, from: 0, to: 1, distance: total, hours: hours(0, 1, total), pinned: undefined }];
    }
    const pinned = trip.points.filter(p => p.kind === 'night');
    const count = nightCount(trip);
    const fixed = fixedNights(trip, stops);
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
        const distance = Math.max(0, to - boundaries[i]) * total;
        const pin = pinned.find(p => p.night === i + 1);
        const split = !pin && fixed.has(i + 1);
        return { number: i + 1, from: boundaries[i], to, distance, hours: hours(boundaries[i], to, distance), pinned: pin, split };
    });
}

/** Stops inside a day in route order, with kilometres from the day start. */
export function dayStops(trip: Trip, day: Pick<Day, 'from' | 'to'>): { point: RoutePoint; km: number }[] {
    const { coordinates, stops } = routeLayout(trip);
    const total = stops.at(-1)?.distance ?? 0;
    const markers = trip.points.filter(p => p.kind === 'marker').map(point => ({ point, distance: nearestProgress(coordinates, point.coordinate) * total }));
    return [...stops.filter(stop => ['pass', 'waypoint', 'detour'].includes(stop.point.kind)), ...markers]
        .filter(stop => stop.distance > day.from * total && stop.distance <= day.to * total)
        .sort((a, b) => a.distance - b.distance)
        .map(stop => ({ point: stop.point, km: stop.distance - day.from * total }));
}

/** Moves a provisional day end along the route, keeping a day of at least 1 km on both sides. */
export function setSplit(trip: Trip, night: number, progress: number): Trip {
    const days = tripDays(trip);
    if (trip.mode === 'route' || night < 1 || night >= days.length || days[night - 1].pinned) return trip;
    const gap = minDayKm / (cumulative(routeCoordinates(trip)).at(-1)! || 1);
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
export function overnightCandidates(trip: Trip, night: number, places: Place[]): OvernightCandidate[] {
    if (trip.mode === 'route' || night < 1 || night >= tripDays(trip).length) return [];
    const coordinates = routeCoordinates(trip);
    const { center } = overnightWindow(trip, night);
    const gap = (place: Place) => Math.abs(nearestProgress(coordinates, place.coordinate) - center);
    return places
        .filter(place => place.category !== 'water')
        .sort((a, b) => gap(a) - gap(b))
        .slice(0, 3)
        .map(place => {
            const day = tripDays(trip, { night, progress: nearestProgress(coordinates, place.coordinate) })[night - 1];
            return { place, distance: day.distance, ascent: profileAscent(day.from, day.to, trip.routing), from: day.from, to: day.to };
        })
        .sort((a, b) => a.distance - b.distance);
}

export function applyBudget(trip: Trip, budget: Trip['budget'], target: number, limit: number): Trip {
    const total = cumulative(routeCoordinates(trip)).at(-1)!;
    const restAfter = trip.restAfter ?? [];
    const restIndices = budget === 'days'
        ? itineraryDays(trip).filter(day => day.rest && day.number <= Math.round(target)).map(day => day.restIndex!)
        : restAfter.map((_, index) => index);
    const wanted = budget === 'days' ? Math.round(target) - restIndices.length : Math.ceil(total / (budget === 'distance' ? target : target * 15));
    const pinnedMinimum = Math.max(1, ...trip.points.filter(p => p.kind === 'night').map(p => (p.night ?? 0) + 1));
    const days = Math.max(pinnedMinimum, Math.min(maxRidingDays, Math.max(1, wanted)));
    const retained = restIndices.filter(index => restAfter[index] <= days);
    return {
        ...trip, budget, target, limit, days, splits: days === trip.days ? trip.splits : undefined,
        restAfter: retained.map(index => restAfter[index]), restNames: retained.map(index => trip.restNames?.[index] ?? ''),
    };
}

export function itineraryDays(trip: Trip): ItineraryDay[] {
    const result: ItineraryDay[] = [];
    for (const day of tripDays(trip)) {
        result.push({ ...day, number: result.length + 1, ridingNumber: day.number, rest: false });
        (trip.mode === 'route' ? [] : trip.restAfter ?? []).forEach((after, restIndex) => {
            if (after === day.number) result.push({
                ...day, number: result.length + 1, ridingNumber: day.number, rest: true, restIndex,
                from: day.to, to: day.to, distance: 0, hours: 0,
            });
        });
    }
    return result;
}

export function addRestDay(trip: Trip, after: number): Trip {
    if (!Number.isInteger(after) || after < 1 || after > tripDays(trip).length) return trip;
    return { ...trip, restAfter: [...(trip.restAfter ?? []), after], restNames: [...(trip.restAfter ?? []).map((_,i)=>trip.restNames?.[i]??''), ''], target: trip.budget === 'days' ? trip.target + 1 : trip.target };
}

export function removeRestDay(trip: Trip, index: number): Trip {
    const restAfter = trip.restAfter ?? [];
    if (!Number.isInteger(index) || index < 0 || index >= restAfter.length) return trip;
    return { ...trip, restAfter: restAfter.filter((_, i) => i !== index), restNames: restAfter.flatMap((_,i)=>i===index?[]:[trip.restNames?.[i]??'']), target: trip.budget === 'days' ? Math.max(1, trip.target - 1) : trip.target };
}

export function pinNight(trip: Trip, night: number, coordinate: Coordinate, label: string, sourceId?: string): Trip {
    if (!Number.isInteger(night) || night < 1 || night >= maxRidingDays) return trip;
    const id = `night-${night}`;
    const source = trip.points.find(p => p.id === (sourceId ?? id));
    if (source?.kind === 'start' || source?.kind === 'finish') return trip;
    const days = Math.max(trip.days, night + 1);
    const others: Trip = {
        ...trip, days, target: trip.budget === 'days' ? trip.target + days - trip.days : trip.target,
        points: trip.points.filter(p => p.id !== id && p.id !== sourceId),
        routeOrder: sourceId ? trip.routeOrder?.filter(pointId => pointId !== id || pointId === sourceId).map(pointId => pointId === sourceId ? id : pointId) : trip.routeOrder,
    };
    const point: RoutePoint = { ...source, id, kind: 'night', night, coordinate, label, autoLabel: undefined, progress: trip.live ? nearestProgress(routeCoordinates(trip), coordinate) : anchorProgress(coordinate) };
    const next = others.routeOrder && !others.routeOrder.includes(id)
        ? intoLeg(others, point, nearestLegEnd(others, coordinate))
        : { ...others, points: [...others.points, point] };
    if (trip.splits?.[night] !== undefined) {
        const { [night]: _, ...splits } = trip.splits;
        next.splits = splits;
    }
    return next;
}

/** A clicked point extends a single route at its end. On a trip it joins its nearest leg, so it stays in the day it lies in. */
export function addClickedPoint(trip: Trip, point: RoutePoint): Trip {
    if (trip.mode !== 'route') return addPointNear(trip, point);
    return point.kind === 'marker' ? { ...trip, points: [...trip.points, point] } : intoLeg(trip, point, orderedRoutePoints(trip).at(-1)!.id);
}

/** Adds a point in the leg nearest to it; the other points keep their order. */
export function addPointNear(trip: Trip, point: RoutePoint): Trip {
    if (!trip.routeOrder || point.kind === 'marker') return { ...trip, points: [...trip.points, point] };
    return intoLeg(trip, point, nearestLegEnd(trip, point.coordinate));
}

/** Inserts a shaping point into the leg that ends at `legEndId`. */
export function insertPoint(trip: Trip, legEndId: string, coordinate: Coordinate): Trip {
    const end = trip.points.find(p => p.id === legEndId);
    if (!end || (end.kind === 'start' && !trip.loop) || end.kind === 'marker') return trip;
    return intoLeg(trip, { id: crypto.randomUUID(), kind: 'via', label: shapeLabel, coordinate: [...coordinate], progress: trip.live ? nearestProgress(routeCoordinates(trip), coordinate) : anchorProgress(coordinate) }, legEndId);
}

export function setLegMode(trip: Trip, id: string, mode: LegMode): Trip {
    return withLeg(trip, id, { leg: mode === 'routed' ? undefined : mode });
}

export function setDrawnLeg(trip: Trip, id: string, coordinates: Coordinate[]): Trip {
    return withLeg(trip, id, { leg: 'drawn', drawn: coordinates.map(c => [...c] as Coordinate) });
}

// A shaped leg fixes the route order, so a later point joins the leg it is placed in, not the one its corridor progress suggests.
function withLeg(trip: Trip, id: string, change: Partial<RoutePoint>): Trip {
    const routeOrder = orderedRoutePoints(trip).slice(1, -1).map(p => p.id);
    return { ...trip, routeOrder, points: trip.points.map(p => p.id === id ? { ...p, ...change } : p) };
}

// Places `point` in the leg that ends at `legEndId`. Both halves keep that leg's mode; a drawing splits at its vertex nearest the point.
function intoLeg(trip: Trip, point: RoutePoint, legEndId: string): Trip {
    const end = trip.points.find(p => p.id === legEndId)!;
    const placed: RoutePoint = { ...point, leg: end.leg };
    let points = trip.points;
    if (end.leg === 'drawn') {
        const drawn = end.drawn ?? [];
        const cut = drawn.reduce((best, c, i) => kilometres(c, point.coordinate) < kilometres(drawn[best], point.coordinate) ? i : best, 0);
        placed.drawn = drawn.slice(0, cut);
        points = points.map(p => p.id === end.id ? { ...p, drawn: drawn.slice(cut + 1) } : p);
    }
    return { ...trip, points: [...points, placed], routeOrder: orderBefore(trip, point.id, legEndId) };
}

function nearestLegEnd(trip: Trip, coordinate: Coordinate): string {
    const { coordinates, stops } = routeLayout(trip);
    const distance = nearestProgress(coordinates, coordinate) * stops.at(-1)!.distance;
    return (stops.find(stop => stop.distance >= distance) ?? stops.at(-1)!).point.id;
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
        label: i ? shapeLabel : label, coordinate: [...coordinate], progress: i / coordinates.length }));
    return { ...trip, loop: true, routing: undefined, splits: undefined, points: [start, ...shape], routeOrder: shape.map(p => p.id) };
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
        points: trip.points.map(p => p.id === id ? { ...p, kind: 'start' as const, progress: 0, anchor: undefined }
            : p.id === old.id ? { ...p, kind: unnamed ? 'via' as const : 'waypoint' as const } : p) };
}

/** "Start the loop here": a new start at `coordinate` on the leg that ends at `legEndId`. */
export function startLoopHere(trip: Trip, legEndId: string, coordinate: Coordinate): Trip {
    const point: RoutePoint = { id: crypto.randomUUID(), kind: 'via', label: startLabel, coordinate: [...coordinate], progress: 0 };
    return canMoveLoopStart(trip) && trip.points.some(p => p.id === legEndId) ? startLoopAt(intoLeg(trip, point, legEndId), point.id) : trip;
}

export function nightOrderConflicts(trip: Trip): [RoutePoint, RoutePoint][] {
    const nights = trip.points.filter(p => p.kind === 'night').sort((a, b) => a.night! - b.night!);
    const stops = routeStops(trip);
    return nights.slice(1).flatMap((point, i) => stopProgress(stops, point) <= stopProgress(stops, nights[i]) ? [[nights[i], point] as [RoutePoint, RoutePoint]] : []);
}

/** What the history keeps: the plan without its route, which the leg cache rebuilds.
 * A picked alternative stays with its plan, because no request for the plan returns it. */
export function planOf(trip: Trip): Trip {
    const { routing, ...plan } = trip;
    return routing?.picked && routing.key === routingKey(trip) ? trip : plan;
}

/**
 * What a draft or a version stores: the plan, and a picked alternative without the other routes, which "Route options" requests again.
 * The routing package stays behind: only a live answer may tell the map which routing data is in use.
 */
export function storedPlan(trip: Trip): Trip {
    const plan = planOf(trip);
    return plan.routing ? { ...plan, routing: { ...plan.routing, package: undefined, alternatives: [], alternativesReady: false } } : plan;
}

/** Keeps plans (see `planOf`). A trip is never changed in place, so the history shares its objects. */
export class TripHistory {
    private past: Trip[] = [];
    private future: Trip[] = [];
    get canUndo() { return this.past.length > 0; }
    get canRedo() { return this.future.length > 0; }
    commit(before: Trip, after: Trip): Trip {
        this.past = [...this.past.slice(-49), planOf(before)];
        this.future = [];
        return after;
    }
    undo(current: Trip): Trip {
        const previous = this.past.pop();
        if (!previous) return current;
        this.future.push(planOf(current));
        return previous;
    }
    redo(current: Trip): Trip {
        const next = this.future.pop();
        if (!next) return current;
        this.past.push(planOf(current));
        return next;
    }
}
