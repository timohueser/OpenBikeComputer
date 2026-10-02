import { describe, expect, it } from 'vitest';
import { emptyTrip, setEndpoint, removeRoutePoint, maxRidingDays, reorderPoint, routeStops, addRestDay, anchorProgress, addClickedPoint, addPointNear, dayStops, applyBudget, coordinateAt, cumulative, initialTrip, insertPoint, itineraryDays, kilometres, nightOrderConflicts, orderedRoutePoints, overnightCandidates, overnightWindow, pinNight, removeRestDay, routeCoordinates, routeSlice, routingKey, setDrawnLeg, setLegMode, setSplit, TripHistory, tripDays, type Place, type Coordinate, type RoutePoint, type Trip } from './editor';

// Fictional places keep their geographic positions when the mock route changes.
const places: Place[] = [
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
    coordinate: coordinateAt(routeCoordinates(initialTrip()), place.progress),
}));


describe('overnight edits', () => {
    it('keeps the incoming leg when replacing an overnight or converting a visit', () => {
        const initial = initialTrip();
        const drawing: Coordinate[] = [[7.3, 47.7], [7.1, 47.6]];
        const pinned = setDrawnLeg(pinNight(initial, 1, [7, 47.5], 'Old camp'), 'night-1', drawing);
        const replaced = pinNight(pinned, 1, [6.9, 47.5], 'New camp');
        expect(replaced.points.find(point => point.id === 'night-1')).toMatchObject({ leg: 'drawn', drawn: drawing, label: 'New camp' });
        const visit: RoutePoint = { id: 'visit', kind: 'waypoint', label: 'Visit', coordinate: [7.2, 47.5], progress: .3 };
        const shaped = setDrawnLeg(addClickedPoint(pinned, visit), 'visit', drawing);
        const converted = pinNight(shaped, 1, visit.coordinate, 'Camp', visit.id);
        expect(converted.points.some(point => point.id === visit.id)).toBe(false);
        expect(converted.points.filter(point => point.kind === 'night')).toHaveLength(1);
        expect(converted.points.find(point => point.id === 'night-1')).toMatchObject({ leg: 'drawn', drawn: drawing });
        expect(orderedRoutePoints(converted).map(point => point.id)).toEqual(['start', 'night-1', 'finish']);
        const straight = setLegMode(converted, 'night-1', 'straight');
        expect(pinNight(straight, 1, [7, 47.5], 'Other camp').points.find(point => point.id === 'night-1')?.leg).toBe('straight');
    });

    it('adds a riding day for a final overnight without exceeding the shared limit', () => {
        const initial = initialTrip();
        const extended = pinNight(initial, initial.days, [6.5, 47.4], 'Last night');
        expect(extended.days).toBe(initial.days + 1);
        expect(extended.target).toBe(initial.target + 1);
        const longest = applyBudget(initial, 'days', maxRidingDays, 50);
        expect(pinNight(longest, maxRidingDays, [6.5, 47.4], 'Too late')).toBe(longest);
        expect(pinNight(longest, maxRidingDays - 1, [6.5, 47.4], 'Last valid night').days).toBe(maxRidingDays);
    });
});

