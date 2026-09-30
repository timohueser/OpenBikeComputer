import { Compression, EtagMismatch, PMTiles, ResolvedValueCache, TileType } from 'pmtiles';

async function decompress(bytes, compression) {
  if (compression === Compression.None || compression === Compression.Unknown) return bytes;
  if (compression !== Compression.Gzip) throw new Error('Unsupported tile compression');
  return new Response(new Response(bytes).body.pipeThrough(new DecompressionStream('gzip'))).arrayBuffer();
}
const directories = new ResolvedValueCache(25, undefined, decompress);
const headers = { 'Access-Control-Allow-Origin': '*', 'Access-Control-Allow-Methods': 'GET, HEAD, OPTIONS',
  'Cache-Control': 'public, max-age=31536000, immutable', 'X-Content-Type-Options': 'nosniff' };

export function tileRoute(path) {
  const match = /^\/releases\/([a-f0-9]{64})\/(basemap|terrain)(?:\.json|\/(0|[1-9]\d*)\/(0|[1-9]\d*)\/(0|[1-9]\d*)\.(mvt|webp))$/.exec(path);
  if (!match) return null;
  const [, release, name, z, x, y, ext] = match;
  if (z !== undefined && (Number(z) > (name === 'basemap' ? 14 : 12)
    || Number(x) >= 2 ** Number(z) || Number(y) >= 2 ** Number(z)
    || ext !== (name === 'basemap' ? 'mvt' : 'webp'))) return null;
  return { release, name, tile: z === undefined ? null : [Number(z), Number(x), Number(y)] };
}

class MissingArchive extends Error {}
class R2Source {
  constructor(bucket, path) { this.bucket = bucket; this.path = path; }
  getKey() { return this.path; }
  async getBytes(offset, length, _signal, etag) {
    const object = await this.bucket.get(this.path, { range: { offset, length }, ...(etag ? { onlyIf: { etagMatches: etag } } : {}) });
    if (!object) throw new MissingArchive();
    if (!object.body) throw new EtagMismatch();
    return { data: await object.arrayBuffer(), etag: object.etag };
  }
}

export default {
  async fetch(request, env, ctx) {
    if (request.method === 'OPTIONS') return new Response(null, { headers });
    if (!['GET', 'HEAD'].includes(request.method)) return new Response(null, { status: 405, headers: { ...headers, Allow: 'GET, HEAD, OPTIONS' } });
    const url = new URL(request.url), route = tileRoute(url.pathname);
    if (!route || url.search) return new Response('Tile not found', { status: 404, headers });
    const cacheKey = new Request(url.href);
    const cached = await caches.default.match(cacheKey);
    if (cached) return new Response(request.method === 'HEAD' ? null : cached.body, cached);
    const archive = new PMTiles(new R2Source(env.BUCKET, `planner/releases/${route.release}/maps/${route.name}.pmtiles`), directories, decompress);
    try {
      const header = await archive.getHeader();
      if (header.tileType !== (route.name === 'basemap' ? TileType.Mvt : TileType.Webp)) throw new Error('Invalid archive type');
      const base = `${url.origin}/releases/${route.release}/${route.name}`;
      const data = route.tile ? await archive.getZxy(...route.tile) : await archive.getTileJson(base);
      const response = new Response(route.tile ? data?.data : JSON.stringify(data), {
        status: route.tile && !data ? 204 : 200,
        headers: { ...headers, 'Content-Type': route.tile ? (route.name === 'basemap' ? 'application/x-protobuf' : 'image/webp') : 'application/json' },
      });
      if (response.status === 200) ctx.waitUntil(caches.default.put(cacheKey, response.clone()));
      return request.method === 'HEAD' ? new Response(null, response) : response;
    } catch (error) {
      return new Response(error instanceof MissingArchive ? 'Archive not found' : 'Tiles are temporarily unavailable', {
        status: error instanceof MissingArchive ? 404 : 503,
        headers: { ...headers, 'Cache-Control': 'no-store' },
      });
    }
  },
};
