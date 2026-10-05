import type { Coordinate } from '../map-types';
import type { SunDay, SunMeta, SunStats } from './sun';
import type { SunRequest } from './sun-worker';

type Job = { resolve(value: unknown): void; reject(reason: unknown): void };
/** Lazily started; a hidden layer with no inspection sends no work. */
export class SunClient {
    private worker?: Worker;
    private serial = 0;
    private pending = new Map<number, Job>();
    readonly metrics = { elapsedMs: 0, stats: null as SunStats | null };
    constructor(private url: string, private dem: string) {}

    private request<T>(request: Omit<Extract<SunRequest, { kind: 'tile' }>, 'id' | 'url' | 'dem'> | Omit<Extract<SunRequest, { kind: 'sample' }>, 'id' | 'url' | 'dem'> | Omit<Extract<SunRequest, { kind: 'day' }>, 'id' | 'url' | 'dem'> | { kind: 'meta' }, signal: AbortSignal): Promise<T> {
        signal.throwIfAborted();
        if (!this.worker) {
            this.worker = new Worker(new URL('./sun-worker.ts', import.meta.url), { type: 'module' });
            this.worker.onmessage = ({ data }) => {
                const job = this.pending.get(data.id);
                if (!job) return;
                this.pending.delete(data.id);
                this.metrics.elapsedMs = data.elapsedMs ?? 0;
                this.metrics.stats = data.stats ?? this.metrics.stats;
                if (data.error) job.reject(new Error(data.error)); else job.resolve(data.result);
            };
            this.worker.onerror = () => { for (const job of this.pending.values()) job.reject(new Error('Sunlight worker failed')); this.dispose(); };
        }
        const id = ++this.serial;
        return new Promise<T>((resolve, reject) => {
            const cancel = () => { this.worker?.postMessage({ cancel: id }); this.pending.delete(id); reject(signal.reason); };
            signal.addEventListener('abort', cancel, { once: true });
            this.pending.set(id, {
                resolve: value => { signal.removeEventListener('abort', cancel); resolve(value as T); },
                reject: error => { signal.removeEventListener('abort', cancel); reject(error); },
            });
            this.worker!.postMessage({ ...request, id, url: this.url, dem: this.dem });
        });
    }

    meta(signal: AbortSignal) { return this.request<SunMeta>({ kind: 'meta' }, signal); }
    tile(z: number, x: number, y: number, date: string, minute: number, signal: AbortSignal, size = 128) { return this.request<Uint8Array>({ kind: 'tile', z, x, y, size, date, minute }, signal); }
    sample(coordinates: Coordinate[], date: string, minute: number, signal: AbortSignal) { return this.request<Uint8Array>({ kind: 'sample', coordinates: coordinates.map(([lon, lat]) => [lon, lat]), date, minute }, signal); }
    day(coordinate: Coordinate, date: string, signal: AbortSignal) { return this.request<SunDay>({ kind: 'day', coordinate: [coordinate[0], coordinate[1]], date }, signal); }
    dispose() { this.worker?.terminate(); this.worker = undefined; for (const job of this.pending.values()) job.reject(new Error('Sunlight worker stopped')); this.pending.clear(); }
}
