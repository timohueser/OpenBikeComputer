import test from 'node:test';
import assert from 'node:assert/strict';
import { gzipSync } from 'node:zlib';
import worker, { tileRoute } from '../src/worker.mjs';
const release = 'a'.repeat(64), base = `/releases/${release}`;
test('tile routes bound archive selection, coordinates and types', () => {
  assert.deepEqual(tileRoute(`${base}/basemap/14/16383/16383.mvt`).tile, [14, 16383, 16383]);
  assert.equal(tileRoute(`${base}/terrain.json`).name, 'terrain');
  for (const path of [`${base}/basemap/15/0/0.mvt`, `${base}/terrain/12/4096/0.webp`,
    `${base}/terrain/0/0/0.mvt`, `${base}/other.json`, '/cell-catalog/catalog.json', `${base}/basemap/01/0/0.mvt`]) {
    assert.equal(tileRoute(path), null, path);
  }
});
test('invalid requests perform no bucket access', async () => {
  const env = { BUCKET: { get() { assert.fail('Unexpected bucket access'); } } };
  for (const request of [new Request('https://tiles.example/nope'), new Request(`https://tiles.example${base}/basemap.json?x=1`)]) {
    assert.equal((await worker.fetch(request, env, {})).status, 404);
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
    if (!path.includes(release)) return null;
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
