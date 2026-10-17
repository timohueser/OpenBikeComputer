/** One baked preset, as `bake.py` writes it. */

export interface Photo {
  file: string;
  heading_deg: number;
  pitch_deg: number;
  lens_mm: number;
  /** [bearing, elevation] in degrees, on a quarter-degree bearing grid. */
  columns: [number, number][];
  rms_limit_deg: number;
  subject: string;
}

export interface Meta {
  id: string;
  lat: number;
  lon: number;
  heading?: number;
  pitch?: number;
  ground_m: number;
  eye_m: number;
  levels: number;
  size: number;
  half0_m: number;
  /** Levels k < dtm_levels carry a national DTM; `glo30` holds GLO-30 for the same levels. */
  dtm_levels: number;
  dtm: string | null;
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

export interface Bake {
  meta: Meta;
  base: string;
  /** Height above the eye per level, row 0 north: the DTM where there is one, GLO-30 beyond. */
  surface: Float32Array[];
  /** GLO-30 for the levels that `surface` takes from the DTM. */
  glo30: Float32Array[];
  peaks: Peak[];
  bytes: number;
}

async function heights(url: string, eye: number): Promise<{ data: Float32Array; bytes: number }> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url}: ${response.status}`);
  const packed = await response.arrayBuffer();
  // A server that sends the file as Content-Encoding gzip hands over the bytes already unpacked.
  const gzipped = new Uint8Array(packed, 0, 2).every((byte, i) => byte === [0x1f, 0x8b][i]);
  const raw = gzipped
    ? await new Response(new Blob([packed]).stream().pipeThrough(new DecompressionStream('gzip'))).arrayBuffer()
    : packed;
  const encoded = new Uint16Array(raw);
  const data = new Float32Array(encoded.length);
  for (let i = 0; i < encoded.length; i++) data[i] = encoded[i] / 10 - 500 - eye;
  // With Content-Encoding the transfer size is the header's, not the unpacked body's.
  return { data, bytes: Number(response.headers.get('content-length')) || packed.byteLength };
}

export async function loadBake(id: string): Promise<Bake> {
  const base = `/${id}/`;
  const response = await fetch(`${base}meta.json`);
  if (!response.ok) throw new Error(`no bake for ${id}: run uv run prototype/trail-view/bake.py ${id}`);
  const meta: Meta = await response.json();
  const levels = [...Array(meta.levels).keys()];
  const [surface, glo30, peaks] = await Promise.all([
    Promise.all(levels.map((k) => heights(`${base}h-${k}.bin.gz`, meta.eye_m))),
    Promise.all(levels.slice(0, meta.dtm_levels).map((k) => heights(`${base}glo30-${k}.bin.gz`, meta.eye_m))),
    fetch(`${base}peaks.json`).then((r) => (r.ok ? r.json() : [])),
  ]);
  const bytes = [...surface, ...glo30].reduce((sum, level) => sum + level.bytes, 0);
  return {
    meta,
    base,
    surface: surface.map((level) => level.data),
    glo30: glo30.map((level) => level.data),
    peaks: (peaks as [number, number, number | null, string][]).map(([x, y, ele, name]) => ({ x, y, ele, name })),
    bytes,
  };
}