describe('planner commitments', () => {
    it('keeps a previous pin intact through a later edit and undo', () => {
        const initial = initialTrip();
        const coordinates = routeCoordinates(initial);
        const first = pinNight(initial, 1, coordinateAt(coordinates, .35), 'Friend’s house');
        const history = new TripHistory();
        const moved = history.commit(first, pinNight(first, 1, coordinateAt(coordinates, .65), 'New spot'));
        expect(first.points.find(p => p.night === 1)?.label).toBe('Friend’s house');
        expect(history.undo(moved)).toEqual(first);
        expect(history.redo(first)).toEqual(moved);
    });

    it('keeps plans without their routes in the history', () => {
        const plan: Trip = { ...initialTrip(), live: true };
        const routed: Trip = { ...plan, routing: { key: routingKey(plan) } as Trip['routing'] };
        const history = new TripHistory();
        const next = history.commit(routed, { ...routed, days: 5 });
        expect(next.routing).toBe(routed.routing);
        const previous = history.undo(next);
        expect(previous).toEqual(plan);
        expect('routing' in previous).toBe(false);
        expect(routed.routing).toBeDefined();
        expect('routing' in history.redo(previous)).toBe(false);
    });

    it('reports crossed overnight choices without changing their day assignments', () => {
        const initial = initialTrip();
        const coordinates = routeCoordinates(initial);
        const first = pinNight(initial, 1, coordinateAt(coordinates, .8), 'Night one');
        const crossed = pinNight(first, 2, coordinateAt(coordinates, .3), 'Night two');
        expect(crossed.points.find(p => p.label === 'Night one')?.night).toBe(1);
        expect(crossed.points.find(p => p.label === 'Night two')?.night).toBe(2);
        expect(nightOrderConflicts(crossed)).toHaveLength(1);
    });

    it('keeps the highest pinned night when the requested budget is smaller', () => {
        const initial = initialTrip();
        const pinned = pinNight(initial, 4, coordinateAt(routeCoordinates(initial), .8), 'Fourth night');
        const updated = applyBudget(pinned, 'days', 2, 50);
        expect(updated.days).toBe(5);
        expect(tripDays(pinned)).toHaveLength(5);
        expect(updated.points.find(p => p.night === 4)?.label).toBe('Fourth night');
    });

    it('allows a manual overnight exception without relaxing the distance rule', () => {
        const initial = { ...initialTrip(), limit: 20 };
        const pinned = pinNight(initial, 1, coordinateAt(routeCoordinates(initial), .6), 'Required hotel');
        expect(pinned.limit).toBe(20);
        expect(pinned.points.some(p => p.label === 'Required hotel')).toBe(true);
        expect(tripDays(pinned)[0].distance).toBeGreaterThan(pinned.limit);
    });
});

describe('overnight suggestions', () => {
    it('clips the window to the distance available on both sides', () => {
        const trip = initialTrip();
        const total = cumulative(routeCoordinates(trip)).at(-1)!;
        trip.limit = total / 3 + .5;
        const window = overnightWindow(trip, 1);
        expect(window.blocked).toBe(false);
        expect(window.to * total).toBeLessThanOrEqual(trip.limit + 1e-8);
        expect((1 - window.from) * total).toBeLessThanOrEqual(2 * trip.limit + 1e-8);
        trip.limit = total / 3 - 1;
        expect(overnightWindow(trip, 1).blocked).toBe(true);
    });

    it('balances between fixed nights while ignoring its own pin as an anchor', () => {
        let trip = { ...initialTrip(), days: 4, limit: 0 };
        const coordinates = routeCoordinates(trip);
        trip = pinNight(trip, 1, coordinateAt(coordinates, .2), 'Keep first night');
        trip = pinNight(trip, 2, coordinateAt(coordinates, .3), 'Move this night');
        trip = pinNight(trip, 3, coordinateAt(coordinates, .8), 'Keep third night');
        const snapshot = structuredClone(trip);
        const window = overnightWindow(trip, 2);
        expect(window.center).toBeCloseTo(.5, 4);
        expect(window.from).toBeCloseTo(.44, 4);
        expect(window.to).toBeCloseTo(.56, 4);
        expect(trip).toEqual(snapshot);
        expect(places.map(p => p.coordinate)).toEqual(places.map(p => coordinateAt(routeCoordinates(initialTrip()), p.progress)));
    });
});

describe('route slices', () => {
    it('interpolates boundaries and retains the vertices between them', () => {
        const coordinates: [number, number][] = [[0, 0], [1, 0], [2, 0], [3, 0]];
        const slice = routeSlice(coordinates, .2, .8);
        expect(slice[0][0]).toBeCloseTo(.6, 10);
        expect(slice.at(-1)![0]).toBeCloseTo(2.4, 10);
        expect(slice.slice(1, -1)).toEqual([[1, 0], [2, 0]]);
        expect(cumulative(slice).at(-1)).toBeCloseTo(cumulative(coordinates).at(-1)! * .6, 7);
        expect(routeSlice(coordinates, .8, .2)).toEqual([...slice].reverse());
    });
});

