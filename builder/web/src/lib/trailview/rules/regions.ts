/**
 * The regions of `rules.json` (specs/trailview-tiles.md#rules), resolved as the prototype's
 * `regions.ts` resolves them. A region id is its index in `ids`, as the region tiles hold it: a
 * region and the place whose values it takes.
 */

export type Season = 'winter' | 'spring' | 'summer' | 'autumn';
export const SEASONS: readonly Season[] = ['winter', 'spring', 'summer', 'autumn'];

/** The season of a date `YYYY-MM-DD`: winter from January. */
export const seasonOf = (date: string): Season => SEASONS[Math.floor((Number(date.slice(5, 7)) - 1) / 3)];
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
    /** By name; they draw as `patchesOf` of ground/rules.ts orders them. */
    patches?: Partial<Record<string, Patch | null>>;
    /** The material whose paint a ledge of turf on exposed rock takes. */
    turf?: string;
    materials?: Partial<Record<string, { colour?: string | Partial<Record<Season, string>>; satellite?: number; clumps?: number }>>;
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
    concrete?: string;
    setts?: string;
    zebra?: string;
    zebraSetts?: string;
    giveWay?: { dash_m: number; triangle: boolean };
    dashes?: { line: [number, number]; lane: [number, number] };
    sea: { deep: string; shallow: string };
    grade: { shadow: [number, number, number]; light: [number, number, number]; saturation: number };
    weather?: Partial<Record<Preset, Partial<WeatherKeys>>>;
}

/**
 * A colour over a share of the ground of the materials that `on` names: blotches about `size_m` and
 * 5.75 × `size_m` across, or dots of bushes `size_m` across.
 */
export interface Patch {
    on: string[];
    colour: string | Partial<Record<Season, string>>;
    share: number;
    size_m: number;
    form: 'blotches' | 'dots';
}

export interface Region {
    /** The region's name; the ids of its places share it. */
    id: string;
    /** Root first. */
    chain: string[];
    leaf: 'needleleaved' | 'broadleaved' | 'mixed';
    look: Look;
    /** Indexes in `styles` of the region's pack: its house style, and the style of its cottages and houses. */
    houses: [number, number];
    /** The trees of the region's pack, by species. */
    trees: TreeRule[];
    /** The herds of the region's pack. */
    herds: HerdRule[];
    /** The ground plants of the region's pack: what the pack says of each, over its plant model's `ground`. */
    ground: GroundRule[];
    /** The model that the pack plants on each material planted in rows, by material id, over the material's own. */
    rows: Record<string, string>;
}

/** A herd of a pack: its model, its groups per eye's circle of 400 m, its animals per group and the materials that it grazes. */
export interface HerdRule {
    model: string;
    groups: number;
    size: [number, number];
    on: string[];
}

/** A ground plant of a pack: its species, its model, and the keys of the model's `ground` that the pack changes. */
export interface GroundRule {
    id: string;
    model: string;
    on?: string[];
    off?: string[];
    height?: [number, number];
    share?: number;
    own?: true;
    clumped?: boolean;
}

/** A tree species of a pack: its model, leaf type, cover, elevation band in metres and weight. */
export interface TreeRule {
    model: string;
    leaf: 'needleleaved' | 'broadleaved';
    cover: 'tree' | 'shrub';
    elevation: [number, number];
    weight: number;
    /** It keeps its summer leaves all year. */
    evergreen?: boolean;
}

/** A house style of `styles/`, as `BuildingStyle` in the prototype's models/buildings.ts. */
export interface HouseStyle {
    name: string;
    pitch: [number, number];
    eave: number;
    verge: number;
    hip: number;
    timber: 'none' | 'upper' | 'all' | 'frame';
    shutters: boolean;
    balconies?: boolean;
    /** Gabled dormers that rise from the front wall into the roof. */
    dormers?: boolean;
    chimneys: 0 | 1 | 2;
    /** A church's tower top: an onion dome, else a spire; `domes`: a church of shingled wood with onion domes on its tower and nave. */
    spire?: 'onion' | 'domes';
    /** The windows' size as a share of the kind's; smaller windows stand farther apart. */
    windows?: number;
    storeys?: number;
    /** Instead of a pitched roof: a stone cone over each bay, or a flat roof behind a parapet. */
    roof?: 'cone' | 'flat';
    palette: Record<'render' | 'wood' | 'roof' | 'door' | 'shutter', string[]> & { plinth: string | string[]; chimney: string };
}

/**
 * How a material is planted in rows (fields.wgsl): its plant, a tree or a plant model, which a pack's
 * `rows` may name instead; the metres across its rows and along them, its plants' heights, the metres
 * from the eye where they stand, and whether the plants of a row touch as a hedge. Each field takes
 * one value of each range.
 */
export interface Rows { model?: string; across_m: [number, number]; along_m: [number, number]; height_m: [number, number]; reach_m: number; hedge?: boolean }

/** The ground vocabulary of `materials.json`: a material or lithology code is its index. */
export interface Vocabulary {
    /** `clumps`: the share of ground in tussocks and low shrubs, 0 by default; `shrubs`: the share of its cover's shrubs that stand on it, 1 by default. */
    materials: { id: string; cover: string; clumps?: number; shrubs?: number; tags?: string[]; rows?: Rows }[];
    lithologies: { id: string; hardness: number }[];
}

