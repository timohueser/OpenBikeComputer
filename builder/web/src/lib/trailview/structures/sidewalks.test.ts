import { describe, expect, it } from 'vitest';
import { KERB } from '../ground/rules';
import { EXTENT, unitScale, type Feature } from '../stream/features';
import { features, metresPerUnit } from '../terrain/bins';
import { cornersOf } from '../terrain/corners';
import { rgb, type MeshBuilder } from './mesh';
import { sidewalks } from './sidewalks';

/** The quads that the slabs of a tile's lines draw inside a square, as their corners' x, y, z and colour. */
function slabs(list: Feature[], tile: { level: number; x: number; y: number }, square: number[]): number[][][] {
    const vertices: number[][] = [], quads: number[][][] = [];
    const builder = { vertex: (...v: number[]) => vertices.push(v) - 1, quad: (q: number[]) => quads.push(q.map(i => vertices[i])), outward: () => [0, 0, 1] } as unknown as MeshBuilder;
    sidewalks(builder, features(list, tile).lines, square);
    return quads;
}

describe('the sidewalks', () => {
    const tile = { level: 13, x: 4287, y: 2887 }, scale = unitScale(tile.level), m = 1 / (scale * metresPerUnit((tile.y * EXTENT + 4000) * scale));
    const road = (xy: number[]) => ({ kind: 'road', tags: { highway: 'residential' }, geometry: 'line' as const, parts: [{ outer: false, xy: Int32Array.from(xy.map(Math.round)), z: Float32Array.from(xy.filter((_, i) => i % 2 === 0).map(() => 1000)) }] });
    const house = (x: number, y: number) => ({ kind: 'building', tags: { building: 'house' }, geometry: 'area' as const, parts: [{ outer: true, xy: Int32Array.from([x, y, x + 10 * m, y, x + 10 * m, y + 8 * m, x, y + 8 * m].map(Math.round)), z: null }] });

    it("end a straight slab where its corner's arc begins", () => {
        // A road east with a side road south 8 m west of a house on their corner.
        const x = 3000 - 8 * m, list = [road([1000, 4000, x, 4000, 5000, 4000]), road([x, 4000, x, 4000 + 30 * m]), house(3000, 4000 + 12.5 * m)] as Feature[];
        const quads = slabs(list, tile, [0, 0, 2 ** 30, 2 ** 30]), granite = rgb(KERB);
        const corner = cornersOf(features(list, tile).lines).find(({ walk }) => walk[0] && walk[1])!, near = (q: number[][], e: number[]) => q.some(([qx, qy]) => Math.hypot(qx - e[0], qy - e[1]) * metresPerUnit(e[1]) < 0.01);
        // At each end the arc's kerb top and the straight slab's meet on one edge.
        for (const end of [corner.a, corner.b]) expect(quads.filter(q => q[0][3] === granite && q.every(v => v[2] === q[0][2]) && near(q, end)).length).toBe(2);
    });

    it("draw each node's part of a street that crosses a tile's border, and meet on it", () => {
        // A street east across the border of tiles x and x + 1, with houses north of it every 15 m.
        const next = { ...tile, x: tile.x + 1 }, border = next.x * EXTENT * scale, y = (tile.y * EXTENT + 8000) * scale, w = 1 / metresPerUnit(y);
        const local = (t: typeof tile, wx: number, wy: number) => [wx / scale - t.x * EXTENT, wy / scale - t.y * EXTENT];
        const ends: number[] = [];
        for (const t of [tile, next]) {
            const list: Feature[] = [road([...local(t, border - 150 * w, y), ...local(t, border + 150 * w, y)])] as Feature[];
            for (let hx = border - 140 * w; hx < border + 130 * w; hx += 15 * w) {
                const [lx, ly] = local(t, hx, y - 16.5 * w);
                if (Math.abs(hx - border) < 80 * w) list.push(house(lx, ly) as unknown as Feature);
            }
            const xs = slabs(list, t, [t.x * EXTENT * scale, t.y * EXTENT * scale, (t.x + 1) * EXTENT * scale, (t.y + 1) * EXTENT * scale]).flat().filter(v => v[1] < y).map(v => (v[0] - border) * metresPerUnit(y));
            ends.push(Math.min(...xs), Math.max(...xs));
        }
        // The first tile draws up to the border, the next on from it.
        expect(ends[1]).toBeCloseTo(0, 1);
        expect(ends[2]).toBeCloseTo(0, 1);
        expect(ends[0]).toBeLessThan(-50);
        expect(ends[3]).toBeGreaterThan(50);
    });
});
