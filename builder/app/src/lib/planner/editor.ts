export type Coordinate = [number, number];
export type PointKind = 'start' | 'finish' | 'pass' | 'via' | 'waypoint' | 'detour' | 'night' | 'marker' | 'place';
export interface RoutePoint {
    id: string;
    coordinate: Coordinate;
    label: string;
    kind: PointKind;
    progress: number;
    night?: number;
}
export type Place = RoutePoint & {
    category: 'hotel' | 'camp' | 'water';
    description: string;
};
export interface Trip {
    bike?: import('./riding-profiles').BikeType;
    preset?: string;
    routeOrder?: string[];
    restNames?: string[];
    mode?: 'route' | 'trip';
    points: RoutePoint[];
    days: number;
    budget: 'days' | 'distance' | 'hours';
    target: number;
    limit: number;
    variant: 'valley' | 'direct';
    restAfter?: number[];
}
export interface Day {
    number: number;
    from: number;
    to: number;
    distance: number;
    hours: number;
    pinned: RoutePoint | undefined;
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

// Fictional places keep their geographic positions when the mock route changes.
export const places: Place[] = [
    { progress: .2, category: 'camp', label: 'Orchard camp', description: 'A quiet overnight spot beside an orchard.' },
    { progress: .3, category: 'hotel', label: 'Canal-side rooms', description: 'A small hotel near the canal.' },
    { progress: .37, category: 'camp', label: 'Willow camp', description: 'A camping option beside a willow grove.' },
    { progress: .46, category: 'hotel', label: 'Village inn', description: 'Rooms in a village along the valley.' },
    { progress: .56, category: 'camp', label: 'Meadow camp', description: 'A camping option on an open meadow.' },
    { progress: .65, category: 'hotel', label: 'Riverside rooms', description: 'A small hotel beside the river.' },
    { progress: .73, category: 'camp', label: 'Forest-edge camp', description: 'A camping option near the edge of a wood.' },
    { progress: .82, category: 'hotel', label: 'Valley inn', description: 'Rooms for an overnight stop in the valley.' },
    { progress: .9, category: 'camp', label: 'Mill meadow camp', description: 'A camping option near an old mill.' },
    { progress: .47, category: 'water', label: 'Water stop', description: 'A water point with unverified availability.' },
].map((place, index) => ({
    ...place,
    category: place.category as Place['category'],
    id: `${place.category}-${index}`,
    kind: 'place' as const,
    coordinate: coordinateAt(valley, place.progress),
}));

export const searchPlaces = places;

export function initialTrip(): Trip {
    return {
        points: [
            { id: 'start', coordinate: [...valley[0]], label: 'Basel', kind: 'start', progress: 0 },
            { id: 'finish', coordinate: [...valley.at(-1)!], label: 'Besançon', kind: 'finish', progress: 1 },
        ],
        days: 3, budget: 'days', target: 3, limit: 50, variant: 'valley', restAfter: [],
    };
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
    return [...trip.points.filter(p => p.kind === 'start'), ...ranked, ...middle.filter(p => !order.includes(p.id)), ...trip.points.filter(p => p.kind === 'finish')];
}

export function reorderPoint(trip: Trip, id: string, direction: -1 | 1): Trip {
    const points = orderedRoutePoints(trip);
    const index = points.findIndex(p => p.id === id);
    const target = index + direction;
    if (index <= 0 || index >= points.length - 1 || target <= 0 || target >= points.length - 1) return trip;
    [points[index], points[target]] = [points[target], points[index]];
    return { ...trip, routeOrder: points.map(p => p.id) };
}

function routeLayout(trip: Trip): {coordinates:Coordinate[]; stops:{point:RoutePoint; distance:number}[]} {
    const base = corridorCoordinates(trip);
    const points = orderedRoutePoints(trip);
    if (!trip.routeOrder) {
        const total = cumulative(base).at(-1)!;
        return {coordinates:base, stops:points.map(point=>({point,distance:point.kind==='start'?0:point.kind==='finish'?total:nearestProgress(base,point.coordinate)*total}))};
    }
    const coordinates:Coordinate[] = [[...points[0].coordinate]];
    const stops = [{point:points[0],distance:0}];
    let distance = 0;
    for (let i=1;i<points.length;i++) {
        const before=points[i-1], point=points[i];
        const from=before.kind==='start'?0:nearestProgress(base,before.coordinate);
        const to=point.kind==='finish'?1:nearestProgress(base,point.coordinate);
        const leg=routeSlice(base,from,to);
        distance+=cumulative(leg).at(-1)!;
        coordinates.push(...leg.slice(1));
        stops.push({point,distance});
    }
    return {coordinates,stops};
}

export function routeCoordinates(trip: Trip): Coordinate[] { return routeLayout(trip).coordinates; }
export function routeStops(trip: Trip): {point:RoutePoint; distance:number}[] { return routeLayout(trip).stops; }

// Stored anchors use one corridor frame; current-route fractions change with every detour.
export function anchorProgress(coordinate: Coordinate): number {
    return nearestProgress(valley, coordinate);
}

export function cumulative(coordinates: Coordinate[]): number[] {
    const result = [0];
    coordinates.slice(1).forEach((p, i) => result.push(result[i] + kilometres(coordinates[i], p)));
    return result;
}

export function coordinateAt(coordinates: Coordinate[], progress: number): Coordinate {
    if (coordinates.length === 0) throw new Error('A route needs at least one coordinate.');
    if (coordinates.length === 1) return [...coordinates[0]];
    const distances = cumulative(coordinates);
    const goal = distances.at(-1)! * Math.max(0, Math.min(1, progress));
    const index = Math.max(1, distances.findIndex(d => d >= goal));
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
    const inside = coordinates.filter((_, i) => distances[i] > start * total && distances[i] < end * total);
    return [first, ...inside.map(p => [...p] as Coordinate), coordinateAt(coordinates, end)];
}

function stopProgress(trip:Trip, coordinates:Coordinate[], point:RoutePoint):number {
    if (!trip.routeOrder) return nearestProgress(coordinates,point.coordinate);
    const stops=routeStops(trip);
    return (stops.find(stop=>stop.point.id===point.id)?.distance??0)/(stops.at(-1)?.distance||1);
}

export function overnightWindow(trip: Trip, night: number): { from: number; to: number; center: number; blocked: boolean } {
    const coordinates = routeCoordinates(trip);
    const total = cumulative(coordinates).at(-1)!;
    const pinned = trip.points.filter(p => p.kind === 'night' && p.night !== night).sort((a, b) => a.night! - b.night!);
    const count = Math.max(trip.days, ...trip.points.filter(p => p.kind === 'night').map(p => p.night! + 1));
    const before = pinned.filter(p => p.night! < night).at(-1);
    const after = pinned.find(p => p.night! > night);
    const lo = before ? stopProgress(trip, coordinates, before) : 0;
    const hi = after ? stopProgress(trip, coordinates, after) : 1;
    const daysBefore = night - (before?.night ?? 0);
    const daysAfter = (after?.night ?? count) - night;
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

export function tripDays(trip: Trip): Day[] {
    if (trip.mode === 'route') {
        const distance = cumulative(routeCoordinates(trip)).at(-1)!;
        return [{ number: 1, from: 0, to: 1, distance, hours: distance / 15, pinned: undefined }];
    }
    const coordinates = routeCoordinates(trip);
    const total = cumulative(coordinates).at(-1)!;
    const pinned = trip.points.filter(p => p.kind === 'night').sort((a, b) => a.night! - b.night!);
    const count = Math.max(trip.days, ...pinned.map(p => (p.night ?? 0) + 1));
    const boundaries = [0];
    for (let i = 1; i < count; i++) {
        const pin = pinned.find(p => p.night === i);
        if (pin) boundaries.push(stopProgress(trip, coordinates, pin));
        else {
            const next = pinned.find(p => (p.night ?? 0) > i);
            const end = next ? stopProgress(trip, coordinates, next) : 1;
            const remaining = (next?.night ?? count) - i + 1;
            boundaries.push(boundaries[i - 1] + (end - boundaries[i - 1]) / remaining);
        }
    }
    boundaries.push(1);
    return boundaries.slice(1).map((to, i) => {
        const distance = Math.max(0, to - boundaries[i]) * total;
        return { number: i + 1, from: boundaries[i], to, distance, hours: distance / 15, pinned: pinned.find(p => p.night === i + 1) };
    });
}

export function applyBudget(trip: Trip, budget: Trip['budget'], target: number, limit: number): Trip {
    const total = cumulative(routeCoordinates(trip)).at(-1)!;
    const wanted = budget === 'days' ? Math.round(target) - (trip.restAfter?.length ?? 0) : Math.ceil(total / (budget === 'distance' ? target : target * 15));
    const pinnedMinimum = Math.max(1, ...trip.points.filter(p => p.kind === 'night').map(p => (p.night ?? 0) + 1));
    const days = Math.max(pinnedMinimum, Math.min(14, Math.max(1, wanted)));
    return { ...trip, budget, target, limit, days, restAfter: (trip.restAfter ?? []).filter(after => after <= days), restNames: (trip.restAfter ?? []).flatMap((after,i)=>after<=days?[trip.restNames?.[i]??'']:[]) };
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

export function pinNight(trip: Trip, night: number, coordinate: Coordinate, label: string): Trip {
    const points = trip.points.filter(p => p.id !== `night-${night}`);
    const progress = anchorProgress(coordinate);
    points.push({ id: `night-${night}`, kind: 'night', night, coordinate, label, progress });
    return { ...trip, points };
}

export function nightOrderConflicts(trip: Trip): [RoutePoint, RoutePoint][] {
    const nights = trip.points.filter(p => p.kind === 'night').sort((a, b) => a.night! - b.night!);
    const coordinates=routeCoordinates(trip);
    return nights.slice(1).flatMap((point, i) => stopProgress(trip,coordinates,point) <= stopProgress(trip,coordinates,nights[i]) ? [[nights[i], point] as [RoutePoint, RoutePoint]] : []);
}

export class TripHistory {
    private past: Trip[] = [];
    private future: Trip[] = [];
    get canUndo() { return this.past.length > 0; }
    get canRedo() { return this.future.length > 0; }
    commit(before: Trip, after: Trip): Trip {
        this.past = [...this.past.slice(-49), structuredClone(before)];
        this.future = [];
        return after;
    }
    undo(current: Trip): Trip {
        const previous = this.past.pop();
        if (!previous) return current;
        this.future.push(structuredClone(current));
        return previous;
    }
    redo(current: Trip): Trip {
        const next = this.future.pop();
        if (!next) return current;
        this.past.push(structuredClone(current));
        return next;
    }
}
