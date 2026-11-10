import { describe, expect, it } from 'vitest';
import type { RoutePoint } from './editor';
import { routeNotes } from './route-notes';

describe('route notes', () => {
    it('merges the three kinds of note in route order', () => {
        const point: RoutePoint = { id: 'f', kind: 'finish', coordinate: [0.02, 0.002], label: 'f' };
        const notes = routeNotes(
            [{ point, coordinates: [[0.02, 0], [0.02, 0.002]], km: 0.22, at: 9 }],
            [{ closures: [{ kind: 'limited', condition: 'customers' }], coordinates: [], km: 0.3, at: 5 }],
            [{ coordinates: [], km: 2, at: 1 }, { coordinates: [], km: 1, at: 7 }],
        );
        expect(notes.map(note => [note.icon, note.km, note.length])).toEqual([['mountain', 1, 2], ['lock', 5, 0.3], ['mountain', 7, 1], ['flag', 9, undefined]]);
        expect(notes[1].label).toBe('Customers only');
    });
});
