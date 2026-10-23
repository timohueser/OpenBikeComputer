import { describe, expect, it } from 'vitest';
import { rulesOf } from './regions';

const materials = { materials: [{ id: 'none', cover: 'grass' }], lithologies: [{ id: 'unknown', hardness: 0.5 }] };
const look = { snap: 0.4, hazeKm: 35, grade: { shadow: [0, 0, 0], light: [0, 0, 0], saturation: 1 }, ramps: { grass: ['#000000', '#111111', '#222222'] } };

describe('rulesOf', () => {
    it('resolves a region as the prototype does: objects merge, ramps come whole, a value by place takes "*"', () => {
        const rules = rulesOf({
            format: 1, materials,
            regions: [
                { id: 'alps', extends: 'world', leaf: 'needleleaved', look: { hazeKm: 22, grade: { saturation: 1.2 }, ramps: { needle: { summer: ['#010101', '#020202', '#030303'] } } } },
                { id: 'alps.insubric', extends: 'alps', look: { roadLine: { 'it-21': '#ffffff', '*': '#d6d4cc' }, ramps: { grass: ['#333333', '#444444', '#555555'] } } },
                { id: 'world', leaf: 'mixed', look },
            ],
            overlays: { alps: { look: { mistTop: 900 } } },
        });
        const insubric = rules.regions[1];
        expect(insubric.chain).toEqual(['world', 'alps', 'alps.insubric']);
        expect(insubric.leaf).toBe('needleleaved');
        expect(insubric.look.hazeKm).toBe(22);
        expect(insubric.look.grade).toEqual({ shadow: [0, 0, 0], light: [0, 0, 0], saturation: 1.2 });
        expect(insubric.look.ramps).toEqual({ grass: ['#333333', '#444444', '#555555'], needle: { summer: ['#010101', '#020202', '#030303'] } });
        expect(insubric.look.roadLine).toBe('#d6d4cc');
        expect(insubric.look.mistTop).toBe(900);
    });

    it("takes a region's trees from its pack: a child adds or changes a species, null drops one, a model defaults to its species", () => {
        const spruce = { leaf: 'needleleaved', cover: 'tree', elevation: [600, 2000], weight: 5 };
        const rules = rulesOf({
            format: 1, materials,
            regions: [{ id: 'alps', leaf: 'needleleaved', look, pack: 'alps' }, { id: 'world', leaf: 'mixed', look }],
            packs: {
                base: { trees: { 'norway-spruce': spruce, 'scots-pine': { ...spruce, weight: 1 } } },
                alps: { extends: 'base', trees: { 'scots-pine': null, 'norway-spruce': { weight: 2 }, 'swiss-stone-pine': { ...spruce, model: 'scots-pine' } } },
            },
        });
        expect(rules.regions[0].trees).toEqual([{ ...spruce, weight: 2, model: 'norway-spruce' }, { ...spruce, model: 'scots-pine' }]);
        expect(rules.regions[1].trees).toEqual([]);
    });
});
