// Prints the corridor mesh of each owner of the TrailView fixture: its pieces, vertices, triangles, bytes and
// build time, then the sum, the mean and the most of each.
//
//   npx vite-node scripts/road-meshes.ts
import { readdirSync, readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { meshBytes, roadMesh } from '../src/lib/trailview/roads/mesh';
import { decodeFeatures, DEEPEST } from '../src/lib/trailview/stream/features';
import { OWNER } from '../src/lib/trailview/terrain/lod';

const root = new URL('../tests/browser/trailview/fixture/features/', import.meta.url);
const rows = readdirSync(new URL(`${DEEPEST}/`, root)).flatMap(x => readdirSync(new URL(`${DEEPEST}/${x}/`, root)).flatMap(file => {
    const tile = { level: DEEPEST, x: Number(x), y: parseInt(file, 10) }, list = decodeFeatures(new Uint8Array(gunzipSync(readFileSync(new URL(`${DEEPEST}/${x}/${file}`, root)))));
    const owners = Array.from({ length: 16 }, (_, i) => ({ level: OWNER, x: tile.x * 4 + i % 4, y: tile.y * 4 + (i >> 2) }));
    return owners.map(owner => {
        const start = performance.now(), mesh = roadMesh(list, tile, owner), ms = performance.now() - start;
        const count = (reach: 'near' | 'far', field: 'count' | 'vertices') => mesh.pieces.reduce((sum, piece) => sum + piece[reach][field], 0);
        return { owner: `${owner.x}/${owner.y}`, pieces: mesh.pieces.length, vertices: mesh.pieces.reduce((sum, { vertices }) => sum + vertices.byteLength / 16, 0), triangles: count('near', 'count') / 3, farVertices: count('far', 'vertices'), farTriangles: count('far', 'count') / 3, bytes: meshBytes(mesh), ms: Math.round(ms) };
    });
}));
for (const row of rows) console.log(Object.values(row).join('\t'));
const columns = Object.keys(rows[0]).slice(1) as (keyof Omit<typeof rows[0], 'owner'>)[];
console.log(`\t${columns.join('\t')}`);
console.log(`sum\t${columns.map(column => rows.reduce((sum, row) => sum + row[column], 0)).join('\t')}`);
console.log(`mean\t${columns.map(column => Math.round(rows.reduce((sum, row) => sum + row[column], 0) / rows.length)).join('\t')}`);
console.log(`most\t${columns.map(column => Math.max(...rows.map(row => row[column]))).join('\t')}`);
