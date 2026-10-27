import { describe, expect, it } from 'vitest';
import TABLE from '../../../../../../data/trailview/kinds.json' with { type: 'json' };
import { EXTENT, type Feature } from '../stream/features';
import { OWNER, structureMeshes } from '../structures/node';
import { KINDS } from '.';
import { facing } from './kind';
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
});
