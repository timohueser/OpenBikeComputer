// Download one digest-pinned catalog object and refuse bytes that do not match
// the published size and SHA-256. Cells and satellite documents use the same
// verification path so the browser never assembles untrusted input.

export interface BytePin {
    bytes: number;
    sha256: string;
}

/**
 * Why the bytes were refused. The distinction is not cosmetic: `short` is what a
 * connection dropped mid-body looks like once the reader has ended cleanly, and
 * that is worth another attempt. `long` and `checksum` mean the origin served
 * *something else* — retrying only serves it again.
 */
export type VerificationFault = "short" | "long" | "checksum";

export class BytesVerificationError extends Error {
    constructor(
        readonly url: string,
        readonly detail: string,
        readonly fault: VerificationFault = "checksum",
    ) {
        super(`${url}: ${detail}`);
        this.name = "BytesVerificationError";
    }
}

/** A response that came back, but not with a body worth reading. */
export class HttpStatusError extends Error {
    constructor(
        readonly url: string,
        readonly status: number,
        statusText: string,
    ) {
        super(`${url}: ${status} ${statusText}`);
        this.name = "HttpStatusError";
    }
}

export interface DownloadProgress {
    received: number;
    total: number;
}

export interface DownloadOptions {
    onProgress?: (progress: DownloadProgress) => void;
    signal?: AbortSignal;
    fetchImpl?: typeof fetch;
    digest?: (bytes: Uint8Array) => Promise<ArrayBuffer>;
    /** How many times one object is fetched before the failure is the answer.
     *  1 disables retrying. */
    attempts?: number;
    /** Injected by tests, so a retry costs no wall clock. */
    sleep?: (ms: number) => Promise<void>;
}

/**
 * Four attempts, ~1.75 s of backoff in total. The failure this exists for is a
 * connection the CDN drops part-way through a multi-megabyte cell — intermittent
 * and uncorrelated, so a second attempt almost always lands. A cell set is
 * hundreds of objects, and without this a single drop anywhere in the run throws
 * the whole download away.
 */
const DEFAULT_ATTEMPTS = 4;
const RETRY_BASE_MS = 250;

/**
 * Whether another attempt could plausibly answer differently.
 *
 * Deliberately narrow on the response side: a 404 or a 403 is the catalog and
 * the store disagreeing, and hammering it neither fixes that nor tells the user
 * anything new. The open-ended `true` at the end is for what `fetch` throws —
 * a dropped connection, a DNS blip, a reset — which is the whole point.
 */
function worthRetrying(cause: unknown): boolean {
    if (cause instanceof BytesVerificationError) return cause.fault === "short";
    if (cause instanceof HttpStatusError) {
        return cause.status >= 500 || cause.status === 408 || cause.status === 429;
    }
    // An abort is the caller's decision, not a transport failure.
    if (cause instanceof DOMException && cause.name === "AbortError") return false;
    if (cause instanceof Error && cause.name === "AbortError") return false;
    return true;
}

/**
 * Run `attempt` until it succeeds, refuses in a way another try cannot mend, or
 * runs out of attempts.
 *
 * Exported because the catalog root is fetched before there is anything to pin
 * it against, and a dropped connection there is just as fatal to the run as one
 * in the middle of a cell.
 */
export async function withRetry<T>(attempt: () => Promise<T>, opts: DownloadOptions = {}): Promise<T> {
    const attempts = Math.max(1, opts.attempts ?? DEFAULT_ATTEMPTS);
    const sleep = opts.sleep ?? ((ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms)));
    for (let n = 1; ; n++) {
        try {
            return await attempt();
        } catch (cause) {
            // The signal is checked as well as the error: an aborted run must not
            // spend its backoff sleeping before it agrees to stop.
            if (n >= attempts || opts.signal?.aborted || !worthRetrying(cause)) throw cause;
            await sleep(RETRY_BASE_MS * 2 ** (n - 1));
        }
    }
}

async function subtleDigest(bytes: Uint8Array): Promise<ArrayBuffer> {
    if (!globalThis.crypto?.subtle) {
        throw new Error(
            "this browser exposes no WebCrypto (a secure context is required), " +
                "so the download cannot be verified",
        );
    }
    return globalThis.crypto.subtle.digest("SHA-256", bytes as unknown as BufferSource);
}

function toHex(digest: ArrayBuffer): string {
    return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

/**
 * One digest-pinned object, fetched again if the first answer was a torn body
 * rather than a different one.
 *
 * Retrying is safe *because* every object is pinned: an attempt that returns the
 * wrong bytes cannot be mistaken for the right ones, so the only thing a second
 * attempt can do is succeed or fail again. Progress is reported per attempt and
 * therefore restarts from zero on a retry — callers track the in-flight figure
 * as an absolute, not a running sum.
 */
export async function fetchVerified(
    url: string,
    pin: BytePin,
    opts: DownloadOptions = {},
): Promise<Uint8Array> {
    return withRetry(() => fetchOnce(url, pin, opts), opts);
}

async function fetchOnce(url: string, pin: BytePin, opts: DownloadOptions): Promise<Uint8Array> {
    const response = await (opts.fetchImpl ?? globalThis.fetch)(url, { signal: opts.signal });
    if (!response.ok) throw new HttpStatusError(url, response.status, response.statusText);

    const total = pin.bytes;
    let bytes: Uint8Array;
    if (response.body) {
        const reader = response.body.getReader();
        const chunks: Uint8Array[] = [];
        let received = 0;
        for (;;) {
            const { done, value } = await reader.read();
            if (done) break;
            chunks.push(value);
            received += value.byteLength;
            if (received > total) {
                void reader.cancel();
                throw new BytesVerificationError(
                    url,
                    `the download is longer than the catalog's ${total} bytes`,
                    "long",
                );
            }
            opts.onProgress?.({ received, total });
        }
        bytes = new Uint8Array(received);
        let offset = 0;
        for (const chunk of chunks) {
            bytes.set(chunk, offset);
            offset += chunk.byteLength;
        }
    } else {
        bytes = new Uint8Array(await response.arrayBuffer());
        opts.onProgress?.({ received: bytes.byteLength, total });
    }

    if (bytes.byteLength !== total) {
        throw new BytesVerificationError(url, `expected ${total} bytes, got ${bytes.byteLength}`, "short");
    }
    const actual = toHex(await (opts.digest ?? subtleDigest)(bytes));
    if (actual !== pin.sha256) {
        throw new BytesVerificationError(
            url,
            `checksum mismatch — the catalog says ${pin.sha256.slice(0, 12)}…, ` +
                `the download is ${actual.slice(0, 12)}…`,
        );
    }
    return bytes;
}

export function saveBytes(bytes: Uint8Array, filename: string, type = "application/octet-stream"): void {
    saveBlob(new Blob([bytes as unknown as BlobPart], { type }), filename);
}

/**
 * Save a Blob the caller already holds. The assembly stages its files as Blobs
 * while the run is still going (#1116 B1) and saves them once the set is
 * complete, so it has nothing left to wrap by then — and a Blob is what a
 * browser can spill to disk rather than keep in the tab's heap.
 */
export function saveBlob(blob: Blob, filename: string): void {
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = filename;
    anchor.click();
    // Deliberately never revoked. Firefox resolves the URL when the user
    // *accepts* the save dialog, not when `click()` runs — a revoke on a timer
    // is a race that kills the download and strands a `.part` file. The cost of
    // keeping it is one registry entry per save (the big blobs are OPFS-backed
    // Files, so no heap is pinned), and the registry dies with the document.
}