describe('rest days', () => {
    it('inserts consecutive rest days without moving nights or changing the route', () => {
        const initial = initialTrip();
        const trip = pinNight(initial, 1, coordinateAt(routeCoordinates(initial), .35), 'Stay two more nights');
        const withRest = addRestDay(addRestDay(trip, 1), 1);
        const itinerary = itineraryDays(withRest);
        expect(itinerary.map(d => [d.number, d.ridingNumber, d.rest])).toEqual([
            [1, 1, false], [2, 1, true], [3, 1, true], [4, 2, false], [5, 3, false],
        ]);
        expect(itinerary[1]).toMatchObject({ distance: 0, hours: 0, from: itinerary[0].to, to: itinerary[0].to, restIndex: 0 });
        expect(withRest.target).toBe(5);
        expect(routeCoordinates(withRest)).toEqual(routeCoordinates(trip));
        expect(withRest.points).toEqual(trip.points);
        const removed = removeRestDay(withRest, itinerary[2].restIndex!);
        expect(removed.restAfter).toEqual([1]);
        expect(removed.target).toBe(4);
        expect(removed.points).toEqual(trip.points);
    });

    it('counts rest days inside the calendar budget while retaining fixed nights', () => {
        const initial = initialTrip();
        const withRest = addRestDay(initial, 1);
        expect(applyBudget(withRest, 'days', 4, 50).days).toBe(3);
        const pinned = pinNight(withRest, 3, coordinateAt(routeCoordinates(initial), .8), 'Required last night');
        const shorter = applyBudget(pinned, 'days', 3, 50);
        expect(shorter.days).toBe(4);
        expect(itineraryDays(shorter)).toHaveLength(5);
        expect(shorter.points).toEqual(pinned.points);
    });
});

describe('point roles', () => {
    it('keeps a later overnight on the same route after an earlier long excursion', () => {
        const initial = initialTrip();
        const base = routeCoordinates(initial);
        const excursion = coordinateAt(base, .15);
        excursion[1] += .4;
        const withDetour: Trip = { ...initial, points: [...initial.points, { id: 'detour', kind: 'detour', label: 'Long visit', coordinate: excursion, progress: .15 }] };
        const overnight = coordinateAt(base, .7);
        const pinned = pinNight(withDetour, 2, overnight, 'Same route overnight');
        expect(pinned.points.find(p => p.kind === 'night')!.progress).toBeCloseTo(.7, 6);
        expect(anchorProgress(overnight)).toBeCloseTo(.7, 6);
        expect(cumulative(routeCoordinates(pinned)).at(-1)).toBeCloseTo(cumulative(routeCoordinates(withDetour)).at(-1)!, 5);
        expect(pinned.points.find(p => p.id === 'detour')).toEqual(withDetour.points.at(-1));
    });

    it('returns from a detour to its anchor while a named waypoint stays on the through-route', () => {
        const initial = initialTrip();
        const base = routeCoordinates(initial);
        const anchor = coordinateAt(base, .5);
        const destination: Coordinate = [anchor[0], anchor[1] + .08];
        const point = { id: 'visit', coordinate: destination, progress: .5, label: 'Hilltop visit' };
        const detourTrip: Trip = { ...initial, points: [...initial.points, { ...point, kind: 'detour' }] };
        const detour = routeCoordinates(detourTrip);
        const index = detour.findIndex(p => p[0] === destination[0] && p[1] === destination[1]);
        expect(detour[index - 1]).toEqual(anchor);
        expect(detour[index + 1]).toEqual(anchor);
        expect(cumulative(detour).at(-1)).toBeCloseTo(cumulative(base).at(-1)! + 2 * kilometres(anchor, destination), 5);
        const waypoint = routeCoordinates({ ...initial, points: [...initial.points, { ...point, kind: 'waypoint' }] });
        const waypointIndex = waypoint.findIndex(p => p[0] === destination[0] && p[1] === destination[1]);
        expect(waypoint[waypointIndex - 1]).not.toEqual(waypoint[waypointIndex + 1]);
        expect(routeCoordinates({ ...initial, points: [...initial.points, { ...point, kind: 'via' }] })).toEqual(waypoint);
        expect(routeCoordinates({ ...initial, points: [...initial.points, { ...point, kind: 'pass' }] })).toEqual(waypoint);
        expect(routeCoordinates({ ...initial, points: [...initial.points, { ...point, kind: 'marker' }] })).toEqual(base);
    });
});


