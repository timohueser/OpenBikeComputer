import { readFileSync } from 'node:fs';
import { beforeAll, describe, expect, it } from 'vitest';
import { metresPerUnit } from '../terrain/bins';
import { size } from '../terrain/lod';
import { DEBUG_WORDS, debugMesh, EDGE_STEP_M, type Vocabulary } from './debug';
import { CELL_LEVEL } from './plan';
import { initSync, vocabulary } from './pkg/obc_ground_plan_bridge.js';

const cell = { x: 4286 * 8, y: 2888 * 8 };
const unit = metresPerUnit((cell.y + 0.5) * size(CELL_LEVEL));
/** Metres as frame units: 16 per world unit. */
const frame = (m: number) => Math.round(m / unit * 16);
const area = (p: number[][]) => p.reduce((sum, a, i) => sum + a[0] * p[(i + 1) % p.length][1] - p[(i + 1) % p.length][0] * a[1], 0) / 2;

/** One plan of faces, each a list of rings of [x, y] in metres from the cell's origin. */
function plan(words: Vocabulary, faces: { surface: string; rings: number[][][] }[]) {
    const vertices: number[] = [], face: number[] = [], rings: number[] = [], ringVertices: number[] = [];
    for (const [id, { surface, rings: own }] of faces.entries()) {
        face.push(id, words.owners.indexOf('way'), 0, words.surfaces.findIndex(s => s.name === surface), 0, 0, 0, 0, 0, 1, 0, rings.length / 2, own.length);
        for (const ring of own) {
            rings.push(ringVertices.length, ring.length);
            for (const [x, y] of ring) ringVertices.push(vertices.push(frame(x), frame(y)) / 2 - 1);
        }
    }
    const empty = { samples: new Int32Array(0), ways: new Uint32Array(0), points: new Uint32Array(0), crossings: new Uint32Array(0), edges: new Uint32Array(0) };
    return { ...empty, vertices: Int32Array.from(vertices), faces: Uint32Array.from(face), rings: Uint32Array.from(rings), ring_vertices: Uint32Array.from(ringVertices) };
}

/** The triangles of a fill in metres. */
function triangles(fills: Int32Array): number[][][] {
    const s = size(CELL_LEVEL), out = [];
    for (let v = 0; v < fills.length; v += 3 * DEBUG_WORDS) {
        out.push([0, 1, 2].map(k => [(fills[v + k * DEBUG_WORDS] - cell.x * s) * unit, (fills[v + k * DEBUG_WORDS + 1] - cell.y * s) * unit]));
    }
    return out;
}

describe('the debug mesh of the faces', () => {
    let words: Vocabulary;
    beforeAll(() => {
        initSync({ module: readFileSync(new URL('./pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url)) });
        words = JSON.parse(vocabulary());
    });

    it('fills a face with its holes and a long strip in short triangles, the strip linear in its length', () => {
        const square = [[[50, 50], [150, 50], [150, 150], [50, 150]], [[80, 80], [80, 120], [120, 120], [120, 80]]];
        const [c, s] = [Math.cos(Math.PI / 6), Math.sin(Math.PI / 6)], along = (t: number, w: number) => [10 + t * c - w * s, 190 + t * s + w * c];
        const strip = [[along(0, 0), along(400, 0), along(400, 6), along(0, 6)]];
        for (const [rings, expected] of [[square, 100 * 100 - 40 * 40], [strip, 400 * 6]] as const) {
            const mesh = debugMesh(plan(words, [{ surface: 'carriageway', rings: rings as unknown as number[][][] }]), words, cell);
            const parts = triangles(mesh.fills);
            expect(Math.abs(parts.reduce((sum, t) => sum + Math.abs(area(t)), 0) / expected - 1)).toBeLessThan(1e-3);
            for (const t of parts) for (let k = 0; k < 3; k++) expect(Math.hypot(t[k][0] - t[(k + 1) % 3][0], t[k][1] - t[(k + 1) % 3][1])).toBeLessThanOrEqual(4);
            // The gate holds the count linear in the length.
            if (rings === strip) expect(parts.length).toBeLessThanOrEqual(1300);
            expect(mesh.unfilled).toBe(0);
        }
    });

    it('gives a terrain face no fill, counts a face that earcut cannot fill and draws every edge in steps', () => {
        const flat = plan(words, [
            { surface: 'terrain', rings: [[[0, 0], [300, 0], [300, 300]]] },
            { surface: 'verge', rings: [[[10, 10], [20, 10], [30, 10]]] },
        ]);
        flat.edges = Uint32Array.from([0, 1, 0, 1, 0, 0, 1, 2, 0, 1, words.edges.findIndex(e => e.name === 'kerb'), 0]);
        const mesh = debugMesh(flat, words, cell);
        expect(mesh.fills.length).toBe(0);
        expect(mesh.unfilled).toBe(1);
        expect(mesh.lines.length).toBeGreaterThan(2 * 300 / EDGE_STEP_M * 2 * DEBUG_WORDS);
        for (let v = 0; v < mesh.lines.length; v += 2 * DEBUG_WORDS) {
            const [ax, ay, , bx, by] = mesh.lines.subarray(v, v + 2 * DEBUG_WORDS);
            expect(Math.hypot(bx - ax, by - ay) * unit).toBeLessThanOrEqual(EDGE_STEP_M + 0.1);
        }
    });
});
