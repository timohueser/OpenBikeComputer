import { build } from '../../../../../builder/app/node_modules/esbuild/lib/main.js';
import { mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { buildTerrainWorker } from '../../../../../builder/app/vite/build-terrain-worker.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '../../../../..');
const dependencies = path.join(root, 'builder/app/node_modules');
const [maps, cutout, output, mode] = process.argv.slice(2);
if (!output) throw new Error('Usage: node build.mjs MAPS CUTOUT_MAPS OUTPUT');
await mkdir(output, { recursive: true });
const shared = { bundle: true, minify: true, format: 'iife', target: 'safari17', nodePaths: [dependencies] };
await build({ ...shared, entryPoints: [path.join(here, mode === '--overlays' ? 'overlays.ts' : 'main.ts')], outfile: path.join(output, 'main.js'), define: { 'import.meta.env': '{}' } });
const demWorker = path.join(output, 'dem-worker.js');
await buildTerrainWorker(demWorker);
await build({ ...shared, entryPoints: [path.join(dependencies, 'maplibre-gl/dist/maplibre-gl-worker.mjs')], outfile: path.join(output, 'map-worker.js') });
await writeFile(path.join(output, 'map.css'), await readFile(path.join(dependencies, 'maplibre-gl/dist/maplibre-gl.css')));
const archives = {};
for (const [name, directory] of [['maps', maps], ['map-cutout', cutout]]) {
    const bytes = await readFile(path.join(directory, 'manifest.json'));
    const manifest = JSON.parse(bytes);
    archives[name] = { bounds: manifest.bounds, sha256: createHash('sha256').update(bytes).digest('hex'), files: manifest.files };
}
const versions = {};
for (const name of ['maplibre-gl', 'maplibre-contour', 'pmtiles', '@protomaps/basemaps']) {
    versions[name] = JSON.parse(await readFile(path.join(dependencies, name, 'package.json'))).version;
}
const fixtures = {};
if (mode === '--overlays') {
    await mkdir(path.join(output, 'overlays'), { recursive: true });
    for (const name of await readdir(path.join(output, 'overlays'))) {
        if (!name.endsWith('.json')) continue;
        const bytes = await readFile(path.join(output, 'overlays', name));
        fixtures[`overlays/${name}`] = { bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') };
    }
}
await writeFile(path.join(output, 'config.json'), JSON.stringify({ archives, versions, fixtures, renderer_sha256: createHash('sha256').update(await readFile(path.join(output, 'main.js'))).digest('hex') }));
await writeFile(path.join(output, 'index.html'), `<!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1"><meta http-equiv="Content-Security-Policy" content="default-src 'self'; script-src 'self' blob:; worker-src 'self' blob:; style-src 'self' 'unsafe-inline'; img-src 'self' blob: data:; connect-src 'self'"><link rel="stylesheet" href="map.css"><style>html,body,#map{margin:0;width:100%;height:100%;overflow:hidden}</style></head><body><div id="map"></div><script src="main.js"></script></body></html>`);
console.log(JSON.stringify({ output, versions }));
