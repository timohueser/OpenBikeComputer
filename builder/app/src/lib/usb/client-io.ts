import { core } from "../core/bridge";
import type { StoreClient } from "../core/pkg/obc_builder_bridge.js";
import { DeviceError, asDeviceError, refusalError } from "./errors";
import { withAbort, type DeviceLink } from "./pipe";
import { Opcode, toSafeNumber, type Refusal } from "./protocol";
import { MAX_DEVICE_RECORD, MAX_HOST_CONTROL_RECORD, MAX_HOST_STREAM_RECORD, MAX_STREAM_PAYLOAD, RecordChannel, frameRecord } from "./records";
import type { ObjectSource } from "./source";
import type { TransferOptions } from "./client";

/**
 * Payload bytes handed to the transport per `write`, batched into whole stream records.
 *
 * A full stream record is 8,192 payload bytes and is not a tuning knob; this is how many of those go
 * into one `transferOut`. Sweep it with {@link UPLOAD_WINDOW}: their product is what the browser
 * keeps queued at the endpoint, which has to cover a device-side flush without the wire going idle.
 */
export const DEFAULT_BATCH_BYTES = 64 * 1024;

/**
 * How many batched writes an upload keeps in flight at once. A chosen value, and the lever is
 * latency rather than bandwidth: with exactly one outstanding, the wire is idle for a renderer to
 * USB-service round trip between every batch. Small on purpose — backpressure is what stops a 300 MB
 * map being read into the tab faster than the card can take it.
 */
export const UPLOAD_WINDOW = 4;

export type NativeOutcome = { kind: string } & Record<string, unknown>;
type NativeError = { kind: string; refusal?: { code: number; detail: number; context: string } };
type Result = { ok: true; outcome: NativeOutcome } | { ok: false; error: NativeError };
type Action =
    | { kind: "send"; token: string; channel: "control" | "stream"; opcode: number | null; requestId: number | null }
    | { kind: "readSource"; token: string; offset: string; maxLength: number }
    | { kind: "writeSink"; token: string; offset: string }
    | { kind: "progress"; done: string; total: string }
    | { kind: "resetSink" | "resetChannels" | "restore" }
    | ({ kind: "complete" } & Result);
type Waiter = { opcode: number; resolve(value: NativeOutcome): void; reject(error: DeviceError): void; cleanup(): void };
type Operation = Waiter & {
    source?: ObjectSource;
    reader?: SourceReader;
    options: TransferOptions;
    writes: AbortController;
    chunks: Uint8Array[];
    sent: boolean;
    sourceReads: number;
    completed: Promise<void>;
    release(): void;
    failure?: unknown;
};
type Frame = { token: bigint; bytes: Uint8Array };

const now = () => BigInt(Math.floor(performance.now()));

export function nativeError(error: NativeError, opcode: number, signal?: AbortSignal): DeviceError {
    if (error.kind === "remote" && error.refusal) {
        return refusalError({ ...error.refusal, context: BigInt(error.refusal.context) } as Refusal, opcode);
    }
    switch (error.kind) {
        case "busy": return new DeviceError("busy", "Another operation is already running. Wait for it to finish.");
        case "cancelled": return new DeviceError(signal?.aborted ? "aborted" : "cancelled", "The transfer was cancelled.", { cause: signal?.reason });
        case "timeout": return new DeviceError("timeout", "The device did not answer in time.");
        case "linkLost": return new DeviceError("link", "The device disconnected.");
        case "checksum": return new DeviceError("checksum", "The payload did not match its checksum. Nothing was kept.");
        case "catalogChanged": return new DeviceError("catalog-changed", "The device's catalog changed while it was being listed.");
        case "invalidInput": return new DeviceError("invalid-request", "The device operation is invalid.");
        case "io": return new DeviceError("device-error", "The transfer could not complete its input or output.");
        case "storeChanged": return new DeviceError("link", "The card changed. Reconnect and refresh the library.");
        case "notCommitted": return new DeviceError("device-error", "The device did not commit the operation.");
        case "outcomeUnknown": return new DeviceError("link", "The device disconnected before its result was known.");
        default: return new DeviceError("protocol", "The device sent an invalid response. Reconnect and try again.");
    }
}

function boundaryError(cause: unknown, opcode: number): DeviceError {
    if (typeof cause === "string") {
        try { return nativeError(JSON.parse(cause) as NativeError, opcode); } catch { /* A validation message has no JSON body. */ }
        return new DeviceError("invalid-request", cause);
    }
    return asDeviceError(cause);
}

