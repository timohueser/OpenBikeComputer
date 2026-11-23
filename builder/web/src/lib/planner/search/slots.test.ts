import { describe, expect, it } from 'vitest';
import { fill, oneKind, requestOf, slotsOf, slotWords } from './slots';

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
        // A change of the plan fills no slot.
        expect(slotsOf({ type: 'route', to: { name: 'Konstanz' } }).slots).toEqual({});
    });

    it('says the values of each slot in words', () => {
        expect(slotWords(slotsOf({ type: 'places', what: ['drinking_water'], where: { day: 1, part: 'middle' }, open: { weekday: 'sun' } }).slots))
            .toEqual({ find: ['Drinking water'], where: ['Middle of Day 1'], when: ['Open on Sunday'] });
        expect(slotWords(slotsOf({ type: 'stretches', what: 'gap:water', where: { scope: 'route', part: 'end' }, min: { value: 20, unit: 'km' } }).slots))
            .toEqual({ find: ['Longest stretch without water', 'At least 20 km'], where: ['End of the route'], when: [] });
    });
});
