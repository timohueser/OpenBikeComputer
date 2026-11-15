import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import type { BikeType } from './riding-profiles';
import type { CatalogRecord } from './signed-routes';

const host = vi.hoisted(() => {
    Object.assign(globalThis, { window: { location: { href: 'http://localhost/' }, addEventListener: () => {} } });
    return { load: (_id: string): Promise<CatalogRecord[] | null> => Promise.resolve(null) };
});
vi.mock('./map-data', async importActual => ({
    ...await importActual<typeof import('./map-data')>(),
    loadRouteCell: (id: string) => host.load(id),
}));
const { RouteCatalog, boxCells, MAX_CELLS } = await import('./route-catalog');
type Box = import('./route-catalog').Box;

const read = (name: string) => JSON.parse(readFileSync(new URL(`../../../../../specs/vectors/${name}`, import.meta.url), 'utf8'));
const vector: { grid: string[]; routes: CatalogRecord[] } = read('signed-routes.json');
const boxes: { boxes: { name: string; box: Box; bike: BikeType; ids: number[] }[] } = read('route-boxes.json');
const asked: string[] = [];
const cell = async (id: string) => {
    asked.push(id);
    return vector.grid.includes(id) ? vector.routes.filter(route => route.cells.includes(id)) : null;
};

describe('route catalog', () => {
    it('lists the cells that a box meets, also at an edge', () => {
        // 7.734375° E is the edge between the columns 266 and 267.
        expect(boxCells([7.734375, 47.8, 7.8, 47.9])).toEqual(['9-266-178', '9-267-178']);
        expect(boxCells([8.1, 47.8, 8.2, 47.9])).toEqual(['9-267-178']);
        expect(boxCells([8.2, 47.8, 8.1, 47.9])).toEqual([]);
    });

    it.each(boxes.boxes)('$name', async ({ box, bike, ids }) => {
        host.load = cell;
        const found = await new RouteCatalog().inBox(box, bike);
        expect(found?.map(route => route.id)).toEqual(ids);
    });

    it('loads nothing for a box of more than the most cells', async () => {
        host.load = cell;
        asked.length = 0;
        const box: Box = [7.1, 47.8, 15.6, 47.9];
        expect(boxCells(box)).toHaveLength(MAX_CELLS + 1);
        expect(await new RouteCatalog().inBox(box, 'hiking')).toBeNull();
        expect(await new RouteCatalog().inBox([-180, -85, 180, 85], 'hiking')).toBeNull();
        expect(asked).toEqual([]);
    });

    it('gives the stages of a long route in order', async () => {
        host.load = cell;
        const catalog = new RouteCatalog();
        const westweg = vector.routes.find(route => route.id === 401)!;
        expect((await catalog.stagesOf(westweg)).map(stage => stage.id)).toEqual([402, 403]);
    });

    it('loads a failed cell again at the next call', async () => {
        const catalog = new RouteCatalog();
        host.load = () => Promise.reject(new Error('offline'));
        await expect(catalog.inBox([8.175, 47.86, 8.19, 47.88], 'hiking')).rejects.toThrow('offline');
        host.load = cell;
        expect((await catalog.inBox([8.175, 47.86, 8.19, 47.88], 'hiking'))?.map(route => route.id)).toEqual([101]);
    });
});
