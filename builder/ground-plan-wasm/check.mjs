// Check 7: the WASM build gives the native digest and far block for every request of a `requests` folder.
//
//   node builder/ground-plan-wasm/check.mjs DIR
//
// Prints `cell digest native-ms wasm-ms` per request; exits 1 on a different digest or far block, or an error.

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { build_bytes, initSync } from '../web/src/lib/trailview/plan/pkg/obc_ground_plan_bridge.js';

const dir = process.argv[2];
if (!dir) throw new Error('usage: node check.mjs DIR');
initSync({ module: readFileSync(new URL('../web/src/lib/trailview/plan/pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url)) });
const index = JSON.parse(readFileSync(join(dir, 'index.json'), 'utf8'));
/** The FNV-1a hex of words' little-endian bytes, as `transport::fnv` of the crate. */
const fnv = words => {
    let h = 0xcbf29ce484222325n;
    for (const b of new Uint8Array(words.buffer, words.byteOffset, words.byteLength)) h = BigInt.asUintN(64, (h ^ BigInt(b)) * 0x100000001b3n);
    return h.toString(16).padStart(16, '0');
};
/** A plan's far block as `requests` writes it: its FNV, or the crate's refusal. */
const farOf = plan => {
    try {
        return fnv(plan.far());
    } catch (error) {
        return `refused: ${error instanceof Error ? error.message : error}`;
    }
};
let failed = 0;
for (const entry of index) {
    const name = entry.cell.join('-');
    try {
        const start = performance.now();
        const plan = build_bytes(readFileSync(join(dir, entry.file)));
        const ms = performance.now() - start;
        const digest = plan.digest(), far = farOf(plan);
        plan.free();
        const same = digest === entry.digest && far === entry.far;
        if (!same) failed++;
        const differ = digest === entry.digest ? ` far ${far} != native ${entry.far}` : ` != native ${entry.digest}`;
        console.log(`${name} ${digest}${same ? '' : differ} ${entry.native.total.toFixed(2)} ${ms.toFixed(2)}`);
    } catch (error) {
        failed++;
        console.log(`${name} error: ${error instanceof Error ? error.message : error}`);
    }
}
console.log(`${index.length - failed} of ${index.length} cells give the native digest and far block`);
process.exit(failed ? 1 : 0);
