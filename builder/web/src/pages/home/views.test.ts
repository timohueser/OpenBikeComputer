import { describe, expect, it } from 'vitest';
import { readManifest, viewAt, type HomeView, type Manifest } from './views';

const view = (id: string, night = true): HomeView => ({ id, name: id, region: 'Region', lat: 46.8, lon: 8.4, sunrise: '07:00', sunset: '17:00', night });
const manifest = (views: HomeView[]): Manifest => ({ format: 1, set: '0123456789abcdef', credits: ['© Data'], views });
const set = manifest([view('a'), view('b'), view('c', false)]);

describe('viewAt', () => {
    it('takes the view of the local day, the next one on the next day, around the list', () => {
        expect(viewAt(set, new Date(1970, 0, 1, 12)).view.id).toBe('a');
        expect(viewAt(set, new Date(1970, 0, 1, 23, 59)).view.id).toBe('a');
        expect(viewAt(set, new Date(1970, 0, 2, 0, 1)).view.id).toBe('b');
        expect(viewAt(set, new Date(1970, 0, 4, 12)).view.id).toBe('a');
        expect(viewAt(set, new Date(2026, 10, 13, 12)).view.id).toBe('b');
    });

    it('takes the slot by sunrise, the middle of the morning and of the afternoon, and sunset', () => {
        const slot = (hours: number, minutes: number) => viewAt(set, new Date(1970, 0, 1, hours, minutes)).slot;
        expect([slot(6, 59), slot(7, 0), slot(9, 29), slot(9, 30), slot(14, 29), slot(14, 30), slot(16, 59), slot(17, 0)])
            .toEqual(['night', 'morning', 'morning', 'noon', 'noon', 'evening', 'evening', 'night']);
    });

    it('shows the evening at night when the view has no night image', () => {
        expect(viewAt(set, new Date(1970, 0, 3, 23))).toMatchObject({ view: { id: 'c' }, slot: 'evening' });
        expect(viewAt(set, new Date(1970, 0, 3, 5))).toMatchObject({ view: { id: 'c' }, slot: 'evening' });
    });
});

describe('readManifest', () => {
    it('reads a manifest of format 1 and refuses another format or a broken view', () => {
        expect(readManifest(JSON.parse(JSON.stringify(set)))).toEqual(set);
        expect(readManifest({ ...set, format: 2 })).toBeUndefined();
        expect(readManifest({ ...set, views: [] })).toBeUndefined();
        expect(readManifest({ ...set, views: [{ ...view('a'), sunrise: '7:00' }] })).toBeUndefined();
    });
});
