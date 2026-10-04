import { describe, expect, it } from 'vitest';
import { closeLoop, loopTrip, startLoopHere, storedPlan, emptyTrip, setEndpoint, removeRoutePoint, maxRidingDays, reorderPoint, addRestDay, addClickedPoint, addPointNear, dayStops, applyBudget, insertPoint, nightOrderConflicts, orderedRoutePoints, overnightCandidates, overnightWindow, pinNight, planView, removeRestDay, replacePoint, routeLegsAround, dragPointOut, routingKey, setDrawnLeg, setLegMode, setSplit, TripHistory, type Place, type RoutePoint, type Trip } from './editor';
import { coordinateAt, kilometres, type Coordinate } from './geo';
import { isTrip } from './trip-validation';
import { routed, testTrip } from '../../../test-support/planner/trip';

// The routed test trip, and positions along its straight line.
const base = () => routed(testTrip());
const line = planView(base()).coordinates;
const along = (fraction: number) => coordinateAt(line, fraction);
const places: Place[] = ([[.2, 'camp', 'Orchard camp'], [.3, 'hotel', 'Canal-side rooms'], [.37, 'camp', 'Willow camp'],
    [.46, 'hotel', 'Village inn'], [.47, 'water', 'Water stop'], [.56, 'camp', 'Meadow camp']] as const)
    .map(([fraction, category, label], index) => ({ id: `${category}-${index}`, kind: 'place', category, label, description: '', coordinate: along(fraction) }));

describe('overnight edits', () => {
    it('keeps the incoming leg when replacing an overnight or converting a visit', () => {
        const drawing: Coordinate[] = [[7.3, 47.7], [7.1, 47.6]];
        const pinned = setDrawnLeg(pinNight(base(), 1, along(.4), 'Old camp'), 'night-1', drawing);
        const replaced = pinNight(pinned, 1, along(.45), 'New camp');
        expect(replaced.points.find(point => point.id === 'night-1')).toMatchObject({ leg: 'drawn', drawn: drawing, label: 'New camp' });
        const visit: RoutePoint = { id: 'visit', kind: 'waypoint', label: 'Visit', coordinate: along(.2) };
        const shaped = setDrawnLeg(addClickedPoint(routed(pinned), visit), 'visit', drawing);
        const converted = pinNight(shaped, 1, visit.coordinate, 'Camp', visit.id);
        expect(converted.points.some(point => point.id === visit.id)).toBe(false);
        expect(converted.points.filter(point => point.kind === 'night')).toHaveLength(1);
        expect(converted.points.find(point => point.id === 'night-1')).toMatchObject({ leg: 'drawn', drawn: drawing });
        expect(orderedRoutePoints(converted).map(point => point.id)).toEqual(['start', 'night-1', 'finish']);
        const straight = setLegMode(converted, 'night-1', 'straight');
        expect(pinNight(straight, 1, along(.4), 'Other camp').points.find(point => point.id === 'night-1')?.leg).toBe('straight');
    });

    it('adds a riding day for a final overnight without exceeding the shared limit', () => {
        const initial = testTrip();
        const extended = pinNight(initial, initial.days, along(.9), 'Last night');
        expect(extended.days).toBe(initial.days + 1);
        expect(extended.target).toBe(initial.target + 1);
        const longest = applyBudget(initial, 'days', maxRidingDays, 50);
        expect(pinNight(longest, maxRidingDays, along(.9), 'Too late')).toBe(longest);
        expect(pinNight(longest, maxRidingDays - 1, along(.9), 'Last valid night').days).toBe(maxRidingDays);
    });
});