/** The codes of the materials that a list names, each entry a cover, a material or `#tag`. */
export function named(vocabulary: Vocabulary, names: readonly string[] = []): number[] {
    return vocabulary.materials.flatMap(({ id, cover, tags = [] }, code) =>
        (names.some(name => name === id || name === cover || (name.startsWith('#') && tags.includes(name.slice(1)))) ? [code] : []));
}

/** Material codes as two words of bits, as the GPU reads a set of materials. */
export function bits(codes: number[]): [number, number] {
    const words: [number, number] = [0, 0];
    for (const code of codes) {
        if (code >= 64) throw new Error(`Material code ${code} does not fit the 6 bits of the ground tiles.`);
        words[code >> 5] = (words[code >> 5] | (1 << (code & 31))) >>> 0;
    }
    return words;
}

export interface Rules { vocabulary: Vocabulary; regions: Region[]; styles: HouseStyle[] }

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);

/** Objects merge, any other value replaces. */
function merge(a: Record<string, unknown>, b: Record<string, unknown>): Record<string, unknown> {
    const out = { ...a };
    for (const [key, value] of Object.entries(b)) out[key] = isObject(value) && isObject(a[key]) ? merge(a[key], value) : value;
    return out;
}

/** The values by place of a look. */
const BY_PLACE = ['roadLine', 'zebra', 'zebraSetts', 'giveWay', 'dashes'];

/** A value by place, `{"es-ga": …, "es": …, "*": …}`: the entry of the place, else of its country, else `"*"`. */
const local = (value: unknown, place: string) =>
    isObject(value) ? (value[place] ?? value[place.split('-')[0]] ?? value['*']) : value;

/** A file of a table with the files that it extends merged in, root first. */
function inherited(table: Record<string, Record<string, unknown>>, name: string, seen: string[] = []): Record<string, unknown> {
    const file = table[name];
    if (!file || seen.includes(name)) throw new Error(`rules.json: ${[...seen, name].join(' → ')} misses a file or loops.`);
    const { extends: up, ...own } = file;
    return typeof up === 'string' ? merge(inherited(table, up, [...seen, name]), own) : own;
}

/** The models that a pack plants on the materials planted in rows, by material id. */
function rowsOf(packs: Record<string, Record<string, unknown>> | undefined, pack: unknown): Record<string, string> {
    if (typeof pack !== 'string' || !packs?.[pack]) return {};
    const rows = (inherited(packs, pack).rows ?? {}) as Record<string, unknown>;
    return Object.fromEntries(Object.entries(rows).filter((entry): entry is [string, string] => typeof entry[1] === 'string'));
}

/** The trees, herds or ground plants of a pack by species: one that a child sets to null is dropped, a model defaults to its species. */
function entriesOf<T extends { model: string }>(packs: Record<string, Record<string, unknown>> | undefined, pack: unknown, field: 'trees' | 'herds' | 'ground'): T[] {
    if (typeof pack !== 'string' || !packs?.[pack]) return [];
    const entries = (inherited(packs, pack)[field] ?? {}) as Record<string, Partial<T> | null>;
    return Object.entries(entries).flatMap(([name, rule]) => (rule ? [{ ...rule, ...(field === 'ground' && { id: name }), model: rule.model ?? name } as T] : []));
}

/** The resolved regions of a `rules.json`; a chain that misses a file or loops throws. */
export function rulesOf(json: {
    format: number; materials: Vocabulary; ids: [string, string][]; regions: Record<string, Record<string, unknown> & { extends?: string }>;
    packs?: Record<string, Record<string, unknown>>; styles?: Record<string, Record<string, unknown>>;
}): Rules {
    if (json.format !== 3) throw new Error(`rules.json has format ${json.format}, not 3.`);
    const table = json.regions;
    const names = Object.keys(json.styles ?? {}).sort(), styles = names.map(name => inherited(json.styles!, name) as unknown as HouseStyle);
    const styleOf = (pack: unknown, place: string): [number, number] => {
        if (typeof pack !== 'string' || !json.packs?.[pack]) return [0, 0];
        const { style, houses } = inherited(json.packs, pack), own = Math.max(0, names.indexOf(local(style, place) as string));
        return [own, typeof houses === 'string' ? Math.max(0, names.indexOf(houses)) : own];
    };
    const regions = json.ids.map(([id, place]): Region => {
        const chain = [id];
        for (let up = table[id].extends; up !== undefined; up = table[up].extends) {
            if (!table[up] || chain.includes(up)) throw new Error(`rules.json: region ${id} extends a missing region or loops.`);
            chain.unshift(up);
        }
        const files = chain.map(name => table[name]);
        const resolved = files.reduce<Record<string, unknown>>((out, { extends: _, ...own }) => merge(out, own), {});
        // A cover's ramp comes whole from the nearest region that names it.
        const ramps = Object.assign({}, ...files.map(file => (file.look as Look | undefined)?.ramps));
        const merged = resolved.look as Record<string, unknown>;
        const look = { ...merged, ramps, ...Object.fromEntries(BY_PLACE.map(key => [key, local(merged[key], place)])) } as Look;
        return { id, chain, leaf: resolved.leaf as Region['leaf'], look, houses: styleOf(resolved.pack, place),
            trees: entriesOf<TreeRule>(json.packs, resolved.pack, 'trees'), herds: entriesOf<HerdRule>(json.packs, resolved.pack, 'herds'),
            ground: entriesOf<GroundRule>(json.packs, resolved.pack, 'ground'), rows: rowsOf(json.packs, resolved.pack) };
    });
    return { vocabulary: json.materials, regions, styles };
}
