import { describe, expect, it } from 'vitest';
import TABLE from '../../../../../../data/trailview/kinds.json' with { type: 'json' };
import { EXTENT, type Feature } from '../stream/features';
import { ANCHOR_WORDS, metresBetween } from '../structures/anchored';
import { structureMeshes } from '../structures/node';
import { OWNER } from '../terrain/lod';
import { KINDS } from '.';
import { facing, type Build, type Placement } from './kind';
import { coresOf } from '../roads/cores';
import { streetLamps } from '../roads/lamps';
import { BRIDGE, metresPerUnit, PROFILES, RAIL, ROAD, TUNNEL, type Line, type Shapes } from '../terrain/bins';
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

    describe('a model at a point keeps out of the ways', () => {
        const y = 2 ** 29, m = 1 / metresPerUnit(y);
        // A road of surface `road` 2.5 m each side with a 0.4 m shoulder: its core reaches 2.9 m from its middle,
        // and its band's first row 0.35 m further, where a moved model's room starts.
        const road = (xy: number[]) => ({ kind: ROAD, xy, z: xy.filter((_, i) => i % 2 === 0).map(() => 0), h: 2.5, code: 2, ford: false, profile: 0 }) as Line;
        const EDGE = 2.9, ROOM = EDGE + 0.35, STEP = 0.1;
        const stand = (kind: string, lines: Line[], x: number, at: number, { cornered = (() => false) as Build['cornered'], buildings = [] as Shapes['buildings'] } = {}) => {
            const placed: Placement[] = [], shapes = { lines, lakes: [], seas: [], areas: [], buildings } as unknown as Shapes;
            const out = { owns: () => true, place: (p: Placement) => placed.push(p), ways: lines, cores: coresOf(shapes), taken: [], cornered } as unknown as Build;
            KINDS.get(kind)!.build({ tags: {}, parts: [{ outer: false, xy: [x, at] }] }, out);
            return placed[0];
        };
        // The model's front (−y), east and north.
        const front = (yaw: number) => [Math.sin(yaw), -Math.cos(yaw)].map(v => Math.round(v * 1e6) / 1e6 + 0);
        const east = [road([-50 * m, y, 50 * m, y])], tee = [road([-50 * m, y, 0, y, 50 * m, y]), road([0, y, 0, y + 50 * m])];

        it('a bench on a road moves to its nearer edge, half its depth past the first row, and faces the road', () => {
            // 1 m south of the middle: it goes to the south edge, its half depth (0.23 m) past the first row.
            const bench = stand('bench', east, 0, y + m);
            expect(bench.x).toBe(0);
            expect((bench.y - y) / m).toBeGreaterThanOrEqual(ROOM + 0.23);
            expect((bench.y - y) / m).toBeLessThan(ROOM + 0.23 + STEP);
            expect(front(bench.yaw)).toEqual([0, 1]);
            // The core's edge lies north of it, a quarter turn from east.
            expect(bench.beside?.band).toBe(true);
            expect(bench.beside?.turn).toBeCloseTo(Math.PI / 2, 6);
        });

        it('a guidepost at a T goes into the widest gap, its arms along the through road', () => {
            // The side road leaves the main road south: the widest gap is north of the main road.
            const post = stand('guidepost', tee, 0, y);
            expect(Math.abs(post.x / m)).toBeLessThan(1e-6);
            expect((y - post.y) / m).toBeGreaterThanOrEqual(ROOM + 0.1);
            expect((y - post.y) / m).toBeLessThan(ROOM + 0.1 + STEP);
            expect(front(post.yaw)).toEqual([0, -1]);
            // A guidepost a metre into the side road goes there too, and a bench there goes to its nearer edge.
            expect((y - stand('guidepost', tee, 0, y + m).y) / m).toBeGreaterThan(EDGE);
            expect(Math.abs(stand('bench', tee, 0.5 * m, y + 6 * m).x / m)).toBeGreaterThan(EDGE);
        });

        it('a guidepost at a split of two ways goes to the nearer edge, not across the road', () => {
            const split = [road([-50 * m, y, 0, y]), road([0, y, 50 * m, y])];
            expect((stand('guidepost', split, 0, y + m).y - y) / m).toBeGreaterThan(EDGE);
        });

        it('a model in a junction corner, in no core, moves out of the corner', () => {
            const cornered = (px: number, py: number) => px > 3 * m && px < 6 * m && py > y + 3 * m && py < y + 6 * m;
            const bench = stand('bench', tee, 4 * m, y + 4 * m, { cornered });
            expect(cornered(bench.x, bench.y)).toBe(false);
            expect(bench.x / m).toBeGreaterThan(4);
            expect((bench.y - y) / m).toBeGreaterThan(4);
        });

        it('a model goes to the far edge where a building stands at the near one', () => {
            const house = { rings: [[-10 * m, y + 3 * m, 10 * m, y + 3 * m, 10 * m, y + 10 * m, -10 * m, y + 10 * m]], outer: [true] };
            expect((y - stand('bench', east, 0, y + m, { buildings: [house] }).y) / m).toBeGreaterThan(EDGE);
        });

        it('a model at a dead end, or between the core and the first row, moves past the first row', () => {
            for (const metres of [0.3, 1, 2]) {
                const bench = stand('bench', east, (50 + metres) * m, y);
                expect(Math.hypot(bench.x / m - 50, (bench.y - y) / m)).toBeGreaterThan(ROOM);
            }
            expect((stand('bench', east, 0, y + (EDGE + 0.15) * m).y - y) / m).toBeGreaterThanOrEqual(ROOM + 0.23);
        });

        it('a model in the corner of a bend moves away from the bend, not toward a junction near by', () => {
            // A road west to a bend, then south; a side road leaves it 60 m west. The corner lies inside the bend.
            const bend = [road([-100 * m, y, -60 * m, y, 0, y]), road([0, y, 0, y + 100 * m]), road([-60 * m, y, -60 * m, y - 50 * m])];
            const cornered = (px: number, py: number) => px > -6 * m && px < -3 * m && py > y + 3 * m && py < y + 6 * m;
            const bench = stand('bench', bend, -4 * m, y + 4 * m, { cornered });
            expect(cornered(bench.x, bench.y)).toBe(false);
            expect(bench.x / m).toBeLessThan(-4);
            expect((bench.y - y) / m).toBeGreaterThan(4);
        });

        it('a moved model keeps clear of a model that the node placed before', () => {
            const placed: Placement[] = [], shapes = { lines: east, lakes: [], seas: [], areas: [], buildings: [] } as unknown as Shapes;
            const out = { owns: () => true, place: (p: Placement) => placed.push(p), ways: east, cores: coresOf(shapes), taken: [], cornered: () => false } as unknown as Build;
            for (const kind of ['bench', 'drinking-water']) KINDS.get(kind)!.build({ tags: {}, parts: [{ outer: false, xy: [0, y + m] }] }, out);
            expect(Math.hypot(placed[0].x - placed[1].x, placed[0].y - placed[1].y) / m).toBeGreaterThan(0.23);
        });

        it('a point off every core stays, on the band out to its last row on the stamped ground', () => {
            for (const [metres, band] of [[EDGE + 1, true], [EDGE + 3, false], [20, undefined]] as const) {
                const bench = stand('bench', east, 0, y + metres * m);
                expect([bench.x, bench.y]).toEqual([0, y + metres * m]);
                expect(bench.beside?.band).toBe(band);
            }
        });

        it('a kind without facesWay moves and keeps the turn of its place', () => {
            const tap = stand('drinking-water', east, 0, y + 2 * m);
            expect((tap.y - y) / m).toBeGreaterThan(EDGE);
            expect(tap.yaw).toBe(((tap.x * 7 + tap.y * 13) % 628) / 100);
        });
    });

    describe('street lamps stand from the stations of a lit road', () => {
        const y = 2 ** 29, m = 1 / metresPerUnit(y), profile = (name: string) => Object.keys(PROFILES).indexOf(name);
        const road = (xy: number[], extra: Partial<Line> = {}) => ({ kind: ROAD, xy, z: xy.filter((_, i) => i % 2 === 0).map(() => 0), h: 2.5, code: 2, ford: false, profile: profile('residential'), ...extra }) as Line;
        // A road 320 m east with a sidewalk on its right, and a house 10 m north of its first 100 m.
        const main = road([0, y, 112 * m, y, 320 * m, y], { sidewalks: [RIGHT, RIGHT] }), house = { rings: [[0, y - 10 * m, 100 * m, y - 10 * m, 100 * m, y - 20 * m]], outer: [true] };
        const places = (lines: Line[], { buildings = [house] as unknown[], cornered = (() => false) as (x: number, y: number) => boolean } = {}) => {
            const placed: Placement[] = [];
            streetLamps({ owns: () => true, place: p => placed.push(p), buildings, cornered } as Parameters<typeof streetLamps>[0], lines);
            return placed;
        };
        const along = (lines: Line[], others?: Parameters<typeof places>[1]) => places(lines, others).map(p => Math.round(p.x / m));
        const every32 = [16, 48, 80, 112, 144, 176, 208, 240, 272, 304];

        it('on the outer part of the sidewalk, the arm over the road', () => {
            for (const lamp of places([main])) {
                expect((lamp.y - y) / m).toBeCloseTo(3.58, 1);
                // The model's arm reaches along its −y: north, to the road.
                expect([Math.sin(lamp.yaw), Math.cos(lamp.yaw)].map(v => Math.round(v))).toEqual([0, -1]);
            }
            expect(along([main])).toEqual(every32);
        });

        it('behind the shoulder where a building is near and the road has no sidewalk', () => {
            const lamps = places([road([0, y, 320 * m, y])]);
            expect(lamps.map(p => Math.round(p.x / m))).toEqual([16, 48, 80, 112]);
            expect((lamps[0].y - y) / m).toBeCloseTo(3.4, 1);
        });

        it('in no mouth of a road or a bridge', () => {
            const side = [112 * m, y, 112 * m, y + 50 * m];
            expect(along([main, road(side)])).toEqual([16, 48, 80, 120, 152, 184, 216, 248, 280, 312]);
            expect(along([main, road(side, { kind: BRIDGE })])).toEqual([16, 48, 80, 120, 152, 184, 216, 248, 280, 312]);
        });

        it('in no mouth where a road runs straight into a bridge or a tunnel', () => {
            for (const kind of [BRIDGE, TUNNEL]) {
                // The lamp due at 80 m moves back past the mouth: 2.5 m of road, 1.5 m of sidewalk and 4 m.
                expect(along([road([0, y, 81 * m, y], { sidewalks: [RIGHT, RIGHT] }), road([81 * m, y, 300 * m, y], { kind })], { buildings: [] })).toEqual([16, 48, 72]);
            }
        });

        it('far from the other lamps of the tile, whichever way they belong to', () => {
            // A second way starts 2 m past the first's start, 1 m to its side: without the rule its lamps stand beside the first's.
            const lamps = places([main, road([2 * m, y + 2 * m, 320 * m, y + 2 * m], { sidewalks: [RIGHT, RIGHT] })], { buildings: [] });
            expect(lamps).toHaveLength(10);
            for (const lamp of lamps) for (const other of lamps) if (other !== lamp) expect(Math.hypot(lamp.x - other.x, lamp.y - other.y) / m).toBeGreaterThanOrEqual(16);
        });

        it('in no corner, on no other carriageway and no track', () => {
            // Without a building, only the sidewalk's side can take a lamp.
            expect(along([main], { buildings: [], cornered: x => Math.abs(x - 48 * m) < 3 * m })).toEqual([16, 52, 84, 116, 148, 180, 212, 244, 276, 308]);
            const across = (kind: number) => road([0, y + 3.6 * m, 40 * m, y + 3.6 * m], { kind, h: 1 });
            expect(along([main, across(ROAD)], { buildings: [] }).slice(0, 2)).toEqual([48, 80]);
            expect(along([main, across(RAIL)], { buildings: [] }).slice(0, 2)).toEqual([48, 80]);
        });

        it('none on a bridge, a track or a motorway', () => {
            const line = (extra: Partial<Line>) => road([0, y, 320 * m, y], extra);
            expect(along([line({ kind: BRIDGE })])).toEqual([]);
            expect(along([line({ profile: profile('track') })])).toEqual([]);
            expect(along([line({ profile: profile('motorway') })])).toEqual([]);
        });

        it('by no wall of a long building whose first corner lies far from the road', () => {
            // At Engelberg, 46.82°N 8.4°E: a road 120 m east, a house 10 m north of it, and a hall from 100 m south of it to 1 m.
            const x = 0.523 * 2 ** 30, y = 2 ** 29 * (1 - Math.asinh(Math.tan(46.82 * Math.PI / 180)) / Math.PI), m = 1 / metresPerUnit(y), line = road([x, y, x + 120 * m, y]);
            const house = { rings: [[x, y - 10 * m, x + 120 * m, y - 10 * m, x + 120 * m, y - 20 * m]], outer: [true] };
            const hall = (near: number, far: number) => ({ rings: [[x, y + far * m, x + 120 * m, y + far * m, x + 120 * m, y + near * m, x, y + near * m]], outer: [true] });
            const south = (buildings: unknown[]) => places([line], { buildings }).map(p => p.y > y);
            expect(south([house])).toEqual([true, true, true, true]);
            // The side of the hall stands free of lamps, the other side takes them.
            expect(south([house, hall(1, 100)])).toEqual([false, false, false, false]);
            // A hall on each side leaves no place.
            expect(south([hall(-100, -1), hall(1, 100)])).toEqual([]);
        });
    });
});
