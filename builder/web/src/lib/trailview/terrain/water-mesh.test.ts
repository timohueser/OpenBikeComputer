import { describe, expect, it } from 'vitest';
import type { Feature } from '../stream/features';
import { metresPerUnit } from './bins';
import { EDGE_M, WATER_VERTEX, waterMesh } from './water-mesh';

/** A zoom-12 tile at Engelberg, 8192 tile units of about 0.8 m; world units per tile unit. */
const TILE = { level: 12, x: 2143, y: 1444 }, SCALE = 2 ** (17 - TILE.level);
const X0 = TILE.x * 8192 * SCALE, Y0 = TILE.y * 8192 * SCALE;
/** A square lake 3.3 km across, and a river 20 m wide that flows east, down from 1010 m to 1000 m. */
const lake = { kind: 'lake', tags: {}, parts: [{ outer: true, xy: new Int32Array([1000, 1000, 5000, 1000, 5000, 5000, 1000, 5000]), z: new Float32Array(4).fill(1000) }] };
const river = { kind: 'river', tags: { waterway: 'river', width: '20' }, parts: [{ outer: false, xy: new Int32Array([500, 7000, 7000, 7500]), z: new Float32Array([1010, 1000]) }] };

describe('the water mesh of a tile', () => {
    const mesh = waterMesh([lake, river] as Feature[], TILE)!;
    const ints = new Int32Array(mesh.vertices), floats = new Float32Array(mesh.vertices);
    const vertices = Array.from({ length: ints.length / WATER_VERTEX }, (_, i) => ({
        x: ints[WATER_VERTEX * i], y: ints[WATER_VERTEX * i + 1], level: floats[WATER_VERTEX * i + 2],
        normal: [floats[WATER_VERTEX * i + 3], floats[WATER_VERTEX * i + 4]], reach: floats[WATER_VERTEX * i + 6], half: floats[WATER_VERTEX * i + 7],
    }));
    const triangles = Array.from({ length: mesh.indices.length / 3 }, (_, t) => [0, 1, 2].map(k => vertices[mesh.indices[3 * t + k]]));

    it('cuts a lake into triangles that cover it, no edge longer than EDGE_M and no vertex on the edge of another triangle', () => {
        // The surface: the triangles without a moving vertex.
        const surface = triangles.filter(t => t.every(v => v.reach === 0 && v.half === 0));
        const area = surface.reduce((sum, [a, b, c]) => sum + Math.abs((b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y)) / 2, 0);
        expect(area / (4000 * SCALE) ** 2).toBeCloseTo(1, 5);
        const uses = new Map<string, number>();
        for (const t of surface) for (let k = 0; k < 3; k++) {
            const [a, b] = [t[k], t[(k + 1) % 3]];
            expect(Math.hypot(a.x - b.x, a.y - b.y) * metresPerUnit(Y0)).toBeLessThanOrEqual(EDGE_M);
            const name = [a, b].map(({ x, y }) => `${x},${y}`).sort().join(' ');
            uses.set(name, (uses.get(name) ?? 0) + 1);
        }
        // An edge with one triangle lies on the shore; every other edge has two.
        const on = (value: number, sides: number[]) => sides.some(side => value === side * SCALE);
        for (const [name, count] of uses) {
            const ends = name.split(' ').map(end => end.split(',').map(Number));
            const shore = ends.every(([x]) => on(x - X0, [1000])) || ends.every(([x]) => on(x - X0, [5000])) || ends.every(([, y]) => on(y - Y0, [1000])) || ends.every(([, y]) => on(y - Y0, [5000]));
            expect(count === 2 || (count === 1 && shore), name).toBe(true);
        }
    });

    it('moves the skirt of a lake out of the water, and the strip of a river to both sides at its levels', () => {
        const [cx, cy] = [X0 + 3000 * SCALE, Y0 + 3000 * SCALE];
        for (const v of vertices.filter(v => v.reach === 1 && v.half === 0)) expect(v.normal[0] * (v.x - cx) + v.normal[1] * (v.y - cy)).toBeGreaterThan(0);
        const strip = vertices.filter(v => v.half === 10);
        expect(strip.length).toBeGreaterThan(4);
        for (let i = 0; i < strip.length; i += 2) {
            expect(strip[i].normal[0] + strip[i + 1].normal[0]).toBeCloseTo(0);
            if (i) expect(strip[i].level).toBeLessThanOrEqual(strip[i - 2].level);
        }
    });
});