describe('planner commitments', () => {
    it('keeps a previous pin intact through a later edit and undo', () => {
        const first = pinNight(testTrip(), 1, along(.35), 'Friend’s house');
        const history = new TripHistory();
        const moved = history.commit(first, pinNight(first, 1, along(.65), 'New spot'));
        expect(first.points.find(p => p.night === 1)?.label).toBe('Friend’s house');
        expect(history.undo(moved)).toEqual(first);
        expect(history.redo(first)).toEqual(moved);
    });

    it('keeps plans without their routes in the history', () => {
        const plan = testTrip(), withLine = routed(plan);
        const history = new TripHistory();
        const next = history.commit(withLine, { ...withLine, days: 5 });
        expect(next.routing).toBe(withLine.routing);
        const previous = history.undo(next);
        expect(previous).toEqual(plan);
        expect('routing' in previous).toBe(false);
        expect('routing' in history.redo(previous)).toBe(false);
    });

    it('reports crossed overnight choices without changing their day assignments', () => {
        const first = pinNight(base(), 1, along(.8), 'Night one');
        const crossed = routed(pinNight(routed(first), 2, along(.3), 'Night two'));
        expect(orderedRoutePoints(crossed).map(p => p.id)).toEqual(['start', 'night-2', 'night-1', 'finish']);
        expect(crossed.points.find(p => p.label === 'Night one')?.night).toBe(1);
        expect(nightOrderConflicts(crossed)).toHaveLength(1);
    });

    it('keeps the highest pinned night when the requested budget is smaller', () => {
        const pinned = pinNight(testTrip(), 4, along(.8), 'Fourth night');
        const updated = applyBudget(pinned, 'days', 2, 50);
        expect(updated.days).toBe(5);
        expect(planView(pinned).days).toHaveLength(5);
        expect(updated.points.find(p => p.night === 4)?.label).toBe('Fourth night');
    });

    it('allows a manual overnight exception without relaxing the distance rule', () => {
        const pinned = routed(pinNight({ ...testTrip(), limit: 20 }, 1, along(.6), 'Required hotel'));
        expect(pinned.limit).toBe(20);
        expect(planView(pinned).days[0].distance).toBeGreaterThan(pinned.limit);
    });
});

describe('overnight suggestions', () => {
    it('clips the window to the distance available on both sides', () => {
        const total = planView(base()).total;
        const trip = { ...base(), limit: total / 3 + .5 };
        const window = overnightWindow(trip, 1);
        expect(window.blocked).toBe(false);
        expect(window.to * total).toBeLessThanOrEqual(trip.limit + 1e-8);
        expect((1 - window.from) * total).toBeLessThanOrEqual(2 * trip.limit + 1e-8);
        expect(overnightWindow({ ...trip, limit: total / 3 - 1 }, 1).blocked).toBe(true);
    });

    it('balances between fixed nights while ignoring its own pin as an anchor', () => {
        let trip: Trip = { ...base(), days: 4, limit: 0 };
        trip = routed(pinNight(trip, 1, along(.2), 'Keep first night'));
        trip = routed(pinNight(trip, 2, along(.3), 'Move this night'));
        trip = routed(pinNight(trip, 3, along(.8), 'Keep third night'));
        const snapshot = structuredClone(trip);
        const window = overnightWindow(trip, 2);
        expect(window.center).toBeCloseTo(.5, 4);
        expect(window.from).toBeCloseTo(.44, 4);
        expect(window.to).toBeCloseTo(.56, 4);
        expect(trip).toEqual(snapshot);
    });
});

describe('rest days', () => {
    it('inserts consecutive rest days without moving nights or changing the route', () => {
        const trip = pinNight(testTrip(), 1, along(.35), 'Stay two more nights');
        const withRest = addRestDay(addRestDay(trip, 1), 1);
        const itinerary = planView(withRest).itinerary;
        expect(itinerary.map(d => [d.number, d.ridingNumber, d.rest])).toEqual([
            [1, 1, false], [2, 1, true], [3, 1, true], [4, 2, false], [5, 3, false],
        ]);
        expect(itinerary[1]).toMatchObject({ distance: 0, hours: 0, from: itinerary[0].to, to: itinerary[0].to, restIndex: 0 });
        expect(withRest.target).toBe(5);
        expect(routingKey(withRest)).toBe(routingKey(trip));
        expect(withRest.points).toEqual(trip.points);
        const removed = removeRestDay(withRest, itinerary[2].restIndex!);
        expect(removed.restAfter).toEqual([1]);
        expect(removed.target).toBe(4);
        expect(removed.points).toEqual(trip.points);
    });

    it('counts rest days inside the calendar budget while retaining fixed nights', () => {
        const withRest = addRestDay(testTrip(), 1);
        expect(applyBudget(withRest, 'days', 4, 50).days).toBe(3);
        const pinned = pinNight(withRest, 3, along(.8), 'Required last night');
        const shorter = applyBudget(pinned, 'days', 3, 50);
        expect(shorter.days).toBe(4);
        expect(planView(shorter).itinerary).toHaveLength(5);
        expect(shorter.points).toEqual(pinned.points);
    });
});

