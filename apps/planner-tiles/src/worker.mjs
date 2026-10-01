import { Compression, EtagMismatch, PMTiles, ResolvedValueCache, TileType } from 'pmtiles';
import { MissingArchive, assetRoute, gridConfig, objectPointer, packName, publicFile } from './grid.mjs';

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
    let asset;
    try { asset = assetRoute(decodeURIComponent(url.pathname)); } catch { asset = null; }
    if ((!route && !asset) || url.search) return new Response('Tile not found', { status: 404, headers });
    const cacheKey = new Request(url.href);
    const cached = await caches.default.match(cacheKey);
    if (cached) return new Response(request.method === 'HEAD' ? null : cached.body, cached);
    try {
      const prefix = `planner/releases/${(route || asset).release}`;
      let response;
      if (asset) {
        response = await publicFile(env.BUCKET, prefix, asset.file, asset.type, headers);
      } else {
        const grid = await gridConfig(env.BUCKET, prefix);
        const base = `${url.origin}/releases/${route.release}/${route.name}`;
        let data;
        if (grid && !route.tile) {
          const pointer = await objectPointer(env.BUCKET, prefix, `maps/${route.name}.json`);
          if (pointer.decoded_bytes > 1024 * 1024) throw new Error('TileJSON exceeds metadata limit');
          data = await (await publicFile(env.BUCKET, prefix, `maps/${route.name}.json`, 'application/json', headers)).json();
          data.tiles = [`${base}/{z}/{x}/{y}.${route.name === 'basemap' ? 'mvt' : 'webp'}`];
        } else {
          let path = `${prefix}/maps/${route.name}.pmtiles`;
          if (grid) {
            const pointer = await objectPointer(env.BUCKET, prefix, packName(route.name, route.tile, grid.map_zoom));
            if (pointer.encoding !== 'identity') throw new Error('PMTiles must support byte ranges');
            path = pointer.path;
          }
          const archive = new PMTiles(new R2Source(env.BUCKET, path), directories, decompress);
          const header = await archive.getHeader();
          if (header.tileType !== (route.name === 'basemap' ? TileType.Mvt : TileType.Webp)) throw new Error('Invalid archive type');
          data = route.tile ? await archive.getZxy(...route.tile) : await archive.getTileJson(base);
        }
        response = new Response(route.tile ? data?.data : JSON.stringify(data), {
          status: route.tile && !data ? 204 : 200,
          headers: { ...headers, 'Content-Type': route.tile ? (route.name === 'basemap' ? 'application/x-protobuf' : 'image/webp') : 'application/json' },
        });
      }
      if (response.status === 200) ctx.waitUntil(caches.default.put(cacheKey, response.clone()));
      return request.method === 'HEAD' ? new Response(null, response) : response;
    } catch (error) {
      if (!(error instanceof MissingArchive)) console.error(JSON.stringify({ event: 'tile_read_failed', path: url.pathname, error: String(error) }));
      return new Response(error instanceof MissingArchive ? 'Archive not found' : 'Tiles are temporarily unavailable', {
        status: error instanceof MissingArchive ? 404 : 503,
        headers: { ...headers, 'Cache-Control': 'no-store' },
      });
    }
  },
};
