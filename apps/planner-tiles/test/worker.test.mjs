import test from 'node:test';
import assert from 'node:assert/strict';
import { gunzipSync, gzipSync } from 'node:zlib';
import worker, { tileRoute } from '../src/worker.mjs';
const release = 'a'.repeat(64), base = `/releases/${release}`;
test('tile routes bound archive selection and coordinates', () => {
  assert.deepEqual(tileRoute(`${base}/basemap/14/16383/16383.mvt`).tile, [14, 16383, 16383]);
  assert.deepEqual(tileRoute(`${base}/snow/13/4290/2911`).tile, [13, 4290, 2911]);
  assert.deepEqual(tileRoute(`${base}/climate/9/218/37`).tile, [9, 218, 37]);
  assert.deepEqual(tileRoute(`${base}/sun/10/535/356.webp`).tile, [10, 535, 356]);
  assert.equal(tileRoute(`${base}/terrain.json`).name, 'terrain');
  for (const path of [`${base}/terrain/12/4096/0.webp`, `${base}/basemap/27/0/0.mvt`, `${base}/places/0/0/0.png`,
    `${base}/Other.json`, `${base}/${'a'.repeat(33)}.json`, '/cell-catalog/catalog.json', `${base}/basemap/01/0/0.mvt`]) {
    assert.equal(tileRoute(path), null, path);
  }
});
test('invalid requests perform no bucket access', async () => {
  const env = { BUCKET: { get() { assert.fail('Unexpected bucket access'); } } };
  for (const request of [new Request('https://tiles.example/nope'), new Request(`https://tiles.example${base}/basemap.json?x=1`)]) {
    const response = await worker.fetch(request, env, {});
    assert.equal(response.status, 404); assert.equal(response.headers.get('Cache-Control'), 'no-store');
  }
  assert.equal((await worker.fetch(new Request('https://tiles.example', { method: 'POST' }), env, {})).status, 405);
});

// One tile in a synthetic PMTiles v3 archive tests the reader and R2 range seam.
function archive(tileType = 1, meta = { attribution: 'Test data' }) {
  const tile = gzipSync(new Uint8Array([26, 0]));
  const directory = gzipSync(new Uint8Array([1, 0, 1, tile.length, 1]));
  const metadata = gzipSync(Buffer.from(JSON.stringify(meta)));
  const header = Buffer.alloc(127);
  header.write('PMTiles'); header[7] = 3;
  const values = [127, directory.length, 127 + directory.length, metadata.length,
    127 + directory.length + metadata.length, 0, 127 + directory.length + metadata.length,
    tile.length, 1, 1, 1];
  values.forEach((v, i) => header.writeBigUInt64LE(BigInt(v), 8 + i * 8));
  header.set([1, 2, 2, tileType, 0, 0], 96);
  header.writeInt32LE(-1800000000, 102); header.writeInt32LE(-850000000, 106);
  header.writeInt32LE(1800000000, 110); header.writeInt32LE(850000000, 114);
  return Buffer.concat([header, directory, metadata, tile]);
}
const pointer = (sha256, bytes, encoding = 'identity', decoded = bytes.length) =>
  JSON.stringify({ sha256, encoding, bytes: bytes.length, decoded_bytes: decoded });
// A grid release in R2: small public pointers and the object pool, read whole or by range. Each archive
// publishes a TileJSON pointer.
function bucket(objects, reads = { count: 0 }) {
  return { async get(path, options) {
    reads.count++;
    const value = objects.get(path); if (value === undefined) return null;
    const raw = typeof value === 'string' ? new TextEncoder().encode(value) : value;
    const slice = options?.range ? raw.subarray(options.range.offset, options.range.offset + options.range.length) : raw;
    return { size: raw.length, etag: path, body: new Response(slice).body, json: async () => JSON.parse(new TextDecoder().decode(slice)),
      arrayBuffer: async () => slice.buffer.slice(slice.byteOffset, slice.byteOffset + slice.byteLength) };
  } };
}
function grid(prefix, packs) {
  return [[`${prefix}/public/grid.json`, JSON.stringify({ format: 2, map_zoom: 11 })],
    ...Object.entries(packs).flatMap(([name, bytes], i) => {
      const digest = String(i).repeat(64);
      return [[`${prefix}/public/maps/${name}.json.json`, pointer('9'.repeat(64), Buffer.from('{}'))],
        [`${prefix}/public/maps/tiles/${name}/0-0-0.pmtiles.json`, pointer(digest, bytes)], [`${prefix}/objects/${digest}`, bytes]];
    })];
}

