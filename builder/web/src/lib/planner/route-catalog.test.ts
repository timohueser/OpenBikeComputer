import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import type { BikeType } from './riding-profiles';
import type { CatalogRecord, FactsFile, TrailEntry } from './signed-routes';

const host = vi.hoisted(() => {
    Object.assign(globalThis, { window: { location: { href: 'http://localhost/' }, addEventListener: () => {} } });
    return {
        load: (_id: string): Promise<CatalogRecord[] | null> => Promise.resolve(null),
        far: (_id: string): Promise<TrailEntry[] | null> => Promise.resolve(null),
        facts: (_id: string): Promise<FactsFile | null> => Promise.resolve(null),
    };
});
vi.mock('./map-data', async importActual => ({
    ...await importActual<typeof import('./map-data')>(),
    loadRouteCell: (id: string) => host.load(id),
    loadTrailCell: (id: string) => host.far(id),
    loadFactsCell: (id: string) => host.facts(id),
}));
const { config } = await import('./map-data');
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
        const catalog = new RouteCatalog();
        // Before the cells load, a box with cells has no answer yet.
        if (boxCells(box).length && boxCells(box).length <= MAX_CELLS) expect(catalog.inBoxNow(box, bike)).toBeUndefined();
        const found = await catalog.inBox(box, bike);
        expect(found?.map(route => route.id)).toEqual(ids);
        expect(catalog.inBoxNow(box, bike)).toEqual(found);
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

    it('joins the pieces of a trail from the far files of a box, and loads none for a box of more than the most cells', async () => {
        config.trails = 'https://tiles.test/routes/tiles/trails/{cell}.json';
        const bounds = config.bounds;
        config.bounds = [5, 45, 10, 48];
        asked.length = 0;
        const entry = (id: number, far_udeg: number[][]): TrailEntry => ({ id, kind: 'hiking', rank: 2, loop: false, length_m: 9e4, ascent_m: 900, far_udeg });
        // 8.4375° E is the edge between the zoom 7 columns 66 and 67.
        const west = [8000000, 46500000, 437500, 0], east = [8437500, 46500000, 500000, 100000];
        const files: Record<string, TrailEntry[]> = { '7-66-45': [entry(1, [west]), entry(2, [[6000000, 46500000, 10000, 0]])], '7-67-45': [entry(1, [east])] };
        host.far = async id => { asked.push(id); return files[id] ?? null; };
        const box: Box = [7.5, 46, 9.5, 46.9];
        expect(boxCells(box, 7)).toEqual(['7-66-45', '7-67-45']);
        // Trail 2 is in a far file of the box, but not in the box.
        const catalog = new RouteCatalog();
        expect(catalog.trailsInBoxNow(box)).toBeUndefined();
        const found = await catalog.trailsInBox(box);
        expect(found).toEqual([entry(1, [west, east])]);
        // With its files loaded, a box has its answer with no wait, and the joined trail keeps its object.
        expect(catalog.trailsInBoxNow(box)![0]).toBe(found![0]);
        // A segment of trail 3 cuts the south-west corner of the box, with no vertex in it.
        const corner = [8200000, 46050000, 120000, -100000];
        files['7-66-45'].push(entry(3, [corner]));
        expect((await new RouteCatalog().trailsInBox([8.25, 46, 9.5, 46.9]))?.map(trail => trail.id)).toEqual([1, 3]);
        asked.length = 0;
        expect(await new RouteCatalog().trailsInBox([5, 43, 20, 47])).toBeNull();
        delete config.trails;
        config.bounds = bounds;
        expect(await new RouteCatalog().trailsInBox(box)).toBeNull();
        expect(asked).toEqual([]);
    });

    it('loads a failed cell again at the next call', async () => {
        const catalog = new RouteCatalog();
        host.load = () => Promise.reject(new Error('offline'));
        await expect(catalog.inBox([8.175, 47.86, 8.19, 47.88], 'hiking')).rejects.toThrow('offline');
        host.load = cell;
        expect((await catalog.inBox([8.175, 47.86, 8.19, 47.88], 'hiking'))?.map(route => route.id)).toEqual([101]);
    });

    it('loads the facts file with each cell, and a failed facts file fails the cell', async () => {
        host.load = cell;
        const box: Box = [8.175, 47.86, 8.19, 47.88], [id] = boxCells(box);
        const catalog = new RouteCatalog();
        host.facts = () => Promise.reject(new Error('Route facts request failed with status 500'));
        await expect(catalog.inBox(box, 'hiking')).rejects.toThrow('500');
        expect(catalog.inBoxNow(box, 'hiking')).toBeUndefined();
        const peak = { kind: 'peak' as const, at_udeg: [8180000, 47870000] as [number, number] };
        host.facts = async cell => cell === id ? { format: 1, places: { n5: peak }, routes: { 101: { places: [['n5', 20]], paved_pct: 0, big_road_pct: 0, water_pct: 0 } } } : null;
        expect((await catalog.inBox(box, 'hiking'))?.map(route => route.id)).toEqual([101]);
        expect([catalog.factsOf(101)?.places, catalog.place('n5')]).toEqual([[['n5', 20]], peak]);
        host.facts = () => Promise.resolve(null);
    });

    it('lists the records of a box from the cells of another box', async () => {
        host.load = cell;
        asked.length = 0;
        // The view meets more than the most cells; the records of the one cell of `from` with a vertex in the view list.
        const view: Box = [7.1, 47.8, 15.6, 47.9], from: Box = [8.175, 47.86, 8.19, 47.88], [id] = boxCells(from);
        const listed = (await new RouteCatalog().inBox(view, 'hiking', from))!;
        expect(listed.length).toBeGreaterThan((await new RouteCatalog().inBox(from, 'hiking'))!.length);
        expect(listed.every(route => route.cells.includes(id))).toBe(true);
        expect(new Set(asked)).toEqual(new Set([id]));
    });
});
