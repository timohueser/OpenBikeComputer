const metadata = new Map();
export class MissingArchive extends Error {}

async function readJSON(bucket, path, optional = false) {
  const cached = metadata.get(path);
  if (cached) {
    metadata.delete(path); metadata.set(path, cached);
    return cached;
  }
  const object = await bucket.get(path);
  if (!object) {
    if (optional) return null;
    throw new MissingArchive();
  }
  if (object.size > 1024) throw new Error('Invalid public object pointer');
  const value = await object.json();
  metadata.set(path, value);
  while (metadata.size > 64) metadata.delete(metadata.keys().next().value);
  return value;
}

export async function gridConfig(bucket, prefix) {
  const grid = await readJSON(bucket, `${prefix}/public/grid.json`, true);
  if (grid && (grid.format !== 2 || !Number.isInteger(grid.map_zoom) || grid.map_zoom < 9 || grid.map_zoom > 14)) {
    throw new Error('Invalid map grid');
  }
  return grid;
}

export async function objectPointer(bucket, prefix, name) {
  const value = await readJSON(bucket, `${prefix}/public/${name}.json`);
  if (!/^[a-f0-9]{64}$/.test(value.sha256) || !['identity', 'gzip'].includes(value.encoding)
    || !Number.isSafeInteger(value.bytes) || value.bytes < 0
    || !Number.isSafeInteger(value.decoded_bytes) || value.decoded_bytes < 0) throw new Error('Invalid public object pointer');
  return { ...value, path: `${prefix}/objects/${value.sha256}` };
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
  const match = /^\/releases\/([a-f0-9]{64})\/(device\/catalog\.json|maps\/assets\/(?:fonts\/[A-Za-z0-9 _,-]+\/[0-9]+-[0-9]+\.pbf|sprites\/v[0-9]+\/[a-z]+(?:@2x)?\.(?:json|png)))$/.exec(path);
  if (!match) return null;
  return { release: match[1], file: match[2], type: match[2].endsWith('.json') ? 'application/json'
    : match[2].endsWith('.png') ? 'image/png' : 'application/x-protobuf' };
}
