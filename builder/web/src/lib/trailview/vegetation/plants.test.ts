import { describe, expect, it } from 'vitest';
import type { Instances, ModelSpec } from '../instances/instances';
import type { ModelFiles } from '../instances/model-files';
import type { Packed } from '../models/model-file';
import { PLANTS } from '../models/plant-library';
import type { GroundRule, Region, Rules, TreeRule, Vocabulary } from '../rules/regions';
import { CLUMPED, groundList, groundRules, materialWord, Plants, SEEDS, SLOTS } from './plants';
import type { TreeModels } from './tree-models';

const region = (ground: GroundRule[]) => ({ ground }) as unknown as Region;
const plants = Object.fromEntries(Object.entries(PLANTS).map(([id, { ground }]) => [id, { ground, seasons: {} }]));
const vocabulary: Vocabulary = {
    materials: [{ id: 'none', cover: 'grass' }, { id: 'heath', cover: 'grass', tags: ['moor'] }, { id: 'scrub', cover: 'shrub' }],
    lithologies: [{ id: 'unknown', hardness: 0.5 }],
};

describe('the ground plants of a region', () => {
    it('keep the commonest when a pack names more than the list holds, the rarer first', () => {
        const plants = Array.from({ length: SLOTS + 4 }, (_, k) => ({ share: k / (SLOTS + 4) }));
        const list = groundList(plants);
        expect(list).toHaveLength(SLOTS);
        expect(list.map(p => p.share)).toEqual(plants.slice(4).map(p => p.share));
    });

    it('take a pack\'s clumped: false over the model\'s stands, and the salt of their stands from the species alone', () => {
        const [own, plain] = groundRules(region([{ id: 'bilberry', model: 'bilberry' }, { id: 'blaeberry', model: 'bilberry', clumped: false, share: 0.5 }]), vocabulary, plants)
            .sort((a, b) => a.id.localeCompare(b.id));
        expect(own.word & CLUMPED).toBe(CLUMPED);
        expect(plain.word & CLUMPED).toBe(0);
        const alone = groundRules(region([{ id: 'blaeberry', model: 'bilberry', share: 0.9 }]), vocabulary, plants)[0];
        expect(alone.word >>> 16).toBe(plain.word >>> 16);
        expect(own.word >>> 16).not.toBe(plain.word >>> 16);
    });

    it('grow on the materials that their on names but their off does not', () => {
        const [heather] = groundRules(region([{ id: 'heather', model: 'heather', on: ['#moor', 'shrub'] }]), vocabulary, plants);
        const [grass] = groundRules(region([{ id: 'grass', model: 'grass', off: ['#moor'] }]), vocabulary, plants);
        expect(heather.materials).toEqual([0b110, 0]);
        expect(grass.materials).toEqual([0b001, 0]);
    });
});

describe('the materials of the plants', () => {
    it('give each its share of its cover\'s shrubs, all of them where it does not say', () => {
        expect(materialWord({ id: 'sand', cover: 'bare', shrubs: 0 }) >>> 16).toBe(0);
        expect(materialWord({ id: 'scrub', cover: 'shrub' }) >>> 16).toBe(255);
        expect(materialWord({ id: 'heath', cover: 'grass', shrubs: 0.5 })).toBe(2 | (128 << 16));
    });
});

