import { describe, expect, it } from 'vitest';
import { worldPixel } from '../../planner/layers/mercator';
import { anchorAt, lodSpace, UNITS } from '../frame/space';
import { cdlod, GRID, MAP, RANGE, select, size, type Eye, type Node, type Rule } from './lod';

const BOX = [7.6, 46.46, 8.95, 47.25] as const;
const [west, north] = worldPixel([BOX[0], BOX[3]], UNITS), [east, south] = worldPixel([BOX[2], BOX[1]], UNITS);
const BOUNDS: [number, number, number, number] = [west, north, east, south];
const eye = (lon: number, lat: number, above: number): Eye => ({ anchor: anchorAt(lon, lat), above });
const EYES = [eye(8.403, 46.8205, 1.6), eye(8.39, 46.8, 900), eye(8.415, 46.7, 12_000), eye(8.2, 46.85, 60_000)];
const RULES: [string, (eye: Eye) => Rule][] = [
    ...[RANGE.least, RANGE.most].map((factor): [string, (eye: Eye) => Rule] => [`cdlod ${factor}`, e => cdlod(e, factor)]),
];

/** The LOD at the grid vertex (i, j) of a node, from whole units, as the vertex shader has it. */
const vertexLod = (rule: Rule, e: Eye, { level, x, y }: Node, i: number, j: number) => {
    const cell = size(level) / GRID;
    return rule.lod(lodSpace(e.anchor, x * size(level) + i * cell, y * size(level) + j * cell));
};

describe.each(RULES)('the %s selection', (_, make) => {
    it.each(EYES.map(e => [e.above, e] as const))('draws each node at %s m above the ground where its LOD allows it', (_, e) => {
        const rule = make(e), { nodes } = select(rule, BOUNDS, () => true);
        for (const node of nodes) for (let j = 0; j <= GRID; j += 8) for (let i = 0; i <= GRID; i += 8) {
            const lod = vertexLod(rule, e, node, i, j);
            expect(lod).toBeGreaterThanOrEqual(node.level - 1);
            expect(lod).toBeLessThanOrEqual(node.level);
        }
    });

    it.each(EYES.map(e => [e.above, e] as const))('covers the box once, and a finer node meets a coarser one on its plateau, at %s m', (_, e) => {
        const rule = make(e), { nodes } = select(rule, BOUNDS, () => true);
        const area = (a: [number, number, number, number]) => (a[2] - a[0]) * (a[3] - a[1]);
        const clip = ({ level, x, y }: Node): [number, number, number, number] => {
            const s = size(level);
            return [Math.max(x * s, BOUNDS[0]), Math.max(y * s, BOUNDS[1]), Math.min((x + 1) * s, BOUNDS[2]), Math.min((y + 1) * s, BOUNDS[3])];
        };
        expect(nodes.reduce((sum, node) => sum + area(clip(node)), 0)).toBeCloseTo(area(BOUNDS), -3);
        const byKey = new Map(nodes.map(node => [`${node.level}/${node.x}/${node.y}`, node]));
        let meetings = 0;
        for (const fine of nodes) {
            // The coarser node beside each edge of this node, if one draws there.
            for (const [dx, dy, edge] of [[-1, 0, 'west'], [1, 0, 'east'], [0, -1, 'north'], [0, 1, 'south']] as const) {
                const coarse = byKey.get(`${fine.level - 1}/${(fine.x + dx) >> 1}/${(fine.y + dy) >> 1}`);
                if (!coarse || (fine.x >> 1 === coarse.x && fine.y >> 1 === coarse.y)) continue;
                meetings++;
                for (let k = 0; k <= GRID; k++) {
                    const [i, j] = edge === 'west' ? [0, k] : edge === 'east' ? [GRID, k] : edge === 'north' ? [k, 0] : [k, GRID];
                    expect(vertexLod(rule, e, fine, i, j)).toBe(coarse.level);
                }
            }
        }
        if (e.above < 1000) expect(meetings).toBeGreaterThan(0);
    });

    it('keeps each level inside its LOD map window', () => {
        for (const e of EYES) {
            const { nodes } = select(make(e), BOUNDS, () => true);
            for (const { level, x, y } of nodes) {
                const centre = Math.floor((e.anchor.whole[0] + e.anchor.part[0]) / size(level)) - MAP / 2;
                const middle = Math.floor((e.anchor.whole[1] + e.anchor.part[1]) / size(level)) - MAP / 2;
                expect([x - centre, y - middle].every(slot => slot >= 0 && slot < MAP)).toBe(true);
            }
        }
    });
});

describe('the CDLOD selection', () => {
    it('draws fewer nodes for a smaller range, and none finer than the finest level', () => {
        const walk = EYES[0];
        const counts = [RANGE.most, 5.5, RANGE.least].map(factor => select(cdlod(walk, factor), BOUNDS, () => true).nodes.length);
        expect(counts[0]).toBeGreaterThan(counts[1]);
        expect(counts[1]).toBeGreaterThan(counts[2]);
        const capped = select(cdlod(walk, RANGE.least, 15), BOUNDS, () => true).nodes;
        expect(Math.max(...capped.map(node => node.level))).toBe(15);
        expect(Math.max(...select(cdlod(walk, RANGE.least), BOUNDS, () => true).nodes.map(node => node.level))).toBe(18);
    });

    it('culls a subtree that is not visible, and wants the pages of the drawn nodes and of the parents that they morph to', () => {
        const walk = EYES[0], rule = cdlod(walk, RANGE.least);
        const all = select(rule, BOUNDS, () => true);
        const east = select(rule, BOUNDS, ({ level, x }) => (x + 1) * size(level) > walk.anchor.whole[0]);
        expect(east.nodes.length).toBeLessThan(all.nodes.length);
        const keys = new Set(east.wanted.map(({ level, x, y }) => `${level}/${x}/${y}`));
        for (const node of east.nodes) {
            const { level, x, y } = node;
            expect(keys.has(`${level}/${x}/${y}`)).toBe(true);
            // A node on its plateau never reads its parent's page.
            let below = false;
            for (let j = 0; j <= GRID; j += 4) for (let i = 0; i <= GRID; i += 4) below ||= vertexLod(rule, walk, node, i, j) < level;
            if (below) expect(keys.has(`${level - 1}/${x >> 1}/${y >> 1}`)).toBe(true);
        }
    });
});
