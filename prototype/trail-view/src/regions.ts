/**
 * Regions, packs and house styles: three hierarchies of data, in regions/, packs/ and styles/. A
 * region is a ruleset: its terrain rules (which bake.py reads) and its look; it names the pack that
 * holds its models, and a pack names its house style. Each file extends a parent and inherits what
 * it does not say: objects merge, anything else replaces. bake.py picks a spot's region and writes
 * it to meta.region. A pack's `style` and a look's `roadLine` may be a map by country,
 * {"ie": …, "*": …}: the spot's country picks its entry, else "*".
 *
 * Beside each interface stands the key table of its file object. validate() checks every file of
 * regions/, packs/, styles/ and spots/ against them: npm run check runs it, and the page at load.
 */

import { HABITATS, type Habitat } from './bake';
import type { BuildingStyle } from './models/buildings';
import { ANIMALS, COVERS, PLANTS, TREES, type Ground } from './models/library';
import { SEASONS, type Season } from './models/trees';
import type { Weather } from './weather';

/** A check returns its problems, each "key.path: what is wrong". */
type Check = (value: unknown, path: string) => string[];

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const said = (v: unknown) => JSON.stringify(v) ?? String(v);
const wrong = (path: string, what: string) => [path ? `${path}: ${what}` : what];
const number = (low: number, high: number): Check => (v, p) =>
  typeof v === 'number' && v >= low && v <= high ? [] : wrong(p, `${said(v)} is not a number from ${low} to ${high}`);
