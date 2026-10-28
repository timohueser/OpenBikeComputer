import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Instances, ModelSpec } from '../instances/instances';
import type { GroundRule, Region, Rules, TreeRule, Vocabulary } from '../rules/regions';
import { CLUMPED, groundList, groundRules, materialWord, Plants, SEEDS, SLOTS } from './plants';
import type { TreeModels } from './tree-models';

const region = (ground: GroundRule[]) => ({ ground }) as unknown as Region;
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
        const [own, plain] = groundRules(region([{ id: 'bilberry', model: 'bilberry' }, { id: 'blaeberry', model: 'bilberry', clumped: false, share: 0.5 }]), vocabulary)
            .sort((a, b) => a.id.localeCompare(b.id));
        expect(own.word & CLUMPED).toBe(CLUMPED);
        expect(plain.word & CLUMPED).toBe(0);
        const alone = groundRules(region([{ id: 'blaeberry', model: 'bilberry', share: 0.9 }]), vocabulary)[0];
        expect(alone.word >>> 16).toBe(plain.word >>> 16);
        expect(own.word >>> 16).not.toBe(plain.word >>> 16);
    });

    it('grow on the materials that their on names but their off does not', () => {
        const [heather] = groundRules(region([{ id: 'heather', model: 'heather', on: ['#moor', 'shrub'] }]), vocabulary);
        const [grass] = groundRules(region([{ id: 'grass', model: 'grass', off: ['#moor'] }]), vocabulary);
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
    afterEach(() => vi.unstubAllGlobals());

    it('give a plant in rows a run of its own, the same each time, not a shrub of the same plant', () => {
        vi.stubGlobal('Worker', class { postMessage() {} });
        const specs: ModelSpec[] = [];
        const instances = { produce: () => {}, add: (...added: ModelSpec[]) => specs.push(...added) - added.length };
        const shrub: TreeRule = { model: 'grapevine', leaf: 'broadleaved', cover: 'shrub', elevation: [0, 1000], weight: 1 };
        const rules = { vocabulary, regions: [{ ground: [], trees: [shrub] }], styles: [] } as unknown as Rules;
        const plants = new Plants(rules, {} as TreeModels, instances as unknown as Instances, () => {}, new AbortController().signal);
        expect(specs).toHaveLength(1);
        const first = plants.rowModel('grapevine');
        expect(first).toBe(1);
        expect(plants.rowModel('grapevine')).toBe(first);
        expect(specs).toHaveLength(1 + SEEDS);
    });
});
