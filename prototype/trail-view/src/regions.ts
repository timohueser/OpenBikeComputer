/**
 * Regions and packs: two hierarchies of data, in regions/ and packs/. A region is a ruleset: the
 * areas it covers, its data sources and coast rules (which bake.py reads), and its look; it names
 * the pack that holds its models. Each file extends a parent and inherits what it does not say:
 * objects merge, anything else replaces. A spot's region is the deepest one whose area holds it,
 * the smallest area on a tie. An area is a box [west, south, east, north] or a ring of [lon, lat].
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
  /** What cottages and houses change from the style, where they differ from farms. */
  houses?: Partial<BuildingStyle>;
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
  tz: string;
  leaf: 'needleleaved' | 'broadleaved' | 'mixed';
  look: RegionLook;
  pack: Pack;
}

type Box = [west: number, south: number, east: number, north: number];
type Ring = [lon: number, lat: number][];
type Area = Box | Ring;
type Entry = Record<string, unknown> & { extends?: string; where?: Area[] };

const byName = (files: Record<string, unknown>) =>
  Object.fromEntries(Object.entries(files).map(([path, data]) => [path.split('/').pop()!.replace(/\.json$/, ''), data as Entry]));
const REGIONS = byName(import.meta.glob('../regions/*.json', { eager: true, import: 'default' }));
const PACKS = byName(import.meta.glob('../packs/*.json', { eager: true, import: 'default' }));

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);

function merge(a: Record<string, unknown>, b: Record<string, unknown>): Record<string, unknown> {
  const out = { ...a };
  for (const [key, value] of Object.entries(b)) out[key] = isObject(value) && isObject(a[key]) ? merge(a[key], value) : value;
  return out;
}

function chain(table: Record<string, Entry>, id: string): string[] {
  const up = table[id].extends;
  return up ? [...chain(table, up), id] : [id];
}

function resolve(table: Record<string, Entry>, id: string): Record<string, unknown> {
  return chain(table, id).reduce<Record<string, unknown>>((out, name) => {
    const { extends: _, where: __, about: ___, ...own } = table[name];
    return merge(out, own);
  }, {});
}

const isRing = (a: Area): a is Ring => Array.isArray(a[0]);

function holds(a: Area, lat: number, lon: number): boolean {
  if (!isRing(a)) return a[0] <= lon && lon <= a[2] && a[1] <= lat && lat <= a[3];
  let inside = false;
  a.forEach(([x0, y0], i) => {
    const [x1, y1] = a[(i + 1) % a.length];
    if (y0 > lat !== y1 > lat && lon < x0 + ((lat - y0) * (x1 - x0)) / (y1 - y0)) inside = !inside;
  });
  return inside;
}

const area = (a: Area) =>
  isRing(a)
    ? Math.abs(a.reduce((sum, [x0, y0], i) => sum + x0 * a[(i + 1) % a.length][1] - a[(i + 1) % a.length][0] * y0, 0)) / 2
    : (a[2] - a[0]) * (a[3] - a[1]);

/** The region of a spot, with its look and pack resolved. */
export function regionAt(lat: number, lon: number): Region {
  const fits = Object.keys(REGIONS).flatMap((id) => (REGIONS[id].where ?? []).filter((a) => holds(a, lat, lon)).map((a) => ({ id, a })));
  fits.sort((a, b) => chain(REGIONS, b.id).length - chain(REGIONS, a.id).length || area(a.a) - area(b.a));
  const id = fits[0]?.id ?? 'world';
  const region = resolve(REGIONS, id) as Omit<Region, 'id' | 'chain' | 'pack'> & { pack: string };
  return { ...region, id, chain: chain(REGIONS, id), pack: packNamed(region.pack) };
}

export const PACK_NAMES = Object.keys(PACKS).filter((name) => name !== 'base');

export function packNamed(name: string): Pack {
  const pack = resolve(PACKS, name);
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
  return { ...pack, trees, ground, herds: listed(pack.herds, 'animal', ANIMALS) } as unknown as Pack;
}