const positive: Check = (v, p) => (typeof v === 'number' && v > 0 ? [] : wrong(p, `${said(v)} is not a number above 0`));
const text: Check = (v, p) => (typeof v === 'string' && v.length > 0 ? [] : wrong(p, `${said(v)} is not text`));
const slug: Check = (v, p) => (typeof v === 'string' && /^[a-z0-9]+(-[a-z0-9]+)*$/.test(v) ? [] : wrong(p, `${said(v)} is not a lower-case slug`));
const oneOf = (...values: unknown[]): Check => (v, p) => (values.includes(v) ? [] : wrong(p, `${said(v)} is not one of ${values.map(said).join(', ')}`));
const colour: Check = (v, p) => (typeof v === 'string' && /^#[0-9a-f]{6}$/i.test(v) ? [] : wrong(p, `${said(v)} is not a #rrggbb colour`));
const list = (item: Check, length?: number): Check => (v, p) =>
  Array.isArray(v) && (length === undefined ? v.length > 0 : v.length === length)
    ? v.flatMap((x, i) => item(x, `${p}[${i}]`))
    : wrong(p, `${said(v)} is not a list of ${length ?? 'one or more'}`);
/** [low, high] with low below high, or with `equal` at most high. */
const span = (item: Check, equal = false): Check => (v, p) => {
  const problems = list(item, 2)(v, p);
  const [low, high] = v as number[];
  return problems.length || low < high || (equal && low === high) ? problems : wrong(p, `low ${low} is not below high ${high}`);
};
/** An object whose keys each have their check; any other key is unknown. */
const object = (table: Record<string, Check>): Check => (v, p) =>
  !isObject(v)
    ? wrong(p, `${said(v)} is not an object`)
    : Object.entries(v).flatMap(([key, x]) => (table[key] ? table[key](x, p ? `${p}.${key}` : key) : wrong(p ? `${p}.${key}` : key, 'is an unknown key')));
const each = (names: readonly string[], check: Check) => object(Object.fromEntries(names.map((name) => [name, check])));
/** Entries keyed by species, where null drops the parent's. */
const entries = (entry: Check): Check => (v, p) =>
  !isObject(v) ? wrong(p, `${said(v)} is not an object`) : Object.entries(v).flatMap(([id, x]) => (x === null ? [] : entry(x, `${p}.${id}`)));
/** A value, or a map by country code with a "*" entry. */
const byCountry = (check: Check): Check => (v, p) =>
  !isObject(v)
    ? check(v, p)
    : [
        ...('*' in v ? [] : wrong(p, 'has no "*" entry')),
        ...Object.entries(v).flatMap(([c, x]) => (/^([a-z]{2}|\*)$/.test(c) ? check(x, `${p}.${c}`) : wrong(`${p}.${c}`, 'is not a country code or "*"'))),
      ];

/** West, south, east, north. */
type Bounds = [number, number, number, number];
const WORLD: Bounds = [-180, -90, 180, 90];
/** tune/ecoregions.py's clip: a region of a short with an ecoregion keeps its areas inside. */
const EUROPE: Bounds = [-32, 27, 60, 72];
const point = ([w, s, e, n]: Bounds): Check => (v, p) =>
  Array.isArray(v) && v.length === 2 && v[0] >= w && v[0] <= e && v[1] >= s && v[1] <= n ? [] : wrong(p, `${said(v)} is not a [lon, lat] within ${w}..${e}, ${s}..${n}`);
/** A box [west, south, east, north] or a ring of three or more [lon, lat]. */
const area = (bounds: Bounds): Check => (v, p) => {
  if (Array.isArray(v) && v.length === 4 && v.every((x) => typeof x === 'number')) {
    const [w, s, e, n] = v;
    return [...point(bounds)([w, s], p), ...point(bounds)([e, n], p), ...(w < e && s < n ? [] : wrong(p, `${said(v)}: west is not below east, or south below north`))];
  }
  return Array.isArray(v) && v.length >= 3 && v.every(Array.isArray)
    ? list(point(bounds))(v, p)
    : wrong(p, `${said(v)} is not a box [west, south, east, north] or a ring of 3 or more [lon, lat]`);
};

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

/** A pack's entry names its model where its key is not the model's id, and its own species' `latin` where the model stands in. */
const SPECIES = object({
  model: text, latin: text, leaf: oneOf('needleleaved', 'broadleaved'), cover: oneOf('tree', 'shrub'),
  elevation: span(number(-500, 9000)), weight: positive, evergreen: oneOf(true, false),
});
const GROUND = object({
  model: text, latin: text, on: list(oneOf(...COVERS)), height: span(number(0, 50)), share: number(0, 1),
  habitat: oneOf('moor', 'lush'), own: oneOf(true), clumped: oneOf(true),
});
const HERD = object({ model: text, latin: text, groups: number(1, 100), size: span(number(1, 1000), true), on: list(oneOf(...HABITATS)) });

const RAMP_CLASSES = ['shrub', 'grass', 'crop', 'bare', 'built', 'moss', 'needle', 'broad'] as const;
export type Ramp = [string, string, string];
/**
 * Low to high NDVI per WorldCover class; `needle` and `broad` colour woodland by its leaf. A class
 * holds one ramp for the year or one per season, as winter-green Mediterranean hills need; a season
 * it leaves out keeps palette.ts's.
 */
type Ramps = Partial<Record<(typeof RAMP_CLASSES)[number], Ramp | Partial<Record<Season, Ramp>>>>;
const RAMP = list(colour, 3);
const RAMPS = each(RAMP_CLASSES, (v, p) => (Array.isArray(v) ? RAMP(v, p) : each(SEASONS, RAMP)(v, p)));

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
  /** Ramps that replace palette.ts's for these WorldCover classes. A class's value replaces its parent's whole. */
  ramps?: Ramps;
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

const WEATHER = object({
  cover: number(0, 1), base: number(0, 9000), dark: number(0, 1), sun: number(0, 2), ambient: number(0, 2), grey: number(0, 1),
  zenith: colour, horizon: colour, haze: number(0, 2), wet: number(0, 1), rain: number(0, 1), towers: number(0, 1),
});
const LOOK = object({
  snap: number(0, 1), hazeKm: number(1, 500), hazeHeight: number(100, 20_000), mistTop: number(0, 5000),
  softness: number(0, 1), gradient: number(0, 1), ramps: RAMPS,
  moor: object({ colour, heather: colour, share: number(0, 1) }),
  speckle: object({ colour, share: number(0, 1), size_m: number(0, 50) }),
  habitats: each(HABITATS, colour), outcrops: number(0, 1), turf_deg: number(0, 90), rock: colour, cliff: colour,
  snowline: number(0, 9000), bedding: object({ strike: number(0, 360), dip: number(0, 90) }), roads: list(colour, 6),
  roadLine: byCountry(colour), sea: object({ deep: colour, shallow: colour }), windswept_m: number(0, 5000), swell: number(0, 360),
  grade: object({ shadow: list(number(-0.1, 0.1), 3), light: list(number(-0.1, 0.1), 3), saturation: number(0, 3) }),
  weather: each(['sunny', 'cloudy', 'rainy'], WEATHER),
});

/**
 * The models a region draws. In the files, trees, ground plants and herds are objects keyed by
 * species, so a pack adds to its parent's or changes one, and null drops one; a species key that is
 * not its model's id names the model in `model`. A ground plant's entry changes what its plant
 * file says of where it grows.
 */
export interface Pack {
  /** Its file name in packs/. */
  id: string;
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

const PACK = object({
  name: text, about: text, extends: text, trees: entries(SPECIES), ground: entries(GROUND), herds: entries(HERD),
  style: byCountry(text), houses: text, orchard: text,
});
const STYLE = object({
  name: text, about: text, extends: text, pitch: span(number(0, 80)), eave: number(0, 5), verge: number(0, 5), hip: number(0, 1),
  timber: oneOf('none', 'upper', 'all'), shutters: oneOf(true, false), chimneys: oneOf(0, 1, 2),
  palette: object({ render: list(colour), wood: list(colour), roof: list(colour), door: list(colour), shutter: list(colour), plinth: colour, chimney: colour }),
});

export interface Region {
  id: string;
  name: string;
  /** Root first. */
  chain: string[];
  leaf: 'needleleaved' | 'broadleaved' | 'mixed';
  look: RegionLook;
  pack: Pack;
}

/** The rules bake.py reads (where, leaf, cliffs, summits, relief) and the page's look. */
const REGION = (bounds: Bounds) => object({
  name: text, about: text, ecoregion: number(1, 1e6), extends: text, where: list(area(bounds)),
  leaf: oneOf('needleleaved', 'broadleaved', 'mixed'), pack: text,
  cliffs: object({ reach_m: number(0, 1000), raise_m: number(0, 500), min_m: number(0, 1000) }),
  summits: object({ reach_m: number(0, 5000), max_cut_m: number(0, 1000) }),
  relief: object({ radius_m: number(0, 1000), amount: number(0, 5) }),
  look: LOOK,
});

/** A spot in spots/<short>.json; a `photo` name takes position and camera from the Peak View fixture. */
const PHOTO = object({ url: text, credit: text, heading_deg: number(0, 360), pitch_deg: number(-90, 90), vfov_deg: number(1, 180), roll_deg: number(-180, 180) });
const SPOT = object({
  id: slug, name: text, dtm: text, landcover: oneOf('nlcm', 'habmos'), geology: oneOf('igme'), lat: number(-90, 90), lon: number(-180, 180),
  heading: number(0, 360), pitch: number(-90, 90), eye_above_ground_m: number(0, 100), ground_m: number(-500, 9000),
  photo: (v, p) => (typeof v === 'string' ? text(v, p) : PHOTO(v, p)),
});
/** A spot this far outside its ecoregion's polygon is in the wrong spots file; the clipped polygons miss some coast. */
const SPOT_REACH_KM = 10;

type Entry = Record<string, unknown> & { extends?: string };
interface Ecoregion { id: number; short?: string; polygons: [number, number][][][] }

const byName = (files: Record<string, unknown>) =>
  Object.fromEntries(Object.entries(files).map(([path, data]) => [path.split('/').pop()!.replace(/\.json$/, ''), data as Entry]));
const REGIONS = byName(import.meta.glob('../regions/*.json', { eager: true, import: 'default' }));
const PACKS = byName(import.meta.glob('../packs/*.json', { eager: true, import: 'default' }));
const STYLES = byName(import.meta.glob('../styles/*.json', { eager: true, import: 'default' }));
const SPOT_FILES: Record<string, unknown> = byName(import.meta.glob('../spots/*.json', { eager: true, import: 'default' }));
const FIXTURES = byName(import.meta.glob('../../../fixtures/sources/peak-view/photos/*.json', { eager: true, import: 'default' }));
const ECOREGIONS = (Object.values(import.meta.glob('../tune/ecoregions.json', { eager: true, import: 'default' }))[0] as { regions: Ecoregion[] }).regions;

function merge(a: Record<string, unknown>, b: Record<string, unknown>): Record<string, unknown> {
  const out = { ...a };
  for (const [key, value] of Object.entries(b)) out[key] = isObject(value) && isObject(a[key]) ? merge(a[key], value) : value;
  return out;
}

/** The ids from the root down to `id`, or null where a file is missing or the chain loops. */
function lineage(table: Record<string, Entry>, id: string): string[] | null {
  const ids = [id];
  for (let up = table[id]?.extends; up !== undefined; up = table[up].extends) {
    if (!table[up] || ids.includes(up)) return null;
    ids.unshift(up);
  }
  return table[id] ? ids : null;
}

function chain(table: Record<string, Entry>, id: string): string[] {
  const ids = lineage(table, id);
  if (!ids) throw new Error(`${id}: a file is missing or extends loops (npm run check)`);
  return ids;
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

export const REGION_NAMES = Object.keys(REGIONS);

/** A region with its look and pack resolved for a spot in `country`. */
export function regionNamed(id: string, country?: string | null): Region {
  const region = resolve(REGIONS, id) as Omit<Region, 'id' | 'chain' | 'pack'> & { pack: string };
  const ids = chain(REGIONS, id);
  const ramps: Ramps = Object.assign({}, ...ids.map((name) => (REGIONS[name].look as RegionLook | undefined)?.ramps));
  const look = { ...region.look, ramps, roadLine: local(region.look.roadLine, country) };
  return { ...region, id, chain: ids, look, pack: packNamed(region.pack, country) };
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
  return { ...pack, id: name, trees, ground, herds: listed(pack.herds, 'animal', ANIMALS), style: style(local(pack.style as string, country)), houses } as unknown as Pack;
}

/** Kilometres from a point to the nearest of these polygons, 0 inside one. */
function outside(polygons: [number, number][][][], lon: number, lat: number): number {
  const kx = 111.32 * Math.cos((lat * Math.PI) / 180);
  let [inside, nearest] = [false, Infinity];
  for (const ring of polygons.flat()) {
    for (let i = 0; i + 1 < ring.length; i++) {
      const [[x0, y0], [x1, y1]] = [ring[i], ring[i + 1]];
      if (y0 > lat !== y1 > lat && lon < x0 + ((lat - y0) * (x1 - x0)) / (y1 - y0)) inside = !inside;
      const [ax, ay, dx, dy] = [(x0 - lon) * kx, (y0 - lat) * 111.32, (x1 - x0) * kx, (y1 - y0) * 111.32];
      const t = Math.min(1, Math.max(0, -(ax * dx + ay * dy) / (dx * dx + dy * dy || 1)));
      nearest = Math.min(nearest, Math.hypot(ax + t * dx, ay + t * dy));
    }
  }
  return inside ? 0 : nearest;
}

/** Every problem in the data files, one line each: the file, the key path and what is wrong. */
export function validate(): string[] {
  const out: string[] = [];
  const add = (file: string, problems: string[]) => out.push(...problems.map((problem) => `${file}: ${problem}`));
  const short = (name: unknown) => String(name).split('.')[0];

  // Keys and types, then each file's parent: it exists, the chain does not loop, and it is of the file's own short.
  for (const name of REGION_NAMES) add(`regions/${name}.json`, REGION(REGIONS[short(name)]?.ecoregion !== undefined ? EUROPE : WORLD)(REGIONS[name], ''));
  for (const [name, data] of Object.entries(PACKS)) add(`packs/${name}.json`, PACK(data, ''));
  for (const [name, data] of Object.entries(STYLES)) add(`styles/${name}.json`, STYLE(data, ''));
  for (const [kind, table, root] of [['region', REGIONS, 'world'], ['pack', PACKS, 'base'], ['style', STYLES, '']] as const) {
    for (const [name, { extends: up }] of Object.entries(table)) {
      const file = `${kind}s/${name}.json`;
      // A subregion is only ever entered from its own short, so it may not extend the root.
      const sub = kind === 'region' && name.includes('.');
      if (root && name !== root && (sub || up !== root) && short(up) !== short(name)) add(file, [`extends: ${said(up)} is not ${sub ? '' : `${root} or `}a ${kind} of ${short(name)}`]);
      if (sub && !(table[name] as { where?: unknown }).where) add(file, ['where: a subregion needs its areas']);
      if (up === undefined) continue;
      if (!table[up]) add(file, [`extends: no ${kind}s/${up}.json`]);
      const loop = [name];
      for (let at: unknown = up; typeof at === 'string' && table[at] && loop.length <= Object.keys(table).length; at = table[at].extends) {
        loop.push(at);
        if (at !== name) continue;
        add(file, [`extends: a loop ${loop.join(' › ')}`]);
        break;
      }
    }
  }

  for (const [name, region] of Object.entries(REGIONS)) {
    const file = `regions/${name}.json`;
    if (typeof region.pack === 'string' && !PACKS[region.pack]) add(file, [`pack: no packs/${region.pack}.json`]);
    if (region.ecoregion !== undefined && ECOREGIONS.find((e) => e.short === name)?.id !== region.ecoregion) {
      add(file, [`ecoregion: ${said(region.ecoregion)} is not the id of ${name} in tune/ecoregions.json`]);
    }
  }

  // Each pack's own entries, as they resolve over its parents, name a model of their kind.
  const kinds = [['trees', { ...TREES, ...PLANTS }, 'tree or plant'], ['ground', PLANTS, 'plant'], ['herds', ANIMALS, 'animal']] as const;
  for (const [name, pack] of Object.entries(PACKS)) {
    const file = `packs/${name}.json`;
    const styles = [...Object.entries(isObject(pack.style) ? pack.style : { '': pack.style }).map(([c, s]) => [c ? `style.${c}` : 'style', s]), ['houses', pack.houses]];
    for (const [key, style] of styles) if (typeof style === 'string' && !STYLES[style]) add(file, [`${key}: no styles/${style}.json`]);
    if (typeof pack.orchard === 'string' && !(pack.orchard in TREES)) add(file, [`orchard: ${pack.orchard} is not a tree model`]);
    if (!lineage(PACKS, name)) continue;
    const resolved = resolve(PACKS, name);
    for (const [table, models, kind] of kinds) {
      const all = isObject(resolved[table]) ? resolved[table] : {};
      for (const [id, own] of Object.entries(isObject(pack[table]) ? pack[table] : {})) {
        const entry = all[id];
        if (own === null || !isObject(entry)) continue;
        const model = String(entry.model ?? id);
        if (!(model in models)) add(file, [`${table}.${id}: ${model} is not a ${kind} model`]);
        else if (table === 'trees' && entry.cover === 'tree' && !(model in TREES)) add(file, [`${table}.${id}: stands as a tree, but ${model} is not a tree model`]);
      }
    }
  }

  // Spots: keys, one id each across the files, a position, and in or near their ecoregion.
  const seen = new Map<string, string>();
  for (const [name, spots] of Object.entries(SPOT_FILES)) {
    const file = `spots/${name}.json`;
    if (!REGIONS[name]) add(file, wrong('', `has no regions/${name}.json, where its spots' region walk starts`));
    if (!Array.isArray(spots)) {
      add(file, wrong('', 'is not a list of spots'));
      continue;
    }
    const ecoregion = ECOREGIONS.find((e) => e.id === REGIONS[name]?.ecoregion);
    spots.forEach((spot: unknown, i) => {
      const at = isObject(spot) && typeof spot.id === 'string' ? spot.id : `[${i}]`;
      add(file, SPOT(spot, at));
      if (!isObject(spot)) return;
      for (const key of ['id', 'name']) if (spot[key] === undefined) add(file, [`${at}.${key}: is missing`]);
      if (typeof spot.id === 'string') {
        if (seen.has(spot.id)) add(file, [`${at}.id: also in ${seen.get(spot.id)}`]);
        else seen.set(spot.id, file);
      }
      const fixture = typeof spot.photo === 'string' ? FIXTURES[spot.photo] : undefined;
      if (typeof spot.photo === 'string' && !fixture) add(file, [`${at}.photo: no fixtures/sources/peak-view/photos/${spot.photo}.json`]);
      const { lat, lon } = spot.lat !== undefined || !fixture ? spot : fixture;
      if (typeof lat !== 'number' || typeof lon !== 'number') {
        if (typeof spot.photo !== 'string') add(file, [`${at}: has no lat and lon`]);
        return;
      }
      const km = ecoregion ? outside(ecoregion.polygons, lon, lat) : 0;
      if (km > SPOT_REACH_KM) add(file, [`${at}: ${km.toFixed(1)} km outside the ${name} ecoregion of tune/ecoregions.json`]);
    });
  }

  // One model per Latin name.
  const latins = new Map<string, string>();
  for (const [folder, models] of [['trees', TREES], ['plants', PLANTS], ['animals', ANIMALS]] as const) {
    for (const [id, { latin }] of Object.entries(models)) {
      const file = `src/models/${folder}/${id}.ts`;
      if (latins.has(latin)) add(file, [`latin: ${latin} is also ${latins.get(latin)}`]);
      else latins.set(latin, file);
    }
  }
  return out;
}
