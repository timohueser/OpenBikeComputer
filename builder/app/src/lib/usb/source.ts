import { Crc32 } from "./crc32";

/** An object to upload, with its length and whole-payload CRC known before the first byte moves. */
export interface ObjectSource {
    readonly totalLen: number;
    readonly crc32: number;
    /** Yield the payload's bytes in order, in slices of at most `chunkSize`. */
    chunks(chunkSize: number): AsyncIterable<Uint8Array>;
}

/** An in-memory object: one CRC pass now, then straight slices. */
export function bytesSource(bytes: Uint8Array): ObjectSource {
    return {
        totalLen: bytes.length,
        crc32: Crc32.of(bytes),
        async *chunks(chunkSize: number) {
            for (let at = 0; at < bytes.length; at += chunkSize) {
                yield bytes.subarray(at, Math.min(at + chunkSize, bytes.length));
            }
        },
    };
}

/**
 * A `Blob` — a fetched map, a picked file — without ever holding it twice. The CRC is needed before
 * the first byte streams and the payload cannot be re-derived from a suffix, so the blob is read
 * twice: once to fingerprint, once to send. For a 200 MB map the alternative is a second 200 MB
 * JavaScript buffer.
 */
export async function blobSource(
    blob: Blob,
    options: { signal?: AbortSignal; onProgress?: (done: number, total: number) => void } = {},
): Promise<ObjectSource> {
    const crc = new Crc32();
    let read = 0;
    for await (const chunk of streamChunks(blob.stream(), options.signal)) {
        crc.update(chunk);
        read += chunk.length;
        options.onProgress?.(read, blob.size);
    }
    return {
        totalLen: blob.size,
        crc32: crc.value(),
        async *chunks(chunkSize: number) {
            for await (const chunk of streamChunks(blob.stream(), options.signal)) {
                for (let at = 0; at < chunk.length; at += chunkSize) {
                    yield chunk.subarray(at, Math.min(at + chunkSize, chunk.length));
                }
            }
        },
    };
}

async function* streamChunks(
    stream: ReadableStream<Uint8Array>,
    signal?: AbortSignal,
): AsyncGenerator<Uint8Array> {
    const reader = stream.getReader();
    const onAbort = () => void reader.cancel(signal?.reason).catch(() => undefined);
    signal?.addEventListener("abort", onAbort, { once: true });
    try {
        signal?.throwIfAborted();
        for (;;) {
            const { done, value } = await reader.read();
            signal?.throwIfAborted();
            if (done) return;
            if (value.length) yield value;
        }
    } finally {
        signal?.removeEventListener("abort", onAbort);
        reader.releaseLock();
    }
}

