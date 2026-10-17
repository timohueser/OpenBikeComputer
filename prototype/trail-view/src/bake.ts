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
  /** [bearing, elevation] in degrees, on a quarter-degree bearing grid. Fixtures only. */
  columns?: [number, number][];
  rms_limit_deg?: number;
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
  half0_m: number;
  /** Levels k < dtm_levels carry a national DTM; `glo30` holds GLO-30 for the same levels. */
  dtm_levels: number;
  dtm: string | null;
  /** Levels k < s2_levels carry Sentinel-2 colour, k < road_levels road surfaces, k < water_levels OSM water. */
  s2_levels: number;
  road_levels: number;
  water_levels: number;
  credits: string[];
  photo?: Photo;
}

export interface Peak {
  /** Metres east and north of the eye. */
  x: number;
  y: number;
  ele: number | null;
  name: string;
}

/** A footprint in metres from the eye: its lowest ground above sea level, the OSM building value, levels and height tags. */
export type Building = [base: number, building: string, levels: number | null, height: number | null, ring: number[]];

export interface Bake {
  meta: Meta;
  base: string;
  /** Height above the eye per level, row 0 north: the DTM where there is one, GLO-30 beyond. */
  surface: Float32Array[];
  /** GLO-30 for the levels that `surface` takes from the DTM. */
  glo30: Float32Array[];
  /** RG8 per texel and level: the class's palette index, and 255 on water. */
  classes: Uint8Array;
  /** RG8 per texel, k < road_levels: distance to the nearest road in 0.1 m, and its surface code. */
  roads: Uint8Array;
  /** R8 per texel, k < water_levels: the share of the texel under OSM water, 0–255. */
  water: Uint8Array;
  /** R8 per texel, k < dtm_levels: canopy height in 0.25 m. */
  canopy: Uint8Array;
  /** x, y, height, broadleaf (0 or 1) per tree, metres from the eye. */
  trees: Float32Array;
  buildings: Building[];
  peaks: Peak[];
  /** Bytes on the wire: heights and classes. */
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

export async function loadBake(id: string): Promise<Bake> {
  const base = `/${id}/`;
  const response = await fetch(`${base}meta.json`);
  if (!response.ok) throw new Error(`no bake for ${id}: run uv run prototype/trail-view/bake.py ${id}`);
  const meta: Meta = await response.json();
  const levels = [...Array(meta.levels).keys()];
  const [surface, glo30, classes, roads, water, canopy, trees, buildings, peaks] = await Promise.all([
    Promise.all(levels.map((k) => heights(`${base}h-${k}.bin.gz`, meta.eye_m))),
    Promise.all(levels.slice(0, meta.dtm_levels).map((k) => heights(`${base}glo30-${k}.bin.gz`, meta.eye_m))),
    layers(base, 'class', meta.levels),
    layers(base, 'road', meta.road_levels),
    layers(base, 'water', meta.water_levels),
    layers(base, 'canopy', meta.dtm_levels),
    unpack(`${base}trees.bin.gz`),
    fetch(`${base}buildings.json`).then((r) => r.json()),
    fetch(`${base}peaks.json`).then((r) => (r.ok ? r.json() : [])),
  ]);
  const bytes = [...surface, ...glo30, classes, roads, water, canopy, trees].reduce((sum, level) => sum + level.bytes, 0);
  return {
    meta,
    base,
    surface: surface.map((level) => level.data),
    glo30: glo30.map((level) => level.data),
    classes: classes.data,
    roads: roads.data,
    water: water.data,
    canopy: canopy.data,
    trees: new Float32Array(trees.data),
    buildings,
    peaks: (peaks as [number, number, number | null, string][]).map(([x, y, ele, name]) => ({ x, y, ele, name })),
    bytes,
  };
}