describe('planning modes', () => {
    it('keeps the route and day decisions when switching to a single route and back', () => {
        const trip = routed(addRestDay(pinNight(base(), 1, along(.35), 'Camp'), 1));
        const single: Trip = { ...trip, mode: 'route' };
        expect(planView(single).coordinates).toEqual(planView(trip).coordinates);
        expect(planView(single).itinerary).toHaveLength(1);
        expect(planView(single).days[0].distance).toBeCloseTo(planView(trip).total);
        const restored: Trip = { ...single, mode: 'trip' };
        expect(planView(restored).itinerary).toEqual(planView(trip).itinerary);
        expect(restored.points).toEqual(trip.points);
    });
});

describe('day budgets', () => {
    it('retains only rest days that fit the requested calendar budget', () => {
        const lateRest = { ...addRestDay(testTrip(), 2), restNames: ['Visit friends'] };
        const shortened = applyBudget(lateRest, 'days', 2, 50);
        expect(shortened.days).toBe(2);
        expect(shortened.restAfter).toEqual([]);
        expect(planView(shortened).itinerary).toHaveLength(2);
        const earlyRests = { ...addRestDay(addRestDay(testTrip(), 1), 1), restNames: ['Walk', 'Museum'] };
        const retained = applyBudget(earlyRests, 'days', 2, 50);
        expect(retained.days).toBe(1);
        expect(retained.restNames).toEqual(['Walk']);
        expect(planView(retained).itinerary).toHaveLength(2);
        const pinned = pinNight(testTrip(), 2, along(.5), 'Camp');
        expect(applyBudget(pinned, 'days', 1, 50).days).toBe(3);
    });
});

describe('single-route stop order', () => {
    it('reorders destinations while retaining shapes and drawings only on unchanged legs', () => {
        const points = ['start','shape-a','a','shape-b','b','shape-c','c','d','shape-finish','finish'].map((id, i) => ({
            id, label: id, kind: id.startsWith('shape') ? 'via' as const : id === 'start' ? 'start' as const : id === 'finish' ? 'finish' as const : 'waypoint' as const,
            coordinate: [7 + i * .001, 48] as Coordinate,
            ...(id === 'd' || id === 'finish' ? {leg: 'drawn' as const, drawn: [[7.1,48.1] as Coordinate]} : {}),
        }));
        const trip: Trip = {...emptyTrip(),points,routeOrder:points.slice(1, -1).map(point => point.id)};
        const changed = reorderPoint(trip,'b',1);
        expect(orderedRoutePoints(changed).map(point => point.id)).toEqual(['start','shape-a','a','c','b','d','shape-finish','finish']);
        expect(changed.points.find(point => point.id === 'd')?.drawn).toBeUndefined();
        expect(changed.points.find(point => point.id === 'finish')?.drawn).toEqual(points.at(-1)!.drawn);
        expect(reorderPoint(trip,'shape-b',1)).toBe(trip);
        const history = new TripHistory();
        history.commit(trip,changed);
        expect(history.undo(changed)).toEqual(trip);
    });
    it('moves a stop across multiple positions without moving places and keeps both endpoints fixed', () => {
        const initial = testTrip();
        const trip: Trip = { ...initial, routeOrder: ['stop-0', 'stop-1', 'stop-2'], points: [...initial.points, ...[.2, .4, .6].map((fraction, index) => ({
            id: `stop-${index}`, label: `Stop ${index}`, kind: 'waypoint' as const, coordinate: along(fraction),
        }))] };
        const moved = reorderPoint(trip, 'stop-0', 2);
        expect(planView(moved).stops.map(stop => stop.point.id)).toEqual(['start', 'stop-1', 'stop-2', 'stop-0', 'finish']);
        expect(moved.points).toEqual(trip.points);
        expect(planView(reorderPoint(moved, 'stop-0', -2)).stops.map(stop => stop.point.id)).toEqual(['start', 'stop-0', 'stop-1', 'stop-2', 'finish']);
        expect(reorderPoint(trip, 'stop-0', -1)).toBe(trip);
        expect(reorderPoint(trip, 'stop-0', 3)).toBe(trip);
        expect(reorderPoint(trip, 'finish', -1)).toBe(trip);
    });
    it('keeps rest-day names attached when another rest day is removed', () => {
        const trip:Trip={...addRestDay(addRestDay(testTrip(),1),2),restNames:['Hike','Visit friends']};
        expect(removeRestDay(trip,0).restNames).toEqual(['Visit friends']);
        expect(removeRestDay(trip,0).restAfter).toEqual([2]);
    });
});

