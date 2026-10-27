/**
 * The rules of every region that paint the ground, packed for look.wgsl: one row of texels per region
 * (the region tiles index it), one column per slot. A port of the prototype's `rules.ts`,
 * `palette.ts` and the mosaic calendar of `calendar.ts`. Colours are linear RGB.
 */

import { named, SEASONS, type Ramp, type Region, type Season, type Vocabulary } from '../rules/regions';

/** WorldCover's groups in the order of the ramps; `COVER_*` in look.wgsl. */
export const COVER_ORDER = ['tree', 'shrub', 'grass', 'crop', 'built', 'bare', 'snow', 'water', 'wetland', 'mangrove', 'moss'] as const;

/** Snow, on the ground and on what it settles on. */
export const SNOW = '#eef2f6';

const FIXED: Record<string, Ramp> = {
    shrub: ['#7a7344', '#6b7a40', '#5a7a3a'],
    // WorldCover's built-up ground in an Alpine village is mostly gardens between houses.
    built: ['#a39a8e', '#99987e', '#8a9568'],
    bare: ['#8f8a80', '#979084', '#8a8a72'],
    snow: [SNOW, SNOW, SNOW],
    water: ['#2c5874', '#2c5874', '#2c5874'],
    wetland: ['#7d8a5a', '#6f8a52', '#5f8a4c'],
    mangrove: ['#3e5a34', '#365a32', '#2f5530'],
    moss: ['#8d8d6a', '#86906a', '#7a8c60'],
};

/** Winter, spring, summer, autumn: the order of SEASONS and of the Sentinel-2 quarters. */
const SEASONAL: Record<'grass' | 'crop' | 'needle' | 'broad', Ramp>[] = [
    { grass: ['#8f8a66', '#87865f', '#7f8459'], crop: ['#9a8e6a', '#8e8a62', '#7f855a'], needle: ['#2f3d2d', '#2d3f2e', '#2b3f2e'], broad: ['#6e6252', '#6a6050', '#655d4d'] },
    { grass: ['#a3a65e', '#8ea657', '#7a9c4d'], crop: ['#a9a066', '#93a35a', '#7e9a50'], needle: ['#35502f', '#33532f', '#30532e'], broad: ['#7aa04e', '#6f9e48', '#62983f'] },
    { grass: ['#a8a05e', '#8a9c52', '#76924a'], crop: ['#b8a764', '#979e58', '#7d944e'], needle: ['#2f4a2c', '#2d4c2c', '#2a4b2b'], broad: ['#55783a', '#4f7a38', '#487834'] },
    { grass: ['#a39c64', '#929a58', '#82944f'], crop: ['#a8946a', '#9a9460', '#8a9058'], needle: ['#334530', '#31472f', '#2f472e'], broad: ['#a57a40', '#9a6f3a', '#8a6636'] },
];

/** Share of broadleaf trees for a region's `leaf`. */
const DECIDUOUS: Record<Region['leaf'], number> = { needleleaved: 0.2, mixed: 0.5, broadleaved: 1 };
/** Road surfaces: main, road, gravel, dirt, grass, path. */
const ROADS = ['#6b6b68', '#7a7873', '#b0a48c', '#8f7a5c', '#8c9a62', '#a39272'];
const ROAD_LINE = '#d6d4cc';
/** The bands of variegated rock, such as Keuper clays: red, gypsum white, green-grey. */
export const VARIEGATED = ['#9e6252', '#ddd2bc', '#869690'];

/**
 * The patches of a region that the ground draws. Each takes a channel of a texel plane of the ground
 * cache (ground.wgsl), which filters its share of the ground between texels; the planes have two
 * channels free, and a third patch would need a fourth plane on every page.
 */
export const PATCHES = 2;

/** An sRGB hex colour in linear light. */
export function linear(hex: string): [number, number, number] {
    return [1, 3, 5].map(i => {
        const c = parseInt(hex.slice(i, i + 2), 16) / 255;
        return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    }) as [number, number, number];
}

