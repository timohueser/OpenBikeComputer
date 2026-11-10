import { readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { FakeCaches } from '../../../../test-support/fake-caches';
import { decodeFeatures } from '../stream/features';
import { floorsOf, portalBoxes, structureMeshes } from '../structures/node';
import { nodeStamps } from './bins';
import { Stamps } from './stamps';
import { waterMesh } from './water-mesh';

const TILE = { level: 13, x: 4286, y: 2888 };
const features = decodeFeatures(new Uint8Array(gunzipSync(readFileSync(new URL(`../../../../tests/browser/trailview/fixture/features/13/${TILE.x}/${TILE.y}.bin`, import.meta.url)))));

/** A stamp worker that makes each job as stamp-worker.ts does, from the fixture's tile. */
class StampWorker {
    static asked = 0;
    onmessage?: (event: { data: unknown }) => void;

    postMessage({ id, node, what }: { id: number; node: typeof TILE; what: string }): void {
        StampWorker.asked++;
        const result = what === 'meshes' ? structureMeshes(features, TILE, node)
            : { ...nodeStamps(features, TILE, node, true), water: waterMesh(features, TILE, node), meshes: null, floors: floorsOf(features, TILE, node), portals: portalBoxes(features, TILE, node) };
        queueMicrotask(() => this.onmessage?.({ data: { id, result } }));
    }

    terminate(): void {}
}

describe('the device cache of stamped nodes', () => {
    afterEach(() => vi.unstubAllGlobals());

    it('gives a node as the worker made it, and misses with another stamp version', async () => {
        vi.stubGlobal('caches', new FakeCaches());
        vi.stubGlobal('Worker', StampWorker);
        // Nodes of the fixture's tile with stamps and a water mesh, at levels 15 and 17.
        const nodes = [{ level: 15, x: 17144, y: 11553 }, { level: 17, x: 68582, y: 46208 }];
        const load = async (version: string) => {
            const stamps = new Stamps(async () => new ArrayBuffer(4), () => {}, new AbortController().signal, true, undefined, { release: 'release', version });
            for (const node of nodes) stamps.get(node);
            stamps.send(() => 0);
            for (let i = 0; i < 50 && (stamps.loading || nodes.some(node => !stamps.known(node))); i++) await new Promise(resolve => setTimeout(resolve, 5));
            // The cache writes after the node arrives.
            await new Promise(resolve => setTimeout(resolve, 20));
            return nodes.map(node => stamps.known(node));
        };
        StampWorker.asked = 0;
        const made = await load('a');
        expect(made.every(node => node?.data && node.water)).toBe(true);
        expect(made.map(node => !!node?.meshes)).toEqual([true, false]);
        const again = await load('a');
        // The stamps of both nodes and the structure meshes of the owner node: the cache holds the owner node whole.
        expect(StampWorker.asked).toBe(3);
        // toStrictEqual compares the classes of the typed arrays and buffers too.
        expect(again).toStrictEqual(made);
        await load('b');
        expect(StampWorker.asked).toBe(6);
    });
});
