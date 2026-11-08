import { afterEach, describe, expect, it, vi } from 'vitest';
import { FakeCaches } from '../../../../test-support/fake-caches';
import type { Instances, ModelSpec } from '../instances/instances';
import type { ModelAnswer, ModelJob } from './model-worker';
import { TreeModels } from './tree-models';

// Workers that keep their jobs, which the test answers.
const jobs: { job: ModelJob; answer: (data: Omit<ModelAnswer, 'id'>) => void }[] = [];
class FakeWorker {
    onmessage?: (event: { data: ModelAnswer }) => void;
    postMessage(job: ModelJob) {
        jobs.push({ job, answer: data => this.onmessage!({ data: { id: job.id, ...data } }) });
    }
}
const built = (crown: number) => ({ tiers: [], crown });

function trees(near = new Map<string, number>(), version?: string) {
    vi.stubGlobal('Worker', FakeWorker);
    jobs.length = 0;
    const specs: ModelSpec[] = [];
    const instances = { add: (spec: ModelSpec) => specs.push(spec) - 1, replace: (m: number, spec: ModelSpec) => (specs[m] = spec), day: 0 };
    const models = new TreeModels(instances as unknown as Instances, () => {}, () => near, new AbortController().signal, version);
    return { models, specs, beech: models.index('beech')! };
}

describe('the tree models', () => {
    afterEach(() => (vi.unstubAllGlobals(), vi.restoreAllMocks()));

    it('draws a winter start bare from the summer builds', () => {
        const { models, specs, beech } = trees();
        models.update('2026-01-20');
        expect(jobs.map(({ job }) => job)).toEqual([{ id: 0, leafy: [{ model: 'beech', evergreen: false }], crowns: [] }]);
        // The summer build of beech and its winter crown both are `beech|0`.
        jobs[0].answer({ leafy: [{ ...built(0.1), tones: [], own: [true] }], crowns: [] });
        expect(jobs[1].job).toEqual({ id: 1, leafy: [], crowns: [{ model: 'beech', phase: 0 }] });
        jobs[1].answer({ leafy: [], crowns: [built(0.2)] });
        models.update('2026-01-20');
        const winter = specs[beech].phases![0].model;
        expect([specs[beech].crown, winter === beech ? undefined : specs[winter].crown, jobs.length]).toEqual([0.1, 0.2, 2]);
    });

    it('gives the free workers the species with the most trees about the eye first', () => {
        const { models } = trees(new Map([['larch', 0.2], ['spruce', 0.7]]));
        for (const model of ['beech', 'oak', 'larch', 'spruce']) models.index(model);
        models.update('2026-07-01');
        expect(jobs.map(({ job }) => job.leafy[0].model)).toEqual(['spruce', 'larch', 'beech']);
        jobs[1].answer({ leafy: [{ ...built(0.1), tones: [], own: [] }], crowns: [] });
        expect(jobs.map(({ job }) => job.leafy[0].model)).toEqual(['spruce', 'larch', 'beech', 'oak']);
    });

    it('asks again for the models of a job that failed', () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const { models } = trees();
        models.update('2026-01-20');
        jobs[0].answer({ error: 'lost' });
        models.update('2026-07-01');
        expect(jobs.map(({ job }) => job.leafy)).toEqual([[{ model: 'beech', evergreen: false }], [{ model: 'beech', evergreen: false }]]);
    });

    it('takes a build from the device cache as the worker made it, and builds again for another version', async () => {
        vi.stubGlobal('caches', new FakeCaches());
        const tier = (k: number) => ({ position: Float32Array.of(k, 0.5, 1 / 3), normal: Float32Array.of(0, 0, 1), color: Float32Array.of(0.1, 0.2, k), index: Uint32Array.of(0, 0, 0), trunk: k });
        // A summer crown with gaps in its tones and in its phases of their own, as a worker makes it.
        const leafy = { tiers: [tier(1), tier(2), tier(3)], crown: 0.125, tones: [[1, 0, 0, 0, 1, 0, 0, 0, 1.5], undefined, undefined, [0.5, 0, 0, 0, 1, 0, 0, 0, 1]], own: [true, false, undefined, false] as boolean[] };
        const load = async (version: string) => {
            const { models, specs, beech } = trees(undefined, version);
            models.update('2026-08-15');
            for (let i = 0; i < 50 && !jobs.length && !models.idle; i++) await new Promise(resolve => setTimeout(resolve, 2));
            jobs[0]?.answer({ leafy: [leafy], crowns: [] });
            // The cache writes after the build lands.
            await new Promise(resolve => setTimeout(resolve, 20));
            models.update('2026-08-15');
            return { asked: jobs.length, spec: specs[beech] };
        };
        const made = await load('a'), again = await load('a'), other = await load('b');
        expect([made.asked, again.asked, other.asked, made.spec.tiers.length]).toEqual([1, 0, 1, 3]);
        // toStrictEqual compares the classes of the typed arrays too.
        expect(again.spec).toStrictEqual(made.spec);
    });
});
