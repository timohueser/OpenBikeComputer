import { describe, expect, it } from 'vitest';
import type { Feature } from '../stream/features';
import { metresPerUnit } from './bins';
import { EDGE_M, WATER_VERTEX, waterMesh } from './water-mesh';

/** A zoom-12 tile at Engelberg, 8192 tile units of about 0.8 m; world units per tile unit. */
const TILE = { level: 12, x: 2143, y: 1444 }, SCALE = 2 ** (17 - TILE.level);
const X0 = TILE.x * 8192 * SCALE, Y0 = TILE.y * 8192 * SCALE;
/** The south-west level-13 node of the tile: tile units 0 to 4096 in x and 4096 to 8192 in y; a mesh cell of 64 of them. */
const NODE = { level: 13, x: 2 * TILE.x, y: 2 * TILE.y + 1 }, CELL = 64 * SCALE, TEXEL = 16 * SCALE;
/** A square lake 3.3 km across, and a river 20 m wide that flows east, down from 1010 m to 1000 m. */
const lake = { kind: 'lake', tags: {}, parts: [{ outer: true, xy: new Int32Array([1000, 1000, 5000, 1000, 5000, 5000, 1000, 5000]), z: new Float32Array(4).fill(1000) }] };
const river = { kind: 'river', tags: { waterway: 'river', width: '20' }, parts: [{ outer: false, xy: new Int32Array([500, 7000, 7000, 7500]), z: new Float32Array([1010, 1000]) }] };

describe('the water mesh of a node', () => {
    const mesh = waterMesh([lake, river] as Feature[], TILE, NODE)!;
    const ints = new Int32Array(mesh.vertices), floats = new Float32Array(mesh.vertices);
    const vertices = Array.from({ length: ints.length / WATER_VERTEX }, (_, i) => ({
        x: ints[WATER_VERTEX * i], y: ints[WATER_VERTEX * i + 1], level: floats[WATER_VERTEX * i + 2],
        across: [floats[WATER_VERTEX * i + 3], floats[WATER_VERTEX * i + 4]], half: floats[WATER_VERTEX * i + 5],
    }));
    const triangles = Array.from({ length: mesh.indices.length / 3 }, (_, t) => [0, 1, 2].map(k => vertices[mesh.indices[3 * t + k]]));

    it('cuts a lake to the node, and a texel past it, into triangles that cover it, no edge longer than EDGE_M and no vertex on the edge of another triangle', () => {
        const surface = triangles.filter(t => t.every(v => v.half === 0)), box = [1000 * SCALE, 4096 * SCALE - TEXEL, 4096 * SCALE + TEXEL, 5000 * SCALE];
        const area = surface.reduce((sum, [a, b, c]) => sum + Math.abs((b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y)) / 2, 0);
        expect(area / ((box[2] - box[0]) * (box[3] - box[1]))).toBeCloseTo(1, 5);
        const uses = new Map<string, number>();
        for (const t of surface) for (let k = 0; k < 3; k++) {
            const [a, b] = [t[k], t[(k + 1) % 3]];
            expect(Math.hypot(a.x - b.x, a.y - b.y) * metresPerUnit(Y0)).toBeLessThanOrEqual(EDGE_M);
            const name = [a, b].map(({ x, y }) => `${x},${y}`).sort().join(' ');
            uses.set(name, (uses.get(name) ?? 0) + 1);
        }
        // An edge with one triangle lies on the outline of the cut lake; every other edge has two.
        for (const [name, count] of uses) {
            const ends = name.split(' ').map(end => end.split(',').map(Number)).map(([x, y]) => [x - X0, y - Y0]);
            const outline = [0, 2].some(i => ends.every(([x]) => x === box[i])) || [1, 3].some(i => ends.every(([, y]) => y === box[i]));
            expect(count === 2 || (count === 1 && outline), name).toBe(true);
        }
    });

    it('lays a river along its line as pairs across it, a mesh cell apart at most, before the lake', () => {
        const ribbon = vertices.filter(v => v.half !== 0);
        expect(ribbon.length).toBeGreaterThan(4);
        for (let i = 0; i < ribbon.length; i += 2) {
            const [left, right] = [ribbon[i], ribbon[i + 1]];
            expect([left.half, right.half]).toEqual([10, -10]);
            expect(left.across[0] + right.across[0]).toBeCloseTo(0);
            expect(left.across[1] + right.across[1]).toBeCloseTo(0);
            if (i) expect(Math.hypot(left.x - ribbon[i - 2].x, left.y - ribbon[i - 2].y)).toBeLessThanOrEqual(CELL + 1);
        }
        const lakes = triangles.findIndex(t => t.every(v => v.half === 0));
        expect(lakes).toBeGreaterThan(0);
        expect(triangles.slice(lakes).every(t => t.every(v => v.half === 0))).toBe(true);
    });

    it('has no water for a node that no water reaches', () => {
        expect(waterMesh([lake, river] as Feature[], TILE, { ...NODE, x: NODE.x + 2 })).toBeNull();
    });
});
