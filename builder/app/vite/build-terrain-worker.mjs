import { build } from 'esbuild';
import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const app = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export async function buildTerrainWorker(output) {
    await build({ bundle: true, minify: true, format: 'iife', target: 'safari17',
        entryPoints: [path.join(app, 'src/lib/planner/terrain-pmtiles-worker.ts')], outfile: output });
    const chunks = await Promise.all(['shared', 'worker'].map(name =>
        readFile(path.join(app, `node_modules/maplibre-contour/dist/staging/${name}.js`), 'utf8')));
    await writeFile(output, await readFile(output, 'utf8')
        + '\nvar contourShared = {}; function define(_, factory) { factory(contourShared); }\n' + chunks.join('\n'));
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
    if (!process.argv[2]) throw new Error('Usage: node build-terrain-worker.mjs OUTPUT');
    await buildTerrainWorker(process.argv[2]);
}
