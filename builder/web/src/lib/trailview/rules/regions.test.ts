import { describe, expect, it } from 'vitest';
import { bits, named, rulesOf } from './regions';

const materials = { materials: [{ id: 'none', cover: 'grass' }], lithologies: [{ id: 'unknown', hardness: 0.5 }] };
const look = { snap: 0.4, hazeKm: 35, grade: { shadow: [0, 0, 0], light: [0, 0, 0], saturation: 1 }, ramps: { grass: ['#000000', '#111111', '#222222'] } };

describe('a list of ground', () => {
    it('names materials by cover, by id and by #tag, as bits of their codes', () => {
        const vocabulary = { ...materials, materials: [{ id: 'none', cover: 'grass' }, { id: 'heath', cover: 'grass', tags: ['moor'] }, { id: 'scrub', cover: 'shrub' }] };
        expect(named(vocabulary, ['shrub'])).toEqual([2]);
        expect(named(vocabulary, ['#moor', 'scrub'])).toEqual([1, 2]);
        expect(named(vocabulary, ['moor'])).toEqual([]);
        expect(bits([1, 33, 63])).toEqual([2, (2 | 2 ** 31) >>> 0]);
        expect(() => bits([64])).toThrow();
    });
});

describe('rulesOf', () => {
    it('resolves a region as the prototype does: objects merge, ramps come whole, a value by place takes "*"', () => {
        const rules = rulesOf({
            format: 3, materials, ids: [['alps', '*'], ['alps.insubric', '*'], ['world', '*']],
            regions: {
                alps: { extends: 'world', leaf: 'needleleaved', look: { hazeKm: 22, grade: { saturation: 1.2 }, ramps: { needle: { summer: ['#010101', '#020202', '#030303'] } } } },
                'alps.insubric': { extends: 'alps', look: { roadLine: { 'it-21': '#ffffff', '*': '#d6d4cc' }, ramps: { grass: ['#333333', '#444444', '#555555'] } } },
                world: { leaf: 'mixed', look },
            },
        });
        const insubric = rules.regions[1];
        expect(insubric.chain).toEqual(['world', 'alps', 'alps.insubric']);
        expect(insubric.leaf).toBe('needleleaved');
        expect(insubric.look.hazeKm).toBe(22);
        expect(insubric.look.grade).toEqual({ shadow: [0, 0, 0], light: [0, 0, 0], saturation: 1.2 });
        expect(insubric.look.ramps).toEqual({ grass: ['#333333', '#444444', '#555555'], needle: { summer: ['#010101', '#020202', '#030303'] } });
        expect(insubric.look.roadLine).toBe('#d6d4cc');
    });

    it('gives a region id the value of its place, else of its country, else "*"', () => {
        const style = (name: string) => ({ name, pitch: [40, 45], eave: 0.5, verge: 0.3, hip: 0, timber: 'none', shutters: false, chimneys: 1, palette: {} });
        const rules = rulesOf({
            format: 3, materials, ids: [['alps', '*'], ['alps', 'ch-ti'], ['alps', 'it-21'], ['world', '*']],
            regions: { alps: { leaf: 'needleleaved', look, pack: 'alps' }, world: { leaf: 'mixed', look } },
            packs: { alps: { style: { '*': 'chalet', 'ch-ti': 'stone-house', it: 'village-house' } } },
            styles: { chalet: style('chalet'), 'stone-house': style('stone-house'), 'village-house': style('village-house') },
        });
        const styleOf = (id: number) => rules.styles[rules.regions[id].houses[0]].name;
        expect([0, 1, 2].map(styleOf)).toEqual(['chalet', 'stone-house', 'village-house']);
        expect(rules.regions.map(region => region.id)).toEqual(['alps', 'alps', 'alps', 'world']);
    });

    it("takes a region's trees and herds from its pack: a child adds or changes one, null drops one, a model defaults to its name", () => {
        const spruce = { leaf: 'needleleaved', cover: 'tree', elevation: [600, 2000], weight: 5 };
        const rules = rulesOf({
            format: 3, materials, ids: [['alps', '*'], ['world', '*']],
            regions: { alps: { leaf: 'needleleaved', look, pack: 'alps' }, world: { leaf: 'mixed', look } },
            packs: {
                base: { trees: { 'norway-spruce': spruce, 'scots-pine': { ...spruce, weight: 1 } }, herds: { sheep: { groups: 1, size: [2, 6], on: ['pasture'] } } },
                alps: {
                    extends: 'base', trees: { 'scots-pine': null, 'norway-spruce': { weight: 2 }, 'swiss-stone-pine': { ...spruce, model: 'scots-pine' } },
                    herds: { sheep: null, cattle: { model: 'brown-cattle', groups: 2, size: [3, 12], on: ['grassland'] } },
                },
            },
        });
        expect(rules.regions[0].trees).toEqual([{ ...spruce, weight: 2, model: 'norway-spruce' }, { ...spruce, model: 'scots-pine' }]);
        expect(rules.regions[0].herds).toEqual([{ model: 'brown-cattle', groups: 2, size: [3, 12], on: ['grassland'] }]);
        expect(rules.regions[1].trees).toEqual([]);
    });

    it("merges a region's patches by name: a child changes one, adds one, and null drops one", () => {
        const patch = { on: ['#moor'], colour: '#5e4552', share: 0.35, size_m: 4, form: 'blotches' };
        const rules = rulesOf({
            format: 3, materials, ids: [['celtic', '*'], ['world', '*']],
            regions: {
                celtic: { extends: 'world', look: { patches: { heather: { colour: '#6e4f63' }, scrub: null, lichen: { ...patch, on: ['moss'] } } } },
                world: { leaf: 'mixed', look: { ...look, patches: { heather: patch, scrub: { ...patch, form: 'dots', size_m: 2 } } } },
            },
        });
        expect(rules.regions[0].look.patches).toEqual({ heather: { ...patch, colour: '#6e4f63' }, scrub: null, lichen: { ...patch, on: ['moss'] } });
    });
});
