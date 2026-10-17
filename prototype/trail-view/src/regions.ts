/**
 * Regions and packs: two hierarchies of data, in regions/ and packs/. A region is a ruleset: the
 * boxes it covers, its data sources and coast rules (which bake.py reads), and its look; it names
 * the pack that holds its models. Each file extends a parent and inherits what it does not say:
 * objects merge, anything else replaces. A spot's region is the deepest one whose box holds it.
 */

import type { Habitat } from './bake';
import type { AnimalName } from './models/animals';
import type { BuildingStyle } from './models/buildings';
import type { PlantName } from './models/plants';
import type { TreeName } from './models/trees';
import type { Weather } from './weather';

export interface Species {
  tree: TreeName;
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

/** How the ground, sea and air read in a region. */
export interface RegionLook {
  /** Palette snap: 0 takes the ground's chroma from Sentinel-2, 1 paints the palette alone. */
  snap: number;
  /** Haze distance at sea level, in km. */
  hazeKm: number;
  /** Ramps that replace palette.ts's for these WorldCover classes, low NDVI to high. */
  ramps?: Partial<Record<'shrub' | 'grass' | 'crop' | 'bare' | 'built' | 'moss', [string, string, string]>>;
  /**
   * Moorland: open ground that Sentinel-2 sees brown rather than green, painted in its own
   * colour with heather patches over a share of it. Heather plants grow there and grass does not.
   */
  moor?: { colour: string; heather: string; share: number };
  /** Colours for the habitats of a fine land cover map, where the bake has one; others keep the palette. */
  habitats?: Partial<Record<Habitat, string>>;
  /** How much open ground breaks through as rock slabs on slopes, ridges and poor grass, 0–1. */
  outcrops: number;
  /** Slabs and outcrops, and cliff walls unless `cliff` says otherwise. */
  rock: string;
  /** Cliff walls: often darker than sunlit slabs, shaded, wet and lichened. */
  cliff?: string;
  /** Out of winter, snow lies only above this altitude; below it a white pixel is rock or spray. */
  snowline: number;
  /** The rock's bedding planes in degrees, so cliffs and slabs show their strata as they lie. */
  bedding?: { strike: number; dip: number };
  /** Open water away from the shore, and the sea beside it. */
  sea: { deep: string; shallow: string };
  /** The bearing the prevailing swell comes from, in degrees: shores open to it get the heavy surf. */
  swell?: number;
  /** The colour grade: shadow and light tints added in linear light, and the saturation. */
  grade: { shadow: [number, number, number]; light: [number, number, number]; saturation: number };
  /** What the climate changes in each weather preset. */
  weather?: Partial<Record<'sunny' | 'cloudy' | 'rainy', Partial<Weather>>>;
}

/** The models a region draws. */
export interface Pack {
  name: string;
  trees: Species[];
  ground: PlantName[];
  style: BuildingStyle;
  /** What cottages and houses change from the style, where they differ from farms. */
  houses?: Partial<BuildingStyle>;
  /** The tree that stands in OSM orchards; without one, the pack's broadleaves do. */
  orchard?: TreeName;
  /** Herds near the eye: how many groups of how many, on which habitats (on WorldCover grass without a map). */
  herds?: { animal: AnimalName; groups: number; size: [number, number]; on: Habitat[] }[];
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
type Entry = Record<string, unknown> & { extends?: string; where?: Box[] };

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

const holds = ([w, s, e, n]: Box, lat: number, lon: number) => w <= lon && lon <= e && s <= lat && lat <= n;
const area = ([w, s, e, n]: Box) => (e - w) * (n - s);

/** The region of a spot, with its look and pack resolved. */
export function regionAt(lat: number, lon: number): Region {
  const fits = Object.keys(REGIONS).flatMap((id) => (REGIONS[id].where ?? []).filter((box) => holds(box, lat, lon)).map((box) => ({ id, box })));
  fits.sort((a, b) => chain(REGIONS, b.id).length - chain(REGIONS, a.id).length || area(a.box) - area(b.box));
  const id = fits[0]?.id ?? 'world';
  const region = resolve(REGIONS, id) as Omit<Region, 'id' | 'chain' | 'pack'> & { pack: string };
  return { ...region, id, chain: chain(REGIONS, id), pack: packNamed(region.pack) };
}

export const PACK_NAMES = Object.keys(PACKS).filter((name) => name !== 'base');

export const packNamed = (name: string) => resolve(PACKS, name) as unknown as Pack;
