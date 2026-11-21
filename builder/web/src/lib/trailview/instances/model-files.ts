// The model files of the build (models/model-file.ts): each read once, from the device cache or else from beside the bundle.
import { decodeModels, type Models, type Packed } from '../models/model-file';
import { CAPS, DeviceCache } from '../terrain/device-cache';

/**
 * The model files that the engine loads. A file's name changes with its content, so the device cache keeps each under its
 * address with no version. A file that fails, such as one that a deploy removed, warns once and is not asked again: its
 * models draw nothing until the page loads again. What the network gave goes to the device cache once the view is idle
 * (`save`), so a load does not wait for the writes.
 */
export class ModelFiles {
    private readonly cache: Promise<DeviceCache | undefined>;
    private readonly loads = new Map<string, Promise<Packed[] | undefined>>();
    private readonly fetched: [string, Uint8Array<ArrayBuffer>][] = [];

    constructor(readonly models: Models, private readonly stop: AbortSignal) {
        this.cache = DeviceCache.open('models', '', '', CAPS.models).catch(() => undefined);
    }

    /** The models of the file at `url`; undefined where it failed. */
    load(url: string): Promise<Packed[] | undefined> {
        let load = this.loads.get(url);
        if (!load) this.loads.set(url, (load = this.read(url)));
        return load;
    }

    private async read(url: string): Promise<Packed[] | undefined> {
        const kept = await this.cache.then(cache => cache?.read(url)).catch(() => undefined);
        if (kept?.length === 1) {
            const models = await decodeModels(new Uint8Array(kept[0].buffer)).catch(() => undefined);
            if (models) return models;
        }
        try {
            const response = await fetch(url, { signal: this.stop });
            if (!response.ok) throw new Error(`HTTP ${response.status}`);
            const bytes = new Uint8Array(await response.arrayBuffer()), models = await decodeModels(bytes);
            this.fetched.push([url, bytes]);
            return models;
        } catch (error) {
            if (!this.stop.aborted) console.warn(`The model file ${url} did not load: ${error instanceof Error ? error.message : error}`);
            return undefined;
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
