// Download one digest-pinned catalog object and refuse bytes that do not match
// the published size and SHA-256. Cells and satellite documents use the same
// verification path so the browser never assembles untrusted input.

export interface BytePin {
    bytes: number;
    sha256: string;
}

export class BytesVerificationError extends Error {
    constructor(
        readonly url: string,
        readonly detail: string,
    ) {
        super(`${url}: ${detail}`);
        this.name = "BytesVerificationError";
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

export async function fetchVerified(
    url: string,
    pin: BytePin,
    opts: DownloadOptions = {},
): Promise<Uint8Array> {
    const response = await (opts.fetchImpl ?? globalThis.fetch)(url, { signal: opts.signal });
    if (!response.ok) throw new Error(`${url}: ${response.status} ${response.statusText}`);

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
        throw new BytesVerificationError(url, `expected ${total} bytes, got ${bytes.byteLength}`);
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
    const url = URL.createObjectURL(new Blob([bytes as unknown as BlobPart], { type }));
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = filename;
    anchor.click();
    setTimeout(() => URL.revokeObjectURL(url), 0);
}
