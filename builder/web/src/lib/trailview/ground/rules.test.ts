import { describe, expect, it } from 'vitest';
import type { Region, Vocabulary } from '../rules/regions';
import { linear, mosaicsAt, packRulesOn, slots } from './rules';

const vocabulary: Vocabulary = {
    materials: [{ id: 'none', cover: 'grass' }, { id: 'heath', cover: 'grass' }, { id: 'bracken', cover: 'grass' }],
    lithologies: [{ id: 'unknown', hardness: 0.5 }, { id: 'granite', hardness: 1 }],
};
const region: Region = {
    id: 'world', chain: ['world'], leaf: 'mixed', look: {
        snap: 0.4, hazeKm: 35, hazeHeight: 2500, mistTop: 0, softness: 0.08, gradient: 0.35, ramps: {}, outcrops: 0, snow_share: 0.5, turf_deg: 42,
        rock: '#8c877d', sea: { deep: '#2c5874', shallow: '#4f8088' }, grade: { shadow: [0, 0, 0], light: [0, 0, 0], saturation: 1 },
        materials: { bracken: { colour: { autumn: '#c86a1e' } } },
    },
    houses: [0, 0],
    trees: [], herds: [],
    ground: [],
};

describe('the rules of the ground', () => {
    it('lays out the slots that look.wgsl reads', () => {
        const SLOT = slots(vocabulary);
        expect(SLOT.materials).toBe(33);
        expect(SLOT.traits).toBe(33 + 3);
        expect(SLOT.lithologies).toBe(33 + 6);
        expect([SLOT.heather, SLOT.speckle, SLOT.terrain, SLOT.bedding, SLOT.snap].map(slot => slot - SLOT.cliff)).toEqual([1, 2, 3, 4, 5]);
        expect(SLOT.kinds).toBe(SLOT.cliff + 15);
        expect(SLOT.hardness).toBe(SLOT.kinds + 3);
    });

    it('fades in a paint that one season leaves out, at its own colour', () => {
        const SLOT = slots(vocabulary), slot = (data: Float32Array, i: number) => Array.from(data.subarray(4 * i, 4 * i + 4));
        // 15 November: autumn alone. Halfway to 15 February: half autumn, half winter.
        expect(mosaicsAt(318)).toEqual({ quarters: [3, 0], weight: 0 });
        expect(mosaicsAt(0).quarters).toEqual([3, 0]);
        const autumn = slot(packRulesOn([region], vocabulary, 318), SLOT.materials + 2), mid = slot(packRulesOn([region], vocabulary, 1), SLOT.materials + 2);
        expect(autumn.slice(0, 3)).toEqual(linear('#c86a1e').map(c => Math.fround(c)));
        expect(mid[3]).toBeCloseTo(1 - mosaicsAt(1).weight, 5);
        expect(mid.slice(0, 3)).toEqual(autumn.slice(0, 3));
    });

    it('marks heath as moor only in a region with heather', () => {
        const SLOT = slots(vocabulary), moor = (look: Region['look']) => packRulesOn([{ ...region, look }], vocabulary, 180)[4 * (SLOT.kinds + 1) + 1];
        expect(moor(region.look)).toBe(0);
        expect(moor({ ...region.look, moor: { heather: '#5e4552', share: 0.35 } })).toBe(1);
    });

    it("gives each material its clumps, the region's where it names them", () => {
        const SLOT = slots(vocabulary), clumps = (look: Region['look'], v = vocabulary) => packRulesOn([{ ...region, look }], v, 180)[4 * (SLOT.traits + 1) + 1];
        expect(clumps(region.look)).toBe(0);
        const rough = { ...vocabulary, materials: vocabulary.materials.map(m => (m.id === 'heath' ? { ...m, clumps: 0.75 } : m)) };
        expect(clumps(region.look, rough)).toBe(0.75);
        expect(clumps({ ...region.look, materials: { heath: { clumps: 0.25 } } }, rough)).toBe(0.25);
    });
});
