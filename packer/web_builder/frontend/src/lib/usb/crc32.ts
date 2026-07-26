/**
 * CRC-32/IEEE — the whole-object, end-to-end integrity check of the interface spec (§6).
 *
 * Reflected, polynomial `0xEDB88320` (the reflected form of `0x04C11DB7`), init and final XOR
 * `0xFFFFFFFF`; check value `crc32("123456789") === 0xCBF43926`, pinned in `crc32.test.ts` and in
 * `protocol-vectors/manifest.json`. This is a straight port of `firmware/obc-ble/src/crc32.rs`,
 * which is itself byte-identical to the app's Swift `CRC32.Hasher`.
 *
 * **Incremental on purpose.** A map is tens of megabytes: the browser streams it to the device in
 * bulk-endpoint-sized slices and folds each slice in as it goes, so nothing needs the whole object
 * in one contiguous buffer and the CRC costs one pass, not two. The same hasher runs the download
 * direction, verifying as bytes arrive rather than after.
 */

/** The reflected CRC-32/IEEE table, built once at module load (256 × 4 bytes). */
const TABLE: Uint32Array = (() => {
    const table = new Uint32Array(256);
    for (let i = 0; i < 256; i++) {
        let c = i;
        for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
        table[i] = c >>> 0;
    }
    return table;
})();

/** An incremental CRC-32/IEEE hasher. Fold bytes in with {@link update}, read {@link value}. */
export class Crc32 {
    /** The running register, pre-final-XOR. Starts at the spec's `0xFFFFFFFF` init. */
    private state = 0xffffffff;

    /** Fold `bytes` into the running CRC. Any segmentation gives the same result. */
    update(bytes: Uint8Array): void {
        let c = this.state;
        for (let i = 0; i < bytes.length; i++) {
            c = TABLE[(c ^ bytes[i]) & 0xff] ^ (c >>> 8);
        }
        this.state = c >>> 0;
    }

    /**
     * The CRC-32 of everything fed so far, as an unsigned 32-bit number. Reading it does not
     * consume the hasher — a mid-stream progress read is free and hashing continues.
     */
    value(): number {
        return (this.state ^ 0xffffffff) >>> 0;
    }

    /** Back to the init state, so one hasher can serve a retried transfer. */
    reset(): void {
        this.state = 0xffffffff;
    }

    /** One-shot: the CRC-32 of a whole buffer. */
    static of(bytes: Uint8Array): number {
        const h = new Crc32();
        h.update(bytes);
        return h.value();
    }
}
