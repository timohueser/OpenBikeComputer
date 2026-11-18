import { readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { cellMesh, meshBytes, SIDE } from './mesh';
import { build, CellRequest, feature_tiles, initSync } from './pkg/obc_ground_plan_bridge.js';

/** Frame units per grid cell of the mesh level, as `params::GRID`. */
const GRID = 1024;

/** An empty cell mesh (`CellMesh::to_bytes`): the head and the region's words. */
const EMPTY_CELL = new Uint8Array(new Uint32Array([0x4f424d06, 1, 2, ...new Array(10).fill(0), ...new Array(Math.ceil((130 * 130) / 32)).fill(0)]).buffer);

/**
 * A storey mesh's bytes as `StoreyMesh::to_bytes` of mesh/shell.rs lays them out, one run per part and per slot kind, then the
 * caps and the lights.
 */
function storeyBytes(corners: number[][], parts: number[][][], kinds: number[][][], caps: number[][], lights: number[][]): Uint8Array {
    const runs = (items: number[][][]) => items.filter(p => p.length).flatMap(p => [p.length, 0]);
    const packed = (items: number[][][]) => {
        const flat = items.flat(2), out = new Uint16Array(2 * Math.ceil(flat.length / 2));
        out.set(flat);
        return [...new Uint32Array(out.buffer)];
    };
    const [tris, slots] = [parts.flat().length, kinds.flat().length];
    const head = [0x4f425303, 3, 4, corners.length, tris, runs(parts).length / 2, slots, runs(kinds).length / 2, kinds[0].length, ...parts.map(p => p.length)];
    head.push(...kinds.slice(1).map(k => k.length), caps.length, lights.length);
    return new Uint8Array(new Uint32Array([...head, ...corners.flat(), ...runs(parts), ...packed(parts), ...runs(kinds), ...packed(kinds), ...caps.flat(), ...lights.flat()]).buffer);
}

describe('the mesh reader', () => {
    it('reads a real cell: every index names a corner, and no terrain vertex sinks while points inside R do', () => {
        initSync({ module: readFileSync(new URL('./pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url)) });
        // The north-west cell of zoom-13 tile 4286/2888, the tile of the vector: its roads lie there.
        const request = new CellRequest(4286 * 8, 2888 * 8);
        request.add_tile(4286, 2888, readFileSync(new URL('../../../../../../specs/vectors/trailview-features.bin', import.meta.url)));
        const plan = build(request, false), input = plan.mesh_input();
        try {
            const mesh = cellMesh(input.mesh(plan.rim())!.buffer as ArrayBuffer), corners = mesh.corners.length / 2;
            expect(mesh.triangles.length).toBeGreaterThan(0);
            let k = 0;
            for (const { count, base } of mesh.runs) for (let n = 0; n < 3 * count; n++, k++) expect(mesh.triangles[k] + base).toBeLessThan(corners);
            expect(k).toBe(mesh.triangles.length);
            expect(mesh.steps.every(k => k < corners)).toBe(true);
            const sunk = (i: number, j: number) => ((mesh.sunk[(j * SIDE + i) >> 5] >>> ((j * SIDE + i) & 31)) & 1) === 1;
            let terrain = 0, inside = 0;
            for (let k = 0; k < corners; k++) {
                const x = mesh.corners[2 * k] & 0x7ffff, y = mesh.corners[2 * k + 1] & 0x7ffff;
                if (mesh.corners[2 * k + 1] >>> 31 === 0) continue;
                terrain++;
                // A terrain vertex is a grid point on R's border: the terrain draws it, so it never sinks.
                expect([x % GRID, y % GRID, +sunk(x / GRID, y / GRID)]).toEqual([0, 0, 0]);
            }
            for (let j = 0; j < SIDE; j++) for (let i = 0; i < SIDE; i++) inside += +sunk(i, j);
            expect(terrain).toBeGreaterThan(0);
            expect(inside).toBeGreaterThan(0);
        } finally {
            input.free();
            plan.free();
        }
    });

    it('reads a storey mesh with the bores: the bores in parts and slot kinds of their own after the decks, the caps and the lights', () => {
        const corners = [0, 1, 2, 3, 4, 5].map(k => [k, k, 0]), tri = (k: number) => [k % 6, (k + 1) % 6, (k + 2) % 6];
        // top, underside, edge, parapet, railing, post, pier, then floor, roof, wall, front.
        const parts = [[tri(0)], [tri(1)], [], [], [], [], [], [tri(2), tri(3)], [tri(4)], [], [tri(5)]];
        // The tops' slots, the risers, then the floors' seams, the roofs' seams and the sills.
        const kinds = [[[0, 1, 2, 3]], [], [[1, 2, 3, 4]], [], [[2, 3, 4, 5]]];
        const caps = [[1, 7, 0xbe99999a], [1, 33554431, 0]], lights = [[4, 0x42c80000, 0], [4, 0x42f00000, 0x41a00000]];
        const s = cellMesh(meshBytes(EMPTY_CELL, storeyBytes(corners, parts, kinds, caps, lights)).slice(0)).storeys!;
        expect([...s.triangles]).toEqual(parts.flat(2));
        expect([s.top, s.railings, s.piers, s.bores, s.roofs, s.fronts]).toEqual([1, 2, 2, 2, 4, 5]);
        expect([...s.slots]).toEqual(kinds.flat(2));
        expect([s.seams, s.sills]).toEqual([1, 2]);
        expect([...s.caps]).toEqual(caps.flat());
        expect([...s.lights]).toEqual(lights.flat());
    });

    it('reads the storeys after a cell mesh: every index names a storey corner, the top comes first, then the shell and the bores in their parts', () => {
        // Two cells of the browser fixture: decks over a road, and a tunnel's openings.
        const fixture = new URL('../../../../tests/browser/trailview/fixture/features/13/', import.meta.url);
        for (const [x, y, bored] of [[34294, 23098, false], [34294, 23097, true]] as const) {
            const request = new CellRequest(x, y), tiles = feature_tiles(x, y);
            for (let t = 0; t < tiles.length; t += 2) {
                const bytes = readFileSync(new URL(`${tiles[t]}/${tiles[t + 1]}.bin`, fixture));
                request.add_tile(tiles[t], tiles[t + 1], bytes[0] === 0x1f && bytes[1] === 0x8b ? gunzipSync(bytes) : bytes);
            }
            const plan = build(request, false), input = plan.mesh_input();
            try {
                const mesh = cellMesh(meshBytes(input.mesh(plan.rim())!, input.storeys())), storeys = mesh.storeys!, corners = storeys.offsets.length;
                expect(storeys.corners.length).toBe(2 * corners);
                expect([...storeys.triangles, ...storeys.slots].every(c => c < corners)).toBe(true);
                const firsts = [storeys.top, storeys.railings, storeys.piers, storeys.bores, storeys.fronts, storeys.triangles.length / 3];
                expect(firsts.every((n, k) => k === 0 || firsts[k - 1] <= n)).toBe(true);
                expect([...new Float32Array(storeys.offsets.buffer)].every(Number.isFinite)).toBe(true);
                expect(mesh.headwalls).toBeLessThanOrEqual(mesh.endFaces);
                // The decks have a top. A bore has its inside and a front over each opening, and its roof's caps name its corners in order.
                const caps = Array.from({ length: storeys.caps.length / 3 }, (_, c) => storeys.caps[3 * c]);
                if (bored) expect([storeys.bores < storeys.roofs, storeys.roofs < storeys.fronts, storeys.fronts < storeys.triangles.length / 3, caps.length > 0, storeys.lights.length > 0]).toEqual([true, true, true, true, true]);
                else expect([storeys.top > 0, storeys.top < storeys.bores]).toEqual([true, true]);
                expect(caps.every((c, k) => c < corners && (k === 0 || caps[k - 1] <= c))).toBe(true);
            } finally {
                input.free();
                plan.free();
            }
        }
    });
});
