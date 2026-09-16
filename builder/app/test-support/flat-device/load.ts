/**
 * Load the test device's wasm module from the build tree.
 *
 * Every suite that drives the device calls this in a `beforeAll`. It is here rather than in
 * `src/lib/usb/flat-device.ts` because it reads a file: nothing under `src/` outside a `*.test.ts`
 * may use a Node builtin, and the browser harness loads the same module over the network instead.
 *
 * Memoized, so a suite that calls it once per file pays for one instantiation per worker.
 */

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { initFlatDevice } from "../../src/lib/usb/flat-device";

let loading: Promise<void> | null = null;

export function loadFlatDevice(): Promise<void> {
    if (!loading) {
        const wasm = join(dirname(fileURLToPath(import.meta.url)), "pkg", "obc_flat_device_bg.wasm");
        try {
            loading = initFlatDevice(readFileSync(wasm));
        } catch (cause) {
            throw new Error(
                `the test device is not built (${wasm}). Run \`npm run build:wasm\` in builder/app.`,
                { cause },
            );
        }
    }
    return loading;
}
