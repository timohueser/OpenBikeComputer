/**
 * Regions, packs and house styles: three hierarchies of data, in regions/, packs/ and styles/. A
 * region is a ruleset: its terrain rules (which bake.py reads) and its look; it names the pack that
 * holds its models, and a pack names its house style. Each file extends a parent and inherits what
 * it does not say: objects merge, anything else replaces. bake.py picks a spot's region and writes
 * it to meta.region. A pack's `style` and a look's `roadLine` may be a map by country,
 * {"ie": …, "*": …}: the spot's country picks its entry, else "*".
 */

import type { Habitat } from './bake';
import type { BuildingStyle } from './models/buildings';
import { ANIMALS, PLANTS, TREES, type Ground } from './models/library';
import type { Weather } from './weather';

export interface Species {
  /** Its key in the pack. */
  id: string;
  /** A tree model, or a plant model that stands as a shrub. */
  tree: string;
  /** The OSM leaf_type it stands for, and the WorldCover class it stands on. */
  leaf: 'needleleaved' | 'broadleaved';
  cover: 'tree' | 'shrub';
  /** Elevation band in metres. */
  elevation: [number, number];
  /** Its share among the species that fit a spot. */
  weight: number;
  /** Drawn in its summer leaves all year: it stands in for an evergreen the library has no model of. */
  evergreen?: boolean;
}

/** Low to high NDVI per WorldCover class; `needle` and `broad` colour woodland by its leaf. */
type Ramps = Partial<Record<'shrub' | 'grass' | 'crop' | 'bare' | 'built' | 'moss' | 'needle' | 'broad', [string, string, string]>>;

/** How the ground, sea and air read in a region. */
export interface RegionLook {
  /** Palette snap: 0 takes the ground's chroma from Sentinel-2, 1 paints the palette alone. */
  snap: number;
  /** Haze distance at sea level, in km. */
  hazeKm: number;
  /** Scale height of the haze in metres, and the top of the valley mist above sea level (0 none). */
  hazeHeight: number;
  mistTop: number;
  /** Width of the steps between the three light bands, and the share of smooth light kept. */
  softness: number;
  gradient: number;
  /** Ramps that replace palette.ts's for these WorldCover classes, low NDVI to high. */
  ramps?: Ramps;
  /** Ramps for one season over `ramps`, as winter-green Mediterranean hills need. */
  seasons?: Partial<Record<'winter' | 'spring' | 'summer' | 'autumn', Ramps>>;
  /**
   * Moorland: open ground that Sentinel-2 sees brown rather than green, painted in its own
   * colour with heather patches over a share of it. Heather plants grow there and grass does not.
   */
  moor?: { colour: string; heather: string; share: number };
  /**
   * Scattered bushes on open ground, too small for WorldCover: their colour, the share of ground
   * they cover, and their size in metres. Far off they become their share of colour.
   */
  speckle?: { colour: string; share: number; size_m: number };
  /** Colours for the habitats of a fine land cover map, where the bake has one; others keep the palette. */
  habitats?: Partial<Record<Habitat, string>>;
  /** How much open ground breaks through as rock slabs on slopes, ridges and poor grass, 0–1. */
  outcrops: number;
  /** Where the satellite sees green, a slope stays turf up to this many degrees; rock that holds steeper keeps its own angle. */
  turf_deg: number;
  /** Slabs and outcrops, and cliff walls unless `cliff` says otherwise. */
  rock: string;
  /** Cliff walls: often darker than sunlit slabs, shaded, wet and lichened. */
  cliff?: string;
  /** Out of winter, snow lies only above this altitude; below it a white pixel is rock or spray. */
  snowline: number;
  /** The rock's bedding planes in degrees, so cliffs and slabs show their strata as they lie. */
  bedding?: { strike: number; dip: number };
  /** Road surfaces in the order of ROAD_WIDTHS in bake.py, where they differ from palette.ts's. */
  roads?: string[];
  /** The line along each edge of a main road, where it is not white. */
  roadLine?: string;
  /** Open water away from the shore, and the sea beside it. */
  sea: { deep: string; shallow: string };
  /** Trees and shrubs keep this far from the sea, where wind and salt keep headlands bare. */
  windswept_m?: number;
  /** The bearing the prevailing swell comes from, in degrees: shores open to it get the heavy surf. */
  swell?: number;
  /** The colour grade: shadow and light tints added in linear light, and the saturation. */
  grade: { shadow: [number, number, number]; light: [number, number, number]; saturation: number };
  /** What the climate changes in each weather preset. */
  weather?: Partial<Record<'sunny' | 'cloudy' | 'rainy', Partial<Weather>>>;
}

