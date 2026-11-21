import { describe, expect, it } from 'vitest';
import type { Instances, ModelSpec } from '../instances/instances';
import type { ModelFiles } from '../instances/model-files';
import type { Models, Packed, TreeEntry } from '../models/model-file';
import { TreeModels, type Reach } from './tree-models';

/** A species whose crown of its own in winter (phase 0) is a file, or none. */
const species = (bare: boolean): TreeEntry => ({ summer: 'summer', keepsHeading: false, tones: [], crowns: [bare ? 'winter' : null, null, null, null] });
const built = (crown: number): Packed => ({ tiers: [], shape: { extent: 1, radius: 1, spread: 1, trunk: false, crown, sinks: [], outlines: new Float32Array(256) } });

/** The loads that the files started, which a test answers: a model, or undefined for a file that failed. */
const loads: { file: string; answer: (model: Packed | undefined) => Promise<void> }[] = [];
const files = (trees: Record<string, TreeEntry>) => ({
    models: { trees: Object.fromEntries(Object.entries(trees).map(([model, entry]) => [model, { ...entry, summer: `${model}/summer`, crowns: entry.crowns.map(crown => crown && `${model}/${crown}`) }])) } as unknown as Models,
    load: (file: string) => new Promise<Packed[] | undefined>(resolve => loads.push({ file, answer: model => (resolve(model && [model]), new Promise(done => setTimeout(done))) })),
}) as unknown as ModelFiles;

function trees(kinds: Record<string, TreeEntry>, near = new Map<string, number>(), reach: Reach = { reach: new Set([0]), near: new Set([0]) }) {
    loads.length = 0;
    const specs: ModelSpec[] = [];
    const instances = { add: (spec: ModelSpec) => specs.push(spec) - 1, replace: (m: number, spec: ModelSpec) => (specs[m] = spec), day: 0 };
    const models = new TreeModels(instances as unknown as Instances, () => near, files(kinds), 3);
    return { models, specs, reach, beech: models.index(0, 'beech')! };
}

describe('the tree models', () => {
    it('draws a winter start bare from the summer crown, then from its own crown', async () => {
        const { models, specs, reach, beech } = trees({ beech: species(true) });
        models.update('2026-01-20', reach);
        expect(loads.map(({ file }) => file)).toEqual(['beech/summer']);
        await loads[0].answer(built(0.1));
        expect(loads[1].file).toBe('beech/winter');
        await loads[1].answer(built(0.2));
        models.update('2026-01-20', reach);
        const winter = specs[beech].phases![0].model;
        expect([specs[beech].shape?.crown, winter === beech ? undefined : specs[winter].shape?.crown, loads.length]).toEqual([0.1, 0.2, 2]);
    });

    it('loads the species with the most trees about the eye first, a few at once', async () => {
        const { models, reach } = trees({ beech: species(false), oak: species(false), larch: species(false), spruce: species(false) }, new Map([['larch', 0.2], ['spruce', 0.7]]));
        for (const model of ['oak', 'larch', 'spruce']) models.index(0, model);
        models.update('2026-07-01', reach);
        expect(loads.map(({ file }) => file)).toEqual(['spruce/summer', 'larch/summer', 'beech/summer']);
        await loads[1].answer(built(0.1));
        expect(loads.map(({ file }) => file)).toEqual(['spruce/summer', 'larch/summer', 'beech/summer', 'oak/summer']);
    });

    it('loads only the species of the regions in reach, those of the near regions first, and is near idle once these are in', async () => {
        const reach = { reach: new Set([0, 1]), near: new Set([1]) }, { models } = trees({ beech: species(false), larch: species(false), spruce: species(false), oak: species(false) }, new Map([['beech', 0.9]]), reach);
        models.index(1, 'larch');
        models.index(1, 'spruce');
        models.index(2, 'oak');
        models.update('2026-07-01', reach);
        expect(loads.map(({ file }) => file)).toEqual(['larch/summer', 'spruce/summer', 'beech/summer']);
        await loads[0].answer(built(0.1));
        await loads[1].answer(built(0.1));
        models.update('2026-07-01', reach);
        expect([models.nearIdle, models.idle, loads.length]).toEqual([true, false, 3]);
        models.update('2026-07-01', { ...reach, reach: new Set([0, 1, 2]) });
        expect(loads.map(({ file }) => file)).toEqual(['larch/summer', 'spruce/summer', 'beech/summer', 'oak/summer']);
    });

    it('is near idle in winter only once the near trees draw their own bare crowns', async () => {
        const reach = { reach: new Set([0, 1]), near: new Set([0]) }, { models, specs, beech } = trees({ beech: species(true), oak: species(false) }, new Map(), reach);
        models.index(1, 'oak');
        models.update('2026-01-20', reach);
        expect(loads.map(({ file }) => file)).toEqual(['beech/summer', 'oak/summer']);
        await loads[0].answer(built(0.1));
        await loads[2].answer(built(0.2));
        models.update('2026-01-20', reach);
        // Loaded, but the far species' summer crown still loads: the bare crown has no instance model yet.
        expect([models.nearIdle, specs[beech].phases![0].model === beech]).toEqual([false, true]);
        await loads[1].answer(built(0.3));
        models.update('2026-01-20', reach);
        expect([models.nearIdle, specs[specs[beech].phases![0].model].shape?.crown]).toEqual([true, 0.2]);
    });

    it('counts a file that failed as done, and draws the bare crowns of the others once it fails', async () => {
        const { models, specs, reach, beech } = trees({ beech: species(true), oak: species(false) });
        models.index(0, 'oak');
        models.update('2026-01-20', reach);
        await loads[0].answer(built(0.1));
        await loads[2].answer(built(0.2));
        models.update('2026-01-20', reach);
        expect(specs[beech].phases![0].model).toBe(beech);
        await loads[1].answer(undefined);
        models.update('2026-01-20', reach);
        models.update('2026-07-01', reach);
        models.update('2026-01-20', reach);
        expect([loads.length, models.idle, models.nearIdle, specs[specs[beech].phases![0].model].shape?.crown]).toEqual([3, true, true, 0.2]);
    });
});
