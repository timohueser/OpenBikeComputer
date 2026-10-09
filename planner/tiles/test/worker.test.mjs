import test from 'node:test';
import assert from 'node:assert/strict';
import { gunzipSync, gzipSync } from 'node:zlib';
import { createHash } from 'node:crypto';
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
function archive(tileType = 1, meta = { attribution: 'Test data' }, tileCompression = 2) {
  const bytes = new Uint8Array([26, 0]), tile = tileCompression === 2 ? gzipSync(bytes) : bytes;
  const directory = gzipSync(new Uint8Array([1, 0, 1, tile.length, 1]));
  const metadata = gzipSync(Buffer.from(JSON.stringify(meta)));
  const header = Buffer.alloc(127);
  header.write('PMTiles'); header[7] = 3;
  const values = [127, directory.length, 127 + directory.length, metadata.length,
    127 + directory.length + metadata.length, 0, 127 + directory.length + metadata.length,
    tile.length, 1, 1, 1];
  values.forEach((v, i) => header.writeBigUInt64LE(BigInt(v), 8 + i * 8));
  header.set([1, 2, tileCompression, tileType, 0, 0], 96);
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
    ...Object.entries(packs).flatMap(([name, bytes]) => {
      const digest = createHash('sha256').update(bytes).digest('hex');
      return [[`${prefix}/public/maps/${name}.json.json`, pointer('9'.repeat(64), Buffer.from('{}'))],
        [`${prefix}/public/maps/tiles/${name}/0-0-0.pmtiles.json`, pointer(digest, bytes)], [`planner/objects/${digest}`, bytes]];
    })];
}

