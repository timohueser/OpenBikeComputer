// One ZIP archive, STORE only, from Blobs the caller already holds. This is the
// browser host's single-download fallback (#1116 B1's staging, delivered): a
// browser without a directory picker gets the whole assembled set as ONE save
// prompt instead of one per file — ten simultaneous downloads is how Firefox
// ends up with a stack of dialogs and orphaned `.part` files.
//
// STORE, never DEFLATE: the entries are OBCM shards, already dense binary that
// deflate would spend seconds not shrinking — and a stored entry lets the
// archive be *composed* rather than built. The returned Blob interleaves the
// header bytes with the caller's Blobs by reference, so an OPFS-backed shard
// never enters the tab's heap here; the one full read this module performs is
// the CRC-32 pass the format requires (§4.4.7 of APPNOTE), streamed in chunks.
//
// Zip64 is emitted per field, exactly where a value overflows its 32-bit slot
// (a country-scale set is bigger than 4 GiB even though each OBCM file is
// bounded by its uint32 offsets), so small archives stay byte-identical to
// what a plain zipper would write and huge ones stay legal.

/** One file to be stored in the archive. */
export interface ZipEntry {
    name: string;
    blob: Blob;
}

/** What `zipLayout` needs to know about a file: its identity, not its bytes. */
export interface ZipFileMeta {
    name: string;
    size: number;
    crc: number;
}

/** The computed skeleton of an archive: every header, and where things land. */
export interface ZipLayout {
    /** Local file header for each entry; entry `i`'s bytes follow `locals[i]`. */
    locals: Uint8Array[];
    /** Central directory + end-of-central-directory records, one tail block. */
    tail: Uint8Array;
    /** Byte offset of each local header in the finished archive. */
    offsets: number[];
    /** Total archive size in bytes. */
    totalBytes: number;
}

const CRC_TABLE = (() => {
    const table = new Uint32Array(256);
    for (let n = 0; n < 256; n++) {
        let c = n;
        for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
        table[n] = c;
    }
    return table;
})();

/** Fold one chunk into a running CRC-32 (IEEE, the zip polynomial). Start from 0. */
export function crc32(chunk: Uint8Array, crc = 0): number {
    let c = ~crc;
    for (let i = 0; i < chunk.length; i++) c = CRC_TABLE[(c ^ chunk[i]) & 0xff] ^ (c >>> 8);
    return ~c >>> 0;
}

/** CRC-32 of a whole Blob, streamed — the only full read this module does. */
export async function crc32OfBlob(blob: Blob, onBytes?: (n: number) => void): Promise<number> {
    const reader = blob.stream().getReader();
    let crc = 0;
    for (;;) {
        const { done, value } = await reader.read();
        if (done) return crc;
        crc = crc32(value, crc);
        onBytes?.(value.byteLength);
    }
}

const U32_MAX = 0xffffffff;
/** Fixed DOS timestamp (1980-01-01), so the same set zips to the same bytes. */
const DOS_TIME = 0;
const DOS_DATE = 0x21;

class ByteWriter {
    private buf: number[] = [];
    u16(v: number) {
        this.buf.push(v & 0xff, (v >>> 8) & 0xff);
    }
    u32(v: number) {
        this.buf.push(v & 0xff, (v >>> 8) & 0xff, (v >>> 16) & 0xff, (v >>> 24) & 0xff);
    }
    /** A 53-bit-safe u64: JS numbers hold every size a Blob can have. */
    u64(v: number) {
        this.u32(v % 0x100000000);
        this.u32(Math.floor(v / 0x100000000));
    }
    bytes(b: Uint8Array) {
        for (const x of b) this.buf.push(x);
    }
    take(): Uint8Array {
        return new Uint8Array(this.buf);
    }
}

/**
 * Compute every header of a stored archive from the entries' names, sizes and
 * CRCs. Pure arithmetic — no Blob is read — which is what makes the zip64
 * paths testable without materializing a 4 GiB fixture.
 */
