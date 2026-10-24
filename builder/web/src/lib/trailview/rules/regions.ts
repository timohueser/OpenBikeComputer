/**
 * The regions of `rules.json` (specs/trailview-tiles.md#rules), resolved as the prototype's
 * `regions.ts` resolves them. A region id is its index in `regions`, as the region tiles hold it.
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
    /** Indexes in `styles` of the region's pack: its house style, and the style of its cottages and houses. */
    houses: [number, number];
    /** The trees of the region's pack, by species. */
    trees: TreeRule[];
    /** The herds of the region's pack. */
    herds: HerdRule[];
    /** The ground plants of the region's pack: what the pack says of each, over its plant model's `ground`. */
    ground: GroundRule[];
    /** The tree model of the pack's orchards. */
    orchard?: string;
}

/** A herd of a pack: its model, its groups per eye's circle of 400 m, its animals per group and the materials that it grazes. */
export interface HerdRule {
    model: string;
    groups: number;
    size: [number, number];
    on: string[];
}

/** A ground plant of a pack: its model, and the keys of the model's `ground` that the pack changes. */
export interface GroundRule {
    model: string;
    on?: string[];
    height?: [number, number];
    share?: number;
    habitat?: 'moor' | 'lush';
    own?: true;
    clumped?: true;
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
    chimneys: 0 | 1 | 2;
    spire?: 'onion';
    storeys?: number;
    palette: Record<'render' | 'wood' | 'roof' | 'door' | 'shutter', string[]> & { plinth: string; chimney: string };
}

/** The ground vocabulary of `materials.json`: a material or lithology code is its index. */
export interface Vocabulary {
    materials: { id: string; cover: string }[];
    lithologies: { id: string; hardness: number }[];
}

export interface Rules { vocabulary: Vocabulary; regions: Region[]; styles: HouseStyle[] }

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

/** A file of a table with the files that it extends merged in, root first. */
function inherited(table: Record<string, Record<string, unknown>>, name: string, seen: string[] = []): Record<string, unknown> {
    const file = table[name];
    if (!file || seen.includes(name)) throw new Error(`rules.json: ${[...seen, name].join(' → ')} misses a file or loops.`);
    const { extends: up, ...own } = file;
    return typeof up === 'string' ? merge(inherited(table, up, [...seen, name]), own) : own;
}

/** The tree model of a pack's orchards. */
function orchardOf(packs: Record<string, Record<string, unknown>> | undefined, pack: unknown): string | undefined {
    if (typeof pack !== 'string' || !packs?.[pack]) return undefined;
    const model = inherited(packs, pack).orchard;
    return typeof model === 'string' ? model : undefined;
}

/** The trees, herds or ground plants of a pack: one that a child sets to null is dropped, a model defaults to its name. */
function entriesOf<T extends { model: string }>(packs: Record<string, Record<string, unknown>> | undefined, pack: unknown, field: 'trees' | 'herds' | 'ground'): T[] {
    if (typeof pack !== 'string' || !packs?.[pack]) return [];
    const entries = (inherited(packs, pack)[field] ?? {}) as Record<string, Partial<T> | null>;
    return Object.entries(entries).flatMap(([name, rule]) => (rule ? [{ ...rule, model: rule.model ?? name } as T] : []));
}

/** The resolved regions of a `rules.json`; a chain that misses a file or loops throws. */
export function rulesOf(json: {
    format: number; materials: Vocabulary; regions: Entry[]; overlays?: Record<string, { look?: Record<string, unknown> }>;
    packs?: Record<string, Record<string, unknown>>; styles?: Record<string, Record<string, unknown>>;
}): Rules {
    if (json.format !== 1) throw new Error(`rules.json has format ${json.format}, not 1.`);
    const table = new Map(json.regions.map(entry => [entry.id, entry]));
    const names = Object.keys(json.styles ?? {}).sort(), styles = names.map(name => inherited(json.styles!, name) as unknown as HouseStyle);
    const styleOf = (pack: unknown): [number, number] => {
        if (typeof pack !== 'string' || !json.packs?.[pack]) return [0, 0];
        const { style, houses } = inherited(json.packs, pack), own = Math.max(0, names.indexOf(local(style) as string));
        return [own, typeof houses === 'string' ? Math.max(0, names.indexOf(houses)) : own];
    };
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
        return { id, chain, leaf: resolved.leaf as Region['leaf'], look, houses: styleOf(resolved.pack),
            trees: entriesOf<TreeRule>(json.packs, resolved.pack, 'trees'), herds: entriesOf<HerdRule>(json.packs, resolved.pack, 'herds'),
            ground: entriesOf<GroundRule>(json.packs, resolved.pack, 'ground'), orchard: orchardOf(json.packs, resolved.pack) };
    });
    return { vocabulary: json.materials, regions, styles };
}
