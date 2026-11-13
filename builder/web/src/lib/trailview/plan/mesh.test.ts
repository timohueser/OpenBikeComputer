import { readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { cellMesh, meshBytes, SIDE } from './mesh';
import { build, CellRequest, feature_tiles, initSync } from './pkg/obc_ground_plan_bridge.js';

/** Frame units per grid cell of the mesh level, as `params::GRID`. */
const GRID = 1024;

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

    it('reads the storeys after a cell mesh: every index names a storey corner, the top comes first', () => {
        // A cell of the browser fixture with decks over a road.
        const fixture = new URL('../../../../tests/browser/trailview/fixture/features/13/', import.meta.url), [x, y] = [34294, 23098];
        const request = new CellRequest(x, y), tiles = feature_tiles(x, y);
        for (let t = 0; t < tiles.length; t += 2) {
            const bytes = readFileSync(new URL(`${tiles[t]}/${tiles[t + 1]}.bin`, fixture));
            request.add_tile(tiles[t], tiles[t + 1], bytes[0] === 0x1f && bytes[1] === 0x8b ? gunzipSync(bytes) : bytes);
        }
        const plan = build(request, false), input = plan.mesh_input();
        try {
            const storeys = cellMesh(meshBytes(input.mesh(plan.rim())!, input.storeys())).storeys!, corners = storeys.offsets.length;
            expect(storeys.corners.length).toBe(2 * corners);
            expect([...storeys.triangles, ...storeys.slots].every(c => c < corners)).toBe(true);
            expect(storeys.top).toBeGreaterThan(0);
            expect(storeys.top).toBeLessThan(storeys.triangles.length / 3);
            expect([...new Float32Array(storeys.offsets.buffer)].every(Number.isFinite)).toBe(true);
        } finally {
            input.free();
            plan.free();
        }
    });
});