describe('planning modes', () => {
    it('keeps the route and day decisions when switching to a single route and back', () => {
        const initial = initialTrip();
        const trip = addRestDay(pinNight(initial, 1, coordinateAt(routeCoordinates(initial), .35), 'Camp'), 1);
        const single: Trip = { ...trip, mode: 'route' };
        expect(routeCoordinates(single)).toEqual(routeCoordinates(trip));
        expect(itineraryDays(single)).toHaveLength(1);
        expect(tripDays(single)[0].distance).toBeCloseTo(cumulative(routeCoordinates(trip)).at(-1)!);
        const restored: Trip = { ...single, mode: 'trip' };
        expect(itineraryDays(restored)).toEqual(itineraryDays(trip));
        expect(restored.points).toEqual(trip.points);
    });
});


describe('day budgets', () => {
    it('retains only rest days that fit the requested calendar budget', () => {
        const lateRest = { ...addRestDay(initialTrip(), 2), restNames: ['Visit friends'] };
        const shortened = applyBudget(lateRest, 'days', 2, 50);
        expect(shortened.days).toBe(2);
        expect(shortened.restAfter).toEqual([]);
        expect(itineraryDays(shortened)).toHaveLength(2);
        const earlyRests = { ...addRestDay(addRestDay(initialTrip(), 1), 1), restNames: ['Walk', 'Museum'] };
        const retained = applyBudget(earlyRests, 'days', 2, 50);
        expect(retained.days).toBe(1);
        expect(retained.restNames).toEqual(['Walk']);
        expect(itineraryDays(retained)).toHaveLength(2);
        const pinned = pinNight(initialTrip(), 2, [6.8, 47.5], 'Camp');
        expect(applyBudget(pinned, 'days', 1, 50).days).toBe(3);
    });
});

