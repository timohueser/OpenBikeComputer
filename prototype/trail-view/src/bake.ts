/** One baked preset, as `bake.py` writes it. */

export interface Photo {
  file: string;
  credit: string;
  heading_deg: number;
  pitch_deg: number;
  /** Peak View fixtures state a 35 mm equivalent lens; aligned photographs a vertical field. */
  lens_mm?: number;
  vfov_deg?: number;
  roll_deg?: number;
}

export interface Meta {
  id: string;
  lat: number;
  lon: number;
  heading?: number;
  pitch?: number;
  ground_m: number;
  eye_m: number;
  eye_above_ground_m: number;
  levels: number;
  size: number;
  /** Height texels per side of a level; an older bake has `size`. */
  height_size?: number;
  half0_m: number;
  /** Levels k < dtm_levels carry a national DTM; `glo30` holds GLO-30 for the same levels. */
  dtm_levels: number;
  dtm: string | null;
  /** Levels k < s2_levels carry Sentinel-2 colour, k < road_levels road surfaces, k < water_levels OSM water. */
  s2_levels: number;
  road_levels: number;
  water_levels: number;
  /** The water levels' shore distance runs to this many metres. */
  shore_reach_m: number;
  credits: string[];
  photo?: Photo;
  /** The eye's region (regions/<region>.json) and its chain from world down. */
  region: string;
  chain: string[];
  /** The regions the region layer indexes. */
  regions: string[];
  /** ISO 3166-1 alpha-2 in lower case, null off every country. */
  country?: string | null;
  /** The country's first-level subdivision, lower case as in tune/countries.json, null off every one. */
  subdivision?: string | null;
  /** The IANA time zone at the spot. */
  tz?: string;
}

/**
 * A footprint in metres from the eye: its lowest ground above sea level, the OSM building value,
 * levels and height tags, and its place: the subdivision, else the country, it stands in.
 */
export type Building = [base: number, building: string, levels: number | null, height: number | null, ring: number[], place: string | null];

export interface Bake {
  meta: Meta;
  base: string;
  /** Height above the eye per level, row 0 north: the DTM where there is one, GLO-30 beyond. */
  surface: Float32Array[];
  /** GLO-30 for the levels that `surface` takes from the DTM. */
  glo30: Float32Array[];
  /** R8 per texel and level: the material (materials.ts), the lithology and the index into meta.regions. */
  material: Uint8Array;
  litho: Uint8Array;
  region: Uint8Array;
  /** R8 per texel, k < s2_levels: the most NDVI any quarter saw, in their alpha code. */
  ndvimax: Uint8Array;
  /** RGB8 per texel, k < road_levels: distance to the nearest road edge in 0.1 m, its surface code and half-width in 0.05 m. */
  roads: Uint8Array;
  /** RG8 per texel, k < water_levels: the share of the texel under OSM water, 0–255, and sqrt(shore distance / shore_reach_m). */
  water: Uint8Array;
  /** x0, y0, x1, y1, kind (0 wall, 1 hedge) per piece of wall or hedge. */
  walls: Float32Array;
  /** R8 per texel, k < dtm_levels: canopy height in 0.25 m. */
  canopy: Uint8Array;
  /** x, y, height, type (0 conifer, 1 broadleaf, 2 orchard) per tree, metres from the eye. */
  trees: Float32Array;
  buildings: Building[];
  /** Cable cars and lifts: the OSM aerialway kind and the line station to station, x, y in metres. */
  aerialways: { kind: string; line: number[] }[];
  /** Bridges within the near field: the road's surface code (ROAD_WIDTHS order, from 1), half-width and line. */
  bridges: { code: number; half: number; line: number[] }[];
  /** Bytes on the wire: heights and the ground layers. */
  bytes: number;
}

