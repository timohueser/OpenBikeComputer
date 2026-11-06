// The CPU half of the TrailView fault census: prints the counts of gaps, objects on ways, short sidewalks and
// torn faces over the fixture, one JSON line each, the worst places of each kind of torn face with a view
// link, and writes the face and the water samples that census.mjs probes.
//
//   npx vite-node scripts/road-census.ts DIR    # writes DIR/faces.bin and DIR/water.bin, float64 rows
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { latitudeAt } from '../src/lib/planner/layers/mercator';
import { UNITS } from '../src/lib/trailview/frame/space';
import { metresPerUnit } from '../src/lib/trailview/terrain/bins';
import { countLine, faceSamples, fixtureTiles, gaps, objectsOnWays, placeOf, shortSidewalks, tornFaces, waterSamples } from '../test-support/trailview/census';

/** Worst places printed per kind of torn face. */
const WORST = 20;
/** The view of a place: from this many metres south and up, looking north and down on it. */
const BACK_M = 3.5, EYE_M = 6, PITCH = (Math.atan2(EYE_M, BACK_M) * 180) / Math.PI;

const out = process.argv[2];
if (!out) throw new Error('usage: npx vite-node scripts/road-census.ts DIR');
const start = performance.now(), tiles = fixtureTiles();
mkdirSync(out, { recursive: true });
for (const [name, rows] of [['faces', faceSamples(tiles)], ['water', waterSamples(tiles)]] as const) writeFileSync(join(out, `${name}.bin`), rows);
console.log(countLine('gaps', gaps(tiles)));
console.log(countLine('objects-on-ways', objectsOnWays(tiles)));
console.log(countLine('short-sidewalks', shortSidewalks(tiles)));
for (const [kind, points] of Object.entries(tornFaces(tiles.flatMap(({ meshes }) => meshes.flatMap(({ pieces }) => pieces))))) {
    console.log(countLine(`torn-${kind}`, points));
    const seen = new Set<string>();
    for (let i = 0; i < points.length && seen.size < WORST; i += 2) {
        const [x, y] = [points[i], points[i + 1]], place = placeOf(x, y);
        if (seen.has(place)) continue;
        seen.add(place);
        const back = y + BACK_M / metresPerUnit(y);
        console.log(`  ?lat=${latitudeAt(back, UNITS).toFixed(6)}&lon=${((x / UNITS) * 360 - 180).toFixed(6)}&heading=0&pitch=-${PITCH.toFixed(1)}&eye=${EYE_M}&date=2026-08-15&time=10:30&weather=sunny`);
    }
}
console.error(`${((performance.now() - start) / 1000).toFixed(0)} s`);