describe('single-route stop order', () => {
    it('reorders destinations while retaining shapes and drawings only on unchanged legs', () => {
        const points = ['start','shape-a','a','shape-b','b','shape-c','c','d','shape-finish','finish'].map((id, i, ids) => ({
            id, label: id, kind: id.startsWith('shape') ? 'via' as const : id === 'start' ? 'start' as const : id === 'finish' ? 'finish' as const : 'waypoint' as const,
            coordinate: [7 + i * .001, 48] as Coordinate, progress: i / (ids.length - 1),
            ...(id === 'd' || id === 'finish' ? {leg: 'drawn' as const, drawn: [[7.1,48.1] as Coordinate]} : {}),
        }));
        const trip: Trip = {...emptyTrip(),points,routeOrder:points.map(point => point.id)};
        const changed = reorderPoint(trip,'b',1);
        expect(orderedRoutePoints(changed).map(point => point.id)).toEqual(['start','shape-a','a','c','b','d','shape-finish','finish']);
        expect(changed.points.find(point => point.id === 'd')?.drawn).toBeUndefined();
        expect(changed.points.find(point => point.id === 'finish')?.drawn).toEqual(points.at(-1)!.drawn);
        expect(reorderPoint(trip,'shape-b',1)).toBe(trip);
        const history = new TripHistory();
        history.commit(trip,changed);
        expect(history.undo(changed)).toEqual(trip);
    });
    it('inserts a stop across multiple positions and keeps both endpoints fixed', () => {
        const initial = initialTrip();
        const base = routeCoordinates(initial);
        const trip: Trip = { ...initial, points: [...initial.points, ...[.2, .4, .6].map((progress, index) => ({
            id: `stop-${index}`, label: `Stop ${index}`, kind: 'waypoint' as const, progress, coordinate: coordinateAt(base, progress),
        }))] };
        const moved = reorderPoint(trip, 'stop-0', 2);
        expect(routeStops(moved).map(stop => stop.point.id)).toEqual(['start', 'stop-1', 'stop-2', 'stop-0', 'finish']);
        expect(routeStops(reorderPoint(moved, 'stop-0', -2)).map(stop => stop.point.id)).toEqual(['start', 'stop-0', 'stop-1', 'stop-2', 'finish']);
        expect(reorderPoint(trip, 'stop-0', -1)).toBe(trip);
        expect(reorderPoint(trip, 'stop-0', 3)).toBe(trip);
        expect(reorderPoint(trip, 'finish', -1)).toBe(trip);
    });
    it('changes the travelled geometry and cumulative stop distances without moving places', () => {
        const initial = initialTrip();
        const base = routeCoordinates(initial);
        const trip: Trip = { ...initial, mode:'route', points:[...initial.points,
            {id:'first',label:'First',kind:'waypoint',progress:.25,coordinate:coordinateAt(base,.25)},
            {id:'second',label:'Second',kind:'waypoint',progress:.7,coordinate:coordinateAt(base,.7)}] };
        const reordered = reorderPoint(trip,'first',1);
        const stops = routeStops(reordered);
        expect(stops.map(s=>s.point.id)).toEqual(['start','second','first','finish']);
        expect(reordered.points).toEqual(trip.points);
        expect(stops[2].distance).toBeGreaterThan(stops[1].distance);
        expect(stops.at(-1)!.distance).toBeCloseTo(cumulative(routeCoordinates(reordered)).at(-1)!);
        expect(stops.at(-1)!.distance).toBeGreaterThan(cumulative(routeCoordinates(trip)).at(-1)!);
        expect(reorderPoint(reordered,'start',1)).toEqual(reordered);
        expect(routeCoordinates({...reordered,mode:'trip'})).toEqual(routeCoordinates(reordered));
    });
    it('keeps rest-day names attached when another rest day is removed', () => {
        const trip:Trip={...addRestDay(addRestDay(initialTrip(),1),2),restNames:['Hike','Visit friends']};
        expect(removeRestDay(trip,0).restNames).toEqual(['Visit friends']);
        expect(removeRestDay(trip,0).restAfter).toEqual([2]);
    });
});


describe('ordered overnight occurrences', () => {
    it('uses the planned visit occurrence on a backtracking route and reports reversed nights', () => {
        const initial=initialTrip();
        const base=routeCoordinates(initial);
        const pinned=pinNight(pinNight(initial,1,coordinateAt(base,.3),'First night'),2,coordinateAt(base,.7),'Second night');
        const reversed=reorderPoint(pinned,'night-1',1);
        const stops=routeStops(reversed);
        const total=stops.at(-1)!.distance;
        const first=stops.find(s=>s.point.id==='night-1')!.distance;
        const second=stops.find(s=>s.point.id==='night-2')!.distance;
        expect(first).toBeGreaterThan(second);
        expect(tripDays(reversed)[0].to).toBeCloseTo(first/total);
        expect(tripDays(reversed)[1].to).toBeCloseTo(second/total);
        expect(nightOrderConflicts(reversed)).toHaveLength(1);
        expect(overnightWindow(reversed,2).center).toBeGreaterThan(first/total);
    });
});


