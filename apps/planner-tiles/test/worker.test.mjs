import test from 'node:test';
import assert from 'node:assert/strict';
import { gzipSync } from 'node:zlib';
import worker, { tileRoute } from '../src/worker.mjs';
const release = 'a'.repeat(64), base = `/releases/${release}`;
test('tile routes bound archive selection, coordinates and types', () => {
  assert.deepEqual(tileRoute(`${base}/basemap/14/16383/16383.mvt`).tile, [14, 16383, 16383]);
  assert.deepEqual(tileRoute(`${base}/places/11/2047/2047.mvt`).tile, [11, 2047, 2047]);
  assert.equal(tileRoute(`${base}/terrain.json`).name, 'terrain');
  for (const path of [`${base}/basemap/15/0/0.mvt`, `${base}/places/12/0/0.mvt`, `${base}/places/0/0/0.webp`,
    `${base}/terrain/12/4096/0.webp`, `${base}/terrain/0/0/0.mvt`, `${base}/other.json`, '/cell-catalog/catalog.json',
    `${base}/basemap/01/0/0.mvt`]) {
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
function archive() {
  const tile = gzipSync(new Uint8Array([26, 0]));
  const directory = gzipSync(new Uint8Array([1, 0, 1, tile.length, 1]));
  const metadata = gzipSync(Buffer.from(JSON.stringify({ attribution: 'Test data' })));
  const header = Buffer.alloc(127);
  header.write('PMTiles'); header[7] = 3;
  const values = [127, directory.length, 127 + directory.length, metadata.length,
    127 + directory.length + metadata.length, 0, 127 + directory.length + metadata.length,
    tile.length, 1, 1, 1];
  values.forEach((v, i) => header.writeBigUInt64LE(BigInt(v), 8 + i * 8));
  header.set([1, 2, 2, 1, 0, 0], 96);
  header.writeInt32LE(-1800000000, 102); header.writeInt32LE(-850000000, 106);
  header.writeInt32LE(1800000000, 110); header.writeInt32LE(850000000, 114);
  return Buffer.concat([header, directory, metadata, tile]);
}
test('range reads deliver decoded tiles, cache full GETs, and do not cache missing archives', async () => {
  const bytes = archive(), cached = new Map(), pending = [];
  globalThis.caches = { default: {
    async match(key) { return cached.get(key.url)?.clone(); },
    async put(key, value) { cached.set(key.url, value); },
  } };
  let reads = 0;
  const env = { BUCKET: { async get(path, options) {
    reads++;
    if (!path.includes(release) || path.endsWith('/public/grid.json')) return null;
    assert.equal(path, `planner/releases/${release}/maps/basemap.pmtiles`);
    const { offset, length } = options.range;
    const slice = bytes.subarray(offset, offset + length);
    return { body: true, etag: 'test', async arrayBuffer() { return slice.buffer.slice(slice.byteOffset, slice.byteOffset + slice.byteLength); } };
  } } };
  const ctx = { waitUntil(promise) { pending.push(promise); } };
  const url = `https://tiles.example${base}/basemap/0/0/0.mvt`;
  const tile = await worker.fetch(new Request(url, { method: 'HEAD' }), env, ctx);
  assert.equal(tile.status, 200); assert.equal(await tile.text(), '');
  await Promise.all(pending);
  const before = reads, get = await worker.fetch(new Request(url), env, ctx);
  assert.deepEqual(new Uint8Array(await get.arrayBuffer()), new Uint8Array([26, 0]));
  assert.equal(get.headers.get('Access-Control-Allow-Origin'), '*');
  assert.equal(reads, before);
  const info = await worker.fetch(new Request(`https://tiles.example${base}/basemap.json`), env, ctx);
  assert.equal((await info.json()).tiles[0], `https://tiles.example${base}/basemap/{z}/{x}/{y}.mvt`);
  const missing = `https://tiles.example/releases/${'b'.repeat(64)}/terrain.json`;
  for (let i = 0; i < 2; i++) {
    const response = await worker.fetch(new Request(missing), env, ctx);
    assert.equal(response.status, 404); assert.equal(response.headers.get('Cache-Control'), 'no-store');
  }
  assert.ok(!cached.has(missing));
  delete globalThis.caches;
});

 test('grid archives share download objects and assets stream from their pointers', async () => {
  const id = 'c'.repeat(64), prefix = `planner/releases/${id}`, bytes = archive();
  const digest = 'd'.repeat(64), asset = new TextEncoder().encode('{"hello":"map"}');
  const glyphs = new Uint8Array([10, 2, 8, 0]), packed = gzipSync(glyphs);
  const objects = new Map([
    [`${prefix}/public/maps/assets/fonts/Noto Sans Regular/0-255.pbf.json`, JSON.stringify({sha256:'f'.repeat(64),encoding:'gzip',bytes:packed.length,decoded_bytes:glyphs.length})],
    [`${prefix}/objects/${'f'.repeat(64)}`, packed],
    [`${prefix}/public/grid.json`, JSON.stringify({format:2,map_zoom:11})],
    [`${prefix}/public/maps/tiles/basemap/0-0-0.pmtiles.json`, JSON.stringify({sha256:digest,encoding:'identity',bytes:bytes.length,decoded_bytes:bytes.length})],
    [`${prefix}/public/maps/tiles/places/0-0-0.pmtiles.json`, JSON.stringify({sha256:digest,encoding:'identity',bytes:bytes.length,decoded_bytes:bytes.length})],
    [`${prefix}/public/device/catalog.json.json`, JSON.stringify({sha256:'e'.repeat(64),encoding:'identity',bytes:asset.length,decoded_bytes:asset.length})],
    [`${prefix}/public/maps/assets/sprites/v4/light@2x.json.json`, JSON.stringify({sha256:'e'.repeat(64),encoding:'identity',bytes:asset.length,decoded_bytes:asset.length})],
    [`${prefix}/objects/${digest}`, bytes], [`${prefix}/objects/${'e'.repeat(64)}`, asset],
  ]);
  globalThis.caches = {default:{async match(){return undefined},async put(){}}};
  const env = {BUCKET:{async get(path,options){
    const value=objects.get(path); if(value===undefined)return null;
    const raw=typeof value==='string'?new TextEncoder().encode(value):value;
    const slice=options?.range?raw.subarray(options.range.offset,options.range.offset+options.range.length):raw;
    return {size:raw.length,etag:path,body:new Response(slice).body,json:async()=>JSON.parse(new TextDecoder().decode(slice)),
      arrayBuffer:async()=>slice.buffer.slice(slice.byteOffset,slice.byteOffset+slice.byteLength)};
  }}};
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
  await Promise.all(pending); delete globalThis.caches;
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
