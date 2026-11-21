import { describe, expect, it } from 'vitest';
import { EMPTY_MESHES } from './node-bytes';
import { Stamps, type BakedNodes } from './stamps';
import { stampRank } from './terrain';
import type { Node } from './lod';
import type { Request } from './stamp-worker';

const WORKER = { ...EMPTY_MESHES, anchors: new ArrayBuffer(4) };
const name = ({ level, x, y }: Node) => `${level}/${x}/${y}`;

/**
 * A stamp worker whose structure meshes hold a 4-byte `anchors` (so they are the worker's). `jobs` are the jobs in the
 * order in which it got them, which is the order in which a worker takes them; `answer` answers the first `count` jobs
 * that wait, `answerJob` the job named `job`. Without `held` each job answers at once.
 */
function fakeWorker(held = false) {
    const jobs: string[] = [], waiting: { job: string; answer: () => void }[] = [];
    Object.assign(globalThis, {
        Worker: class {
            onmessage?: (event: { data: unknown }) => void;
            postMessage({ id, what, node }: { id: number; what: Request; node: Node }) {
                const job = `${what} ${name(node)}`;
                jobs.push(job);
                const stamps = { data: null, heights: false, mask: false, water: null, meshes: null, floors: null, portals: null };
                const answer = () => this.onmessage?.({ data: { id, result: what === 'meshes' ? WORKER : stamps } });
                if (held) waiting.push({ job, answer });
                else queueMicrotask(answer);
            }
            terminate() {}
        },
    });
    return {
        jobs,
        answer: (count = waiting.length) => waiting.splice(0, count).forEach(({ answer }) => answer()),
        answerJob: (job: string) => waiting.splice(waiting.findIndex(held => held.job === job), 1)[0].answer(),
    };
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
        const answerJob = async (job: string) => {
            worker.answerJob(job);
            await settle();
        };
        await wish(nodes, rank);
        return { stamps, jobs: worker.jobs, wish, answer, answerJob };
    };

    it('go one at a time after the wishes about the eye and before the farther ones, and pass no wish of a lower rank on its way', async () => {
        // In the terrain's rank: the owner nodes hold the eye, so their stamps are first, and their structure jobs come
        // after every first wish. Twelve far wishes follow, more than go out at once; then nine wishes about the eye.
        const far = Array.from({ length: 12 }, (_, i) => near(i)), eye = Array.from({ length: 9 }, (_, i) => near(20 + i)), about = new Set(eye.map(name));
        const stampsOf = (nodes: Node[]) => nodes.map(node => `stamps ${name(node)}`), meshesOf = (node: Node) => `meshes ${name(node)}`;
        const rank = (owner: (node: Node) => number) => (node: Node, meshes?: boolean) =>
            node.level === 15 ? stampRank(true, owner(node), meshes) : stampRank(about.has(name(node)), about.has(name(node)) ? 5 : 10, meshes);
        const { stamps, jobs, wish, answer } = await frame([...owners, ...far], rank(node => node.x - L15.x));
        expect(jobs).toEqual(stampsOf([...owners, ...far.slice(0, 5)]));
        // The owner nodes' stamps came: the structure job of the first goes before the far wishes that it outranks.
        await answer(3);
        expect(jobs.slice(8)).toEqual([meshesOf(owners[0]), ...stampsOf(far.slice(5, 8))]);
        // The camera moves: the wishes about the eye come, and the last owner node ranks first. No other structure job
        // goes while one is on its way, nor while a wish about the eye waits or is on its way.
        await wish(eye, rank(node => L15.x + 2 - node.x));
        expect(jobs.length).toBe(12);
        await answer();
        expect(stamps.known(owners[0])?.meshes).toBe(WORKER);
        expect(jobs.slice(12)).toEqual(stampsOf(eye.slice(0, 8)));
        await answer(1);
        expect(jobs.slice(20)).toEqual(stampsOf(eye.slice(8)));
        await answer();
        expect(jobs.slice(21)).toEqual([meshesOf(owners[2])]);
        await answer();
        expect(jobs.slice(22)).toEqual([meshesOf(owners[1])]);
        await answer();
        expect(stamps.loading).toBe(false);
        expect(owners.map(node => stamps.known(node)?.meshes)).toEqual([WORKER, WORKER, WORKER]);
    });

    it('wait for a wish about the eye that waits for a free request', async () => {
        // Two owner nodes hold the eye. Their stamps and the requests of eight far wishes fill the worker.
        const far = Array.from({ length: 8 }, (_, i) => near(i)), eye = near(20);
        const rank = (node: Node, meshes?: boolean) =>
            node.level === 15 ? stampRank(true, node.x - L15.x, meshes) : stampRank(node.x === eye.x, node.x === eye.x ? 5 : 10, meshes);
        const { jobs, wish, answer, answerJob } = await frame([...owners.slice(0, 2), ...far], rank);
        await answer(2);
        expect(jobs.length).toBe(11);
        expect(jobs).toContain(`meshes ${name(owners[0])}`);
        // A frame wishes for a node about the eye while every request is far. When the first job answers, the second
        // waits for that wish, not only for the requests on their way.
        await wish([eye], rank);
        await answerJob(`meshes ${name(owners[0])}`);
        expect(jobs.length).toBe(11);
        await answer(1);
        expect(jobs.slice(11)).toEqual([`stamps ${name(eye)}`]);
        await answer();
        expect(jobs.slice(12)).toEqual([`meshes ${name(owners[1])}`]);
    });
});