class SourceReader {
    private readonly iterator: AsyncIterator<Uint8Array>;
    private chunk: Uint8Array = new Uint8Array(0);
    private at = 0;
    private offset = 0;

    constructor(private readonly source: ObjectSource, size: number) {
        this.iterator = source.chunks(size)[Symbol.asyncIterator]();
    }

    async read(offset: number, length: number): Promise<Uint8Array> {
        if (offset !== this.offset) throw new DeviceError("protocol", "The upload source did not provide the requested offset.");
        const bytes = new Uint8Array(length);
        let count = 0;
        while (count < length) {
            if (this.at === this.chunk.length) {
                const next = await this.iterator.next();
                if (next.done) break;
                this.chunk = next.value;
                this.at = 0;
                if (!this.chunk.length) continue;
            }
            const take = Math.min(length - count, this.chunk.length - this.at);
            bytes.set(this.chunk.subarray(this.at, this.at + take), count);
            count += take;
            this.at += take;
        }
        this.offset += count;
        if (this.offset === this.source.totalLen) {
            if (this.at < this.chunk.length || !(await this.iterator.next()).done) {
                throw new DeviceError("protocol", "The upload source yielded more bytes than it declared.");
            }
        }
        return bytes.subarray(0, count);
    }

    close(): void { void this.iterator.return?.().catch(() => undefined); }
}

/** Executes I/O effects without awaiting a write before it can drain a control cancellation. */
export class ClientIO {
    readonly engine: StoreClient;
    private readonly control: RecordChannel;
    private readonly stream: RecordChannel;
    private reads = new AbortController();
    private closed = false;
    private draining = false;
    private timer: ReturnType<typeof setTimeout> | undefined;
    private ready: Promise<void> = Promise.resolve();
    private operation: Operation | null = null;
    private readonly queries = new Map<bigint, Waiter>();
    private frames: Frame[] = [];
    private recordsPerBatch = 8;

    constructor(private readonly link: DeviceLink, timeout: number) {
        this.engine = new (core().StoreClient)(MAX_DEVICE_RECORD, MAX_HOST_STREAM_RECORD, timeout, 0);
        this.control = new RecordChannel(link.control, MAX_HOST_CONTROL_RECORD, MAX_DEVICE_RECORD);
        this.stream = new RecordChannel(link.stream, MAX_HOST_STREAM_RECORD, MAX_DEVICE_RECORD);
        this.receive(this.control, "control", this.reads);
        this.receive(this.stream, "stream", this.reads);
    }

    get liveTransfer(): number | null { return this.closed ? null : this.engine.activeTransfer() ?? null; }

    private checkOpen(): void {
        if (this.closed || !this.link.control.open || !this.link.stream.open) {
            throw new DeviceError("link", "The device link is closed.");
        }
    }

    async run(request: Record<string, unknown>, opcode: number, options: TransferOptions = {}, source?: ObjectSource): Promise<NativeOutcome> {
        this.checkOpen();
        if (options.signal?.aborted) throw new DeviceError("aborted", "The transfer was cancelled.", { cause: options.signal.reason });
        for (;;) {
            await this.ready;
            this.checkOpen();
            if (options.signal?.aborted) throw new DeviceError("aborted", "The transfer was cancelled.", { cause: options.signal.reason });
            const active = this.operation;
            if (!active) break;
            const transfer = (op: number) => op === Opcode.Get || op === Opcode.Put;
            if (transfer(opcode) && transfer(active.opcode)) {
                throw new DeviceError("busy", "Another transfer is already running. Wait for it to finish.");
            }
            try { await withAbort(active.completed, options.signal, "the device operation"); }
            catch (cause) { throw asDeviceError(cause); }
        }
        if (source) {
            const records = Math.max(1, Math.ceil((options.batchBytes ?? DEFAULT_BATCH_BYTES) / MAX_STREAM_PAYLOAD));
            if (!Number.isSafeInteger(records) || records * UPLOAD_WINDOW > 0xffffffff) {
                throw new DeviceError("invalid-request", "The upload batch size is invalid.");
            }
            try { this.engine.setUploadWindow(records, records * UPLOAD_WINDOW); } catch (cause) { throw boundaryError(cause, opcode); }
            this.recordsPerBatch = records;
        }
        try { this.engine.start(JSON.stringify(request), undefined, now()); } catch (cause) { throw boundaryError(cause, opcode); }
        return new Promise<NativeOutcome>((resolve, reject) => {
            let release!: () => void;
            const completed = new Promise<void>((resolve) => { release = resolve; });
            const abort = () => {
                operation.writes.abort(options.signal?.reason);
                this.frames = [];
                this.engine.cancel(now());
                this.drain();
            };
            const operation: Operation = {
                opcode, source, options, resolve, reject, sent: false, sourceReads: 0, chunks: [],
                completed, release,
                writes: new AbortController(),
                cleanup: () => { options.signal?.removeEventListener("abort", abort); operation.reader?.close(); },
            };
            this.operation = operation;
            options.signal?.addEventListener("abort", abort, { once: true });
            this.drain();
        });
    }

