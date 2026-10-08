import * as bridge from "./pkg/obc_builder_bridge.js";
import type { InitInput, InitOutput } from "./pkg/obc_builder_bridge.js";

export type Bridge = typeof bridge;
export type { InitInput };

let loading: Promise<Bridge> | null = null;
let exports: InitOutput | null = null;

/** One module and heap per browser realm. A failed fetch can be retried. */
export function initCore(source?: InitInput): Promise<Bridge> {
    if (!loading) {
        const pending = bridge.default(source === undefined ? undefined : { module_or_path: source }).then((output) => {
            exports = output;
            return bridge;
        });
        loading = pending;
        pending.catch(() => { if (loading === pending) loading = null; });
    }
    return loading;
}

/** Synchronous data APIs run after the host initializes the shared core. */
export function core(): Bridge {
    if (!exports) throw new Error("The builder core is not initialized.");
    return bridge;
}

export function coreMemoryBytes(): number {
    return exports?.memory.buffer.byteLength ?? 0;
}
