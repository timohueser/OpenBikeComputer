import { describe, expect, it } from 'vitest';
import type { GroundRule, Region } from '../rules/regions';
import { CLUMPED, groundList, groundRules, materialWord, SLOTS } from './plants';

const region = (ground: GroundRule[]) => ({ ground }) as unknown as Region;

describe('the ground plants of a region', () => {
    it('keep the commonest when a pack names more than the list holds, the rarer first', () => {
        const plants = Array.from({ length: SLOTS + 4 }, (_, k) => ({ share: k / (SLOTS + 4) }));
        const list = groundList(plants);
        expect(list).toHaveLength(SLOTS);
        expect(list.map(p => p.share)).toEqual(plants.slice(4).map(p => p.share));
    });

    it('take a pack\'s clumped: false over the model\'s stands, and the salt of their stands from the species alone', () => {
        const [own, plain] = groundRules(region([{ id: 'bilberry', model: 'bilberry' }, { id: 'blaeberry', model: 'bilberry', clumped: false, share: 0.5 }]))
            .sort((a, b) => a.id.localeCompare(b.id));
        expect(own.word & CLUMPED).toBe(CLUMPED);
        expect(plain.word & CLUMPED).toBe(0);
        const alone = groundRules(region([{ id: 'blaeberry', model: 'bilberry', share: 0.9 }]))[0];
        expect(alone.word >>> 16).toBe(plain.word >>> 16);
        expect(own.word >>> 16).not.toBe(plain.word >>> 16);
    });
});

describe('the materials of the plants', () => {
    it('give each its share of its cover\'s shrubs, all of them where it does not say', () => {
        expect(materialWord({ id: 'sand', cover: 'bare', shrubs: 0 }) >>> 16).toBe(0);
        expect(materialWord({ id: 'scrub', cover: 'shrub' }) >>> 16).toBe(255);
        expect(materialWord({ id: 'heath', cover: 'grass', shrubs: 0.5 }) & 0xffff).toBe(2 | 256);
    });
});