describe('the plant models', () => {
    it('give a plant in rows a run of its own, the same each time, not a shrub of the same plant', () => {
        const specs: ModelSpec[] = [];
        const instances = { produce: () => {}, add: (...added: ModelSpec[]) => specs.push(...added) - added.length };
        const shrub: TreeRule = { model: 'grapevine', leaf: 'broadleaved', cover: 'shrub', elevation: [0, 1000], weight: 1 };
        const rules = { vocabulary, regions: [{ ground: [], trees: [shrub] }], styles: [] } as unknown as Rules;
        const models = new Plants(rules, {} as TreeModels, instances as unknown as Instances, { models: { plants, trees: {} } } as unknown as ModelFiles);
        expect(specs).toHaveLength(1);
        const first = models.rowModel(0, 'grapevine');
        expect(first).toBe(1);
        expect(models.rowModel(0, 'grapevine')).toBe(first);
        expect(specs).toHaveLength(1 + SEEDS);
    });

    it('take the shapes of the season that a file brings, and count a file that failed as done until the season changes', async () => {
        const files: { url: string; answer: (models: Packed[] | undefined) => Promise<void> }[] = [];
        const load = (url: string) => new Promise<Packed[] | undefined>(resolve => files.push({ url, answer: models => (resolve(models), new Promise(done => setTimeout(done))) }));
        const specs: ModelSpec[] = [];
        const instances = { produce: () => {}, add: (...added: ModelSpec[]) => specs.push(...added) - added.length, replace: (m: number, spec: ModelSpec) => (specs[m] = spec) };
        const rules = { vocabulary, regions: [{ ground: [{ id: 'grass', model: 'grass' }], trees: [] }], styles: [] } as unknown as Rules;
        const models = { plants: { grass: { ground: PLANTS.grass.ground, seasons: { summer: 'grass/summer', winter: 'grass/winter' } } }, trees: {} };
        const plants = new Plants(rules, {} as TreeModels, instances as unknown as Instances, { models, load } as unknown as ModelFiles);
        const reach = { reach: new Set([0]), near: new Set([0]) }, shape = (k: number) => ({ tiers: [{ vertices: new Uint8Array(16 * k), index: new Uint32Array(3) }], shape: {} }) as unknown as Packed;
        plants.update([8.4, 46.8], '2026-07-01', 0, reach);
        expect(files.map(({ url }) => url)).toEqual(['grass/summer', 'grass/summer', 'grass/summer']);
        // A file of a season that passed while it loaded changes nothing.
        plants.update([8.4, 46.8], '2026-01-20', 0, reach);
        await Promise.all(files.slice(0, 3).map(file => file.answer([shape(1), shape(2), shape(3)])));
        expect(specs.every(spec => !spec.tiers.length)).toBe(true);
        await Promise.all(files.slice(3).map(file => file.answer(undefined)));
        expect([plants.idle, plants.nearIdle]).toEqual([true, true]);
        plants.update([8.4, 46.8], '2026-07-01', 0, reach);
        expect(files.length).toBe(9);
    });

    it('draw nothing of a model whose file for the new season failed', async () => {
        const answers: ((models: Packed[] | undefined) => void)[] = [];
        const load = () => new Promise<Packed[] | undefined>(resolve => answers.push(resolve));
        const specs: ModelSpec[] = [];
        const instances = { produce: () => {}, add: (...added: ModelSpec[]) => specs.push(...added) - added.length, replace: (m: number, spec: ModelSpec) => (specs[m] = spec) };
        const rules = { vocabulary, regions: [{ ground: [{ id: 'grass', model: 'grass' }], trees: [] }], styles: [] } as unknown as Rules;
        const models = { plants: { grass: { ground: PLANTS.grass.ground, seasons: { summer: 'grass/summer', winter: 'grass/winter' } } }, trees: {} };
        const plants = new Plants(rules, {} as TreeModels, instances as unknown as Instances, { models, load } as unknown as ModelFiles);
        const reach = { reach: new Set([0]), near: new Set([0]) }, shape = { tiers: [{ vertices: new Uint8Array(16), index: new Uint32Array(3) }], shape: {} } as unknown as Packed;
        plants.update([8.4, 46.8], '2026-07-01', 0, reach);
        answers.splice(0).forEach(answer => answer([shape, shape, shape]));
        await new Promise(resolve => setTimeout(resolve));
        expect(specs.every(spec => spec.tiers.length === 1)).toBe(true);
        plants.update([8.4, 46.8], '2026-01-20', 0, reach);
        answers.splice(0).forEach(answer => answer(undefined));
        await new Promise(resolve => setTimeout(resolve));
        expect([specs.every(spec => !spec.tiers.length), plants.nearIdle]).toEqual([true, true]);
    });
});