describe('provisional day ends', () => {
    it('holds a dragged day end while the free nights re-balance around it', () => {
        const trip = setSplit({ ...initialTrip(), days: 4 }, 2, .7);
        const days = tripDays(trip);
        expect(days.map(d => d.to)).toEqual([expect.closeTo(.35, 9), .7, expect.closeTo(.85, 9), 1]);
        expect(days.map(d => !!d.split)).toEqual([false, true, false, false]);
        const pinned = pinNight(trip, 1, coordinateAt(routeCoordinates(trip), .2), 'Camp');
        expect(tripDays(pinned)[1].to).toBe(.7);
        expect(overnightWindow(pinned, 3).center).toBeCloseTo(.85, 9);
    });

    it('never moves a day end past its neighbours and keeps a day of 1 km beside them', () => {
        const trip = setSplit({ ...initialTrip(), days: 4 }, 2, .7);
        const squeezedAfter = tripDays(setSplit(trip, 3, .1));
        expect(squeezedAfter[1].to).toBe(.7);
        expect(squeezedAfter[2].distance).toBeCloseTo(1, 6);
        const squeezedBefore = tripDays(setSplit(trip, 1, .95));
        expect(squeezedBefore[1].to).toBe(.7);
        expect(squeezedBefore[1].distance).toBeCloseTo(1, 6);
    });

    it('drops a dragged split that a pinned night leaves no day for', () => {
        const trip = setSplit({ ...initialTrip(), days: 4 }, 2, .7);
        const pinned = pinNight(trip, 1, coordinateAt(routeCoordinates(trip), .8), 'Late camp');
        const days = tripDays(pinned);
        expect(days.map(d => d.to)).toEqual([expect.closeTo(.8, 4), expect.closeTo(.8 + .2 / 3, 4), expect.closeTo(.8 + .4 / 3, 4), 1]);
        expect(days.every(d => d.distance > 1)).toBe(true);
        expect(days[1].split).toBe(false);
    });

    it('clears a split when its night is pinned or the day count changes', () => {
        const trip = setSplit(setSplit({ ...initialTrip(), days: 4 }, 1, .1), 2, .7);
        const pinned = pinNight(trip, 2, coordinateAt(routeCoordinates(trip), .6), 'Inn');
        expect(pinned.splits).toEqual({ 1: .1 });
        expect(tripDays(pinned)[1].split).toBe(false);
        expect(applyBudget(trip, 'days', 4, 50).splits).toEqual(trip.splits);
        expect(applyBudget(trip, 'days', 5, 50).splits).toBeUndefined();
    });

    it('offers three overnight candidates without water, shortest predicted day first', () => {
        const candidates = overnightCandidates(initialTrip(), 1, places);
        expect(candidates).toHaveLength(3);
        expect(candidates.every(c => c.place.category !== 'water')).toBe(true);
        expect(candidates.map(c => c.distance)).toEqual([...candidates.map(c => c.distance)].sort((a, b) => a - b));
    });
});