describe('ordered overnight occurrences', () => {
    it('uses the planned visit occurrence on a backtracking route and reports reversed nights', () => {
        const pinned = pinNight(routed(pinNight(base(), 1, along(.3), 'First night')), 2, along(.7), 'Second night');
        const reversed = routed(reorderPoint(pinned, 'night-1', 1));
        const stops = planView(reversed).stops;
        const total = stops.at(-1)!.distance;
        const first = stops.find(s => s.point.id === 'night-1')!.distance;
        const second = stops.find(s => s.point.id === 'night-2')!.distance;
        expect(first).toBeGreaterThan(second);
        expect(planView(reversed).days[0].to).toBeCloseTo(first / total);
        expect(planView(reversed).days[1].to).toBeCloseTo(second / total);
        expect(nightOrderConflicts(reversed)).toHaveLength(1);
        expect(overnightWindow(reversed, 2).center).toBeGreaterThan(first / total);
    });
});

describe('provisional day ends', () => {
    it('holds a dragged day end while the free nights re-balance around it', () => {
        const trip = setSplit({ ...base(), days: 4 }, 2, .7);
        const days = planView(trip).days;
        expect(days.map(d => d.to)).toEqual([expect.closeTo(.35, 9), .7, expect.closeTo(.85, 9), 1]);
        expect(days.map(d => !!d.split)).toEqual([false, true, false, false]);
        const pinned = routed(pinNight(trip, 1, along(.2), 'Camp'));
        expect(planView(pinned).days[1].to).toBe(.7);
        expect(overnightWindow(pinned, 3).center).toBeCloseTo(.85, 9);
    });

    it('never moves a day end past its neighbours and keeps a day of 1 km beside them', () => {
        const trip = setSplit({ ...base(), days: 4 }, 2, .7);
        const squeezedAfter = planView(setSplit(trip, 3, .1)).days;
        expect(squeezedAfter[1].to).toBe(.7);
        expect(squeezedAfter[2].distance).toBeCloseTo(1, 6);
        const squeezedBefore = planView(setSplit(trip, 1, .95)).days;
        expect(squeezedBefore[1].to).toBe(.7);
        expect(squeezedBefore[1].distance).toBeCloseTo(1, 6);
    });

    it('drops a dragged split that a pinned night leaves no day for', () => {
        const trip = setSplit({ ...base(), days: 4 }, 2, .7);
        const days = planView(routed(pinNight(trip, 1, along(.8), 'Late camp'))).days;
        expect(days.map(d => d.to)).toEqual([expect.closeTo(.8, 4), expect.closeTo(.8 + .2 / 3, 4), expect.closeTo(.8 + .4 / 3, 4), 1]);
        expect(days.every(d => d.distance > 1)).toBe(true);
        expect(days[1].split).toBe(false);
    });

    it('clears a split when its night is pinned or the day count changes', () => {
        const trip = setSplit(setSplit({ ...base(), days: 4 }, 1, .1), 2, .7);
        const pinned = pinNight(trip, 2, along(.6), 'Inn');
        expect(pinned.splits).toEqual({ 1: .1 });
        expect(planView(routed(pinned)).days[1].split).toBe(false);
        expect(applyBudget(trip, 'days', 4, 50).splits).toEqual(trip.splits);
        expect(applyBudget(trip, 'days', 5, 50).splits).toBeUndefined();
    });

    it('offers three overnight candidates without water, shortest predicted day first', () => {
        const candidates = overnightCandidates(base(), 1, places);
        expect(candidates).toHaveLength(3);
        expect(candidates.every(c => c.place.category !== 'water')).toBe(true);
        expect(candidates.map(c => c.distance)).toEqual([...candidates.map(c => c.distance)].sort((a, b) => a - b));
    });
});