test('range reads deliver decoded tiles, cache tiles and empty coverage, and do not cache missing releases', async () => {
  const prefix = `planner/releases/${release}`, tilejson = Buffer.from('{"tilejson":"3.0.0","attribution":"Test data"}');
  const objects = new Map([...grid(prefix, { basemap: archive() }), [`${prefix}/public/maps/basemap.json.json`, pointer('e'.repeat(64), tilejson)],
    [`${prefix}/objects/${'e'.repeat(64)}`, tilejson]]);
  const cached = new Map(), pending = [], reads = { count: 0 };
  globalThis.caches = { default: {
    async match(key) { return cached.get(key.url)?.clone(); },
    async put(key, value) { cached.set(key.url, value); },
  } };
  const env = { BUCKET: bucket(objects, reads) };
  const ctx = { waitUntil(promise) { pending.push(promise); } };
  const url = `https://tiles.example${base}/basemap/0/0/0.mvt`;
  const tile = await worker.fetch(new Request(url, { method: 'HEAD' }), env, ctx);
  assert.equal(tile.status, 200); assert.equal(await tile.text(), '');
  await Promise.all(pending);
  const before = reads.count, get = await worker.fetch(new Request(url), env, ctx);
  assert.deepEqual(new Uint8Array(await get.arrayBuffer()), new Uint8Array([26, 0]));
  assert.equal(get.headers.get('Access-Control-Allow-Origin'), '*');
  assert.equal(reads.count, before);
  const info = await (await worker.fetch(new Request(`https://tiles.example${base}/basemap.json`), env, ctx)).json();
  assert.deepEqual([info.attribution, info.tiles], ['Test data', [`https://tiles.example${base}/basemap/{z}/{x}/{y}`]]);
  assert.equal((await worker.fetch(new Request(`https://tiles.example${base}/basemap/0/0/0.webp`), env, ctx)).status, 404);
  // Zoom 1 has no pack.
  const emptyUrl = `https://tiles.example${base}/basemap/1/0/0.mvt`;
  assert.equal((await worker.fetch(new Request(emptyUrl), env, ctx)).status, 204);
  await Promise.all(pending);
  assert.equal(cached.get(emptyUrl).status, 204);
  const readsBeforeEmpty = reads.count;
  assert.equal((await worker.fetch(new Request(emptyUrl), env, ctx)).status, 204);
  assert.equal(reads.count, readsBeforeEmpty);
  // A missing release, and an archive that the release does not publish.
  for (const missing of [`https://tiles.example/releases/${'b'.repeat(64)}/terrain.json`, `https://tiles.example/releases/${'b'.repeat(64)}/terrain/0/0/0`,
    `https://tiles.example${base}/sun.json`, `https://tiles.example${base}/sun/0/0/0`]) {
    const response = await worker.fetch(new Request(missing), env, ctx);
    assert.equal(response.status, 404); assert.equal(response.headers.get('Cache-Control'), 'no-store');
    assert.ok(!cached.has(missing));
  }
  // Valid empty sunlight has metadata, while each absent shard is unknown coverage.
  const sun = Buffer.from('{"tilejson":"3.0.0","sun_format":3,"minzoom":0,"maxzoom":12}');
  const digest = '8'.repeat(64);
  objects.set(`${prefix}/public/maps/sun.json.json`, pointer(digest, sun));
  objects.set(`${prefix}/objects/${digest}`, sun);
  const sunMeta = await (await worker.fetch(new Request(`https://tiles.example${base}/sun.json`), env, ctx)).json();
  assert.equal(sunMeta.sun_format, 3);
  assert.equal((await worker.fetch(new Request(sunMeta.tiles[0].replace('{z}', '12').replace('{x}', '1').replace('{y}', '1')), env, ctx)).status, 204);
  delete globalThis.caches;
});