test('range reads deliver decoded tiles, cache tiles and empty coverage, and do not cache missing releases', async () => {
  const prefix = `planner/releases/${release}`, tilejson = Buffer.from('{"tilejson":"3.0.0","attribution":"Test data"}');
  const objects = new Map([...grid(prefix, { basemap: archive() }), [`${prefix}/public/maps/basemap.json.json`, pointer('e'.repeat(64), tilejson)],
    [`planner/objects/${'e'.repeat(64)}`, tilejson]]);
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
  const readsBeforeEmpty = reads.count;
  assert.equal((await worker.fetch(new Request(emptyUrl), env, ctx)).status, 204);
  assert.equal(reads.count, readsBeforeEmpty);
  // A missing release, and an archive that the release does not publish.
  for (const missing of [`https://tiles.example/releases/${'b'.repeat(64)}/terrain.json`, `https://tiles.example/releases/${'b'.repeat(64)}/terrain/0/0/0`,
    `https://tiles.example${base}/sun.json`, `https://tiles.example${base}/sun/0/0/0`]) {
    const response = await worker.fetch(new Request(missing), env, ctx);
    assert.equal(response.status, 404); assert.equal(response.headers.get('Cache-Control'), 'no-store');
    assert.ok(![...cached.keys()].some(key => key.split('?')[0] === missing));
  }
  // Valid empty sunlight has metadata, while each absent shard is unknown coverage.
  const sun = Buffer.from('{"tilejson":"3.0.0","sun_format":3,"minzoom":0,"maxzoom":12}');
  const digest = '8'.repeat(64);
  objects.set(`${prefix}/public/maps/sun.json.json`, pointer(digest, sun));
  objects.set(`planner/objects/${digest}`, sun);
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
    [`planner/objects/${'f'.repeat(64)}`, packed],
    [`${prefix}/public/routes/tiles/9-268-178.json.json`, pointer('b'.repeat(64), packedRoutes, 'gzip', routes.length)],
    [`planner/objects/${'b'.repeat(64)}`, packedRoutes],
    [`${prefix}/public/device/catalog.json.json`, pointer('e'.repeat(64), asset)],
    [`${prefix}/public/maps/assets/sprites/v4/light@2x.json.json`, pointer('e'.repeat(64), asset)],
    [`planner/objects/${'e'.repeat(64)}`, asset],
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

test('gzip tile bytes and runtime encoding survive the edge cache for every tile type', async () => {
  const id = 'd'.repeat(64), prefix = `planner/releases/${id}`;
  // Node ignores `encodeBody`; the Workers runtime compresses again without 'manual'.
  const NodeResponse = globalThis.Response;
  globalThis.Response = class extends NodeResponse { constructor(body, init) { super(body, init); this.encodeBody = init?.encodeBody; } };
  const cached = new Map();
  const cache = { async match(key) { return cached.get(key.url)?.clone(); }, async put(key, value) { cached.set(key.url, value); } };
  const env = { BUCKET: bucket(new Map(grid(prefix, { snow: archive(0), basemap: archive(1), terrain: archive(4),
    places: archive(1, {}, 1) }))) };
  const pending = [], ctx = { waitUntil(promise) { pending.push(promise); } };
  try {
    for (const [name, type] of [['snow', 'application/octet-stream'], ['basemap', 'application/x-protobuf'], ['terrain', 'image/webp']]) {
      const request = method => new Request(`https://tiles.example/releases/${id}/${name}/0/0/0`,
        { method, headers: { 'Accept-Encoding': 'gzip' } });
      const tile = await worker.fetch(request('GET'), env, ctx, cache);
      await Promise.all(pending);
      const hit = await worker.fetch(request('GET'), env, ctx, cache);
      const stored = [...cached.values()].at(-1);
      for (const response of [tile, stored, hit]) {
        assert.equal(response.headers.get('Content-Type'), type);
        assert.equal(response.headers.get('Content-Encoding'), 'gzip');
        assert.equal(response.headers.get('Vary'), 'Accept-Encoding');
        assert.equal(response.encodeBody, 'manual');
        assert.deepEqual(Buffer.from(await response.clone().arrayBuffer()), gzipSync(new Uint8Array([26, 0])));
      }
      const head = await worker.fetch(request('HEAD'), env, ctx, cache);
      const coldHead = await worker.fetch(request('HEAD'), env, ctx, null);
      for (const response of [head, coldHead]) {
        assert.equal(response.headers.get('Content-Encoding'), 'gzip');
        assert.equal(response.encodeBody, 'manual');
        assert.equal(await response.text(), '');
      }
    }
    const plain = await worker.fetch(new Request(`https://tiles.example/releases/${id}/places/0/0/0`,
      { headers: { 'Accept-Encoding': 'gzip' } }), env, ctx, null);
    assert.equal(plain.headers.get('Content-Encoding'), null);
    assert.deepEqual(Buffer.from(await plain.arrayBuffer()), Buffer.from([26, 0]));
  } finally {
    globalThis.Response = NodeResponse;
  }
});

test('gzip negotiation separates cache entries and bypasses tile inflation', async () => {
  const id = '1'.repeat(64), prefix = `planner/releases/${id}`, reads = { count: 0 };
  const env = { BUCKET: bucket(new Map(grid(prefix, { basemap: archive(1, { attribution: 'Negotiation fixture' }) })), reads) };
  const cached = new Map(), pending = [], ctx = { waitUntil(promise) { pending.push(promise); } };
  const cache = { async match(key) { return cached.get(key.url)?.clone(); }, async put(key, value) { cached.set(key.url, value); } };
  const NativeDecompressionStream = globalThis.DecompressionStream;
  let inflations = 0;
  globalThis.DecompressionStream = class extends NativeDecompressionStream { constructor(format) { super(format); inflations++; } };
  try {
    const encodings = [['gzip', true], ['', false], ['br, GZIP;q=0.5', true], ['gzip;q=0, *;q=1', false],
      ['identity', false], ['br', false], ['*;q=0.3', true], ['gzip;q=0', false], ['gzip;q=bogus', false],
      ['br, gzip', false, 'gzip;q=0'], ['br', true, 'gzip'], ['br, gzip', false, '']];
    let warmReads;
    for (const [index, [encoding, compressed, original]] of encodings.entries()) {
      const request = new Request(`https://tiles.example/releases/${id}/basemap/0/0/0.mvt`,
        { headers: { 'Accept-Encoding': encoding } });
      if (original !== undefined) request.cf = { clientAcceptEncoding: original };
      const response = await worker.fetch(request, env, ctx, cache);
      await Promise.all(pending);
      assert.equal(response.status, 200);
      assert.equal(response.headers.get('Content-Encoding'), compressed ? 'gzip' : null);
      assert.equal(response.headers.get('Vary'), 'Accept-Encoding');
      const bytes = Buffer.from(await response.arrayBuffer());
      assert.deepEqual(compressed ? gunzipSync(bytes) : bytes, Buffer.from([26, 0]));
      // The first gzip request inflates the directory; the first identity request also inflates its tile.
      assert.equal(inflations, index === 0 ? 1 : 2);
      if (index === 1) warmReads = reads.count;
      if (index > 1) assert.equal(reads.count, warmReads);
    }
    assert.equal(cached.size, 2);
  } finally {
    globalThis.DecompressionStream = NativeDecompressionStream;
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

test('Local files serve the same grid bytes over loopback without an R2 cache', async () => {
  const {mkdtemp, mkdir, writeFile, rm, realpath} = await import('node:fs/promises');
  const {tmpdir} = await import('node:os');
  const {join, dirname} = await import('node:path');
  const {Files, serve} = await import('../src/local.mjs');
  const root = await mkdtemp(join(tmpdir(), 'obc-local-tiles-'));
  const id = 'f'.repeat(64), prefix = `planner/releases/${id}`;
  const tilejson = Buffer.from('{"tilejson":"3.0.0","attribution":"Local data"}');
  const hash = createHash('sha256').update(tilejson).digest('hex');
  const objects = new Map([[`cell-catalog/releases/${id}/catalog.json`, '{"format":1}'],
    [`cell-catalog/objects/${hash}`, tilejson], ...grid(prefix, {basemap: archive()}),
    [`${prefix}/public/maps/basemap.json.json`, pointer(hash, tilejson)], [`planner/objects/${hash}`, tilejson]]);
  let server;
  try {
    for (const [key, bytes] of objects) {
      const path = join(root, key);
      await mkdir(dirname(path), {recursive:true});
      await writeFile(path, bytes);
    }
    const files = new Files(await realpath(root));
    assert.equal(await files.get('planner/objects/../secret'), null);
    const tail = await files.get(`planner/objects/${hash}`, {range:{offset:5, length:16384}});
    assert.deepEqual(Buffer.from(await tail.arrayBuffer()), tilejson.subarray(5));
    server = await serve(root, 0);
    const origin = `http://127.0.0.1:${server.address().port}`;
    const tile = `${origin}/releases/${id}/basemap/0/0/0.mvt`;
    assert.deepEqual(Buffer.from(await (await fetch(tile)).arrayBuffer()), Buffer.from([26, 0]));
    // Node fetch decodes HTTP gzip; use the HTTP client to check the bytes on the wire.
    const { get } = await import('node:http');
    const encoded = await new Promise((resolve, reject) => {
      get(tile, { headers: { 'Accept-Encoding': 'gzip' } }, response => {
        const chunks = [];
        response.on('data', chunk => chunks.push(chunk));
        response.on('end', () => resolve({ headers: response.headers, bytes: Buffer.concat(chunks) }));
        response.on('error', reject);
      }).on('error', reject);
    });
    assert.equal(encoded.headers['content-encoding'], 'gzip');
    assert.deepEqual(encoded.bytes, gzipSync(new Uint8Array([26, 0])));
    assert.equal((await fetch(tile, {method:'HEAD'})).status, 200);
    const info = await (await fetch(`${origin}/releases/${id}/basemap.json`)).json();
    assert.equal(info.attribution, 'Local data');
    assert.deepEqual(info.tiles, [`${origin}/releases/${id}/basemap/{z}/{x}/{y}`]);
    assert.equal((await fetch(`${origin}/releases/${'1'.repeat(64)}/basemap.json`)).status, 404);
    const catalog = `${origin}/cell-catalog/releases/${id}/catalog.json`;
    assert.deepEqual(await (await fetch(catalog)).json(), {format:1});
    const object = `${origin}/cell-catalog/objects/${hash}`;
    const range = await fetch(object, {headers: {Range:'bytes=5-12'}});
    assert.equal(range.status, 206);
    assert.equal(range.headers.get('Content-Range'), `bytes 5-12/${tilejson.length}`);
    assert.deepEqual(Buffer.from(await range.arrayBuffer()), tilejson.subarray(5,13));
    const head = await fetch(object, {method:'HEAD'});
    assert.equal(head.headers.get('Content-Length'), String(tilejson.length));
    assert.equal(await head.text(), '');
    assert.equal((await fetch(object, {headers:{Range:'bytes=9999-'}})).status,416);
    const cors = await fetch(object, {method:'OPTIONS', headers:{'Access-Control-Request-Headers':'range'}});
    assert.equal(cors.status, 204);
    assert.equal(cors.headers.get('Access-Control-Allow-Headers'),'Range');
    assert.equal((await fetch(`${origin}/cell-catalog/unknown`)).status,404);
  } finally {
    if (server) await new Promise(resolve => server.close(resolve));
    await rm(root, {recursive:true, force:true});
  }
});
