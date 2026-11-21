// The model files of the build (models/model-file.ts): each read once, from the device cache or else from beside the bundle.
import { decodeModels, type Models, type Packed } from '../models/model-file';
import { CAPS, DeviceCache } from '../terrain/device-cache';

/**
 * The model files that the engine loads. A file's name changes with its content, so the device cache keeps each under its
 * address with no version. A file that the server refuses (an HTTP 4xx, such as a file that a deploy removed) or that does
 * not decode warns once and is not asked again: its models draw nothing until the page loads again. A file that the
 * network or the server failed to give can be asked again. What the network gave goes to the device cache once the view
 * is idle (`save`), so a load does not wait for the writes. Each load that ends asks for a frame (`wake`).
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
        if (load) return load;
        this.loading++;
        load = this.read(url).then(({ models, again }) => {
            this.loading--;
            this.ended.add(url);
            this.arrived ||= !!models;
            if (again) this.loads.delete(url);
            this.wake();
            return models;
        });
        this.loads.set(url, load);
        return load;
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
    private async read(url: string): Promise<{ models?: Packed[]; again?: boolean }> {
        const kept = await this.cache.then(cache => cache?.read(url)).catch(() => undefined);
        if (kept?.length === 1) {
            const models = await decodeModels(new Uint8Array(kept[0].buffer)).catch(() => undefined);
            if (models) return { models };
        }
        // The network and a server error may pass; a refusal or bytes that do not decode stay.
        let again = true;
        try {
            const response = await fetch(url, { signal: this.stop });
            if (!response.ok) {
                again = response.status >= 500;
                throw new Error(`HTTP ${response.status}`);
            }
            const bytes = new Uint8Array(await response.arrayBuffer());
            again = false;
            const models = await decodeModels(bytes);
            this.fetched.push([url, bytes]);
            return { models };
        } catch (error) {
            if (!this.stop.aborted) console.warn(`The model file ${url} did not load: ${error instanceof Error ? error.message : error}`);
            return { again };
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
