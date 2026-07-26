/**
 * Staging: the step that stands between a CDN response and the device.
 *
 * `OBCC_Spec.md` §7 makes one demand of a consumer — verify the size and the SHA-256 **before**
 * writing to a device — and the only way to honour it for a hundreds-of-megabyte artifact is to put
 * the bytes somewhere local first. These tests are about the two things that can go wrong with
 * that: a mismatch reaching the device anyway, and a scratch file outliving its upload.
 */

import { describe, expect, it } from "vitest";
import { createHash } from "node:crypto";
import { readdirSync } from "node:fs";

import { Crc32 } from "../usb/crc32";
import { StagingError, stageStream } from "./staging";
import { syntheticBody, syntheticBytes, tempStaging } from "./testing";

const SIZE = 300_000;

function digestOf(bytes: Uint8Array): string {
    return createHash("sha256").update(bytes).digest("hex");
}

async function collect(source: { chunks: (n: number) => AsyncIterable<Uint8Array> }, chunk: number) {
    const parts: Uint8Array[] = [];
    let len = 0;
    for await (const part of source.chunks(chunk)) {
        parts.push(part);
        len += part.length;
    }
    const out = new Uint8Array(len);
    let at = 0;
    for (const part of parts) {
        out.set(part, at);
        at += part.length;
    }
    return out;
}

describe("stageStream", () => {
    it("fingerprints on the way in and reads the same bytes back out", async () => {
        const { area, cleanup, dir } = tempStaging();
        const expected = syntheticBytes(SIZE);
        try {
            const staged = await stageStream(syntheticBody(SIZE), {
                area,
                name: "map.obcm",
                expect: { bytes: SIZE, sha256: digestOf(expected) },
            });
            expect(staged.bytes).toBe(SIZE);
            expect(staged.crc32).toBe(Crc32.of(expected));
            expect(staged.sha256).toBe(digestOf(expected));
            // The CRC the descriptor announces has to be a CRC of what will actually be sent, so
            // the staged file is read back and compared, not trusted.
            expect(await collect(staged.source, 8192)).toEqual(expected);
            // Twice: a retry after a failed upload re-reads the same staged file.
            expect(await collect(staged.source, 4096)).toEqual(expected);
            await staged.discard();
            expect(readdirSync(dir)).toEqual([]);
        } finally {
            cleanup();
        }
    });

    it("honours the caller's chunk size when it reads back", async () => {
        const { area, cleanup } = tempStaging();
        try {
            const staged = await stageStream(syntheticBody(70_000), { area, name: "m" });
            const sizes: number[] = [];
            for await (const chunk of staged.source.chunks(1024)) sizes.push(chunk.length);
            expect(Math.max(...sizes)).toBeLessThanOrEqual(1024);
            await staged.discard();
        } finally {
            cleanup();
        }
    });

    it("rejects a digest mismatch and keeps nothing", async () => {
        const { area, cleanup, dir } = tempStaging();
        try {
            const wrong = digestOf(syntheticBytes(SIZE + 1));
            await expect(
                stageStream(syntheticBody(SIZE), {
                    area,
                    name: "map.obcm",
                    expect: { bytes: SIZE, sha256: wrong },
                }),
            ).rejects.toMatchObject({ code: "digest-mismatch" });
            expect(readdirSync(dir), "the partial must not survive a failed check").toEqual([]);
        } finally {
            cleanup();
        }
    });

    it("rejects a short body and keeps nothing", async () => {
        const { area, cleanup, dir } = tempStaging();
        try {
            await expect(
                stageStream(syntheticBody(SIZE - 100), {
                    area,
                    name: "map.obcm",
                    expect: { bytes: SIZE, sha256: digestOf(syntheticBytes(SIZE)) },
                }),
            ).rejects.toMatchObject({ code: "size-mismatch" });
            expect(readdirSync(dir)).toEqual([]);
        } finally {
            cleanup();
        }
    });

    it("stops an over-long body mid-flight rather than filling the disk", async () => {
        const { area, cleanup } = tempStaging();
        try {
            const error = await stageStream(syntheticBody(SIZE * 4), {
                area,
                name: "map.obcm",
                expect: { bytes: SIZE, sha256: digestOf(syntheticBytes(SIZE)) },
            }).catch((e: unknown) => e);
            expect(error).toBeInstanceOf(StagingError);
            expect((error as StagingError).code).toBe("size-mismatch");
        } finally {
            cleanup();
        }
    });

    it("cancels mid-download and leaves nothing behind", async () => {
        const { area, cleanup, dir } = tempStaging();
        const controller = new AbortController();
        try {
            const staging = stageStream(syntheticBody(SIZE * 20, 4096), {
                area,
                name: "map.obcm",
                signal: controller.signal,
                onProgress: (done) => {
                    if (done > SIZE) controller.abort();
                },
            });
            await expect(staging).rejects.toMatchObject({ code: "aborted" });
            expect(readdirSync(dir)).toEqual([]);
        } finally {
            cleanup();
        }
    });

    it("reports a stream that dies mid-body without keeping the partial", async () => {
        const { area, cleanup, dir } = tempStaging();
        let sent = 0;
        const body = new ReadableStream<Uint8Array>({
            pull(controller) {
                if (sent >= 40_000) {
                    controller.error(new Error("connection reset"));
                    return;
                }
                sent += 4096;
                controller.enqueue(new Uint8Array(4096));
            },
        });
        try {
            const error = await stageStream(body, {
                area,
                name: "map.obcm",
                expect: { bytes: SIZE, sha256: digestOf(syntheticBytes(SIZE)) },
            }).catch((e: unknown) => e);
            expect(error).toBeInstanceOf(StagingError);
            expect((error as StagingError).code).toBe("network");
            expect(readdirSync(dir)).toEqual([]);
        } finally {
            cleanup();
        }
    });
});
