/**
 * Test-only scaffolding for the device flows. **Imported by tests, never by the app.**
 *
 * The two things here are the ones a browser API cannot provide under Node:
 *
 * - a {@link StagingArea} over `node:fs`, so the staging pipeline — the thing that has to keep a
 *   300 MB map out of memory — can be driven and *measured* in CI rather than argued about;
 * - a synthetic `ReadableStream` that produces a large body without ever holding it, so the
 *   measurement is of the code under test and not of the test's own fixture.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { open, unlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { STAGE_READ_CHUNK, type StagedFile, type StagedWriter, type StagingArea } from "./staging";

/** A staging area in a fresh temp directory. `cleanup()` removes the whole thing. */
export function tempStaging(): { area: StagingArea; cleanup: () => void; dir: string } {
    const dir = mkdtempSync(join(tmpdir(), "obc-staging-"));
    const area: StagingArea = {
        async open(name: string): Promise<StagedWriter> {
            const path = join(dir, name);
            const handle = await open(path, "w");
            const remove = async () => {
                try {
                    await unlink(path);
                } catch {
                    // Already gone.
                }
            };
            return {
                async write(chunk) {
                    await handle.write(chunk);
                },
                async finish(): Promise<StagedFile> {
                    const { size } = await handle.stat();
                    await handle.close();
                    return {
                        bytes: size,
                        chunks: () => fileChunks(path),
                        discard: remove,
                    };
                },
                async abort() {
                    await handle.close().catch(() => undefined);
                    await remove();
                },
            };
        },
    };
    return { area, cleanup: () => rmSync(dir, { recursive: true, force: true }), dir };
}

async function* fileChunks(path: string): AsyncGenerator<Uint8Array> {
    const handle = await open(path, "r");
    try {
        for (;;) {
            // A fresh buffer per read: the consumer may hold a slice past the next read, and a
            // recycled buffer is the classic way a transfer arrives scrambled.
            const buffer = new Uint8Array(STAGE_READ_CHUNK);
            const { bytesRead } = await handle.read(buffer, 0, buffer.length);
            if (bytesRead === 0) return;
            yield buffer.subarray(0, bytesRead);
        }
    } finally {
        await handle.close();
    }
}

/**
 * A `ReadableStream` of `total` bytes, generated `chunk` at a time.
 *
 * The pattern is position-dependent so a mis-ordered or duplicated chunk changes the digest, and
 * the whole body is never materialised — otherwise a "the pipeline streams" test would be measuring
 * a 300 MB fixture sitting in the heap beside it.
 */
export function syntheticBody(total: number, chunk = 64 * 1024): ReadableStream<Uint8Array> {
    let sent = 0;
    return new ReadableStream<Uint8Array>({
        pull(controller) {
            if (sent >= total) {
                controller.close();
                return;
            }
            const size = Math.min(chunk, total - sent);
            const out = new Uint8Array(size);
            for (let i = 0; i < size; i++) out[i] = (sent + i) & 0xff;
            sent += size;
            controller.enqueue(out);
        },
    });
}

/** The same bytes {@link syntheticBody} would produce, as one array. Small inputs only.
 *  Typed over a plain `ArrayBuffer` so the result is a `BlobPart` — tests wrap it in a `File`. */
export function syntheticBytes(total: number): Uint8Array<ArrayBuffer> {
    const out = new Uint8Array(total);
    for (let i = 0; i < total; i++) out[i] = i & 0xff;
    return out;
}
