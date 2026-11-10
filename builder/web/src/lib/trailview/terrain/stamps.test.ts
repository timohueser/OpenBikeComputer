import { describe, expect, it } from 'vitest';
import { EMPTY_MESHES } from './node-bytes';
import { Stamps, type BakedNodes } from './stamps';
import type { Node } from './lod';

const WORKER = { ...EMPTY_MESHES, anchors: new ArrayBuffer(4) };

/** A stamp worker whose meshes hold a 4-byte `anchors` (so they are the worker's), and the `meshes` of the messages that it got. */
function fakeWorker(): { meshes: boolean[] } {
    const seen = { meshes: [] as boolean[] };
    Object.assign(globalThis, {
        Worker: class {
            onmessage?: (event: { data: unknown }) => void;
            postMessage({ id, meshes }: { id: number; meshes: boolean }) {
                seen.meshes.push(meshes);
                queueMicrotask(() => this.onmessage?.({ data: { id, result: { data: null, heights: false, mask: false, water: null, meshes: meshes ? WORKER : null, floors: null, portals: null } } }));
            }
            terminate() {}
        },
    });
    return seen;
}

/** The features of `node` through Stamps with the stamp layer `baked`. */
async function load(node: Node, baked: BakedNodes) {
    const seen = fakeWorker();
    let done!: () => void;
    const changed = new Promise<void>(resolve => (done = resolve));
    const stamps = new Stamps(async () => new ArrayBuffer(8), () => done(), new AbortController().signal, true, baked);
    expect(stamps.get(node)).toBeUndefined();
    stamps.send(() => 0);
    await changed;
    return { features: stamps.known(node), seen: seen.meshes };
}

const L12 = { level: 12, x: 2143, y: 1443 }, L15 = { level: 15, x: 17153, y: 11551 };
const mask = (bits: number) => new Uint8Array(new Uint16Array([bits]).buffer);
const corrupt = new Uint8Array([3, 0, 0, 0, 1]);

describe('stamps from the stamp layer', () => {
    it('come from the worker where a node does not decode', async () => {
        const coarse = await load(L12, async () => corrupt);
        expect(coarse.seen).toEqual([true]);
        expect(coarse.features?.meshes).toBe(WORKER);
        // The mask holds the node (bit 1 + 4 × 3), but its bytes are corrupt: a second job makes the meshes.
        const owner = await load(L15, async z => (z === 13 ? mask(1 << 13) : corrupt));
        expect(owner.seen).toEqual([false, true]);
        expect(owner.features?.meshes).toBe(WORKER);
    });

    it('give an owner node that the mask does not hold no meshes, without a request', async () => {
        const asked: number[] = [];
        const owner = await load(L15, async z => (asked.push(z), z === 13 ? mask(1 << 2) : corrupt));
        expect(asked).toEqual([13]);
        expect(owner.seen).toEqual([false]);
        expect(owner.features?.meshes).toEqual(EMPTY_MESHES);
    });
});

describe('the stamp requests', () => {
    it('go out as answers come, the last frame\'s wishes by rank, without another frame', async () => {
        fakeWorker();
        const stamps = new Stamps(async () => new ArrayBuffer(8), () => {}, new AbortController().signal);
        // More wishes than requests in flight.
        const nodes = Array.from({ length: 12 }, (_, i) => ({ level: L15.level, x: L15.x + i, y: L15.y }));
        for (const node of nodes) stamps.get(node);
        stamps.send(node => node.x);
        for (let i = 0; i < 20 && nodes.some(node => !stamps.known(node)); i++) await new Promise(resolve => setTimeout(resolve, 0));
        expect(nodes.filter(node => !stamps.known(node))).toEqual([]);
    });
});