const lerp = (a: number[], b: number[], t: number) => a.map((v, i) => v + (b[i] - v) * t);

/**
 * The slots: three ramp colours per cover; a paint per material, alpha 1 where the region names one;
 * per material its traits: the share of the satellite's hue that its paint keeps, and its share of
 * ground in tussocks and low shrubs; a colour per lithology, alpha 1 where it is banded; the cliff
 * (alpha 1 where named), the colour of each patch (alpha its share), the terrain scalars (outcrops,
 * cos turf_deg, snow_share), the bedding normal (alpha the season's bare_luma, 0 for none) and the
 * palette snap; the sea's deep and shallow colours; per patch its size and 1 for dots; the paint of
 * the turf (alpha 1 where it has one); the road surfaces and the edge line of main roads. Then the
 * vocabulary: per material its cover, the patches over it as bits, rocky and rough; per lithology
 * its hardness.
 */
export function slots(vocabulary: Vocabulary) {
    const materials = 3 * COVER_ORDER.length, m = vocabulary.materials.length, l = vocabulary.lithologies.length;
    const traits = materials + m, lithologies = traits + m, cliff = lithologies + l, roads = cliff + 10;
    const kinds = roads + ROADS.length + 1, hardness = kinds + m;
    return {
        materials, traits, lithologies, cliff, patches: cliff + 1, terrain: cliff + 3, bedding: cliff + 4, snap: cliff + 5,
        sea: cliff + 6, shapes: cliff + 8, turf: cliff + 9, roads, roadLine: roads + ROADS.length, kinds, hardness, count: hardness + l,
    };
}

/** A colour of the year or by season, in a season. */
const inSeason = (colour: string | Partial<Record<Season, string>> | undefined, season: number) => (typeof colour === 'string' ? colour : colour?.[SEASONS[season]]);

/** The cover ramps of a season, three colours each in COVER_ORDER; woodland mixes needle and broad by the leaf. */
export function ramps(season: number, region: Region): number[][] {
    const own = Object.fromEntries(Object.entries(region.look.ramps).flatMap(([cover, ramp]) => {
        const now = Array.isArray(ramp) ? ramp : ramp?.[SEASONS[season]];
        return now ? [[cover, now]] : [];
    }));
    const table: Record<string, Ramp> = { ...FIXED, ...SEASONAL[season], ...own };
    const tree = table.needle.map((needle, i) => lerp(linear(needle), linear(table.broad[i]), DECIDUOUS[region.leaf]));
    return COVER_ORDER.flatMap(cover => (cover === 'tree' ? tree : table[cover].map(linear)));
}

