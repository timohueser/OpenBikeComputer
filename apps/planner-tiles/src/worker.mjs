import { Compression, EtagMismatch, PMTiles, ResolvedValueCache, TileType, tileTypeExt } from 'pmtiles';
import { MissingArchive, assetRoute, gridConfig, objectPointer, packName, publicFile } from './grid.mjs';

async function decompress(bytes, compression) {
  if (compression === Compression.None || compression === Compression.Unknown) return bytes;
  if (compression !== Compression.Gzip) throw new Error('Unsupported tile compression');
  return new Response(new Response(bytes).body.pipeThrough(new DecompressionStream('gzip'))).arrayBuffer();
}
const directories = new ResolvedValueCache(25, undefined, decompress);
const headers = { 'Access-Control-Allow-Origin': '*', 'Access-Control-Allow-Methods': 'GET, HEAD, OPTIONS',
  'Cache-Control': 'public, max-age=31536000, immutable', 'X-Content-Type-Options': 'nosniff' };

const contentTypes = { [TileType.Mvt]: 'application/x-protobuf', [TileType.Webp]: 'image/webp' };
const keep = async bytes => bytes;
// A body with a Content-Encoding is stored that way. Without `encodeBody: 'manual'`, the runtime would
// compress it again, also when the cache stores or returns a copy.
export function reply(body, { status, headers }) {
  return new Response(body, { status, headers, ...(new Headers(headers).has('Content-Encoding') ? { encodeBody: 'manual' } : {}) });
}
const notFound = () => new Response('Tile not found', { status: 404, headers: { ...headers, 'Cache-Control': 'no-store' } });

// Zooms and tile format come from each archive's header; PMTiles tile IDs end at zoom 26. An archive
// name is served when the release publishes its TileJSON pointer.
export function tileRoute(path) {
  const match = /^\/releases\/([a-f0-9]{64})\/([a-z]{1,32})(?:\.json|\/(0|[1-9]\d?)\/(0|[1-9]\d*)\/(0|[1-9]\d*)(\.mvt|\.webp)?)$/.exec(path);
  if (!match) return null;
  const [, release, name, z, x, y, ext] = match;
  if (z !== undefined && (Number(z) > 26 || Number(x) >= 2 ** Number(z) || Number(y) >= 2 ** Number(z))) return null;
  return { release, name, ext, tile: z === undefined ? null : [Number(z), Number(x), Number(y)] };
}

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

export async function fetch(request, env, ctx, cache = caches.default) {
    if (request.method === 'OPTIONS') return new Response(null, { headers });
    if (!['GET', 'HEAD'].includes(request.method)) return new Response(null, { status: 405, headers: { ...headers, Allow: 'GET, HEAD, OPTIONS' } });
    const url = new URL(request.url), route = tileRoute(url.pathname);
    let asset;
    try { asset = assetRoute(decodeURIComponent(url.pathname)); } catch { asset = null; }
    if ((!route && !asset) || url.search) return notFound();
    const cacheKey = new Request(url.href);
    const cached = await cache?.match(cacheKey);
    if (cached) return reply(request.method === 'HEAD' ? null : cached.body, cached);
    // Only a cache miss reads the bucket, so only a miss counts against the limit.
    if (env.LIMITER && !(await env.LIMITER.limit({ key: request.headers.get('cf-connecting-ip') ?? '' })).success) {
      return new Response('Too many requests', { status: 429, headers: { ...headers, 'Cache-Control': 'no-store', 'Retry-After': '60' } });
    }
    try {
      const prefix = `planner/releases/${(route || asset).release}`;
      let response;
      if (asset) {
        response = await publicFile(env.BUCKET, prefix, asset.file, asset.type, headers);
      } else if (!route.tile) {
        const pointer = await objectPointer(env.BUCKET, prefix, `maps/${route.name}.json`);
        if (pointer.decoded_bytes > 1024 * 1024) throw new Error('TileJSON exceeds metadata limit');
        const data = await (await publicFile(env.BUCKET, prefix, `maps/${route.name}.json`, 'application/json', headers)).json();
        data.tiles = [`${url.origin}/releases/${route.release}/${route.name}/{z}/{x}/{y}`];
        response = reply(JSON.stringify(data), { status: 200, headers: { ...headers, 'Content-Type': 'application/json' } });
      } else {
        await objectPointer(env.BUCKET, prefix, `maps/${route.name}.json`);
        const grid = await gridConfig(env.BUCKET, prefix);
        // A grid has packs only where an archive has tiles, so a tile without a pack is absent.
        const pointer = await objectPointer(env.BUCKET, prefix, packName(route.name, route.tile, grid.map_zoom))
          .catch(error => { if (error instanceof MissingArchive) return null; throw error; });
        let data, tileHeaders = {};
        if (pointer) {
          if (pointer.encoding !== 'identity') throw new Error('PMTiles must support byte ranges');
          const source = new R2Source(env.BUCKET, pointer.path);
          const archive = new PMTiles(source, directories, decompress);
          const header = await archive.getHeader();
          if (route.ext !== undefined && route.ext !== tileTypeExt(header.tileType)) return notFound();
          if (route.tile[0] >= header.minZoom && route.tile[0] <= header.maxZoom) {
            // Edge compression skips unknown content types, so these tiles keep their stored encoding.
            const raw = !(header.tileType in contentTypes);
            data = await (raw ? new PMTiles(source, directories, keep) : archive).getZxy(...route.tile);
            tileHeaders = { 'Content-Type': contentTypes[header.tileType] ?? 'application/octet-stream',
              ...(raw && header.tileCompression === Compression.Gzip ? { 'Content-Encoding': 'gzip' } : {}) };
          }
        }
        response = reply(data?.data, { status: data ? 200 : 204, headers: { ...headers, ...tileHeaders } });
      }
      if (cache) ctx.waitUntil(cache.put(cacheKey, reply(response.clone().body, response)));
      return request.method === 'HEAD' ? new Response(null, response) : response;
    } catch (error) {
      if (!(error instanceof MissingArchive)) console.error(JSON.stringify({ event: 'tile_read_failed', path: url.pathname, error: String(error) }));
      return new Response(error instanceof MissingArchive ? 'Archive not found' : 'Tiles are temporarily unavailable', {
        status: error instanceof MissingArchive ? 404 : 503,
        headers: { ...headers, 'Cache-Control': 'no-store' },
      });
    }
}

export default { fetch };