export function zipLayout(files: ZipFileMeta[]): ZipLayout {
    const encoder = new TextEncoder();
    const names = files.map((f) => encoder.encode(f.name));
    const locals: Uint8Array[] = [];
    const offsets: number[] = [];
    let at = 0;

    for (let i = 0; i < files.length; i++) {
        const f = files[i];
        const big = f.size > U32_MAX;
        const w = new ByteWriter();
        w.u32(0x04034b50);
        w.u16(big ? 45 : 20); // version needed to extract
        w.u16(0x0800); // general purpose: UTF-8 names
        w.u16(0); // method: STORE
        w.u16(DOS_TIME);
        w.u16(DOS_DATE);
        w.u32(f.crc);
        w.u32(big ? U32_MAX : f.size); // compressed == uncompressed under STORE
        w.u32(big ? U32_MAX : f.size);
        w.u16(names[i].length);
        w.u16(big ? 20 : 0); // extra length
        w.bytes(names[i]);
        if (big) {
            w.u16(0x0001); // zip64 extra
            w.u16(16);
            w.u64(f.size); // uncompressed, then compressed — the mandated order
            w.u64(f.size);
        }
        const header = w.take();
        locals.push(header);
        offsets.push(at);
        at += header.length + f.size;
    }

    const cdStart = at;
    const tail = new ByteWriter();
    for (let i = 0; i < files.length; i++) {
        const f = files[i];
        const bigSize = f.size > U32_MAX;
        const bigOffset = offsets[i] > U32_MAX;
        // The zip64 extra carries only the overflowed fields, in the spec's
        // fixed order: uncompressed size, compressed size, local offset.
        const extraLen = (bigSize ? 16 : 0) + (bigOffset ? 8 : 0);
        tail.u32(0x02014b50);
        tail.u16(45); // version made by
        tail.u16(bigSize || bigOffset ? 45 : 20);
        tail.u16(0x0800);
        tail.u16(0);
        tail.u16(DOS_TIME);
        tail.u16(DOS_DATE);
        tail.u32(f.crc);
        tail.u32(bigSize ? U32_MAX : f.size);
        tail.u32(bigSize ? U32_MAX : f.size);
        tail.u16(names[i].length);
        tail.u16(extraLen ? extraLen + 4 : 0);
        tail.u16(0); // comment
        tail.u16(0); // disk start
        tail.u16(0); // internal attrs
        tail.u32(0); // external attrs
        tail.u32(bigOffset ? U32_MAX : offsets[i]);
        tail.bytes(names[i]);
        if (extraLen) {
            tail.u16(0x0001);
            tail.u16(extraLen);
            if (bigSize) {
                tail.u64(f.size);
                tail.u64(f.size);
            }
            if (bigOffset) tail.u64(offsets[i]);
        }
    }
    const cdSize = tail.take().length;
    const cdEnd = cdStart + cdSize;

    // Zip64 end-of-central-directory, only when some 32-bit slot overflowed.
    const needs64 = cdEnd > U32_MAX || files.some((f, i) => f.size > U32_MAX || offsets[i] > U32_MAX);
    if (needs64) {
        tail.u32(0x06064b50);
        tail.u64(44); // size of the record past this field
        tail.u16(45);
        tail.u16(45);
        tail.u32(0);
        tail.u32(0);
        tail.u64(files.length);
        tail.u64(files.length);
        tail.u64(cdSize);
        tail.u64(cdStart);
        tail.u32(0x07064b50); // zip64 EOCD locator
        tail.u32(0);
        tail.u64(cdEnd);
        tail.u32(1);
    }
    tail.u32(0x06054b50);
    tail.u16(0);
    tail.u16(0);
    tail.u16(files.length); // a set is ≤ 32 shards, never near 0xFFFF
    tail.u16(files.length);
    tail.u32(cdSize > U32_MAX ? U32_MAX : cdSize);
    tail.u32(cdStart > U32_MAX ? U32_MAX : cdStart);
    tail.u16(0);

    const tailBytes = tail.take();
    return { locals, tail: tailBytes, offsets, totalBytes: cdStart + tailBytes.length };
}

/**
 * Store `entries` into one archive Blob.
 *
 * The CRC pass reads every entry once (that is what `onProgress` measures); the
 * result is a Blob *composed of* the given Blobs, not a copy of their bytes.
 */
export async function storeZip(
    entries: ZipEntry[],
    onProgress?: (readBytes: number, totalBytes: number) => void,
): Promise<Blob> {
    const total = entries.reduce((n, e) => n + e.blob.size, 0);
    let read = 0;
    const metas: ZipFileMeta[] = [];
    for (const e of entries) {
        const crc = await crc32OfBlob(e.blob, (n) => {
            read += n;
            onProgress?.(read, total);
        });
        metas.push({ name: e.name, size: e.blob.size, crc });
    }
    const layout = zipLayout(metas);
    const parts: BlobPart[] = [];
    for (let i = 0; i < entries.length; i++) {
        parts.push(layout.locals[i] as unknown as BlobPart, entries[i].blob);
    }
    parts.push(layout.tail as unknown as BlobPart);
    return new Blob(parts, { type: "application/zip" });
}
