import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { cellMesh, SIDE } from './mesh';
import { build, CellRequest, initSync } from './pkg/obc_ground_plan_bridge.js';

/** Frame units per grid cell of the mesh level, as `params::GRID`. */
const GRID = 1024;

describe('the mesh reader', () => {
    it('reads a real cell: every index names a corner, and no terrain vertex sinks while points inside R do', () => {
        initSync({ module: readFileSync(new URL('./pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url)) });
        // The north-west cell of zoom-13 tile 4286/2888, the tile of the vector: its roads lie there.
        const request = new CellRequest(4286 * 8, 2888 * 8);
        request.add_tile(4286, 2888, readFileSync(new URL('../../../../../../specs/vectors/trailview-features.bin', import.meta.url)));
        const plan = build(request), input = plan.mesh_input();
        try {
            const mesh = cellMesh(input.mesh(plan.rim())!.buffer as ArrayBuffer), corners = mesh.corners.length / 2;
            expect(mesh.triangles.length).toBeGreaterThan(0);
            expect(mesh.triangles.every(k => k < corners) && mesh.steps.every(k => k < corners)).toBe(true);
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
            request.free();
        }
    });
});
