const metadata = new Map();
export class MissingArchive extends Error {}
/** The grid formats that the Worker reads: format 3 keeps the pointers of the packs of a grid cell in a
 * block index of 16 × 16 cells (specs/planner-release.md). */
export const GRIDS = [2, 3];
const BLOCK_ZOOMS = 4;
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
  while (metadata.size > 64) metadata.delete(metadata.keys().next().value);
  return value;
}

export async function gridConfig(bucket, prefix) {
  const grid = await readJSON(bucket, `${prefix}/public/grid.json`);
  if (!GRIDS.includes(grid.format) || !Number.isInteger(grid.map_zoom) || grid.map_zoom < 9 || grid.map_zoom > 14) {
    throw new Error('Invalid map grid');
  }
  return grid;
}

async function readPointer(bucket, prefix, name) {
  const cell = CELL.exec(name);
  if (!cell || (await gridConfig(bucket, prefix)).format < 3) return readJSON(bucket, `${prefix}/public/${name}.json`);
  const [z, x, y] = cell.slice(2, 5).map(Number);
  const zoom = Math.max(z - BLOCK_ZOOMS, 0), shift = z - zoom;
  const block = await readJSON(bucket, `${prefix}/public/${cell[1]}/${zoom}-${x >> shift}-${y >> shift}.json`, 64 * 1024);
  const file = name.slice(cell[1].length + 1);
  if (!Object.hasOwn(block, file)) throw new MissingArchive();
  return block[file];
}

export async function objectPointer(bucket, prefix, name) {
  const value = await readPointer(bucket, prefix, name);
  if (!/^[a-f0-9]{64}$/.test(value.sha256) || !['identity', 'gzip'].includes(value.encoding)
    || !Number.isSafeInteger(value.bytes) || value.bytes < 0
    || !Number.isSafeInteger(value.decoded_bytes) || value.decoded_bytes < 0) throw new Error('Invalid public object pointer');
  return { ...value, path: `objects/${value.sha256}` };
}

export async function publicFile(bucket, prefix, name, contentType, headers) {
  const pointer = await objectPointer(bucket, prefix, name);
  const object = await bucket.get(pointer.path);
  if (!object) throw new MissingArchive();
  if (object.size !== pointer.bytes) throw new Error('Public object size mismatch');
  const body = pointer.encoding === 'gzip' ? object.body.pipeThrough(new DecompressionStream('gzip')) : object.body;
  return new Response(body, { headers: { ...headers, 'Content-Type': contentType } });
}

export function packName(name, tile, zoom) {
  const [z, x, y] = tile, level = Math.min(z, zoom), divisor = 2 ** (z - level);
  return `maps/tiles/${name}/${level}-${Math.floor(x / divisor)}-${Math.floor(y / divisor)}.pmtiles`;
}

export function assetRoute(path) {
  const match = /^\/releases\/([a-f0-9]{64})\/(device\/catalog\.json|trailview\/rules\.json|routes\/tiles\/9-[0-9]+-[0-9]+\.json|maps\/assets\/(?:fonts\/[A-Za-z0-9 _,-]+\/[0-9]+-[0-9]+\.pbf|sprites\/v[0-9]+\/[a-z]+(?:@2x)?\.(?:json|png)))$/.exec(path);
  if (!match) return null;
  return { release: match[1], file: match[2], type: match[2].endsWith('.json') ? 'application/json'
    : match[2].endsWith('.png') ? 'image/png' : 'application/x-protobuf' };
}
