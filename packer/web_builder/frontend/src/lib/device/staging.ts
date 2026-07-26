/**
 * Staging: getting a hundreds-of-megabyte artifact onto a device without ever holding it (C4, #903).
 *
 * ## Why a map is not simply piped from the CDN into the cable
 *
 * That is the shape everyone reaches for, and two independent rules forbid it:
 *
 * 1. **The transfer descriptor announces the whole-object CRC-32 before the first byte moves**
 *    ([interface spec](../../../../../../obc-ble-interface-spec.md) §4.2). You cannot know a
 *    checksum of bytes you have not seen, so the object has to be readable **twice** — once to
 *    fingerprint, once to send.
 * 2. **[`OBCC_Spec.md`](../../../../../../OBCC_Spec.md) §7**: a consumer MUST verify a download
 *    against the manifest's `bytes` and `sha256` *before writing it to a device*. A single pass
 *    that discovers the mismatch at the end has already written the corrupt file.
 *
 * So the artifact lands somewhere local first. The only question is *where*, and the answer must
 * not be "the tab's heap": `await response.arrayBuffer()` on a country map is a 300 MB allocation,
 * and `response.blob()` only moves the problem into the browser's blob store, which keeps blobs in
 * memory until its own quota pushes them to disk — a threshold measured in gigabytes on a normal
 * machine, so a 300 MB map stays resident.
 *
 * **The origin-private file system is the answer.** It is a real file, written through a
 * `WritableStream`, so the bytes go to disk as they arrive and nothing bigger than one chunk is ever
 * live. The fetch is read once, the file is read once more to send, and peak memory is a chunk.
 * OPFS is invisible to the user and evictable — which is why #894 rejected it for a *ride library* —
 * but that is precisely right for a scratch copy that exists for the length of one upload and is
 * deleted after it.
 *
 * ## The seam
 *
 * {@link StagingArea} exists so the pipeline can be driven under Node, where there is no OPFS: the
 * flat-memory measurement in `memory.test.ts` runs this exact code against a `node:fs` staging area,
 * which is the difference between claiming the path streams and showing it.
 */

import { Crc32 } from "../usb/crc32";
import type { ObjectSource } from "../usb/client";
import { Sha256 } from "./sha256";

/** Bytes pulled from a staged file per read. Large enough that a 300 MB file is ~4600 reads, small
 *  enough that peak memory is measured in tens of kilobytes. */
export const STAGE_READ_CHUNK = 64 * 1024;

/** Why staging failed. Each one is a different sentence for the rider, which is why they are split. */
export type StagingErrorCode =
    | "unsupported"
    | "quota"
    | "network"
    | "size-mismatch"
    | "digest-mismatch"
    | "aborted"
    | "io";

/** A staging failure, with a message written for a rider. */
export class StagingError extends Error {
    readonly code: StagingErrorCode;

    constructor(code: StagingErrorCode, message: string, options?: { cause?: unknown }) {
        super(message, options);
        this.name = "StagingError";
        this.code = code;
    }
}

/** An open staged file being written. */
export interface StagedWriter {
    write(chunk: Uint8Array): Promise<void>;
    /** Close the file and hand back a re-readable handle. */
    finish(): Promise<StagedFile>;
    /** Give up and remove the partial. Never throws. */
    abort(): Promise<void>;
}

/** A staged file: readable as many times as needed, deleted when the caller is done with it. */
export interface StagedFile {
    readonly bytes: number;
    /** Read the file back in order, in slices of at most {@link STAGE_READ_CHUNK}. */
    chunks(): AsyncIterable<Uint8Array>;
    /** Delete it. Never throws — a scratch file that outlives its upload is a wasted megabyte,
     *  not a failure worth showing anyone. */
    discard(): Promise<void>;
}

/** Somewhere to put a scratch copy. OPFS in a browser, a temp directory under Node. */
export interface StagingArea {
    /** Open `name` for writing, replacing anything already staged under it. */
    open(name: string): Promise<StagedWriter>;
}

// --- the OPFS implementation --------------------------------------------------

/** The slice of OPFS this file uses, declared structurally so Node can be handed a stand-in. */
interface OpfsDirectory {
    getFileHandle(name: string, options?: { create?: boolean }): Promise<OpfsFileHandle>;
    removeEntry(name: string, options?: { recursive?: boolean }): Promise<void>;
}

