import { describe, expect, it } from 'vitest';
import { OWNER, size, type Node } from '../terrain/lod';
import type { Stamps } from '../terrain/stamps';
import type { NodeMeshes } from './node';
import { Structures } from './structures';

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
        // The eye stands far from the squares about it that cast into the near shadow map: in owner 100's square.
        const node = (x: number): Node => ({ level: OWNER, x, y: 0 });
        structures.update([node(120), node(103), node(100)], { x: 100.5 * side, y: 0.5 * side, unit: 1e6 });
        const order = (structures as unknown as { draws: unknown[] }).draws.map(mesh => [...meshes].find(([, m]) => m.far.structures === mesh)?.[0]);
        expect(order).toEqual([100, 103, 120]);
    });
});
