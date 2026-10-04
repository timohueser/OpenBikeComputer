import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import type { CatalogRecord } from './signed-routes';

const host = vi.hoisted(() => {
    Object.assign(globalThis, { window: { location: { href: 'http://localhost/' } } });
    return { bounds: undefined as [number, number, number, number] | undefined, load: (_id: string): Promise<CatalogRecord[] | null> => Promise.resolve(null) };
});
vi.mock('./client-config', () => ({ clientConfig: { get bounds() { return host.bounds; } } }));
vi.mock('./map-data', async importActual => ({
    ...await importActual<typeof import('./map-data')>(),
    get MAP_BOUNDS() { return host.bounds; },
    loadRouteCell: (id: string) => host.load(id),
}));
const { RouteFinder } = await import('./route-finder.svelte');

const vector: { grid: string[]; routes: CatalogRecord[] } = JSON.parse(readFileSync(new URL('../../../../../specs/vectors/signed-routes.json', import.meta.url), 'utf8'));
const cell = async (id: string) => vector.grid.includes(id) ? vector.routes.filter(route => route.cells.includes(id)) : null;
const finder = (filters: Partial<InstanceType<typeof RouteFinder>['filters']> = {}) => {
    const found = new RouteFinder();
    found.start = { coordinate: [8.15, 47.87], name: 'Titisee' };
    Object.assign(found.filters, filters);
    return found;
};
const ids = (found: InstanceType<typeof RouteFinder>) => found.matches.map(match => match.route.id);

describe('route finder', () => {
    it('names the next radius with matches when none is near', async () => {
        host.load = cell;
        const found = finder({ radiusKm: 10, shape: 'loop' });
        await found.search('touring');
        expect(found.matches).toEqual([]);
        expect(found.wider).toEqual({ radiusKm: 25, count: 1 });
    });

    it('names the filter that removes every match near the start', async () => {
        host.load = cell;
        const found = finder({ radiusKm: 25, shape: 'loop', distanceKm: { from: 200 } });
        await found.search('hiking');
        expect(found.matches).toEqual([]);
        expect(found.blocker).toBe('distanceKm');
        found.clear('distanceKm');
        await found.search('hiking');
        expect(found.matches.length).toBeGreaterThan(0);
    });

    it('lists offline only the routes wholly inside the downloaded cells', async () => {
        // The box of cell 9-267-178: the download.
        const latitude = (row: number) => Math.atan(Math.sinh(Math.PI * (1 - 2 * row / 512))) * 180 / Math.PI;
        host.bounds = [267 / 512 * 360 - 180, latitude(179), 268 / 512 * 360 - 180, latitude(178)];
        const asked: string[] = [];
        host.load = id => { asked.push(id); return cell(id); };
        const found = finder({ radiusKm: 50, shape: 'any', hardest: [0, 3] });
        host.bounds = undefined;
        await found.search('hiking');
        expect(asked).toEqual(['9-267-178']);
        expect(ids(found)).toContain(101);
        expect(found.matches.every(({ route }) => route.cells.every(id => id === '9-267-178'))).toBe(true);
    });

    it('drops the answer of a search that a newer search replaced', async () => {
        const pending: (() => void)[] = [];
        host.load = id => new Promise(resolve => pending.push(() => resolve(cell(id))));
        const found = finder({ radiusKm: 5, shape: 'any', hardest: [0, 3] });
        const slow = found.search('hiking');
        // The newer start loads other cells, which answer at once.
        host.load = cell;
        found.start = { coordinate: [8.6, 47.89], name: 'Lenzkirch' };
        await found.search('hiking');
        const newer = ids(found);
        pending.forEach(release => release());
        await slow;
        expect(newer).not.toContain(101);
        expect(ids(found)).toEqual(newer);
        expect(found.status).toBe('ready');
    });
});
