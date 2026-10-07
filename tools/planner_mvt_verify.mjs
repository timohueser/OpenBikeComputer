import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { pathToFileURL } from 'node:url';

export function validateTile(data, { VectorTile, PbfReader }) {
  const tile = new VectorTile(new PbfReader(data));
  for (const layer of Object.values(tile.layers)) {
    for (let i = 0; i < layer.length; i++) layer.feature(i).loadGeometry();
  }
}

async function main() {
  const require = createRequire(new URL('../builder/web/package.json', import.meta.url));
  const lock = JSON.parse(readFileSync(new URL('../builder/web/package-lock.json', import.meta.url)));
  const modules = {};
  for (const name of ['@mapbox/vector-tile', 'pbf']) {
    const entry = require.resolve(name);
    const installed = JSON.parse(readFileSync(join(dirname(entry), 'package.json')));
    if (installed.version !== lock.packages[`node_modules/${name}`].version) {
      throw new Error(`${name} differs from the locked reader; prepare builder dependencies first`);
    }
    Object.assign(modules, await import(pathToFileURL(entry).href));
  }
  process.stdout.write('ready\n');
  const lines = createInterface({ input: process.stdin });
  for await (const line of lines) {
    validateTile(Buffer.from(JSON.parse(line), 'base64'), modules);
    process.stdout.write('ok\n');
  }
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  main().catch(error => {
    console.error(`Planner MVT verification: ${error.message}; prepare the locked Node reader first`);
    process.exitCode = 1;
    process.stdin.destroy();
  });
}