describe('legs', () => {
    const visit = (trip: Trip, progress: number): RoutePoint => ({ id: 'visit', kind: 'waypoint', label: 'Visit', progress, coordinate: coordinateAt(routeCoordinates(trip), progress) });
    const length = (trip: Trip) => cumulative(routeCoordinates(trip)).at(-1)!;

    it('follows the geometry of straight, drawn and routed legs', () => {
        const initial = initialTrip();
        const trip: Trip = { ...initial, points: [...initial.points, visit(initial, .5)] };
        const straight = setLegMode(trip, 'visit', 'straight');
        expect(length(straight)).toBeLessThan(length(trip));
        const start = trip.points[0].coordinate;
        const end = trip.points.find(p => p.id === 'visit')!.coordinate;
        expect(routeCoordinates(straight).slice(0, 2)).toEqual([start, end]);
        const sketch: Coordinate[] = [[7.4, 47.7], [7.2, 47.7]];
        const drawn = setDrawnLeg(trip, 'visit', sketch);
        expect(routeCoordinates(drawn).slice(0, 4)).toEqual([start, ...sketch, end]);
        expect(routeStops(drawn)[1].distance).toBeCloseTo(cumulative([start, ...sketch, end]).at(-1)!, 9);
        expect(routeCoordinates(setLegMode(drawn, 'visit', 'routed'))).toEqual(routeCoordinates(trip));
    });

    it('inserts a point into one leg and keeps the other points in order', () => {
        const initial = initialTrip();
        const trip = setLegMode({ ...initial, points: [...initial.points, visit(initial, .5)] }, 'visit', 'straight');
        const between = coordinateAt(routeCoordinates(trip), .25);
        const inserted = insertPoint(trip, 'visit', between);
        const order = orderedRoutePoints(inserted);
        expect(order.map(p => p.id)).toEqual(['start', order[1].id, 'visit', 'finish']);
        expect(order[1]).toMatchObject({ kind: 'via', coordinate: between, leg: 'straight' });
        expect(routeCoordinates(inserted).slice(0, 3)).toEqual([trip.points[0].coordinate, between, trip.points.find(p => p.id === 'visit')!.coordinate]);
    });

    it('adds a clicked point at the end of a single route and a pinned night in its nearest leg', () => {
        const initial: Trip = { ...initialTrip(), mode: 'route' };
        const early = visit(initial, .1);
        const appended = addClickedPoint({ ...initial, points: [...initial.points, visit(initial, .5)] }, { ...early, id: 'early' });
        expect(orderedRoutePoints(appended).map(p => p.id)).toEqual(['start', 'visit', 'early', 'finish']);
        const pinned = pinNight(appended, 1, coordinateAt(routeCoordinates(initial), .3), 'Camp');
        expect(orderedRoutePoints(pinned).map(p => p.id)).toEqual(['start', 'night-1', 'visit', 'early', 'finish']);
    });

    it('keeps a visit added before a pinned night inside that night\'s day', () => {
        const initial = initialTrip();
        // A leg mode fixes the route order, which is where a clicked point used to land after the night.
        const pinned = setLegMode(pinNight(initial, 1, coordinateAt(routeCoordinates(initial), .4), 'Camp'), 'night-1', 'routed');
        const nightKm = (trip: Trip) => routeStops(trip).find(stop => stop.point.id === 'night-1')!.distance;
        const dayEndKm = (trip: Trip) => tripDays(trip)[0].to * cumulative(routeCoordinates(trip)).at(-1)!;
        expect(dayEndKm(pinned)).toBeCloseTo(nightKm(pinned), 9);
        const added = addClickedPoint(pinned, visit(initial, .2));
        expect(orderedRoutePoints(added).map(p => p.id)).toEqual(['start', 'visit', 'night-1', 'finish']);
        expect(nightKm(added)).toBeCloseTo(nightKm(pinned), 6);
        expect(dayEndKm(added)).toBeCloseTo(nightKm(added), 9);
        const [first] = dayStops(added, tripDays(added)[0]);
        expect(first.point.id).toBe('visit');
        expect(first.km).toBeCloseTo(routeStops(added)[1].distance, 9);
        expect(dayStops(added, tripDays(added)[1])).toEqual([]);
    });

    it('splits a drawn leg where a pinned night joins it', () => {
        const initial = initialTrip();
        const drawn = setDrawnLeg(initial, 'finish', [[7.2, 47.7], [6.8, 47.7], [6.4, 47.5]]);
        const pinned = pinNight(drawn, 1, [6.8, 47.69], 'Camp');
        expect(routeCoordinates(pinned)).toEqual([initial.points[0].coordinate, [7.2, 47.7], [6.8, 47.69], [6.4, 47.5], initial.points[1].coordinate]);
    });
});


