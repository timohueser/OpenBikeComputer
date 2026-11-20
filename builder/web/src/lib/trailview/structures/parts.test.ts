import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { DEEPEST, unitScale, type Feature } from '../stream/features';
import { metresPerUnit, RULES, SLEEPER_M, worldPoint } from '../terrain/bins';
import { OWNER } from '../terrain/lod';
import { partInput, partMeshes, partVolumes } from './parts';
import { STRUCTURE_VERTEX } from './mesh';
import { VOID_M } from './tunnels';

const table = JSON.parse(readFileSync(new URL('../../../../../../data/trailview/parts.json', import.meta.url), 'utf8'));

/** A zoom-13 tile in the Alps, its owner node about a point, and metres per tile unit there. */
const tile = { level: DEEPEST, x: 4287, y: 2887 }, at = [6000, 6000];
const unit = unitScale(DEEPEST) * metresPerUnit(worldPoint(tile, at, 0)[1]);
const point = (east: number) => [Math.round(at[0] + east / unit), at[1]];
const owner = (() => {
    const [x, y] = worldPoint(tile, at, 0), side = 2 ** (30 - OWNER);
    return { level: OWNER, x: Math.floor(x / side), y: Math.floor(y / side) };
})();

/** The crate's test end: an arch into a road tube that runs 40 m east, and its chords. */
function records(): Feature[] {
    const ends = 'arch arch', floor = [[0, 500], [4, 499.9], [8, 500.1], [12, 500.4], [16, 500.6], [24, 501], [40, 501.5]];
    const tube = (k: number): Feature => ({
        kind: 'tunnel-tube', geometry: 'line',
        tags: { section: 'round', half: '4.5', height: '5', look: 'concrete', class: 'primary', road: '3.5', along: `${floor[k][0]}`, ends },
        parts: [{ outer: false, xy: Int32Array.from([...point(floor[k][0]), ...point(floor[k + 1][0])]), z: Float32Array.from([floor[k][1], floor[k + 1][1]]) }],
    });
    const end: Feature = {
        kind: 'tunnel-end', geometry: 'point',
        tags: { part: 'arch', heading: '0', section: 'round', half: '4.5', height: '5', look: 'concrete', collar: '6', face: '7.1', left: '6 4 2', right: '3', foot: '-2', way: '0 0', shift: '0' },
        parts: [{ outer: false, xy: Int32Array.from(point(0)), z: Float32Array.from([500]) }],
    };
    return [end, ...floor.slice(1).map((_, k) => tube(k))];
}

describe('the parts', () => {
    it('hold the values of parts.json that the viewer uses too', () => {
        const { portal_cells, mesh_texels, sleeper_m, void_m, lift_m } = table.design;
        expect([portal_cells, mesh_texels, sleeper_m, void_m, lift_m]).toEqual([RULES.PORTAL_CELLS, RULES.MESH_TEXELS, SLEEPER_M, VOID_M, RULES.LIFT_M]);
    });

    it('give the box and the mesh of an arch from the records of its feature tile', () => {
        const input = partInput(records(), tile, owner), { boxes, floors } = partVolumes(input);
        const [x, y] = worldPoint(tile, point(0), 0);
        expect(boxes.length).toBe(9);
        expect([...boxes.slice(0, 4)]).toEqual([x, y, 1, 0]);
        expect(boxes[5]).toBeCloseTo(4.5 + table.bake.shell_m, 6);
        expect(boxes[7]).toBeCloseTo(500 + table.design.lift_m, 6);
        expect(floors.length).toBe(6 * 7);
        const mesh = partMeshes(input)!;
        expect(mesh.vertices.byteLength % (4 * STRUCTURE_VERTEX)).toBe(0);
        expect(Math.max(...mesh.indices)).toBeLessThan(mesh.vertices.byteLength / (4 * STRUCTURE_VERTEX));
    });
});