describe('legs', () => {
    const visit = (fraction: number): RoutePoint => ({ id: 'visit', kind: 'waypoint', label: 'Visit', coordinate: along(fraction) });
    const withVisit = (trip: Trip): Trip => ({ ...trip, points: [...trip.points, visit(.5)], routeOrder: ['visit'] });

    it('inserts a point into one leg and keeps the other points in order', () => {
        const trip = setLegMode(withVisit(testTrip()), 'visit', 'straight');
        const between = along(.25);
        const inserted = insertPoint(trip, 'visit', between);
        const order = orderedRoutePoints(inserted);
        expect(order.map(p => p.id)).toEqual(['start', order[1].id, 'visit', 'finish']);
        expect(order[1]).toMatchObject({ kind: 'via', coordinate: between, leg: 'straight' });
    });

    it('adds a clicked point at the end of a single route and a pinned night in its nearest leg', () => {
        const appended = addClickedPoint(withVisit({ ...base(), mode: 'route' }), { ...visit(.1), id: 'early' });
        expect(orderedRoutePoints(appended).map(p => p.id)).toEqual(['start', 'visit', 'early', 'finish']);
        const pinned = pinNight(routed(appended), 1, along(.3), 'Camp');
        expect(orderedRoutePoints(pinned).map(p => p.id)).toEqual(['start', 'night-1', 'visit', 'early', 'finish']);
    });

    it('adds a point behind the start to the first leg', () => {
        const points = (['start', 'visit', 'finish'] as const).map((id, i): RoutePoint => ({ id, label: id, kind: id === 'visit' ? 'waypoint' : id,
            coordinate: [8 + i * .1, 48] }));
        const trip = routed({ ...emptyTrip(), points, routeOrder: ['visit'] });
        const cafe: RoutePoint = { id: 'cafe', kind: 'waypoint', label: 'Café', coordinate: [7.95, 48] };
        expect(orderedRoutePoints(addPointNear(trip, cafe)).map(p => p.id)).toEqual(['start', 'cafe', 'visit', 'finish']);
    });

    it('keeps a visit added before a pinned night inside that night\'s day', () => {
        const pinned = routed(pinNight(base(), 1, along(.4), 'Camp'));
        const nightKm = (trip: Trip) => planView(trip).stops.find(stop => stop.point.id === 'night-1')!.distance;
        const dayEndKm = (trip: Trip) => planView(trip).days[0].to * planView(trip).total;
        expect(dayEndKm(pinned)).toBeCloseTo(nightKm(pinned), 9);
        const added = routed(addClickedPoint(pinned, visit(.2)));
        expect(orderedRoutePoints(added).map(p => p.id)).toEqual(['start', 'visit', 'night-1', 'finish']);
        expect(nightKm(added)).toBeCloseTo(nightKm(pinned), 6);
        expect(dayEndKm(added)).toBeCloseTo(nightKm(added), 9);
        const [first] = dayStops(added, planView(added).days[0]);
        expect(first.point.id).toBe('visit');
        expect(first.km).toBeCloseTo(planView(added).stops[1].distance, 9);
        expect(dayStops(added, planView(added).days[1])).toEqual([]);
    });

    it('splits a drawn leg where a pinned night joins it and keeps every drawn vertex', () => {
        const drawn = setDrawnLeg(testTrip(), 'finish', [[7.2, 47.7], [6.8, 47.7], [6.4, 47.5]]);
        const pinned = pinNight(drawn, 1, [6.8, 47.69], 'Camp');
        expect(orderedRoutePoints(pinned).map(p => p.drawn)).toEqual([undefined, [[7.2, 47.7], [6.8, 47.7]], [[6.4, 47.5]]]);
    });

    it('adds a point on a drawn line without changing the line, and a drag routes only the legs beside it', () => {
        const sketch: Coordinate[] = [[7.4, 47.6], [7.0, 47.6], [6.6, 47.4]];
        const drawn = setDrawnLeg(testTrip(), 'finish', sketch);
        const before = planView(routed(drawn)).coordinates;
        const on: Coordinate = [7.2, 47.6];
        const split = insertPoint(drawn, 'finish', on);
        expect(planView(routed(split)).coordinates).toEqual([...before.slice(0, 2), on, ...before.slice(2)]);
        const added = orderedRoutePoints(split)[1];
        const again = insertPoint(split, 'finish', [6.8, 47.5]);
        const middle = orderedRoutePoints(again)[2];
        const moved = routeLegsAround(again, middle.id);
        expect(orderedRoutePoints(moved).map(point => point.leg)).toEqual([undefined, 'drawn', undefined, undefined]);
        expect(moved.points.find(point => point.id === added.id)!.drawn).toEqual([sketch[0]]);
        expect(orderedRoutePoints(dragPointOut(drawn, 'finish', [7.2, 47.65])).map(point => point.leg)).toEqual([undefined, undefined, undefined]);
    });

    it('puts a marker that becomes a route point in its nearest leg, and takes a route point that becomes a marker out of the route', () => {
        // A kept line: one drawn leg holds the whole line.
        const sketch: Coordinate[] = [[7.7, 47.4], [7.5, 47.1]];
        const marker: RoutePoint = { id: 'spring', kind: 'marker', label: 'Spring', coordinate: [7.6, 47.25] };
        const kept = setDrawnLeg(testTrip(), 'finish', sketch);
        const visit = replacePoint(routed({ ...kept, points: [...kept.points, marker] }), 'spring', { ...marker, kind: 'waypoint' });
        expect(orderedRoutePoints(visit).map(p => [p.id, p.drawn])).toEqual([['start', undefined], ['spring', [sketch[0]]], ['finish', [sketch[1]]]]);
        expect(planView(routed(visit)).total).toBeCloseTo(planView(routed(kept)).total, 1);
        const back = replacePoint(visit, 'spring', { ...marker, drawn: undefined });
        expect([back.routeOrder, orderedRoutePoints(back).map(p => p.id)]).toEqual([[], ['start', 'finish']]);
    });

    it('gives a point added on a drawn line with elevations the elevation of the line there', () => {
        const drawn = setDrawnLeg(testTrip(), 'finish', [[7.4, 47.6, 300], [7.0, 47.6, 500]]);
        const split = insertPoint(drawn, 'finish', [7.3, 47.6]);
        const [, added, finish] = orderedRoutePoints(split);
        expect(added.drawn).toEqual([[7.4, 47.6, 300], [7.3, 47.6, expect.closeTo(350, 0)]]);
        expect(finish.drawn).toEqual([[7.3, 47.6, expect.closeTo(350, 0)], [7.0, 47.6, 500]]);
    });
});

