// Prototype: dump the solved lines of a trailview-features archive in a box, one JSON line per part.
// node dumplines.mjs ARCHIVE.pmtiles W,S,E,N OUT.jsonl
// Each line: kind, tags, tile "x-y", feature index f, part index p, own (per segment: 1 when the
// chord midpoint lies in the tile's own square, so a segment in two tiles' buffers counts once),
// v = per vertex [lon, lat, z (null = no height), grade (rise per metre), tangent (radians from
// +x east to +y south, Web Mercator)], grade and tangent null without an alignment.
// PMTILES_FROM names a package.json whose node_modules holds `pmtiles`.
import { createRequire } from 'module';
import fs from 'fs';
import zlib from 'zlib';
import { decodeFeatures } from '../../../builder/web/src/lib/trailview/stream/features.ts';

const require = createRequire(process.env.PMTILES_FROM ?? '/media/timo/OSM1/tmp/rebake/wt-3275/builder/web/package.json');
const { PMTiles } = await import(require.resolve('pmtiles'));
class Source {
  constructor(path) { this.fd = fs.openSync(path, 'r'); this.path = path; }
  getKey() { return this.path; }
  async getBytes(offset, length) {
    const b = Buffer.alloc(length); fs.readSync(this.fd, b, 0, length, offset);
    return { data: b.buffer.slice(b.byteOffset, b.byteOffset + length) };
  }
}
const [file, box, outFile] = process.argv.slice(2);
const [W, S, E, N] = box.split(',').map(Number);
const Z = 13, EXT = 16384;
const tileOf = (lon, lat) => [Math.floor((lon + 180) / 360 * 2 ** Z), Math.floor((1 - Math.log(Math.tan(lat * Math.PI / 180) + 1 / Math.cos(lat * Math.PI / 180)) / Math.PI) / 2 * 2 ** Z)];
const [xa, ya] = tileOf(W, N), [xb, yb] = tileOf(E, S);
const SOLVED = new Set(['main-road', 'road', 'path', 'railway', 'tunnel', 'gallery']);
const pm = new PMTiles(new Source(file));
const out = fs.openSync(outFile, 'w');
let tiles = 0, parts = 0, vertices = 0, nan = 0;
const r6 = (v) => Math.round(v * 1e7) / 1e7;
for (let x = xa; x <= xb; x++) for (let y = ya; y <= yb; y++) {
  const r = await pm.getZxy(Z, x, y); if (!r) continue;
  tiles++;
  let bytes = Buffer.from(r.data); if (bytes[0] === 0x1f && bytes[1] === 0x8b) bytes = zlib.gunzipSync(bytes);
  const lines = [];
  decodeFeatures(new Uint8Array(bytes)).forEach((feature, f) => {
    if (feature.geometry !== 'line' || !SOLVED.has(feature.kind)) return;
    feature.parts.forEach((part, p) => {
      if (!part.z) return;
      const n = part.z.length, v = [], own = [];
      for (let i = 0; i < n; i++) {
        const tx = (x + part.xy[2 * i] / EXT) / 2 ** Z, ty = (y + part.xy[2 * i + 1] / EXT) / 2 ** Z;
        const z = part.z[i];
        if (Number.isNaN(z)) nan++;
        v.push([r6(tx * 360 - 180), r6(Math.atan(Math.sinh(Math.PI * (1 - 2 * ty))) * 180 / Math.PI), Number.isNaN(z) ? null : Math.round(z * 100) / 100,
          part.plan ? Math.round(part.plan.grade[i] * 1e4) / 1e4 : null, part.plan ? Math.round(part.plan.tangent[i] * 1e5) / 1e5 : null]);
        if (i + 1 < n) {
          const mx = (part.xy[2 * i] + part.xy[2 * i + 2]) / 2, my = (part.xy[2 * i + 1] + part.xy[2 * i + 3]) / 2;
          own.push(mx >= 0 && mx < EXT && my >= 0 && my < EXT ? 1 : 0);
        }
      }
      vertices += n; parts++;
      lines.push(JSON.stringify({ kind: feature.kind, tags: feature.tags, tile: `${x}-${y}`, f, p, own, v }));
    });
  });
  if (lines.length) fs.writeSync(out, lines.join('\n') + '\n');
}
fs.closeSync(out);
console.log(JSON.stringify({ out: outFile, tiles, parts, vertices, nan_z: nan }));
