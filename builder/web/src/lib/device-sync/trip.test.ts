import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { emptyTrip, type RoutePoint } from '../planner/editor';
import { partName, tripKey, tripObject } from './trip';

const vector = (name: string) => readFileSync(new URL(`../../../../../specs/vectors/${name}`, import.meta.url));

describe('tripObject', () => {
    it('writes the bytes of the trip vector', () => {
        const { days } = JSON.parse(vector('manifest.json').toString()).fixtures['trip-v3.bin'] as { days: { route: number; join_m: number; leave_m: number }[] };
        const bytes = tripObject(0x0123_4567_89ab_cdefn, 'Alpen Traverse', 20360, days.map(day => ({ route: BigInt(day.route), joinM: day.join_m, leaveM: day.leave_m })));
        expect(bytes).toEqual(new Uint8Array(vector('trip-v3.bin')));
    });
});

describe('tripKey', () => {
    it('is FNV-1a 64 of the plan id, the ids of the first two route points and the part', () => {
        const point = (id: string, kind: RoutePoint['kind']): RoutePoint => ({ id, kind, label: id, coordinate: [8, 47] });
        const trip = { ...emptyTrip('trip'), points: [point('f', 'finish'), point('s', 'start')] };
        // FNV-1a 64 of "p|s|f|1", computed outside this code: part 1 has the key that one trip object had.
        expect(tripKey('p', trip, 1)).toBe(0xd084_bc7b_5f21_2befn);
        expect(tripKey('p', trip, 2)).not.toBe(tripKey('p', trip, 1));
    });
});

describe('partName', () => {
    it('cuts a long plan name, never the part number', () => {
        expect([partName('Alps', 1, 1), partName('Alps', 2, 2)]).toEqual(['Alps', 'Alps 2/2']);
        // The name has 47 bytes; with " 2/2" it is cut on a character boundary.
        expect(partName('Grande Traversée des Alpes et du Jura à vélo', 2, 2)).toBe('Grande Traversée des Alpes et du Jura à v 2/2');
    });
});
