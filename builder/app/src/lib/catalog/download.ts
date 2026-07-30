// Fetching a baked map, and refusing to hand over one that doesn't match the
// manifest.
//
// OBCC §7 puts the obligation plainly: a consumer verifies a downloaded
// artifact against `bytes` and `sha256` *before* writing it to a device, and
// surfaces a mismatch as an error rather than a corrupt file on the rider's
// card. A corrupt `.obcm` is not a visible failure later — it is a device that
// boots to a fault screen halfway up a mountain — so the check is not optional
// and not deferred.
//
// That is why the whole artifact is buffered in memory before anything is
// saved: a digest can only be checked over the complete bytes, and the one
// browser API that could stream to disk and truncate afterwards (File System
// Access) is Chromium-only — the same reach problem that makes the desktop app
// the universal path (#894). Buffering costs RAM proportional to the map, which
// is the honest price of never writing an unverified file.

import type { CatalogArtifact } from "./manifest";

/** What a document or an artifact has to match before it is believed: the size
 *  and the digest the catalog published for it. */
export interface BytePin {
    bytes: number;
    sha256: string;
}

/** Fetched bytes disagreed with the pin the catalog published. Nothing has been
 *  saved, and the caller holds nothing it could be tempted to use. */
export class BytesVerificationError extends Error {
    constructor(
        readonly url: string,
        readonly detail: string,
    ) {
        super(`${url}: ${detail}`);
        this.name = "BytesVerificationError";
    }
}

/** The artifact's bytes disagreed with the manifest. Nothing has been saved. */
export class ArtifactVerificationError extends Error {
    constructor(
        readonly artifact: CatalogArtifact,
        readonly detail: string,
    ) {
        super(`${artifact.region_id} / ${artifact.preset_id}: ${detail}`);
        this.name = "ArtifactVerificationError";
    }
}

export interface DownloadProgress {
    /** Bytes received so far. */
    received: number;
    /** The manifest's size — known before the first byte arrives (§2). */
    total: number;
}

export interface DownloadOptions {
    onProgress?: (p: DownloadProgress) => void;
    signal?: AbortSignal;
    /** Injected by the tests; defaults to the global. */
    fetchImpl?: typeof fetch;
    /** Injected by the tests; defaults to `crypto.subtle`. */
    digest?: (bytes: Uint8Array) => Promise<ArrayBuffer>;
}

async function subtleDigest(bytes: Uint8Array): Promise<ArrayBuffer> {
    if (!globalThis.crypto?.subtle) {
        // WebCrypto is secure-context only. Rather than skip the check, refuse:
        // an unverifiable download is exactly what §7 forbids handing on.
        throw new Error(
            "this browser exposes no WebCrypto (a secure context is required), " +
                "so the download cannot be verified",
        );
    }
    return globalThis.crypto.subtle.digest("SHA-256", bytes as unknown as BufferSource);
}

function toHex(digest: ArrayBuffer): string {
    return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/**
 * Fetch one pinned object and return its bytes, or throw.
 *
 * This is the whole verification discipline in one function — buffer the body,
 * check the length as it arrives, check the digest over the complete bytes — and
 * it is deliberately generic over *what* was pinned. A v1 artifact and a v2 cell
 * or satellite document (`OBCC_Spec.md` §11.1) make the same promise and deserve
 * the same enforcement; two copies of this would be two chances to weaken one of
 * them.
 */
export async function fetchVerified(
    url: string,
    pin: BytePin,
    opts: DownloadOptions = {},
): Promise<Uint8Array> {
    const doFetch = opts.fetchImpl ?? globalThis.fetch;
    const res = await doFetch(url, { signal: opts.signal });
    if (!res.ok) throw new Error(`${url}: ${res.status} ${res.statusText}`);

    const total = pin.bytes;
    let bytes: Uint8Array;
    if (res.body) {
        const reader = res.body.getReader();
        const chunks: Uint8Array[] = [];
        let received = 0;
        for (;;) {
            const { done, value } = await reader.read();
            if (done) break;
            chunks.push(value);
            received += value.byteLength;
            // A body longer than the manifest says is already a mismatch; stop
            // rather than buffering an unbounded stream on the way to failing.
            if (received > total) {
                reader.cancel().catch(() => {});
                throw new BytesVerificationError(
                    url,
                    `the download is longer than the manifest's ${total} bytes`,
                );
            }
            opts.onProgress?.({ received, total });
        }
        bytes = new Uint8Array(received);
        let at = 0;
        for (const chunk of chunks) {
            bytes.set(chunk, at);
            at += chunk.byteLength;
        }
    } else {
        bytes = new Uint8Array(await res.arrayBuffer());
        opts.onProgress?.({ received: bytes.byteLength, total });
    }

    if (bytes.byteLength !== total) {
        throw new BytesVerificationError(url, `expected ${total} bytes, got ${bytes.byteLength}`);
    }
    const actual = toHex(await (opts.digest ?? subtleDigest)(bytes));
    if (actual !== pin.sha256) {
        // Enough digest to tell two files apart in a bug report, not so much
        // that the sentence stops being readable.
        throw new BytesVerificationError(
            url,
            `checksum mismatch — the catalog says ${pin.sha256.slice(0, 12)}…, ` +
                `the download is ${actual.slice(0, 12)}…`,
        );
    }
    return bytes;
}

/**
 * Fetch one artifact and return its bytes, or throw. The returned buffer has
 * been checked against the manifest's `bytes` and `sha256`; nothing else in
 * this module hands out unverified bytes.
 *
 * The verification failure is re-thrown as an `ArtifactVerificationError` so it
 * still carries the artifact a caller was asking for — a URL is not something a
 * rider can be shown.
 */
export async function fetchArtifact(
    artifact: CatalogArtifact,
    opts: DownloadOptions = {},
): Promise<Uint8Array> {
    try {
        return await fetchVerified(artifact.url, artifact, opts);
    } catch (e) {
        if (e instanceof BytesVerificationError) throw new ArtifactVerificationError(artifact, e.detail);
        throw e;
    }
}

/**
 * What the file is called once it lands. The device loads the first `*.obcm` it
 * finds in the card root (any name), so this is for the rider's own filesystem:
 * the region's last path segment and the preset, which is what tells two
 * downloads apart in a Downloads folder.
 */
export function artifactFilename(artifact: CatalogArtifact): string {
    const leaf = artifact.region_id.split("/").pop() ?? artifact.region_id;
    return `${leaf}-${artifact.preset_id}.obcm`;
}

/**
 * Hand verified bytes to the browser's downloader.
 *
 * `type` defaults to the opaque one a map is; a ride export passes `application/gpx+xml` so the OS
 * files it as what it is (C5 #904). One implementation rather than two, because the revoke timing
 * below is the kind of detail a second copy gets subtly wrong.
 */
export function saveBytes(bytes: Uint8Array, filename: string, type = "application/octet-stream"): void {
    const url = URL.createObjectURL(new Blob([bytes as unknown as BlobPart], { type }));
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    a.click();
    // Revoked on a turn of the event loop: Safari needs the element to have
    // been clicked with a live URL before it is released.
    setTimeout(() => URL.revokeObjectURL(url), 0);
}
