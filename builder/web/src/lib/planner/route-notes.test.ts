import { describe, expect, it } from 'vitest';
import type { RoutePoint } from './editor';
import type { Coordinate } from './geo';
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

    it('joins a stretch to the last note of its icon and label that ends less than 2 km before it', () => {
        const point: RoutePoint = { id: 'f', kind: 'finish', coordinate: [0.3, 0.002], label: 'f' };
        const gap = (at: number) => ({ point, coordinates: [[0.3, 0], [0.3, 0.002]] as [Coordinate, Coordinate], km: 0.22, at });
        const closed = (kind: 'seasonal' | 'permit', at: number, km: number, coordinates: Coordinate[] = []) =>
            ({ closures: [{ kind, condition: kind === 'seasonal' ? 'nov-may' : '' }], coordinates, km, at });
        const first: Coordinate[] = [[0.1, 0], [0.11, 0]], second: Coordinate[] = [[0.13, 0], [0.135, 0]];
        const notes = routeNotes(
            [gap(20), gap(20.5)],
            [closed('seasonal', 10, 1, first), closed('permit', 11.2, 0.3), closed('seasonal', 12.9, 0.5, second), closed('seasonal', 15.5, 0.4)],
            [{ coordinates: [], km: 0.2, at: 11.5 }],
        );
        expect(notes.map(note => [note.icon, note.km, note.end !== undefined])).toEqual([
            ['lock', 10, true], ['lock', 11.2, false], ['mountain', 11.5, false], ['lock', 15.5, false], ['flag', 20, false], ['flag', 20.5, false],
        ]);
        const [pass] = notes;
        expect(pass.label).toBe('May be closed Nov–May');
        expect(pass.end).toBeCloseTo(13.4);
        expect(pass.length).toBeCloseTo(1.5);
        expect(pass.coordinates).toEqual([first, second]);
    });
});
