import { readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { decodeFeatures } from '../stream/features';
import { OWNER, size, type Node } from '../terrain/lod';
import type { Stamps } from '../terrain/stamps';
import { STRUCTURE_VERTEX, type StructureMesh } from './mesh';
import { structureMeshes, type NodeMeshes } from './node';
import { ownRange, Structures } from './structures';

/** The faces of a mesh that the structures' pipeline draws, each as the words of its vertices, sorted. */
function faces(mesh: StructureMesh, plan: boolean): string[] {
    const words = new Uint32Array(mesh.vertices), { skip, count } = ownRange(mesh, plan), out: string[] = [];
    const vertex = (i: number) => words.subarray(STRUCTURE_VERTEX * i, STRUCTURE_VERTEX * (i + 1)).join(',');
    for (let k = skip; k < skip + count; k += 3) out.push([0, 1, 2].map(j => vertex(mesh.indices[k + j])).sort().join('|'));
    return out.sort();
}

describe('the structures', () => {
    it('give the pools the meshes of the nearest owner first, whatever node the terrain draws first', () => {
        const meshes = new Map<number, NodeMeshes>(), side = size(OWNER);
        const stamps = {
            get: ({ x }: Node) => {
                if (!meshes.has(x)) meshes.set(x, { far: { structures: { vertices: new ArrayBuffer(4), indices: new Uint32Array(1) }, anchored: null }, near: { structures: null, anchored: null }, anchors: null, points: null });
                return { meshes: meshes.get(x) };
            },
        } as unknown as Stamps;
        const structures = new Structures(stamps);
        // The eye stands in owner 100's square. The reach of the near casters is tiny at this unit: only that square is added.
        const node = (x: number): Node => ({ level: OWNER, x, y: 0 });
        structures.update([node(120), node(103), node(100)], { x: 100.5 * side, y: 0.5 * side, unit: 1e6 });
        const order = (structures as unknown as { draws: unknown[] }).draws.map(mesh => [...meshes].find(([, m]) => m.far.structures === mesh)?.[0]);
        expect(order).toEqual([100, 103, 120]);
    });

    it('draw the parts only with the ground plan, and a bridge only without it', () => {
        const mesh = { vertices: new ArrayBuffer(0), indices: new Uint32Array(30), planOnly: 6, decks: 9, offPlan: 3 };
        expect(ownRange(mesh, false)).toEqual({ skip: 6, count: 24 });
        expect(ownRange(mesh, true)).toEqual({ skip: 0, count: 18 });
    });

    it('give the app without the ground plan the same faces from the meshes that the ground plan makes', () => {
        // The fixture's owner node with a rail tunnel whose chain draws as parts.
        const tile = { level: 13, x: 4287, y: 2888 }, node = { level: OWNER, x: 17150, y: 11555 };
        const list = decodeFeatures(new Uint8Array(gunzipSync(readFileSync(new URL(`../../../../tests/browser/trailview/fixture/features/13/${tile.x}/${tile.y}.bin`, import.meta.url)))));
        const today = structureMeshes(list, tile, node, false).far.structures!, plan = structureMeshes(list, tile, node, true).far.structures!;
        expect(today.planOnly).toBeUndefined();
        expect(plan.planOnly).toBeGreaterThan(0);
        expect(faces(plan, false)).toEqual(faces(today, false));
    });
});
