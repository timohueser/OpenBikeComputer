import { describe, expect, it } from 'vitest';
import TABLE from '../../../../../../data/trailview/kinds.json' with { type: 'json' };
import { EXTENT, type Feature } from '../stream/features';
import { OWNER, structureMeshes } from '../structures/node';
import { KINDS } from '.';
import { facing, type Build, type Placement } from './kind';
import { lampKind } from '../structures/lamps';
import { metresPerUnit, type Line } from '../terrain/bins';

describe('kinds', () => {
    it('draws only kinds of the table', () => {
        const names = new Set(TABLE.map(entry => entry.kind));
        expect([...KINDS.keys()].filter(name => !names.has(name))).toEqual([]);
    });

    it('closes a wall at its end in the node that owns the end, also past a tile border', () => {
        // A wall from the west half of tile a into tile b, east of it; each tile holds the whole segment.
        const a = { level: 14, x: 8600, y: 5800 }, b = { ...a, x: a.x + 1 };
        const wall = (x0: number, x1: number): Feature[] => [{ kind: 'wall', tags: {}, geometry: 'line', parts: [{ outer: false, xy: new Int32Array([x0, 100, x1, 100]), z: null }] }];
        const triangles = (list: Feature[], tile: typeof a, x: number) => (structureMeshes(list, tile, { level: OWNER, x, y: 2 * tile.y }).near.structures?.indices.length ?? 0) / 3;
        // The node of b that holds the end draws only its cap; the node of a draws the faces and the cap at the start.
        expect(triangles(wall(1000 - EXTENT, 300), b, 2 * b.x)).toBe(2);
        expect((triangles(wall(1000, EXTENT + 300), a, 2 * a.x) - 2) % 6).toBe(0);
        expect(triangles(wall(1000, EXTENT + 300), a, 2 * a.x + 1)).toBe(0);
    });

    it('stands a model along the nearest way within reach, its front toward it', () => {
        const y = 2 ** 29, m = 1 / metresPerUnit(y), way = (xy: number[]) => ({ xy }) as Line;
        // An east–west way 5 m south of the point (world y runs south), and one 8 m north.
        const ways = [way([-50 * m, y + 5 * m, 50 * m, y + 5 * m]), way([-50 * m, y - 8 * m, 50 * m, y - 8 * m])];
        const front = (yaw: number) => [Math.sin(yaw), -Math.cos(yaw)].map(v => Math.round(v * 1e6) / 1e6 + 0);
        expect(front(facing(ways, 0, y)!)).toEqual([0, -1]);
        expect(front(facing(ways.slice(1), 0, y)!)).toEqual([0, 1]);
        expect(facing([way([-50 * m, y + 20 * m, 50 * m, y + 20 * m])], 0, y)).toBeUndefined();
    });

    it('stands street lamps along a lit road where a building is near, none in or by a building, on a bridge or a track', () => {
        const y = 2 ** 29, m = 1 / metresPerUnit(y);
        // A road 320 m east; one building 10 m north of its first 100 m.
        const road = { outer: false, xy: [0, y, 320 * m, y] }, house = { rings: [[0, y - 10 * m, 100 * m, y - 10 * m, 100 * m, y - 20 * m]], outer: [true] };
        // A shed on the right of the road at 48 m, and a wall 1 m from the lamp at 80 m.
        const square = (x: number, y0: number, y1: number) => ({ rings: [[x - 3 * m, y + y0 * m, x + 3 * m, y + y0 * m, x + 3 * m, y + y1 * m, x - 3 * m, y + y1 * m]], outer: [true] });
        const lamps = (tags: Record<string, string>, buildings = [house]) => {
            const placed: Placement[] = [];
            const out = { owns: () => true, place: (p: Placement) => placed.push(p), ways: [], buildings } as unknown as Build;
            lampKind(3.5).build({ tags, parts: [road] }, out);
            return placed.map(p => Math.round(p.x / m));
        };
        expect(lamps({ highway: 'residential' })).toEqual([16, 48, 80, 112]);
        expect(lamps({ highway: 'residential' }, [house, square(48 * m, 2, 8), square(80 * m, 4.5, 8)])).toEqual([16, 112]);
        expect(lamps({ highway: 'residential', bridge: 'yes' })).toEqual([]);
        expect(lamps({ highway: 'track' })).toEqual([]);
    });
});
