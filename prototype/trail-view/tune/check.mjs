// Checks the data files: loads src/regions.ts and the model library through Vite, as the page
// does, and prints each problem validate() finds. With --assets it then lists every model and the
// census of stand-in aliases: pack entries whose `latin` is not their model's.
//
//   node tune/check.mjs [--assets]        (npm run check runs tsc first)

import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createServer } from 'vite';

const ROOT = join(import.meta.dirname, '..');
const cacheDir = mkdtempSync(join(tmpdir(), 'trail-view-vite-'));
const server = await createServer({
  root: ROOT, configFile: false, logLevel: 'error', appType: 'custom', cacheDir,
  server: { middlewareMode: true, hmr: false, ws: false, watch: null }, optimizeDeps: { noDiscovery: true },
});
try {
  const { validate, packNamed, regionNamed, PACK_NAMES, REGION_NAMES } = await server.ssrLoadModule('/src/regions.ts');
  const problems = validate();
  for (const problem of problems) console.error(problem);
  if (problems.length) {
    console.error(`${problems.length} problems`);
    process.exitCode = 1;
  } else if (process.argv.includes('--assets')) {
    const { animal, ANIMALS, plant, PLANTS, tree, TREES } = await server.ssrLoadModule('/src/models/library.ts');
    const { triangles } = await server.ssrLoadModule('/src/models/shape.ts');
    const packs = Object.fromEntries(['base', ...PACK_NAMES].map((name) => [name, packNamed(name)]));
    const regionsOf = (pack) => REGION_NAMES.filter((id) => regionNamed(id).pack.id === pack);
    const uses = (pack) => [...pack.trees.map((s) => [s, s.tree]), ...pack.ground.map((g) => [g, g.plant]), ...pack.herds.map((h) => [h, h.animal]), ...(pack.orchard ? [[{}, pack.orchard]] : [])];
    const models = [
      ...Object.entries(TREES).map(([id, m]) => [id, 'tree', m.latin, [0, 1, 2].map((lod) => triangles(tree(id, 'summer', lod)))]),
      ...Object.entries(PLANTS).map(([id, m]) => [id, 'plant', m.latin, [triangles(plant(id, 'summer'))]]),
      ...Object.entries(ANIMALS).map(([id, m]) => [id, 'animal', m.latin, [triangles(animal(id))]]),
    ];
    console.log('Models: id, kind, latin, triangles per LOD, the packs that use it');
    for (const [id, kind, latin, lods] of models) {
      const users = Object.keys(packs).filter((name) => uses(packs[name]).some(([, model]) => model === id));
      console.log(`  ${id.padEnd(20)} ${kind.padEnd(6)} ${latin.padEnd(48)} ${lods.join(' / ').padEnd(20)} ${users.join(', ') || '-'}`);
    }
    const latinOf = Object.fromEntries(models.map(([id, , latin]) => [id, latin]));
    const census = {};
    for (const [name, pack] of Object.entries(packs)) {
      for (const [entry, model] of uses(pack)) {
        if (!entry.latin || entry.latin === latinOf[model]) continue;
        const stand = (census[entry.latin] ??= { entries: new Set(), regions: new Set() });
        stand.entries.add(`${name}: ${entry.id} → ${model}`);
        for (const region of regionsOf(name)) stand.regions.add(region);
      }
    }
    console.log('\nCensus: stand-in aliases by latin');
    for (const [latin, { entries, regions }] of Object.entries(census).sort()) {
      console.log(`  ${latin}\n    ${[...entries].join('\n    ')}\n    regions: ${[...regions].join(', ') || '-'}`);
    }
    if (!Object.keys(census).length) console.log('  none');
  }
} finally {
  await server.close();
  rmSync(cacheDir, { recursive: true, force: true });
}
