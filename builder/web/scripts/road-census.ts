// The CPU half of the TrailView fault census: prints the counts of gaps, objects on ways and short sidewalks
// over the fixture, one JSON line each, and writes the face and the water samples that census.mjs probes.
//
//   npx vite-node scripts/road-census.ts DIR    # writes DIR/faces.bin and DIR/water.bin, float64 rows
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { countLine, faceSamples, fixtureTiles, gaps, objectsOnWays, shortSidewalks, waterSamples } from '../test-support/trailview/census';

const out = process.argv[2];
if (!out) throw new Error('usage: npx vite-node scripts/road-census.ts DIR');
const start = performance.now(), tiles = fixtureTiles();
mkdirSync(out, { recursive: true });
for (const [name, rows] of [['faces', faceSamples(tiles)], ['water', waterSamples(tiles)]] as const) writeFileSync(join(out, `${name}.bin`), rows);
console.log(countLine('gaps', gaps(tiles)));
console.log(countLine('objects-on-ways', objectsOnWays(tiles)));
console.log(countLine('short-sidewalks', shortSidewalks(tiles)));
console.error(`${((performance.now() - start) / 1000).toFixed(0)} s`);