interface OpfsFileHandle {
    createWritable(options?: { keepExistingData?: boolean }): Promise<WritableStream<Uint8Array>>;
    getFile(): Promise<Blob>;
}

/** The origin-private root, or `null` where the browser has none (no `navigator.storage`, or a
 *  context where OPFS is unavailable). */
export function opfsRoot(): Promise<OpfsDirectory> | null {
    const storage = (globalThis.navigator as { storage?: { getDirectory?: () => Promise<OpfsDirectory> } } | undefined)
        ?.storage;
    return storage?.getDirectory ? storage.getDirectory() : null;
}

/** A {@link StagingArea} over the origin-private file system, or `null` where there is none. */
export function opfsStaging(): StagingArea | null {
    if (!opfsRoot()) return null;
    return {
        async open(name: string): Promise<StagedWriter> {
            let dir: OpfsDirectory;
            let handle: OpfsFileHandle;
            let writer: WritableStreamDefaultWriter<Uint8Array>;
            try {
                dir = await (opfsRoot() as Promise<OpfsDirectory>);
                handle = await dir.getFileHandle(name, { create: true });
                writer = (await handle.createWritable()).getWriter();
            } catch (cause) {
                throw new StagingError(
                    "io",
                    `This browser would not open a scratch file for the download (${describe(cause)}).`,
                    { cause },
                );
            }
            let written = 0;
            const remove = async () => {
                try {
                    await dir.removeEntry(name);
                } catch {
                    // Already gone, or the origin's storage was cleared under us. Either way there
                    // is nothing left to clean up and nothing worth telling anyone.
                }
            };
            return {
                async write(chunk) {
                    try {
                        // `ready` is the backpressure: without awaiting it the writes queue in
                        // memory and the whole point of writing to a file is lost.
                        await writer.ready;
                        await writer.write(chunk);
                        written += chunk.length;
                    } catch (cause) {
                        throw stagingWriteError(cause);
                    }
                },
                async finish() {
                    try {
                        await writer.close();
                    } catch (cause) {
                        throw stagingWriteError(cause);
                    }
                    const file = await handle.getFile();
                    return {
                        bytes: file.size,
                        chunks: () => blobChunks(file),
                        discard: remove,
                    };
                },
                async abort() {
                    try {
                        await writer.abort();
                    } catch {
                        // The stream is already broken; the file removal below is what matters.
                    }
                    await remove();
                    void written;
                },
            };
        },
    };
}

function stagingWriteError(cause: unknown): StagingError {
    const message = describe(cause);
    // Chromium reports an exhausted origin quota as a QuotaExceededError; the remedy is the user's
    // (free some disk, or clear site data), so it gets its own sentence rather than "write failed".
    const quota = /quota/i.test(message) || (cause as { name?: string } | null)?.name === "QuotaExceededError";
    return quota
        ? new StagingError(
              "quota",
              "There isn't enough free space on this computer to hold the map while it transfers.",
              { cause },
          )
        : new StagingError("io", `Writing the download to disk failed (${message}).`, { cause });
}

async function* blobChunks(blob: Blob): AsyncGenerator<Uint8Array> {
    const reader = blob.stream().getReader();
    try {
        for (;;) {
            const { done, value } = await reader.read();
            if (done) return;
            // A blob stream picks its own slicing; re-slice so a consumer's memory is bounded by
            // STAGE_READ_CHUNK rather than by whatever the browser felt like handing over.
            for (let at = 0; at < value.length; at += STAGE_READ_CHUNK) {
                yield value.subarray(at, Math.min(at + STAGE_READ_CHUNK, value.length));
            }
        }
    } finally {
        reader.releaseLock();
    }
}

// --- staging a download -------------------------------------------------------

/** What the manifest says the artifact must be. Both are checked; both are `OBCC_Spec.md` §7. */
export interface ExpectedArtifact {
    readonly bytes: number;
    /** Lowercase hex SHA-256. */
    readonly sha256: string;
}

/** A verified local copy, ready to send. */
export interface StagedArtifact {
    readonly bytes: number;
    /** Computed on the way in, so the upload descriptor needs no extra pass. */
    readonly crc32: number;
    readonly sha256: string;
    /** The upload source. Reads the staged file; safe to iterate more than once (a retry). */
    readonly source: ObjectSource;
    /** Delete the scratch copy. Call it when the upload has committed, or failed for good. */
    discard(): Promise<void>;
}