/** One file's bytes, unpacked, and what it cost on the wire. */
async function unpack(url: string): Promise<{ data: ArrayBuffer; bytes: number }> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url}: ${response.status}`);
  const packed = await response.arrayBuffer();
  // A server that sends the file as Content-Encoding gzip hands over the bytes already unpacked.
  const gzipped = packed.byteLength >= 2 && new Uint8Array(packed, 0, 2).every((byte, i) => byte === [0x1f, 0x8b][i]);
  const data = gzipped
    ? await new Response(new Blob([packed]).stream().pipeThrough(new DecompressionStream('gzip'))).arrayBuffer()
    : packed;
  // With Content-Encoding the transfer size is the header's, not the unpacked body's.
  return { data, bytes: Number(response.headers.get('content-length')) || packed.byteLength };
}

/** The levels of `name-{k}.bin.gz` for k < count, end to end. */
async function layers(base: string, name: string, count: number): Promise<{ data: Uint8Array; bytes: number }> {
  const parts = await Promise.all([...Array(count).keys()].map((k) => unpack(`${base}${name}-${k}.bin.gz`)));
  const data = new Uint8Array(parts.reduce((sum, part) => sum + part.data.byteLength, 0));
  let at = 0;
  for (const part of parts) {
    data.set(new Uint8Array(part.data), at);
    at += part.data.byteLength;
  }
  return { data, bytes: parts.reduce((sum, part) => sum + part.bytes, 0) };
}

/** RGBA8 per texel for the colour levels of quarter 1–4: visible reflectance^(1/2.2), and NDVI. */
export function loadQuarter(bake: Bake, quarter: number) {
  return layers(bake.base, `s2-q${quarter}`, bake.meta.s2_levels);
}

async function heights(url: string, eye: number): Promise<{ data: Float32Array; bytes: number }> {
  const { data: raw, bytes } = await unpack(url);
  const encoded = new Uint16Array(raw);
  const data = new Float32Array(encoded.length);
  for (let i = 0; i < encoded.length; i++) data[i] = encoded[i] / 10 - 500 - eye;
  return { data, bytes };
}

/** A list an older bake may lack. */
const optional = (url: string): Promise<unknown[]> => fetch(url).then((r) => (r.ok ? r.json() : [])).catch(() => []);

export async function loadBake(id: string): Promise<Bake> {
  const base = `/${id}/`;
  const response = await fetch(`${base}meta.json`);
  if (!response.ok) throw new Error(`no bake for ${id}: run uv run prototype/trail-view/bake.py ${id}`);
  const meta: Meta = await response.json();
  const levels = [...Array(meta.levels).keys()];
  const [surface, glo30, material, litho, region, ndvimax, roads, water, canopy, trees, buildings, walls, aerialways, bridges] = await Promise.all([
    Promise.all(levels.map((k) => heights(`${base}h-${k}.bin.gz`, meta.eye_m))),
    Promise.all(levels.slice(0, meta.dtm_levels).map((k) => heights(`${base}glo30-${k}.bin.gz`, meta.eye_m))),
    layers(base, 'material', meta.levels),
    layers(base, 'litho', meta.levels),
    layers(base, 'region', meta.levels),
    layers(base, 'ndvimax', meta.s2_levels),
    layers(base, 'road', meta.road_levels),
    layers(base, 'water', meta.water_levels),
    layers(base, 'canopy', meta.dtm_levels),
    unpack(`${base}trees.bin.gz`),
    fetch(`${base}buildings.json`).then((r) => r.json()),
    unpack(`${base}walls.bin.gz`),
    optional(`${base}aerialways.json`),
    optional(`${base}bridges.json`),
  ]);
  const bytes = [...surface, ...glo30, material, litho, region, ndvimax, roads, water, canopy, trees, walls].reduce((sum, level) => sum + level.bytes, 0);
  return {
    meta,
    base,
    surface: surface.map((level) => level.data),
    glo30: glo30.map((level) => level.data),
    material: material.data,
    litho: litho.data,
    region: region.data,
    ndvimax: ndvimax.data,
    roads: roads.data,
    water: water.data,
    canopy: canopy.data,
    trees: new Float32Array(trees.data),
    buildings,
    walls: new Float32Array(walls.data),
    aerialways: (aerialways as [string, number[]][]).map(([kind, line]) => ({ kind, line })),
    bridges: (bridges as [number, number, number[]][]).map(([code, half, line]) => ({ code, half, line })),
    bytes,
  };
}

export const heightSize = (meta: Meta) => meta.height_size ?? meta.size;

/** The height above the eye at texel (i, j) of the other layers' grid on level k. */
export function heightAtTexel(bake: Bake, k: number, i: number, j: number): number {
  const f = heightSize(bake.meta) / bake.meta.size;
  return bake.surface[k][j * f * heightSize(bake.meta) + i * f];
}

/** The bare ground's height above the eye at x, y, bilinear from the finest level that holds it. */
export function groundAt(bake: Bake, x: number, y: number): number {
  const { half0_m, levels } = bake.meta;
  const size = heightSize(bake.meta);
  const k = Math.min(Math.max(0, Math.ceil(Math.log2(Math.max(Math.abs(x), Math.abs(y), 1) / half0_m))), levels - 1);
  const half = half0_m * 2 ** k;
  const px = (2 * half) / size;
  const [u, v] = [(x + half) / px - 0.5, (half - y) / px - 0.5];
  const [i, j] = [Math.min(Math.max(Math.floor(u), 0), size - 2), Math.min(Math.max(Math.floor(v), 0), size - 2)];
  const [fu, fv] = [u - i, v - j];
  const h = (di: number, dj: number) => bake.surface[k][(j + dj) * size + i + di];
  return (h(0, 0) * (1 - fu) + h(1, 0) * fu) * (1 - fv) + (h(0, 1) * (1 - fu) + h(1, 1) * fu) * fv;
}