/** The rows of a season (0 winter to 3 autumn). */
export function packRules(regions: Region[], vocabulary: Vocabulary, season: number): Float32Array<ArrayBuffer> {
    const SLOT = slots(vocabulary), data = new Float32Array(regions.length * SLOT.count * 4);
    regions.forEach((region, row) => {
        const { look } = region;
        const set = (slot: number, rgb: number[], a: number) => data.set([...rgb, a], (row * SLOT.count + slot) * 4);
        const paint = (colour: string | undefined, a: number) => [linear(colour ?? '#000000'), colour ? a : 0] as const;
        const patches = (look.patches ?? []).slice(0, PATCHES);
        const under = patches.map(patch => named(vocabulary, patch.on));
        ramps(season, region).forEach((c, i) => set(i, c, 1));
        vocabulary.materials.forEach(({ id, cover, clumps, tags = [] }, i) => {
            set(SLOT.materials + i, ...paint(inSeason(look.materials?.[id]?.colour, season), 1));
            set(SLOT.traits + i, [look.materials?.[id]?.satellite ?? 0, look.materials?.[id]?.clumps ?? clumps ?? 0, 0], 1);
            const patched = under.reduce((bits, codes, k) => bits | (codes.includes(i) ? 1 << k : 0), 0);
            set(SLOT.kinds + i, [COVER_ORDER.indexOf(cover as (typeof COVER_ORDER)[number]), patched, +tags.includes('rocky')], +tags.includes('rough'));
        });
        vocabulary.lithologies.forEach(({ id, hardness }, i) => {
            const rock = look.lithology?.[id];
            set(SLOT.lithologies + i, linear(rock?.colour ?? look.rock), rock?.banded ? 1 : 0);
            set(SLOT.hardness + i, [hardness, 0, 0], 1);
        });
        set(SLOT.cliff, linear(look.cliff ?? '#000000'), look.cliff ? 1 : 0);
        patches.forEach((patch, k) => set(SLOT.patches + k, ...paint(inSeason(patch.colour, season), patch.share)));
        const shape = (k: number) => [patches[k]?.size_m ?? 1, +(patches[k]?.form === 'dots')];
        set(SLOT.shapes, [...shape(0), shape(1)[0]], shape(1)[1]);
        set(SLOT.turf, ...paint(inSeason(look.turf ? look.materials?.[look.turf]?.colour : undefined, season), 1));
        set(SLOT.terrain, [look.outcrops, Math.cos(look.turf_deg * Math.PI / 180), look.snow_share], 0);
        // The bedding plane's normal: up, tipped by the dip toward the dip direction, 90° right of the strike.
        const { strike, dip } = look.bedding ?? { strike: 0, dip: 0 }, toward = (strike + 90) * Math.PI / 180, tilt = dip * Math.PI / 180;
        const bare = Array.isArray(look.bare_luma) ? look.bare_luma[season] : look.bare_luma;
        set(SLOT.bedding, [Math.sin(toward) * Math.sin(tilt), Math.cos(toward) * Math.sin(tilt), Math.cos(tilt)], bare ?? 0);
        set(SLOT.snap, [look.snap, 0, 0], 0);
        set(SLOT.sea, linear(look.sea.deep), 1);
        set(SLOT.sea + 1, linear(look.sea.shallow), 1);
        (look.roads ?? ROADS).forEach((hex, i) => set(SLOT.roads + i, linear(hex), 1));
        set(SLOT.roadLine, linear(look.roadLine ?? ROAD_LINE), 1);
    });
    return data;
}

/**
 * The rows of a day of the year: those of the two quarters around it, blended as their mosaics are.
 * A colour blends by its alpha, so one that a season leaves out fades in, not from black.
 */
export function packRulesOn(regions: Region[], vocabulary: Vocabulary, day: number): Float32Array<ArrayBuffer> {
    const { quarters: [a, b], weight } = mosaicsAt(day);
    const from = packRules(regions, vocabulary, a), to = packRules(regions, vocabulary, b), out = new Float32Array(from.length);
    for (let i = 0; i < out.length; i += 4) {
        const wa = from[i + 3] * (1 - weight), wb = to[i + 3] * weight;
        for (let c = i; c < i + 3; c++) out[c] = wa + wb > 0 ? (from[c] * wa + to[c] * wb) / (wa + wb) : from[c] + (to[c] - from[c]) * weight;
        out[i + 3] = wa + wb;
    }
    return out;
}

/** The days that the quarterly mosaics centre on: 15 February, 15 May, 15 August and 15 November. */
const CENTRES = [45, 134, 226, 318];
const DAYS = 365;

/** The day of the year of a `YYYY-MM-DD` date, 0 on 1 January. */
export function dayOfYear(date: string): number {
    const [year, month, day] = date.split('-').map(Number);
    return Math.round((Date.UTC(year, month - 1, day) - Date.UTC(year, 0, 1)) / 86_400_000);
}

/** The two mosaics around a day, 0–3, and the weight of the second, linear in days between their centres. */
export function mosaicsAt(day: number): { quarters: [number, number]; weight: number } {
    const b = Math.max(CENTRES.findIndex(centre => centre > day), 0), a = (b + 3) % 4;
    const from = CENTRES[a] - (CENTRES[a] > day ? DAYS : 0), to = CENTRES[b] + (CENTRES[b] > day ? 0 : DAYS);
    return { quarters: [a, b], weight: (day - from) / (to - from) };
}