test('grid archives share download objects and assets stream from their pointers', async () => {
  const id = 'c'.repeat(64), prefix = `planner/releases/${id}`, asset = new TextEncoder().encode('{"hello":"map"}');
  const glyphs = new Uint8Array([10, 2, 8, 0]), packed = gzipSync(glyphs);
  const routes = Buffer.from('{"format":1,"routes":[]}'), packedRoutes = gzipSync(routes);
  const objects = new Map([...grid(prefix, { basemap: archive(), places: archive() }),
    [`${prefix}/public/maps/assets/fonts/Noto Sans Regular/0-255.pbf.json`, pointer('f'.repeat(64), packed, 'gzip', glyphs.length)],
    [`${prefix}/objects/${'f'.repeat(64)}`, packed],
    [`${prefix}/public/routes/tiles/9-268-178.json.json`, pointer('b'.repeat(64), packedRoutes, 'gzip', routes.length)],
    [`${prefix}/objects/${'b'.repeat(64)}`, packedRoutes],
    [`${prefix}/public/device/catalog.json.json`, pointer('e'.repeat(64), asset)],
    [`${prefix}/public/maps/assets/sprites/v4/light@2x.json.json`, pointer('e'.repeat(64), asset)],
    [`${prefix}/objects/${'e'.repeat(64)}`, asset],
  ]);
  globalThis.caches = {default:{async match(){return undefined},async put(){}}};
  const env = { BUCKET: bucket(objects) };
  const pending=[],ctx={waitUntil(p){pending.push(p)}};
  for (const name of ['basemap','places']) {
    const response=await worker.fetch(new Request(`https://tiles.example/releases/${id}/${name}/0/0/0.mvt`),env,ctx);
    assert.equal(response.status,200);assert.deepEqual(new Uint8Array(await response.arrayBuffer()),new Uint8Array([26,0]));
  }
  // Places are sparse: a tile without a pack is an absent tile, not a missing release.
  assert.equal((await worker.fetch(new Request(`https://tiles.example/releases/${id}/places/11/5/5.mvt`),env,ctx)).status,204);
  const catalog=await worker.fetch(new Request(`https://tiles.example/releases/${id}/device/catalog.json`),env,ctx);
  assert.deepEqual(await catalog.json(),{hello:'map'});
  const sprite=await worker.fetch(new Request(`https://tiles.example/releases/${id}/maps/assets/sprites/v4/light@2x.json`),env,ctx);
  assert.equal(sprite.status,200);assert.deepEqual(await sprite.json(),{hello:'map'});
  const font=await worker.fetch(new Request(`https://tiles.example/releases/${id}/maps/assets/fonts/Noto%20Sans%20Regular/0-255.pbf`),env,ctx);
  assert.equal(font.headers.get('Content-Type'),'application/x-protobuf');assert.deepEqual(new Uint8Array(await font.arrayBuffer()),glyphs);
  const cell=await worker.fetch(new Request(`https://tiles.example/releases/${id}/routes/tiles/9-268-178.json`),env,ctx);
  assert.equal(cell.headers.get('Content-Type'),'application/json');assert.deepEqual(await cell.json(),{format:1,routes:[]});
  // A cell outside the grid has no file.
  assert.equal((await worker.fetch(new Request(`https://tiles.example/releases/${id}/routes/tiles/9-0-0.json`),env,ctx)).status,404);
  await Promise.all(pending); delete globalThis.caches;
});

test('tiles of an unknown type keep their gzip encoding through the edge cache', async () => {
  const id = 'd'.repeat(64), prefix = `planner/releases/${id}`;
  // Node ignores `encodeBody`; the Workers runtime compresses again without 'manual'.
  const NodeResponse = globalThis.Response;
  globalThis.Response = class extends NodeResponse { constructor(body, init) { super(body, init); this.encodeBody = init?.encodeBody; } };
  const cached = new Map();
  globalThis.caches = { default: { async match(key) { return cached.get(key.url); }, async put(key, value) { cached.set(key.url, value); } } };
  const env = { BUCKET: bucket(new Map(grid(prefix, { snow: archive(0, { seasons: 9 }) }))) };
  const pending = [], ctx = { waitUntil(promise) { pending.push(promise); } };
  const url = `https://tiles.example/releases/${id}/snow/0/0/0`;
  try {
    const tile = await worker.fetch(new Request(url), env, ctx);
    await Promise.all(pending);
    const hit = await worker.fetch(new Request(url), env, ctx);
    for (const response of [tile, cached.get(url), hit]) {
      assert.equal(response.headers.get('Content-Type'), 'application/octet-stream');
      assert.equal(response.headers.get('Content-Encoding'), 'gzip');
      assert.equal(response.encodeBody, 'manual');
    }
    assert.deepEqual(gunzipSync(new Uint8Array(await hit.arrayBuffer())), Buffer.from([26, 0]));
  } finally {
    globalThis.Response = NodeResponse;
    delete globalThis.caches;
  }
});

test('a client over its limit is refused on a cache miss only', async () => {
  const cached = new Map(), url = `https://tiles.example${base}/basemap.json`;
  globalThis.caches = { default: { async match(key) { return cached.get(key.url)?.clone(); }, async put() {} } };
  let keys = [];
  const env = { BUCKET: { get() { assert.fail('A refused request must not read the bucket'); } },
    LIMITER: { async limit({ key }) { keys.push(key); return { success: false }; } } };
  const request = () => new Request(url, { headers: { 'cf-connecting-ip': '203.0.113.7' } });
  const refused = await worker.fetch(request(), env, {});
  assert.equal(refused.status, 429); assert.equal(refused.headers.get('Cache-Control'), 'no-store');
  assert.deepEqual(keys, ['203.0.113.7']);
  cached.set(url, new Response('{}'));
  assert.equal((await worker.fetch(request(), env, {})).status, 200);
  assert.equal(keys.length, 1);
  delete globalThis.caches;
});