describe('route endpoints', () => {
    it.each(['route', 'trip'] as const)('builds a %s from either endpoint and promotes points in route order', mode => {
        const empty = emptyTrip(mode);
        expect(planView(empty).coordinates).toEqual([]);
        expect(planView(empty).days).toEqual([]);
        const finish = setEndpoint(empty, 'finish', [8, 48], 'Destination');
        expect(planView(finish).coordinates).toEqual([]);
        expect(planView(finish).days).toEqual([]);
        const complete = setEndpoint(finish, 'start', [7.8, 48], 'Origin');
        const startId = complete.points.find(p => p.kind === 'start')!.id;
        const finishId = finish.points[0].id;
        const a: RoutePoint = { id: 'a', kind: 'waypoint', label: 'A', coordinate: [7.9, 48] };
        const b: RoutePoint = { ...a, id: 'b', label: 'B' };
        const trip = { ...complete, points: [...complete.points, a, b, { ...a, id: 'marker', kind: 'marker' as const }], routeOrder: ['a', 'b'] };
        const shorter = removeRoutePoint(trip, startId);
        expect(orderedRoutePoints(shorter).map(p => [p.id, p.kind])).toEqual([['a', 'start'], ['b', 'waypoint'], [finishId, 'finish']]);
        const both = removeRoutePoint(shorter, finishId);
        expect(orderedRoutePoints(both).map(p => [p.id, p.kind])).toEqual([['a', 'start'], ['b', 'finish']]);
        const one = removeRoutePoint(both, 'a');
        expect(one.points.find(p => p.id === 'b')?.kind).toBe('finish');
        expect(planView(one).coordinates).toEqual([]);
        expect(planView(one).days).toEqual([]);
        const cleared = removeRoutePoint(one, 'b');
        expect(orderedRoutePoints(cleared)).toEqual([]);
        expect(cleared.points.map(p => p.id)).toEqual(['marker']);
        expect(planView(cleared).stops).toEqual([]);
    });

    it('promotes a shaping point to a named endpoint when the start or finish goes', () => {
        const point = (id: string, kind: RoutePoint['kind'], lon: number): RoutePoint => ({ id, kind, label: kind === 'via' ? 'Shaping point' : id, coordinate: [lon, 48] });
        const trip: Trip = { ...emptyTrip(), routeOrder: ['v1', 'v2'], points: [point('start', 'start', 8), point('v1', 'via', 8.2), point('v2', 'via', 8.4), point('finish', 'finish', 9)] };
        const noStart = removeRoutePoint(trip, 'start', ([lon]) => lon === 8.2 ? 'Hinterzarten' : undefined);
        expect(orderedRoutePoints(noStart)[0]).toMatchObject({ id: 'v1', kind: 'start', label: 'Hinterzarten' });
        const noFinish = removeRoutePoint(trip, 'finish');
        expect(orderedRoutePoints(noFinish).at(-1)).toMatchObject({ id: 'v2', kind: 'finish', label: 'Finish' });
    });

    it('clears overnight and detour semantics when promoting endpoints, and replaces an endpoint in place', () => {
        const trip = pinNight(testTrip(), 1, along(.2), 'Camp');
        const next = removeRoutePoint(trip, 'start');
        const start = next.points.find(p => p.kind === 'start')!;
        expect(start.label).toBe('Camp');
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
        const original = setLegMode({ ...pinNight(testTrip(), 1, along(.2), 'Camp'), mode: 'trip', restAfter: [1], splits: { 2: .8 } }, 'finish', 'straight');
        const next = setEndpoint(original, 'finish', [5.9, 47.2], 'New destination');
        const order = orderedRoutePoints(next);
        expect(order.slice(-2)).toEqual([
            expect.objectContaining({ id: 'finish', kind: 'waypoint', label: 'Thun', leg: 'straight' }),
            expect.objectContaining({ kind: 'finish', label: 'New destination' }),
        ]);
        expect(next).toMatchObject({ mode: 'trip', days: original.days, target: original.target, restAfter: [1], splits: { 2: .8 } });
        expect(next.points.find(p => p.kind === 'night')).toEqual(original.points.find(p => p.kind === 'night'));
    });

    it('gives a pinned overnight its explicit name when it comes from an automatically named visit', () => {
        const visit: RoutePoint = { id: 'visit', kind: 'waypoint', autoLabel: true, label: '48.00000, 7.84000', coordinate: [7.84, 48] };
        const trip = addPointNear(testTrip(), visit);
        const pinned = pinNight(trip, 1, visit.coordinate, visit.label, visit.id);
        expect(pinned.points.find(p => p.night === 1)?.autoLabel).toBeUndefined();
        const replaced = pinNight(pinned, 1, [8.088, 48.279], 'Fuxxbau');
        expect(replaced.points.find(p => p.night === 1)).toMatchObject({ label: 'Fuxxbau', coordinate: [8.088, 48.279] });
        expect(replaced.points.find(p => p.night === 1)?.autoLabel).toBeUndefined();
    });
});