/**
 * The models a region draws. In the files, trees, ground plants and herds are objects keyed by
 * species, so a pack adds to its parent's or changes one, and null drops one; a species key that is
 * not its model's id names the model in `model`. A ground plant's entry changes what its plant
 * file says of where it grows.
 */
export interface Pack {
  name: string;
  trees: Species[];
  ground: (Ground & { id: string; plant: string })[];
  style: BuildingStyle;
  /** The style of cottages and houses, where it differs from farms'. */
  houses?: BuildingStyle;
  /** The tree that stands in OSM orchards; without one, the pack's broadleaves do. */
  orchard?: string;
  /** Herds near the eye: how many groups of how many, on which habitats (on WorldCover grass without a map). */
  herds: { id: string; animal: string; groups: number; size: [number, number]; on: Habitat[] }[];
}

export interface Region {
  id: string;
  name: string;
  /** Root first. */
  chain: string[];
  leaf: 'needleleaved' | 'broadleaved' | 'mixed';
  look: RegionLook;
  pack: Pack;
}

type Entry = Record<string, unknown> & { extends?: string };

const byName = (files: Record<string, unknown>) =>
  Object.fromEntries(Object.entries(files).map(([path, data]) => [path.split('/').pop()!.replace(/\.json$/, ''), data as Entry]));
const REGIONS = byName(import.meta.glob('../regions/*.json', { eager: true, import: 'default' }));
const PACKS = byName(import.meta.glob('../packs/*.json', { eager: true, import: 'default' }));
const STYLES = byName(import.meta.glob('../styles/*.json', { eager: true, import: 'default' }));

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);

function merge(a: Record<string, unknown>, b: Record<string, unknown>): Record<string, unknown> {
  const out = { ...a };
  for (const [key, value] of Object.entries(b)) out[key] = isObject(value) && isObject(a[key]) ? merge(a[key], value) : value;
  return out;
}

function chain(table: Record<string, Entry>, id: string): string[] {
  if (!table[id]) throw new Error(`no file for ${id}`);
  const up = table[id].extends;
  return up ? [...chain(table, up), id] : [id];
}

function resolve(table: Record<string, Entry>, id: string): Record<string, unknown> {
  return chain(table, id).reduce<Record<string, unknown>>((out, name) => {
    const { extends: _, where: __, about: ___, ...own } = table[name];
    return merge(out, own);
  }, {});
}

/** A value or a map by country: the country's entry, else "*". */
const local = <T>(value: T | Record<string, T>, country?: string | null): T =>
  isObject(value) ? (value[country ?? '*'] ?? value['*']) : value;

/** A region with its look and pack resolved for a spot in `country`. */
export function regionNamed(id: string, country?: string | null): Region {
  const region = resolve(REGIONS, id) as Omit<Region, 'id' | 'chain' | 'pack'> & { pack: string };
  const look = { ...region.look, roadLine: local(region.look.roadLine, country) };
  return { ...region, id, chain: chain(REGIONS, id), look, pack: packNamed(region.pack, country) };
}

export const PACK_NAMES = Object.keys(PACKS).filter((name) => name !== 'base');

export function packNamed(name: string, country?: string | null): Pack {
  const pack = resolve(PACKS, name);
  const style = (id: string) => resolve(STYLES, id) as unknown as BuildingStyle;
  const unknown = (species: string, model: unknown) => new Error(`pack ${name}: ${species} names unknown model ${model}`);
  const listed = <T>(table: unknown, key: string, ...models: Record<string, unknown>[]) =>
    Object.entries((table ?? {}) as Record<string, Record<string, unknown> | null>)
      .filter(([, entry]) => entry)
      .map(([id, entry]) => {
        const model = (entry!.model ?? id) as string;
        if (!models.some((m) => model in m)) throw unknown(id, model);
        return { id, [key]: model, ...entry } as T;
      });
  const trees = listed<Species>(pack.trees, 'tree', TREES, PLANTS);
  const tall = trees.find((s) => s.cover === 'tree' && !(s.tree in TREES));
  if (tall) throw new Error(`pack ${name}: ${tall.id} stands as a tree, but ${tall.tree} is not a tree model`);
  if (pack.orchard !== undefined && !((pack.orchard as string) in TREES)) throw unknown('orchard', pack.orchard);
  const ground = listed<Pack['ground'][number]>(pack.ground, 'plant', PLANTS).map((g) => ({ ...PLANTS[g.plant].ground, ...g }));
  const houses = pack.houses && style(pack.houses as string);
  return { ...pack, trees, ground, herds: listed(pack.herds, 'animal', ANIMALS), style: style(local(pack.style as string, country)), houses } as unknown as Pack;
}
