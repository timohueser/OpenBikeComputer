// The model files of the build (models/model-file.ts): each read once, from the device cache or else from beside the bundle.
import { decodeModels, type Models, type Packed } from '../models/model-file';
import { CAPS, DeviceCache } from '../terrain/device-cache';

/** The waits before each new try of a load that failed in a way that can pass: the network, a server error, a rate limit. */
const RETRY_MS = [1000, 3000, 9000];

/**
 * The model files that the engine loads. A file's name changes with its content, so the device cache keeps each under its
 * address with no version. A load tries again after a failure that can pass, RETRY_MS apart. A file that still fails,
 * that the server refuses (such as one that a deploy removed) or that does not decode warns once: its models draw nothing
 * until the page loads again. What the network gave goes to the device cache once the view is idle (`save`), so a load
 * does not wait for the writes. Each caller's load asks for a frame (`wake`) when it settles.
 */
export class ModelFiles {
    private readonly cache: Promise<DeviceCache | undefined>;
    private readonly loads = new Map<string, Promise<Packed[] | undefined>>();
    private readonly fetched: [string, Uint8Array<ArrayBuffer>][] = [];
    /** The files whose load ended, whether or not it failed. */
    private readonly ended = new Set<string>();
    private loading = 0;
    private arrived = false;
    /** Whether models arrived before the last update. */
    newContent = false;

    constructor(readonly models: Models, private readonly wake: () => void, private readonly stop: AbortSignal) {
        this.cache = DeviceCache.open('models', '', '', CAPS.models).catch(() => undefined);
    }

    /** The models of the file at `url`; undefined where it failed. */
    load(url: string): Promise<Packed[] | undefined> {
        let load = this.loads.get(url);
        if (!load) {
            this.loading++;
            this.loads.set(url, (load = this.tries(url).then(models => {
                this.loading--;
                this.ended.add(url);
                return models;
            })));
        }
        // Each caller changes what the frames draw once the load settles, also a caller of a file that arrived earlier.
        return load.then(models => {
            this.arrived ||= !!models;
            this.wake();
            return models;
        });
    }

    /** A file's models, with a new try after each failure that can pass while RETRY_MS lasts. */
    private async tries(url: string): Promise<Packed[] | undefined> {
        for (let attempt = 0; ; attempt++) {
            const { models, again, error } = await this.read(url);
            const last = !again || attempt === RETRY_MS.length;
            if (!models && last && !this.stop.aborted) console.warn(`The model file ${url} did not load: ${error}`);
            if (models || last || this.stop.aborted) return models;
            await new Promise(resolve => setTimeout(resolve, RETRY_MS[attempt]));
        }
    }

    get idle(): boolean {
        return this.loading === 0;
    }

    /** Whether the load of each of these files ended. */
    done(...urls: string[]): boolean {
        return urls.every(url => this.ended.has(url));
    }

    /** Takes what arrived since the last update, once a frame. */
    update(): void {
        this.newContent = this.arrived;
        this.arrived = false;
    }

    /** A file's models, and whether a failed load may be asked again. */
    private async read(url: string): Promise<{ models?: Packed[]; again?: boolean; error?: string }> {
        const kept = await this.cache.then(cache => cache?.read(url)).catch(() => undefined);
        if (kept?.length === 1) {
            const models = await decodeModels(new Uint8Array(kept[0].buffer)).catch(() => undefined);
            if (models) return { models };
        }
        // The network, a server error and a rate limit may pass; a refusal or bytes that do not decode stay.
        let again = true;
        try {
            const response = await fetch(url, { signal: this.stop });
            if (!response.ok) {
                again = response.status >= 500 || response.status === 408 || response.status === 429;
                throw new Error(`HTTP ${response.status}`);
            }
            const bytes = new Uint8Array(await response.arrayBuffer());
            again = false;
            const models = await decodeModels(bytes);
            this.fetched.push([url, bytes]);
            return { models };
        } catch (error) {
            return { again, error: error instanceof Error ? error.message : String(error) };
        }
    }

    /** Whether files came from the network that the device cache lacks. */
    get unsaved(): boolean {
        return this.fetched.length > 0;
    }

    /** Writes the files that came from the network to the device cache. */
    save(): void {
        for (const [url, bytes] of this.fetched.splice(0)) void this.cache.then(cache => cache?.write(url, [new Uint32Array(bytes.buffer, bytes.byteOffset, bytes.byteLength / 4)])).catch(() => {});
    }
}
