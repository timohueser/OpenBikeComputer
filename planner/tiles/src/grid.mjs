// At most 256 blocks of 64 KiB: about 20 MiB parsed, of the Worker's 128 MB.
const METADATA_ENTRIES = 256;
const metadata = new Map();
export class MissingArchive extends Error {}
/** The grid formats that the Worker reads: format 3 keeps the pointers of the packs of a grid cell in a
 * block index of 16 × 16 cells; format 4 packs the tiles of a base square, split where a pack is too large,
 * with the pointers of all its packs in the block index of the base square (specs/planner-release.md). */
export const GRIDS = [3, 4];
const BLOCK_ZOOMS = 4;
// The base levels of format 4, deepest first: a tile zoom belongs to the first level not deeper than it.
const LEVELS = [9, 7, 0];
const CELL = /^((?:maps|routes)\/tiles(?:\/.+)?)\/([0-9]+)-([0-9]+)-([0-9]+)\.[a-z]+$/;

async function readJSON(bucket, path, limit = 1024) {
  const cached = metadata.get(path);
  if (cached) {
    metadata.delete(path); metadata.set(path, cached);
    return cached;
  }
  const object = await bucket.get(path);
  if (!object) throw new MissingArchive();
  if (object.size > limit) throw new Error('Invalid public object pointer');
  const value = await object.json();
  metadata.set(path, value);
  while (metadata.size > METADATA_ENTRIES) metadata.delete(metadata.keys().next().value);
  return value;
}

export async function gridConfig(bucket, prefix) {
  const grid = await readJSON(bucket, `${prefix}/public/grid.json`);
  if (grid.format === 4 || (grid.format === 3 && Number.isInteger(grid.map_zoom) && grid.map_zoom >= 9 && grid.map_zoom <= 14)) {
    return grid;
  }
  throw new Error('Invalid map grid');
}

async function readPointer(bucket, prefix, name) {
  const cell = CELL.exec(name);
  if (!cell) return readJSON(bucket, `${prefix}/public/${name}.json`);
  const [z, x, y] = cell.slice(2, 5).map(Number);
  const zoom = Math.max(z - BLOCK_ZOOMS, 0), shift = z - zoom;
  const block = await readJSON(bucket, `${prefix}/public/${cell[1]}/${zoom}-${x >> shift}-${y >> shift}.json`, 64 * 1024);
  const file = name.slice(cell[1].length + 1);
  if (!Object.hasOwn(block, file)) throw new MissingArchive();
  return block[file];
}

function validPointer(value) {
  if (!/^[a-f0-9]{64}$/.test(value.sha256) || !['identity', 'gzip'].includes(value.encoding)
    || !Number.isSafeInteger(value.bytes) || value.bytes < 0
    || !Number.isSafeInteger(value.decoded_bytes) || value.decoded_bytes < 0) throw new Error('Invalid public object pointer');
  return { ...value, path: `objects/${value.sha256}` };
}

export async function objectPointer(bucket, prefix, name) {
  return validPointer(await readPointer(bucket, prefix, name));
}

export async function publicFile(bucket, prefix, name, contentType, headers) {
  const pointer = await objectPointer(bucket, prefix, name);
  const object = await bucket.get(pointer.path);
  if (!object) throw new MissingArchive();
  if (object.size !== pointer.bytes) throw new Error('Public object size mismatch');
  const body = pointer.encoding === 'gzip' ? object.body.pipeThrough(new DecompressionStream('gzip')) : object.body;
  return new Response(body, { headers: { ...headers, 'Content-Type': contentType } });
}

function packName(name, tile, zoom) {
  const [z, x, y] = tile, level = Math.min(z, zoom), divisor = 2 ** (z - level);
  return `maps/tiles/${name}/${level}-${Math.floor(x / divisor)}-${Math.floor(y / divisor)}.pmtiles`;
}

/** The pointer of the pack that holds a tile, null where the grid has none: in format 4 the deepest pack
 * of the tile's square from the tile zoom back to its base level. */
export async function packPointer(bucket, prefix, name, tile) {
  const grid = await gridConfig(bucket, prefix);
  const absent = error => { if (error instanceof MissingArchive) return null; throw error; };
  if (grid.format === 3) return objectPointer(bucket, prefix, packName(name, tile, grid.map_zoom)).catch(absent);
  const [z, x, y] = tile, base = LEVELS.find(level => level <= z), zoom = Math.max(base - 2, 0);
  const block = await readJSON(bucket, `${prefix}/public/maps/tiles/${name}/${zoom}-${x >> (z - zoom)}-${y >> (z - zoom)}.json`, 64 * 1024)
    .catch(absent);
  for (let level = z; block && level >= base; level--) {
    const file = `${level}-${x >> (z - level)}-${y >> (z - level)}.pmtiles`;
    if (Object.hasOwn(block, file)) return validPointer(block[file]);
  }
  return null;
}

export function assetRoute(path) {
  const match = /^\/releases\/([a-f0-9]{64})\/(device\/catalog\.json|trailview\/rules\.json|routes\/tiles\/9-[0-9]+-[0-9]+\.json|maps\/assets\/(?:fonts\/[A-Za-z0-9 _,-]+\/[0-9]+-[0-9]+\.pbf|sprites\/v[0-9]+\/[a-z]+(?:@2x)?\.(?:json|png)))$/.exec(path);
  if (!match) return null;
  return { release: match[1], file: match[2], type: match[2].endsWith('.json') ? 'application/json'
    : match[2].endsWith('.png') ? 'image/png' : 'application/x-protobuf' };
}
