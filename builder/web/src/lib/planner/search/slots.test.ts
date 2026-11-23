import { describe, expect, it } from 'vitest';
import { fill, NEAREST, oneKind, partOf, requestOf, slotsOf, slotWords, stretchOf, stretchWhere, withAct } from './slots';
import type { QueryRequest } from './types';

describe('the slots of the planner search', () => {
    it('takes one thing to find: the nearest kind over all kinds, else the first', () => {
        expect(oneKind(['cafe', 'restaurant'])).toEqual({ kind: 'food', left: [] });
        expect(oneKind(['hotel', 'campsite'])).toEqual({ kind: 'sleep', left: [] });
        expect(oneKind(['drinking_water', 'water'])).toEqual({ kind: 'water', left: [] });
        expect(oneKind(['cafe', 'bakery', 'atm'])).toEqual({ kind: 'cafe', left: ['bakery', 'atm'] });
    });

    it('fills each slot that an answer holds a value of, and gives each type of request only the fields that it takes', () => {
        const slots = slotsOf({ type: 'places', what: ['supermarket'], where: { day: 1, part: 'middle' }, radius: { value: 2, unit: 'km' }, open: { now: true } }).slots;
        const named = fill(slots, slotsOf({ type: 'place', name: 'Bahnhof', near: { name: 'Teningen' } }).slots);
        expect(named).toEqual({ find: { name: 'Bahnhof' }, where: { near: [{ name: 'Teningen' }] }, radius: undefined, open: { now: true } });
        expect(requestOf(named)).toEqual({ type: 'place', name: 'Bahnhof', near: { name: 'Teningen' } });
        const climbs = fill(slots, slotsOf({ type: 'stretches', what: 'climb', min: { value: 300, unit: 'm' } }).slots);
        expect(requestOf(climbs)).toEqual({ type: 'stretches', what: 'climb', where: { day: 1, part: 'middle' }, min: { value: 300, unit: 'm' } });
        // A part goes with an area on the route only.
        expect(slotsOf({ type: 'places', what: ['water'], where: { near: [{ name: 'Teningen' }], part: 'end' } }).slots.where).toEqual({ near: [{ name: 'Teningen' }] });
    });

    it('holds the action of a change of the plan on a kind or a name, and lists its places as "Find" does', () => {
        const route = slotsOf({ type: 'route', to: { kind: 'supermarket' } }).slots;
        expect(route).toEqual({ act: 'route', find: { kind: 'supermarket' } });
        expect(slotsOf({ type: 'route', to: { name: 'Konstanz' } }).slots).toEqual({ act: 'route', find: { name: 'Konstanz' } });
        expect(slotsOf({ type: 'add_point', point: { kind: 'water' }, where: { day: 2 } }).slots).toEqual({ act: 'add_point', find: { kind: 'water' }, where: { day: 2 } });
        // A day end is searched at the end of its day.
        const night = slotsOf({ type: 'end_day', day: 2, at: { kind: 'campsite' } }).slots;
        expect(requestOf(night)).toEqual({ type: 'places', what: ['campsite'], where: { day: 2, part: 'end' } });
        // A change that the slots cannot hold fills none: the list shows it as one action row.
        const kept: QueryRequest[] = [{ type: 'route', from: { here: true }, to: { name: 'Konstanz' } }, { type: 'add_point', point: { kind: 'water' }, every: { value: 30, unit: 'km' } },
            { type: 'add_point', point: { name: 'Titisee' }, kind: 'pass' }, { type: 'end_day', day: 1, at: { along: { ref: 'km', at: { value: 40, unit: 'km' } } } },
            { type: 'end_day', day: 'every', at: { kind: 'campsite' } }, { type: 'reverse' }, { type: 'remove_point', point: { name: 'Kandel' } }];
        for (const request of kept) expect(slotsOf(request).slots, request.type).toEqual({});
        // A new thing to find brings its action or none, an area alone keeps the action, and an action leaves out a fact.
        expect(fill(route, slotsOf({ type: 'places', what: ['water'] }).slots).act).toBeUndefined();
        expect(fill(route, { where: { day: 2 } })).toMatchObject({ act: 'route', find: { kind: 'supermarket' } });
        expect(withAct({ find: { fact: 'climb' } }, 'end_day')).toEqual({ act: 'end_day' });
        expect(withAct(route, undefined)).toEqual({ find: { kind: 'supermarket' } });
    });

    it('says the values of each slot in words', () => {
        expect(slotWords(slotsOf({ type: 'places', what: ['drinking_water'], where: { day: 1, part: 'middle' }, open: { weekday: 'sun' } }).slots))
            .toEqual({ find: ['Drinking water'], where: ['Middle of Day 1'], when: ['Open on Sunday'] });
        expect(slotWords(slotsOf({ type: 'stretches', what: 'gap:water', where: { scope: 'route', part: 'end' }, min: { value: 20, unit: 'km' } }).slots))
            .toEqual({ find: ['Longest stretch without water', 'At least 20 km'], where: ['End of the route'], when: [] });
    });

    it('gives an area on the route as its stretch: a part is 20 % of it at an end, at least 10 km, and its middle 40 %', () => {
        expect([partOf([0, 100], 'start'), partOf([0, 100], 'end'), partOf([0, 100], 'middle')]).toEqual([[0, 20], [80, 100], [30, 70]]);
        expect([partOf([20, 40], 'start'), partOf([20, 26], 'end')]).toEqual([[20, 30], [20, 26]]);
        const plan = { km: [0, 60], days: [{ number: 1, from: 0, to: 30, rest: false }, { number: 2, from: 30, to: 30, rest: true }, { number: 3, from: 30, to: 60, rest: false }] };
        expect([stretchOf({ day: 3, part: 'end' }, plan), stretchOf({ scope: 'route' }, plan), stretchOf({ day: 2 }, plan)]).toEqual([[50, 60], [0, 60], undefined]);
        expect([NEAREST, { scope: 'view' }, { near: [{ name: 'Freiburg' }] }].map(where => stretchOf(where, plan))).toEqual([undefined, undefined, undefined]);
        // With no route, the request keeps the area as the slot holds it.
        const slots = { find: { kind: 'water' }, where: { day: 1, part: 'start' } };
        expect(requestOf(slots, plan)?.where).toEqual({ along: { ref: 'km', from: { value: 0, unit: 'km' }, to: { value: 10, unit: 'km' } } });
        expect(requestOf(slots, { km: [], days: [] })?.where).toEqual({ day: 1, part: 'start' });
        expect(slotWords({ where: NEAREST }).where).toEqual(['Nearest to me']);
    });

    it('keeps a stretch within the route, and sends its ends out to a tenth of a km', () => {
        const plan = { km: [0, 25], days: [] }, along = (from: number, to: number) => ({ along: { ref: 'km', from: { value: from, unit: 'km' }, to: { value: to, unit: 'km' } } });
        // The finish moved: a stretch past the end of the route is cut at it, and one that the route does not reach is none.
        expect([stretchOf(along(20, 38), plan), stretchOf(along(30, 38), plan)]).toEqual([[20, 25], undefined]);
        expect(stretchWhere([1.23, 7.43])).toEqual(along(1.2, 7.5));
        expect(stretchWhere([4.9, 6.1])).toEqual(along(4.9, 6.1));
    });
});
