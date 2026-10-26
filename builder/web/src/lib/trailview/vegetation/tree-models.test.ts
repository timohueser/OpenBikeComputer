import { afterEach, describe, expect, it, vi } from 'vitest';
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

function trees(near = new Map<string, number>()) {
    vi.stubGlobal('Worker', FakeWorker);
    jobs.length = 0;
    const specs: ModelSpec[] = [];
    const instances = { add: (spec: ModelSpec) => specs.push(spec) - 1, replace: (m: number, spec: ModelSpec) => (specs[m] = spec), day: 0 };
    const models = new TreeModels(instances as unknown as Instances, () => {}, () => near, new AbortController().signal);
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
});
