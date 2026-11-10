// The CPU half of the TrailView fault census: prints the counts of gaps, objects on ways, short sidewalks and
// torn faces over the fixture, one JSON line each, the worst places of each kind of torn face with a view
// link, and writes the face and the water samples that census.mjs probes. It exits with an error when a
// count is over its gate or the torn faces differ from census-baseline.json.
//
//   npx vite-node scripts/road-census.ts DIR    # writes DIR/faces.bin and DIR/water.bin, float64 rows
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { latitudeAt } from '../src/lib/planner/layers/mercator';
import { UNITS } from '../src/lib/trailview/frame/space';
import { metresPerUnit } from '../src/lib/trailview/terrain/bins';
import { countLine, faceSamples, fixtureTiles, gaps, objectsOnWays, placeOf, shortSidewalks, tornFaces, waterSamples } from '../test-support/trailview/census';
import baseline from '../test-support/trailview/census-baseline.json' with { type: 'json' };

/** The gates of the counts on the fixture. A face of no area covers no gap sample; most short sidewalks are beside tight bends. */
const GATES = { gaps: 20600, 'objects-on-ways': 0, 'short-sidewalks': 119 };
/** Worst places printed per kind of torn face. */
const WORST = 20;
/** The view of a place: from this many metres south and up, looking north and down on it. */
const BACK_M = 3.5, EYE_M = 6, PITCH = (Math.atan2(EYE_M, BACK_M) * 180) / Math.PI;

const out = process.argv[2];
if (!out) throw new Error('usage: npx vite-node scripts/road-census.ts DIR');
const start = performance.now(), tiles = fixtureTiles();
mkdirSync(out, { recursive: true });
for (const [name, rows] of [['faces', faceSamples(tiles)], ['water', waterSamples(tiles)]] as const) writeFileSync(join(out, `${name}.bin`), rows);
const faults: string[] = [];
for (const [count, points] of [['gaps', gaps(tiles)], ['objects-on-ways', objectsOnWays(tiles)], ['short-sidewalks', shortSidewalks(tiles)]] as const) {
    console.log(countLine(count, points));
    if (points.length / 2 > GATES[count]) faults.push(`${count}: ${points.length / 2} samples, the gate is ${GATES[count]}`);
}
const torn = tornFaces(tiles.flatMap(({ meshes }) => meshes.flatMap(({ pieces }) => pieces)));
for (const kind of Object.keys(torn)) if (!(kind in baseline)) faults.push(`torn-${kind}: not in census-baseline.json`);
for (const [kind, most] of Object.entries(baseline)) {
    const count = (torn[kind as keyof typeof torn]?.length ?? 0) / 2;
    // A lower count lowers the baseline in the same commit, so that it cannot hide a new torn face.
    if (count !== most) faults.push(`torn-${kind}: ${count}, census-baseline.json holds ${most}: ${count > most ? 'fix the faces, do not raise the baseline' : 'lower the baseline'}`);
}
for (const [kind, points] of Object.entries(torn)) {
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
for (const fault of faults) console.error(fault);
if (faults.length) process.exitCode = 1;
