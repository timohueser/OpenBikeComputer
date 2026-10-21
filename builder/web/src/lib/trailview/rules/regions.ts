/**
 * The regions of `rules.json` (specs/trailview-tiles.md#rules), resolved as the prototype's
 * `regions.ts` resolves them. A region id is its index in `regions`, as the region tiles hold it.
 */

export type Season = 'winter' | 'spring' | 'summer' | 'autumn';
export const SEASONS: readonly Season[] = ['winter', 'spring', 'summer', 'autumn'];
export type Ramp = [string, string, string];
export type Preset = 'sunny' | 'cloudy' | 'rainy';

/** The weather keys of a preset, as `Weather` in light/light.ts. */
export interface WeatherKeys {
    cover: number; base: number; dark: number; sun: number; ambient: number; grey: number;
    zenith: string; horizon: string; haze: number; wet: number; rain: number; towers?: number;
}

/** The look of a region, as `RegionLook` in the prototype's regions.ts. */
export interface Look {
    snap: number;
    hazeKm: number;
    hazeHeight: number;
    mistTop: number;
    softness: number;
    gradient: number;
    ramps: Partial<Record<string, Ramp | Partial<Record<Season, Ramp>>>>;
    moor?: { heather: string; share: number };
    speckle?: { colour: string; share: number; size_m: number };
    materials?: Partial<Record<string, { colour?: string | Partial<Record<Season, string>>; satellite?: number }>>;
    lithology?: Partial<Record<string, { colour: string; banded?: true }>>;
    outcrops: number;
    turf_deg: number;
    rock: string;
    cliff?: string;
    bare_luma?: number | [number, number, number, number];
    snow_share: number;
    bedding?: { strike: number; dip: number };
    roads?: string[];
    roadLine?: string;
    sea: { deep: string; shallow: string };
    grade: { shadow: [number, number, number]; light: [number, number, number]; saturation: number };
    weather?: Partial<Record<Preset, Partial<WeatherKeys>>>;
}

export interface Region {
    id: string;
    /** Root first. */
    chain: string[];
    leaf: 'needleleaved' | 'broadleaved' | 'mixed';
    look: Look;
}

/** The ground vocabulary of `materials.json`: a material or lithology code is its index. */
export interface Vocabulary {
    materials: { id: string; cover: string }[];
    lithologies: { id: string; hardness: number }[];
}

export interface Rules { vocabulary: Vocabulary; regions: Region[] }

type Entry = Record<string, unknown> & { id: string; extends?: string };

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);

/** Objects merge, any other value replaces. */
function merge(a: Record<string, unknown>, b: Record<string, unknown>): Record<string, unknown> {
    const out = { ...a };
    for (const [key, value] of Object.entries(b)) out[key] = isObject(value) && isObject(a[key]) ? merge(a[key], value) : value;
    return out;
}

/** A value by place, `{"es-ga": …, "es": …, "*": …}`: the release has no layer of the place, so `"*"`. */
const local = (value: unknown) => (isObject(value) && '*' in value ? value['*'] : value);

/** The resolved regions of a `rules.json`; a chain that misses a file or loops throws. */
export function rulesOf(json: { format: number; materials: Vocabulary; regions: Entry[]; overlays?: Record<string, { look?: Record<string, unknown> }> }): Rules {
    if (json.format !== 1) throw new Error(`rules.json has format ${json.format}, not 1.`);
    const table = new Map(json.regions.map(entry => [entry.id, entry]));
    const regions = json.regions.map(({ id }): Region => {
        const chain = [id];
        for (let up = table.get(id)!.extends; up !== undefined; up = table.get(up)!.extends) {
            if (!table.has(up) || chain.includes(up)) throw new Error(`rules.json: region ${id} extends a missing region or loops.`);
            chain.unshift(up);
        }
        const files = chain.map(name => table.get(name)!);
        const resolved = files.reduce<Record<string, unknown>>((out, { id: _, extends: __, ...own }) => merge(out, own), {});
        // A cover's ramp comes whole from the nearest region that names it.
        const ramps = Object.assign({}, ...files.map(file => (file.look as Look | undefined)?.ramps));
        const overlaid = chain.reduce((look, name) => merge(look, json.overlays?.[name]?.look ?? {}), resolved.look as Record<string, unknown>);
        const look = { ...overlaid, ramps, roadLine: local(overlaid.roadLine) } as Look;
        return { id, chain, leaf: resolved.leaf as Region['leaf'], look };
    });
    return { vocabulary: json.materials, regions };
}
