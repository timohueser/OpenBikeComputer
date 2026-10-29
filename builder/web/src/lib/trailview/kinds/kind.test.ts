import { describe, expect, it } from 'vitest';
import TABLE from '../../../../../../data/trailview/kinds.json' with { type: 'json' };
import { EXTENT, type Feature } from '../stream/features';
import { ANCHOR_WORDS, metresBetween } from '../structures/anchored';
import { OWNER, structureMeshes } from '../structures/node';
import { KINDS } from '.';
import { facing, type Build, type Placement } from './kind';
import { lampKind } from '../structures/lamps';
import { metresPerUnit, PROFILES, ROAD, type Line } from '../terrain/bins';
import { RIGHT } from '../terrain/town';

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

    it('hangs the cable of a via ferrata from bolts at most 3 m apart, in the node that owns each segment', () => {
        // A via ferrata from the west half of a tile into its east half: one node of each.
        const a = { level: 14, x: 8600, y: 5800 }, west = { level: OWNER, x: 2 * a.x, y: 2 * a.y };
        const feature: Feature[] = [{ kind: 'path', tags: { highway: 'via_ferrata' }, geometry: 'line', parts: [{ outer: false, xy: new Int32Array([1000, 100, EXTENT - 1000, 100]), z: null }] }];
        const anchors = (x: number) => (structureMeshes(feature, a, { ...west, x }).anchors?.byteLength ?? 0) / (ANCHOR_WORDS * 4);
        const metres = metresBetween([0, a.y * 2 ** 16], [(EXTENT - 2000) * 4, a.y * 2 ** 16]);
        // The west node hangs every span; the east node sets the bolt at the end.
        expect(anchors(west.x)).toBe(Math.ceil(metres / 3));
        expect(anchors(west.x + 1)).toBe(1);
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

    it('stands street lamps from the stations of a lit road, on its sidewalk, none in a junction or by a building', () => {
        const y = 2 ** 29, m = 1 / metresPerUnit(y), profile = Object.keys(PROFILES).indexOf('residential');
        // A road 320 m east with a sidewalk on its right; a house 10 m north of its first 100 m; a side road at 112 m.
        const road = (xy: number[], sidewalks?: number[]) => ({ kind: ROAD, xy, z: xy.filter((_, i) => i % 2 === 0).map(() => 0), h: 2.5, code: 2, ford: false, profile, sidewalks }) as Line;
        const house = { rings: [[0, y - 10 * m, 100 * m, y - 10 * m, 100 * m, y - 20 * m]], outer: [true] };
        const lamps = (ways: Line[]) => {
            const placed: Placement[] = [];
            const out = { owns: () => true, place: (p: Placement) => placed.push(p), ways, buildings: [house], walks: [], cornered: () => false } as unknown as Build;
            lampKind(3.5).build({ tags: {}, parts: [] }, out);
            return placed.map(p => [Math.round(p.x / m), Math.round((p.y - y) / m * 10) / 10]);
        };
        const main = road([0, y, 112 * m, y, 320 * m, y], [RIGHT, RIGHT]);
        expect(lamps([main])).toEqual([[16, 3.6], [48, 3.6], [80, 3.6], [112, 3.6], [144, 3.6], [176, 3.6], [208, 3.6], [240, 3.6], [272, 3.6], [304, 3.6]]);
        expect(lamps([main, road([112 * m, y, 112 * m, y + 50 * m])]).map(([x]) => x)).toEqual([16, 48, 80, 120, 152, 184, 216, 248, 280, 312]);
    });
});
