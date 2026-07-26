/**
 * The claim #903 refuses to take on faith: **memory during a 300 MB map upload stays flat**.
 *
 * Streaming is the easy thing to fake. `await response.arrayBuffer()`, `await response.blob()`, an
 * array of chunks joined "just before sending" — all three read like streaming code and all three
 * hold the artifact. So this test drives the *real* pipeline (fetch response → staging area →
 * fingerprint → protocol client → device) over 300 MB and watches the process's own heap the whole
 * way. A buffered implementation cannot pass it: it would have to hold 300 MB somewhere.
 *
 * Two substitutions, both deliberate and neither of them the thing under test:
 *
 * - The staging area is `node:fs` rather than OPFS, because Node has no OPFS. It is the same
 *   `stageStream` pipeline either way — what changes is which `WritableStream` the bytes land in.
 * - The device sinks its uploads (`sinkUploads`), which is what a microcontroller writing to an SD
 *   card actually does. Without it the *simulated device* would be the thing holding 300 MB, and
 *   the measurement would be of the fixture rather than of the code.
 */

import { describe, expect, it, vi } from "vitest";
import { createHash } from "node:crypto";

import { loopbackDevice } from "../usb/loopback";
import { ObjectType } from "../usb/protocol";
import { sendCatalogMap } from "./write";
import type { JobPhase } from "./progress";
import { syntheticBody, tempStaging } from "./testing";

/** The size `docs/content/software/architecture.md` names as the big case, and #903's acceptance. */
const TOTAL = 300 * 1024 * 1024;

/** What the CDN hands over per chunk. A real fetch delivers 16–64 KB slices. */
const RESPONSE_CHUNK = 64 * 1024;

/**
 * The ceiling the assertion uses.
 *
 * Peak live heap should be a handful of chunk buffers plus V8's own young generation, so tens of
 * megabytes. 64 MB leaves room for GC timing on a busy CI box while staying **far** below the
 * 300 MB a single buffered copy would cost — the failure this exists to catch is off by a factor
 * of five, not by a few percent.
 */
const HEAP_CEILING = 64 * 1024 * 1024;

/** The digest of `syntheticBody(TOTAL)`, computed without ever holding it. */
function expectedDigest(): string {
    const hash = createHash("sha256");
    let sent = 0;
    const chunk = new Uint8Array(RESPONSE_CHUNK);
    while (sent < TOTAL) {
        const size = Math.min(RESPONSE_CHUNK, TOTAL - sent);
        for (let i = 0; i < size; i++) chunk[i] = (sent + i) & 0xff;
        hash.update(chunk.subarray(0, size));
        sent += size;
    }
    return hash.digest("hex");
}

describe("a 300 MB map upload", () => {
    it("never materialises the artifact — peak heap stays a few chunks", async () => {
        const sha256 = expectedDigest();
        const { area, cleanup } = tempStaging();
        // 64 KB packets and a 256 KB high-water mark: a high-speed bulk endpoint's shape, and a
        // bound on how much the writer may have outstanding — which is itself part of the claim.
        const { client, device, close } = loopbackDevice({
            sinkUploads: true,
            bulkPacketSize: 64 * 1024,
            bulkHighWaterMark: 256 * 1024,
        });
        vi.stubGlobal("fetch", async () => new Response(syntheticBody(TOTAL, RESPONSE_CHUNK), { status: 200 }));

        const base = process.memoryUsage();
        let peakHeap = base.heapUsed;
        let peakRss = base.rss;
        const trace: string[] = [];
        let nextMark = 0.25;
        // Both halves are traced, because they stress different things: the download fills the
        // scratch file, the send reads it back out through the upload source.
        let phase: JobPhase = "idle";

        try {
            const result = await sendCatalogMap(
                client,
                { filename: "france.obcm", url: "https://cdn.example/france.obcm", bytes: TOTAL, sha256 },
                area,
                {
                    signal: new AbortController().signal,
                    phase: (next: JobPhase) => {
                        phase = next;
                        nextMark = 0.25;
                    },
                    progress: (done, total) => {
                        const now = process.memoryUsage();
                        peakHeap = Math.max(peakHeap, now.heapUsed);
                        peakRss = Math.max(peakRss, now.rss);
                        const fraction = total ? done / total : 0;
                        if (fraction >= nextMark) {
                            trace.push(`${phase} ${Math.round(fraction * 100)}% — ${mb(now.heapUsed)} heap`);
                            nextMark += 0.25;
                        }
                    },
                },
            );

            expect(result.committedOffset).toBe(TOTAL);
            expect(device.storedLength(ObjectType.Map, result.objectId)).toBe(TOTAL);

            const heapDelta = peakHeap - base.heapUsed;
            const rssDelta = peakRss - base.rss;
            // Printed, not just asserted: "flat" is a number someone should be able to read in the
            // CI log without re-deriving it from a passing test.
            console.log(
                `300 MB upload: peak heap +${mb(heapDelta)} (ceiling ${mb(HEAP_CEILING)}), ` +
                    `peak RSS +${mb(rssDelta)}\n  ${trace.join("\n  ")}`,
            );
            expect(heapDelta, `peak heap grew by ${mb(heapDelta)} over a ${mb(TOTAL)} artifact`).toBeLessThan(
                HEAP_CEILING,
            );
        } finally {
            vi.unstubAllGlobals();
            cleanup();
            await close();
        }
    }, 300_000);
});

function mb(bytes: number): string {
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
