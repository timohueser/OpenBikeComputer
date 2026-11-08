// The page's one copy of the ground plan bridge: fetched and compiled once, from the first call on, for the main
// thread (which only names a cell's inputs) and every plan worker.
import { sha256 } from '../terrain/device-cache';
import type { Vocabulary } from './debug';
import init, { vocabulary } from './pkg/obc_ground_plan_bridge.js';

const WASM = new URL('./pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url);

/** The compiled module for the workers, the bridge's names and stages, and the SHA-256 of its bytes, which keys the device cache. */
export interface Bridge { module: WebAssembly.Module; vocabulary: Vocabulary; digest: string }

let loaded: Promise<Bridge> | undefined;
/** Plan workers that `preload` started: their scripts load while the page waits for the release and the device. */
const spares: Worker[] = [];
const newWorker = () => new Worker(new URL('./plan-worker.ts', import.meta.url), { type: 'module' });

/** At page load: the bridge and `workers` plan workers start to load. */
export function preload(workers: number): void {
    bridge().catch(() => {});
    while (spares.length < workers) spares.push(newWorker());
}

/** A plan worker that `preload` started, or a new one. */
export function planWorker(): Worker {
    return spares.shift() ?? newWorker();
}

/** The bridge, once it is up on the main thread. A load that fails loads again at the next call. */
export function bridge(): Promise<Bridge> {
    if (!loaded) {
        loaded = load();
        loaded.catch(() => (loaded = undefined));
    }
    return loaded;
}

async function load(): Promise<Bridge> {
    const response = await fetch(WASM);
    if (!response.ok) throw new Error(`the plan bridge answered ${response.status}`);
    const bytes = response.clone().arrayBuffer();
    const module = await compiled(response, bytes);
    await init({ module_or_path: module });
    return { module, vocabulary: JSON.parse(vocabulary()), digest: await sha256(await bytes) };
}

/**
 * A module compiled while it streams in, which lets the browser keep its machine code for the next visit. A server
 * that sends another type than `application/wasm`, or a browser without the streaming compile, gets the bytes compiled.
 */
function compiled(response: Response, bytes: Promise<ArrayBuffer>): Promise<WebAssembly.Module> {
    const whole = async () => WebAssembly.compile(await bytes);
    return typeof WebAssembly.compileStreaming === 'function' ? WebAssembly.compileStreaming(response).catch(whole) : whole();
}
