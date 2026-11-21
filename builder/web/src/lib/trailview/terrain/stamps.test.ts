import { describe, expect, it } from 'vitest';
import { EMPTY_MESHES } from './node-bytes';
import { Stamps, type BakedNodes } from './stamps';
import type { Node } from './lod';
import type { Request } from './stamp-worker';

const WORKER = { ...EMPTY_MESHES, anchors: new ArrayBuffer(4) };
const name = ({ level, x, y }: Node) => `${level}/${x}/${y}`;

/**
 * A stamp worker whose structure meshes hold a 4-byte `anchors` (so they are the worker's). `jobs` are the jobs in the
 * order in which it got them, which is the order in which a worker takes them; `answer` answers the first `count` jobs
 * that wait. Without `held` each job answers at once.
 */
function fakeWorker(held = false) {
    const jobs: string[] = [], waiting: (() => void)[] = [];
    Object.assign(globalThis, {
        Worker: class {
            onmessage?: (event: { data: unknown }) => void;
            postMessage({ id, what, node }: { id: number; what: Request; node: Node }) {
                jobs.push(`${what} ${name(node)}`);
                const stamps = { data: null, heights: false, mask: false, water: null, meshes: null, floors: null, portals: null };
                const answer = () => this.onmessage?.({ data: { id, result: what === 'meshes' ? WORKER : stamps } });
                if (held) waiting.push(answer);
                else queueMicrotask(answer);
            }
            terminate() {}
        },
    });
    return { jobs, answer: (count = waiting.length) => waiting.splice(0, count).forEach(answer => answer()) };
}

const settle = () => new Promise(resolve => setTimeout(resolve, 0));

/** The features of `node` through Stamps with the stamp layer `baked`, once nothing is on its way. */
async function load(node: Node, baked: BakedNodes) {
    const { jobs } = fakeWorker();
    const stamps = new Stamps(async () => new ArrayBuffer(8), () => {}, new AbortController().signal, true, baked);
    expect(stamps.get(node)).toBeUndefined();
    stamps.send(() => 0);
    for (let i = 0; i < 20 && (stamps.loading || !stamps.known(node)); i++) await settle();
    return { features: stamps.known(node), jobs };
}

const L12 = { level: 12, x: 2143, y: 1443 }, L15 = { level: 15, x: 17153, y: 11551 };
const mask = (bits: number) => new Uint8Array(new Uint16Array([bits]).buffer);
const corrupt = new Uint8Array([3, 0, 0, 0, 1]);

describe('stamps from the stamp layer', () => {
    it('come from the worker where a node does not decode', async () => {
        const coarse = await load(L12, async () => corrupt);
        expect(coarse.jobs).toEqual([`stamps ${name(L12)}`]);
        // The mask holds the node (bit 1 + 4 × 3), but its bytes are corrupt: the worker's structure job makes the meshes.
        const owner = await load(L15, async z => (z === 13 ? mask(1 << 13) : corrupt));
        expect(owner.jobs).toEqual([`stamps ${name(L15)}`, `meshes ${name(L15)}`]);
        expect(owner.features?.meshes).toBe(WORKER);
    });

    it('give an owner node that the mask does not hold no meshes, without a request', async () => {
        const asked: number[] = [];
        const owner = await load(L15, async z => (asked.push(z), z === 13 ? mask(1 << 2) : corrupt));
        expect(asked).toEqual([13]);
        expect(owner.jobs).toEqual([`stamps ${name(L15)}`]);
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
        for (let i = 0; i < 40 && (stamps.loading || nodes.some(node => !stamps.known(node))); i++) await settle();
        expect(nodes.filter(node => !stamps.known(node)?.meshes)).toEqual([]);
    });
});

describe('the structure meshes of an owner node', () => {
    const owners = [0, 1, 2].map(i => ({ level: 15, x: L15.x + i, y: L15.y })), near = (i: number) => ({ level: 18, x: 8 * L15.x + i, y: 8 * L15.y });
    /** Stamps on a worker that holds its answers, after a frame that wishes for `nodes` with `rank`. */
    const frame = async (nodes: Node[], rank: (node: Node) => number) => {
        const worker = fakeWorker(true), stamps = new Stamps(async () => new ArrayBuffer(8), () => {}, new AbortController().signal);
        const wish = async (more: Node[], order: (node: Node) => number) => {
            for (const node of more) stamps.get(node);
            stamps.send(order);
            await settle();
        };
        const answer = async (count?: number) => {
            worker.answer(count);
            await settle();
        };
        await wish(nodes, rank);
        return { stamps, jobs: worker.jobs, wish, answer };
    };

    it('go one at a time in the rank of the stamp wishes: before those of a higher rank, after those of a lower one', async () => {
        // The owner nodes hold the eye, so they rank first; twelve wishes of the far field follow, more than go out at once.
        const far = Array.from({ length: 12 }, (_, i) => near(i)), stampsOf = (nodes: Node[]) => nodes.map(node => `stamps ${name(node)}`);
        const { stamps, jobs, wish, answer } = await frame([...owners, ...far], node => (node.level === 15 ? node.x - L15.x : 10));
        expect(jobs).toEqual(stampsOf([...owners, ...far.slice(0, 5)]));
        // The owner nodes' stamps came: the structure job of the first goes before the four far wishes that still wait.
        await answer(3);
        expect(jobs.slice(8).sort()).toEqual([`meshes ${name(owners[0])}`, ...stampsOf(far.slice(5, 8))].sort());
        // The camera moves: nine new wishes rank before the owner nodes, of which the last now ranks first. No other
        // structure job goes while one is on its way, nor while a wish of a lower rank waits.
        const eye = Array.from({ length: 9 }, (_, i) => near(20 + i));
        await wish(eye, node => (node.level === 15 ? L15.x + 2 - node.x : node.x >= 8 * L15.x + 20 ? -1 : 10));
        expect(jobs.length).toBe(12);
        await answer();
        expect(stamps.known(owners[0])?.meshes).toBe(WORKER);
        expect(jobs.slice(12)).toEqual(stampsOf(eye.slice(0, 8)));
        await answer(1);
        expect(jobs.slice(20).sort()).toEqual([`meshes ${name(owners[2])}`, ...stampsOf(eye.slice(8))]);
        await answer();
        expect(jobs.slice(22)).toEqual([`meshes ${name(owners[1])}`]);
        await answer();
        expect(stamps.loading).toBe(false);
        expect(owners.map(node => stamps.known(node)?.meshes)).toEqual([WORKER, WORKER, WORKER]);
    });
});
