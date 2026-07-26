// The check OBCC §7 makes non-optional: an artifact is verified against the
// manifest's `bytes` and `sha256` *before* it can be written anywhere, and a
// mismatch is an error rather than a corrupt map on the rider's card. These
// tests run the real digest (node's WebCrypto is the same `crypto.subtle` the
// browser gives us), so the hex comparison is exercised end to end.

import { describe, expect, it, vi } from "vitest";
import { ArtifactVerificationError, artifactFilename, fetchArtifact } from "./download";
import type { CatalogArtifact } from "./manifest";

const BODY = new TextEncoder().encode("not really a map, but bytes are bytes");

async function sha256Of(bytes: Uint8Array): Promise<string> {
    const digest = await crypto.subtle.digest("SHA-256", bytes as unknown as BufferSource);
    return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

function artifact(over: Partial<CatalogArtifact> = {}): CatalogArtifact {
    return {
        region_id: "europe/switzerland",
        region_name: "Switzerland",
        preset_id: "default",
        preset_version: 3,
        obcm_version: 10,
        bytes: BODY.byteLength,
        sha256: "0".repeat(64),
        bbox: { min_lat: 0, min_lon: 0, max_lat: 1, max_lon: 1 },
        built_at: "2026-07-20T02:14:07Z",
        source_snapshot: "2026-07-19",
        url: "https://maps.example.org/regions/europe/switzerland/default.obcm",
        ...over,
    };
}

/** A fetch that streams `body` back in small chunks, like a real one. */
function servingFetch(body: Uint8Array, chunk = 8): typeof fetch {
    return vi.fn(async () => {
        const stream = new ReadableStream<Uint8Array>({
            start(controller) {
                for (let at = 0; at < body.byteLength; at += chunk) {
                    controller.enqueue(body.slice(at, at + chunk));
                }
                controller.close();
            },
        });
        return new Response(stream, { status: 200 });
    }) as unknown as typeof fetch;
}

describe("fetchArtifact", () => {
    it("returns the bytes when size and checksum match", async () => {
        const a = artifact({ sha256: await sha256Of(BODY) });
        const bytes = await fetchArtifact(a, { fetchImpl: servingFetch(BODY) });
        expect(bytes).toEqual(BODY);
    });

    it("reports progress against the manifest's size", async () => {
        const a = artifact({ sha256: await sha256Of(BODY) });
        const seen: number[] = [];
        await fetchArtifact(a, {
            fetchImpl: servingFetch(BODY),
            onProgress: (p) => {
                expect(p.total).toBe(a.bytes);
                seen.push(p.received);
            },
        });
        expect(seen.at(-1)).toBe(BODY.byteLength);
        expect(seen.length).toBeGreaterThan(1);
    });

    it("refuses a checksum mismatch", async () => {
        // The catalog says one thing, the bytes are another: the exact failure
        // that would otherwise reach the card as an unreadable map.
        const a = artifact({ sha256: "f".repeat(64) });
        await expect(fetchArtifact(a, { fetchImpl: servingFetch(BODY) })).rejects.toThrow(
            ArtifactVerificationError,
        );
        await expect(fetchArtifact(a, { fetchImpl: servingFetch(BODY) })).rejects.toThrow(
            /checksum mismatch/,
        );
    });

    it("refuses a body shorter than the manifest says", async () => {
        const a = artifact({ sha256: await sha256Of(BODY), bytes: BODY.byteLength + 16 });
        await expect(fetchArtifact(a, { fetchImpl: servingFetch(BODY) })).rejects.toThrow(
            /expected \d+ bytes, got \d+/,
        );
    });

    it("stops a body longer than the manifest says instead of buffering it", async () => {
        const a = artifact({ sha256: await sha256Of(BODY), bytes: 4 });
        await expect(fetchArtifact(a, { fetchImpl: servingFetch(BODY) })).rejects.toThrow(
            /longer than the manifest/,
        );
    });

    it("surfaces an HTTP failure as itself", async () => {
        const failing = vi.fn(async () => new Response("nope", { status: 404, statusText: "Not Found" }));
        await expect(
            fetchArtifact(artifact(), { fetchImpl: failing as unknown as typeof fetch }),
        ).rejects.toThrow(/404/);
    });
});

describe("artifactFilename", () => {
    it("names the file after the region and the style", () => {
        // The device loads the first `*.obcm` in the card root whatever it is
        // called, so this is for the rider's own filesystem.
        expect(artifactFilename(artifact())).toBe("switzerland-default.obcm");
        expect(
            artifactFilename(artifact({ region_id: "europe/germany/bayern", preset_id: "minimal" })),
        ).toBe("bayern-minimal.obcm");
    });
});