    async query(request: Record<string, unknown>, opcode: number, signal?: AbortSignal): Promise<NativeOutcome> {
        this.checkOpen();
        if (signal?.aborted) throw new DeviceError("aborted", "The request was cancelled.", { cause: signal.reason });
        await this.ready;
        this.checkOpen();
        if (signal?.aborted) throw new DeviceError("aborted", "The request was cancelled.", { cause: signal.reason });
        let id: bigint;
        try {
            id = request.op === "catalog"
                ? this.engine.queryCatalog(request.kind as number | undefined, undefined, now())
                : this.engine.query(JSON.stringify(request), undefined, now());
        } catch (cause) { throw boundaryError(cause, opcode); }
        return new Promise<NativeOutcome>((resolve, reject) => {
            const abort = () => {
                this.queries.delete(id);
                this.engine.cancelQuery(id);
                signal?.removeEventListener("abort", abort);
                reject(new DeviceError("aborted", "The request was cancelled.", { cause: signal?.reason }));
                this.drain();
            };
            this.queries.set(id, { opcode, resolve, reject, cleanup: () => signal?.removeEventListener("abort", abort) });
            signal?.addEventListener("abort", abort, { once: true });
            this.drain();
        });
    }

    private receive(channel: RecordChannel, name: "control" | "stream", controller: AbortController): void {
        void (async () => {
            try {
                while (!controller.signal.aborted) {
                    const bytes = await channel.next(controller.signal);
                    if (controller.signal.aborted || this.closed) return;
                    this.engine[name](bytes, now());
                    this.drain();
                }
            } catch {
                if (!controller.signal.aborted && !this.closed) { this.engine.linkLost(now()); this.drain(); }
            }
        })();
    }

    private sent(operation: Operation): void {
        if (operation.sent) return;
        operation.sent = true;
        try { operation.options.onSent?.(); } catch (cause) { console.warn("obc: an upload's onSent hook threw; ignoring it", cause); }
    }

    private send(action: Extract<Action, { kind: "send" }>, bytes: Uint8Array): void {
        const token = BigInt(action.token);
        if (action.channel === "stream") {
            this.frames.push({ token, bytes: frameRecord(bytes) });
            if (this.frames.length >= this.recordsPerBatch) this.flush();
            return;
        }
        if (action.opcode === Opcode.Cancel) { this.frames = []; this.operation?.writes.abort(); }
        const operation = this.operation;
        void this.control.send(bytes, this.reads.signal).then(() => {
            if (this.closed) return;
            if (action.opcode === Opcode.Put && operation?.source?.totalLen === 0) this.sent(operation);
            this.engine.written(token, now());
            this.drain();
        }, (cause: unknown) => this.failed(token, cause, operation));
    }

    private flush(): void {
        if (!this.frames.length) return;
        const frames = this.frames;
        this.frames = [];
        const bytes = new Uint8Array(frames.reduce((n, frame) => n + frame.bytes.length, 0));
        let at = 0;
        for (const frame of frames) { bytes.set(frame.bytes, at); at += frame.bytes.length; }
        const operation = this.operation;
        void this.link.stream.write(bytes, operation?.writes.signal).then(() => {
            if (this.closed) return;
            // All tokens in this batch refer to bytes the transport has actually taken.
            for (const frame of frames) this.engine.written(frame.token, now());
            this.drain();
        }, (cause: unknown) => {
            for (const frame of frames) this.failed(frame.token, cause, operation);
        });
    }

    private failed(token: bigint, cause: unknown, operation: Operation | null): void {
        if (this.closed) return;
        if (operation) operation.failure ??= cause;
        this.engine.ioFailed(token, now());
        this.drain();
    }

    private readSource(action: Extract<Action, { kind: "readSource" }>): void {
        const operation = this.operation;
        const token = BigInt(action.token);
        if (!operation?.source) { this.engine.ioFailed(token, now()); return; }
        const offset = toSafeNumber(BigInt(action.offset), "the upload source offset");
        if (offset === 0) { operation.reader?.close(); operation.reader = new SourceReader(operation.source, action.maxLength); }
        operation.sourceReads++;
        void operation.reader!.read(offset, action.maxLength).then((bytes) => {
            if (!this.closed) this.engine.source(token, BigInt(offset), bytes, now());
        }, (cause: unknown) => {
            if (!this.closed) { operation.failure ??= cause; this.engine.ioFailed(token, now()); }
        }).finally(() => {
            operation.sourceReads--;
            if (!this.closed) this.drain();
        });
    }

