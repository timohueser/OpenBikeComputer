import { describe, expect, it } from 'vitest';
import { addRestDay, coordinateAt, cumulative, initialTrip, pinNight, routeCoordinates } from './editor';
import { deleteVersion, keepVersion, listVersions, readVersion, saveVersion, versionSummary } from './versions';

function memoryStore() {
    const items = new Map<string, string>();
    return { getItem: (key: string) => items.get(key) ?? null, setItem: (key: string, value: string) => void items.set(key, value) };
}

describe('saved versions', () => {
    it('keeps the ten newest unnamed versions and every named one', () => {
        const store = memoryStore();
        const named = saveVersion(initialTrip(), 'Before the pass', store);
        const unnamed = Array.from({ length: 12 }, () => saveVersion(initialTrip(), undefined, store));
        const kept = listVersions(store);
        expect(kept.filter(v => !v.name).map(v => v.id)).toEqual(unnamed.slice(2).reverse().map(v => v.id));
        expect(kept.at(-1)).toMatchObject({ id: named.id, name: 'Before the pass' });
        expect(readVersion(named.id, store)).toEqual(initialTrip());
        deleteVersion(named.id, store);
        expect(listVersions(store)).toHaveLength(10);
    });

    it('keeps a plan once when the newest version holds it already', () => {
        const store = memoryStore();
        keepVersion(initialTrip(), store);
        keepVersion(initialTrip(), store);
        expect(listVersions(store)).toHaveLength(1);
        keepVersion({ ...initialTrip(), mode: 'route' }, store);
        expect(listVersions(store)).toHaveLength(2);
    });

    it('summarises calendar days, pinned nights and distance', () => {
        const initial = initialTrip();
        const trip = addRestDay(pinNight(initial, 1, coordinateAt(routeCoordinates(initial), .4), 'Camp'), 1);
        const distance = cumulative(routeCoordinates(trip)).at(-1)!.toFixed(1);
        expect(versionSummary(trip)).toBe(`4 days · 1 night pinned · ${distance} km`);
        expect(versionSummary({ ...initial, mode: 'route' })).toBe(`Single route · ${cumulative(routeCoordinates(initial)).at(-1)!.toFixed(1)} km`);
    });
});