describe('route endpoints', () => {
    it.each(['route', 'trip'] as const)('builds a %s from either endpoint and promotes points in route order', mode => {
        const empty = emptyTrip(mode);
        expect(routeCoordinates(empty)).toEqual([]);
        expect(tripDays(empty)).toEqual([]);
        const finish = setEndpoint(empty, 'finish', [8, 48], 'Destination');
        expect(routeCoordinates(finish)).toEqual([]);
        expect(tripDays(finish)).toEqual([]);
        const complete = setEndpoint(finish, 'start', [7.8, 48], 'Origin');
        const startId = complete.points.find(p => p.kind === 'start')!.id;
        const finishId = finish.points[0].id;
        const a: RoutePoint = { id: 'a', kind: 'waypoint', label: 'A', coordinate: [7.9, 48], progress: .7 };
        const b: RoutePoint = { ...a, id: 'b', label: 'B', progress: .3 };
        const trip = { ...complete, points: [...complete.points, a, b, { ...a, id: 'marker', kind: 'marker' as const }], routeOrder: ['a', 'b'] };
        const shorter = removeRoutePoint(trip, startId);
        expect(orderedRoutePoints(shorter).map(p => [p.id, p.kind])).toEqual([['a', 'start'], ['b', 'waypoint'], [finishId, 'finish']]);
        const both = removeRoutePoint(shorter, finishId);
        expect(orderedRoutePoints(both).map(p => [p.id, p.kind])).toEqual([['a', 'start'], ['b', 'finish']]);
        const one = removeRoutePoint(both, 'a');
        expect(one.points.find(p => p.id === 'b')?.kind).toBe('finish');
        expect(routeCoordinates(one)).toEqual([]);
        expect(tripDays(one)).toEqual([]);
        const cleared = removeRoutePoint(one, 'b');
        expect(orderedRoutePoints(cleared)).toEqual([]);
        expect(cleared.points.map(p => p.id)).toEqual(['marker']);
        expect(routeStops(cleared)).toEqual([]);
    });

    it('clears overnight and detour semantics when promoting endpoints, and replaces an endpoint in place', () => {
        const trip = pinNight(initialTrip(), 1, [7.3, 47.6], 'Camp');
        const next = removeRoutePoint(trip, 'start');
        const start = next.points.find(p => p.kind === 'start')!;
        expect(start).toMatchObject({ label: 'Camp', progress: 0 });
        expect(start.night).toBeUndefined();
        expect(start.id).not.toBe('night-1');
        expect(setEndpoint(next, 'start', [7.4, 47.6], 'New start').points.filter(p => p.kind === 'start')).toEqual([
            expect.objectContaining({ id: start.id, label: 'New start', coordinate: [7.4, 47.6] }),
        ]);
        const history = new TripHistory();
        history.commit(trip, next);
        expect(history.undo(next)).toEqual(trip);
        expect(history.redo(trip)).toEqual(next);
    });

    it('extends the finish through the previous destination and preserves the day plan and manual leg', () => {
        const original = setLegMode({ ...pinNight(initialTrip(), 1, [7.3, 47.6], 'Camp'), mode: 'trip', restAfter: [1], splits: { 2: .8 } } as Trip, 'finish', 'straight');
        const next = setEndpoint(original, 'finish', [5.9, 47.2], 'New destination');
        const order = orderedRoutePoints(next);
        expect(order.slice(-2)).toEqual([
            expect.objectContaining({ id: 'finish', kind: 'waypoint', label: 'Besançon', leg: 'straight' }),
            expect.objectContaining({ kind: 'finish', label: 'New destination' }),
        ]);
        expect(next).toMatchObject({ mode: 'trip', days: original.days, target: original.target, restAfter: [1], splits: { 2: .8 } });
        expect(next.points.find(p => p.kind === 'night')).toEqual(original.points.find(p => p.kind === 'night'));
    });

    it('gives a pinned overnight its explicit name when it comes from an automatically named visit', () => {
        const visit: RoutePoint = { id: 'visit', kind: 'waypoint', autoLabel: true, label: '48.00000, 7.84000', coordinate: [7.84, 48], progress: .3 };
        const trip = addPointNear(initialTrip(), visit);
        const pinned = pinNight(trip, 1, visit.coordinate, visit.label, visit.id);
        expect(pinned.points.find(p => p.night === 1)?.autoLabel).toBeUndefined();
        const replaced = pinNight(pinned, 1, [8.088, 48.279], 'Fuxxbau');
        expect(replaced.points.find(p => p.night === 1)).toMatchObject({ label: 'Fuxxbau', coordinate: [8.088, 48.279] });
        expect(replaced.points.find(p => p.night === 1)?.autoLabel).toBeUndefined();
    });
});