    private reset(): void {
        this.frames = [];
        this.operation?.writes.abort();
        this.reads.abort();
        this.ready = Promise.all([this.control.reset(), this.stream.reset()]).then(() => {
            if (this.closed) return;
            this.reads = new AbortController();
            this.receive(this.control, "control", this.reads);
            this.receive(this.stream, "stream", this.reads);
        }).catch(() => { if (!this.closed) { this.engine.linkLost(now()); this.drain(); } });
    }

    private finish(result: Result): void {
        const operation = this.operation;
        if (!operation) return;
        this.operation = null;
        this.frames = [];
        operation.cleanup();
        operation.release();
        if (result.ok) {
            if (result.outcome.kind === "get") result.outcome.chunks = operation.chunks;
            operation.resolve(result.outcome);
        } else {
            operation.reject((result.error.kind === "io" || result.error.kind === "cancelled") && operation.failure && !operation.options.signal?.aborted
                ? asDeviceError(operation.failure)
                : nativeError(result.error, operation.opcode, operation.options.signal));
        }
    }

    private drain(): void {
        if (this.draining || this.closed) return;
        this.draining = true;
        try {
            for (;;) {
                const next = this.engine.nextAction();
                if (!next) break;
                let action: Action;
                let bytes: Uint8Array = new Uint8Array(0);
                try {
                    action = JSON.parse(next.metadata) as Action;
                    if (action.kind === "send" || action.kind === "writeSink") bytes = next.bytes;
                } finally { next.free(); }
                switch (action.kind) {
                    case "send": this.send(action, bytes); break;
                    case "readSource": this.readSource(action); break;
                    case "writeSink":
                        this.operation?.chunks.push(bytes);
                        this.engine.sinkWritten(BigInt(action.token), BigInt(action.offset), bytes.length, now());
                        break;
                    case "resetSink": if (this.operation) this.operation.chunks = []; break;
                    case "resetChannels": this.reset(); break;
                    case "restore": this.engine.linkLost(now()); break;
                    case "progress": {
                        const operation = this.operation;
                        if (!operation) break;
                        const done = toSafeNumber(BigInt(action.done), "the transferred byte count");
                        const total = toSafeNumber(BigInt(action.total), "the transfer length");
                        operation.options.onProgress?.(done, operation.source ? total : operation.options.expectedLength ?? total);
                        if (operation.source && done === total && total > 0) this.sent(operation);
                        break;
                    }
                    case "complete": this.finish(action); break;
                }
            }
            for (;;) {
                const text = this.engine.nextQueryResult();
                if (text === undefined) break;
                const result = JSON.parse(text) as Result & { query: string };
                const id = BigInt(result.query);
                const waiter = this.queries.get(id);
                if (!waiter) continue;
                this.queries.delete(id);
                waiter.cleanup();
                if (result.ok) waiter.resolve(result.outcome);
                else waiter.reject(nativeError(result.error, waiter.opcode));
            }
            if (!this.operation?.sourceReads) this.flush();
        } catch (cause) {
            if (!this.operation) throw cause;
            this.operation.failure ??= cause;
            this.frames = [];
            this.operation.writes.abort();
            this.engine.cancel(now());
            queueMicrotask(() => this.drain());
        } finally {
            this.draining = false;
            clearTimeout(this.timer);
            const deadline = this.engine.nextDeadline();
            if (deadline !== undefined) {
                this.timer = setTimeout(() => { if (!this.closed) { this.engine.tick(now()); this.drain(); } },
                    Math.max(0, Number(deadline - now())) + 1);
            }
        }
    }

    async close(): Promise<void> {
        if (this.closed) return;
        this.closed = true;
        clearTimeout(this.timer);
        this.reads.abort();
        this.operation?.writes.abort();
        this.operation?.cleanup();
        this.operation?.reject(new DeviceError("link", "The device link was closed."));
        this.operation?.release();
        this.operation = null;
        this.frames = [];
        for (const waiter of this.queries.values()) { waiter.cleanup(); waiter.reject(new DeviceError("link", "The device link was closed.")); }
        this.queries.clear();
        try { await this.link.close(); } finally { this.engine.free(); }
    }
}
