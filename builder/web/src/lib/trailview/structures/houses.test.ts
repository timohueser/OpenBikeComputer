import { describe, expect, it } from 'vitest';
import type { HouseStyle, Rules } from '../rules/regions';
import { CONE_ROOF, DORMERS, FLAT_ROOF, ONION_DOME, SHUTTERS, STYLE_WORDS, styleWords, WOODEN } from './houses';

const style = (own: Partial<HouseStyle>): HouseStyle => ({
    name: 'House', pitch: [30, 40], eave: 0.5, verge: 0.4, hip: 0, timber: 'none', shutters: false, chimneys: 1,
    palette: { render: ['#ffffff'], wood: ['#000000'], roof: ['#000000'], door: ['#000000'], shutter: ['#000000'], plinth: '#102030', chimney: '#000000' }, ...own,
});

describe('the style table of the houses', () => {
    it('packs the flags, the window scale and a list of plinth colours as houses.wgsl reads them', () => {
        const cube = style({ roof: 'flat', windows: 0.75, shutters: true, palette: { ...style({}).palette, plinth: ['#010203', '#040506'] } });
        const church = style({ spire: 'domes', dormers: true, roof: 'cone' });
        const { styles } = styleWords({ styles: [cube, church, style({})], regions: [] } as unknown as Rules);
        const words = (i: number) => styles.subarray(i * STYLE_WORDS, (i + 1) * STYLE_WORDS), f32 = (u: Uint32Array) => new Float32Array(u.buffer, u.byteOffset, u.length);
        expect(words(0)[6]).toBe(SHUTTERS | FLAT_ROOF);
        expect(words(1)[6]).toBe(ONION_DOME | WOODEN | DORMERS | CONE_ROOF);
        expect(f32(words(0))[9]).toBe(0.75);
        expect(f32(words(2))[9]).toBe(1);
        // The sixth list: its length at bits 20 to 23 of `counts`, its colours as 0xbbggrr after the other five lists.
        expect((words(0)[11] >> 20) & 15).toBe(2);
        expect(Array.from(words(0).subarray(32, 34))).toEqual([0x030201, 0x060504]);
        expect((words(2)[11] >> 20) & 15).toBe(1);
        expect(words(2)[32]).toBe(0x302010);
    });
});