export interface StageOptions {
    readonly area: StagingArea;
    /** The scratch file's name. One per artifact, so a re-run replaces rather than accumulates. */
    readonly name: string;
    readonly expect?: ExpectedArtifact;
    readonly signal?: AbortSignal;
    /** Called as bytes land. `total` is `expect.bytes` when known, else the response's length. */
    readonly onProgress?: (done: number, total: number) => void;
}

/**
 * Drain `body` into the staging area, fingerprinting as it goes, and verify before returning.
 *
 * Nothing bigger than one stream chunk is ever live: the CRC-32 and the SHA-256 both fold in, the
 * bytes go straight to the file, and the reference is dropped. A size or digest mismatch deletes
 * the partial and throws — the caller never gets a handle to bytes that failed their check, which
 * is what makes "verify before writing to a device" structural rather than a rule to remember.
 */
export async function stageStream(
    body: ReadableStream<Uint8Array>,
    options: StageOptions,
): Promise<StagedArtifact> {
    const { area, name, expect, signal, onProgress } = options;
    const writer = await area.open(name);
    const crc = new Crc32();
    const sha = new Sha256();
    let written = 0;
    const total = expect?.bytes ?? 0;

    const reader = body.getReader();
    try {
        onProgress?.(0, total);
        for (;;) {
            if (signal?.aborted) throw new StagingError("aborted", "The download was cancelled.");
            const { done, value } = await reader.read();
            if (done) break;
            if (!value.length) continue;
            // Fail fast on an over-long body rather than filling the disk: the manifest is the
            // authority on the size, so a response that exceeds it is already wrong.
            if (expect && written + value.length > expect.bytes) {
                throw new StagingError(
                    "size-mismatch",
                    `The download is longer than the catalog says (${expect.bytes} bytes). Nothing was kept.`,
                );
            }
            crc.update(value);
            sha.update(value);
            await writer.write(value);
            written += value.length;
            onProgress?.(written, total || written);
        }
    } catch (cause) {
        await writer.abort();
        throw asStagingError(cause);
    } finally {
        reader.releaseLock();
    }

    let staged: StagedFile;
    try {
        staged = await writer.finish();
    } catch (cause) {
        await writer.abort();
        throw asStagingError(cause);
    }

    const digest = sha.hex();
    const fail = verdict(expect, written, digest);
    if (fail) {
        await staged.discard();
        throw fail;
    }
    return {
        bytes: written,
        crc32: crc.value(),
        sha256: digest,
        source: {
            totalLen: written,
            crc32: crc.value(),
            chunks: (chunkSize: number) => resliced(staged, chunkSize),
        },
        discard: () => staged.discard(),
    };
}

function verdict(expect: ExpectedArtifact | undefined, written: number, digest: string): StagingError | null {
    if (!expect) return null;
    if (written !== expect.bytes) {
        return new StagingError(
            "size-mismatch",
            `The download is ${written} bytes; the catalog says ${expect.bytes}. Nothing was kept — try again.`,
        );
    }
    if (digest !== expect.sha256.toLowerCase()) {
        return new StagingError(
            "digest-mismatch",
            `The download failed its checksum (the catalog says ${short(expect.sha256)}, the file is ` +
                `${short(digest)}). Nothing was sent to the device — try again.`,
        );
    }
    return null;
}

async function* resliced(file: StagedFile, chunkSize: number): AsyncGenerator<Uint8Array> {
    for await (const chunk of file.chunks()) {
        if (chunk.length <= chunkSize) {
            yield chunk;
            continue;
        }
        for (let at = 0; at < chunk.length; at += chunkSize) {
            yield chunk.subarray(at, Math.min(at + chunkSize, chunk.length));
        }
    }
}

function asStagingError(cause: unknown): StagingError {
    if (cause instanceof StagingError) return cause;
    if (cause instanceof DOMException && cause.name === "AbortError") {
        return new StagingError("aborted", "The download was cancelled.", { cause });
    }
    return new StagingError("network", `The download stopped (${describe(cause)}). Nothing was kept.`, { cause });
}

function short(hex: string): string {
    return hex.slice(0, 12) + "…";
}

function describe(cause: unknown): string {
    return cause instanceof Error ? cause.message : String(cause);
}