describe('loops', () => {
    // Start, two visits, a shaping point and a finish, joined by straight legs.
    function plan(): Trip {
        const points = ['start', 'a', 'b', 'shape', 'finish'].map((id, i): RoutePoint => ({ id, label: id === 'start' ? 'Home' : id,
            coordinate: [7.8 + i * .01, 48 + (i % 2) * .01], leg: i ? 'straight' : undefined,
            kind: id === 'start' ? 'start' : id === 'finish' ? 'finish' : id === 'shape' ? 'via' : 'waypoint' }));
        return { ...emptyTrip(), points, routeOrder: ['a', 'b', 'shape'] };
    }
    const ids = (trip: Trip) => orderedRoutePoints(trip).map(p => p.id);
    const total = (trip: Trip) => planView(routed(trip)).total;
    const at = (trip: Trip, id: string) => trip.points.find(p => p.id === id)!.coordinate;

    it('closes into a loop whose figures include the leg back to the start, and undo restores the open plan', () => {
        const open = plan();
        const history = new TripHistory();
        const loop = history.commit(open, setLegMode(closeLoop(open), 'start', 'straight'));
        expect(ids(loop)).toEqual(['start', 'a', 'b', 'shape', 'finish', 'start']);
        expect(loop.points.filter(p => p.kind === 'start' || p.kind === 'finish').map(p => p.id)).toEqual(['start']);
        expect(total(loop)).toBeCloseTo(total(open) + kilometres(at(open, 'finish'), at(open, 'start')));
        expect(isTrip(storedPlan(loop))).toBe(true);
        expect(history.undo(loop)).toEqual(open);
    });

    it('stays a loop through adding, inserting and reordering points', () => {
        const loop = closeLoop(plan());
        const visit: RoutePoint = { id: 'c', kind: 'waypoint', label: 'c', coordinate: [7.9, 48.02] };
        const added = addClickedPoint(loop, visit);
        expect(ids(added)).toEqual(['start', 'a', 'b', 'shape', 'finish', 'c', 'start']);
        const shaped = insertPoint(loop, 'start', [7.85, 48.03]);
        const back = ids(shaped).at(-2)!;
        expect(ids(shaped)).toEqual(['start', 'a', 'b', 'shape', 'finish', back, 'start']);
        // The shape of the leg back to the start stays on that leg.
        const reordered = reorderPoint(shaped, 'a', 1);
        expect(ids(reordered)).toEqual(['start', 'b', 'a', 'finish', back, 'start']);
        expect(ids(reorderPoint(shaped, 'finish', -2))).toEqual(['start', 'finish', 'a', 'b', 'start']);
        const removed = removeRoutePoint(reordered, 'a');
        expect(ids(removed)).toEqual(['start', 'b', 'finish', back, 'start']);
        expect([reordered, removed].map(trip => isTrip(storedPlan(trip)))).toEqual([true, true]);
    });

    it('moves the start and keeps the order around the loop and every leg', () => {
        const loop = setLegMode(closeLoop(plan()), 'start', 'straight');
        const moved = startLoopHere(loop, 'b', [7.815, 48.005]);
        const start = moved.points.find(p => p.kind === 'start')!;
        expect(ids(moved)).toEqual([start.id, 'b', 'shape', 'finish', 'start', 'a', start.id]);
        // A named start becomes a visit, and its leg is still the one that ends there.
        expect(moved.points.find(p => p.id === 'start')).toMatchObject({ kind: 'waypoint', label: 'Home', leg: 'straight' });
        expect(start.leg).toBe('straight');
        expect(total(moved)).toBeCloseTo(total(loop) + kilometres(at(loop, 'a'), start.coordinate)
            + kilometres(start.coordinate, at(loop, 'b')) - kilometres(at(loop, 'a'), at(loop, 'b')));
        // An unnamed start stays on the line as a shaping point.
        const again = startLoopHere(moved, 'finish', [7.83, 48.02]);
        const next = again.points.find(p => p.kind === 'start')!.id;
        expect(ids(again)).toEqual([next, 'finish', 'start', 'a', start.id, 'b', 'shape', next]);
        expect(again.points.find(p => p.id === start.id)?.kind).toBe('via');
        expect(startLoopHere(plan(), 'a', [7.81, 48])).toEqual(plan());
        // Day ends follow the route order, so a pinned night keeps the start.
        const nights = pinNight({ ...loop, mode: 'trip' }, 1, at(loop, 'b'), 'Camp', 'b');
        expect(startLoopHere(nights, 'shape', [7.835, 48.01])).toBe(nights);
    });

    it('opens with a new finish, and when its last point goes', () => {
        const open = setEndpoint(setLegMode(closeLoop(plan()), 'start', 'straight'), 'finish', [8, 48], 'Away');
        expect(open.loop).toBeUndefined();
        expect(orderedRoutePoints(open).map(p => p.kind)).toEqual(['start', 'waypoint', 'waypoint', 'via', 'waypoint', 'finish']);
        expect(open.points.find(p => p.id === 'start')?.leg).toBeUndefined();
        const made = loopTrip(emptyTrip(), [[7.8, 48], [7.9, 48]]);
        expect(orderedRoutePoints(made).map(p => p.kind)).toEqual(['start', 'via', 'start']);
        const shaped = setLegMode(made, made.points[0].id, 'straight');
        expect(removeRoutePoint(shaped, made.points[1].id)).toMatchObject({ loop: undefined, points: [{ kind: 'start', leg: undefined }] });
        expect(loopTrip(emptyTrip(), [[7.8, 48]])).toEqual(emptyTrip());
    });
});
