/** Writes and reads ZIP files whose entries are stored with no compression, so the file is made of the entry blobs. */

export interface Entry {
    name: string;
    data: Blob;
}

const crcTable = Uint32Array.from({ length: 256 }, (_, i) => {
    let c = i;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
});

async function crc32(blob: Blob): Promise<number> {
    const bytes = new Uint8Array(await blob.arrayBuffer());
    let c = 0xffffffff;
    for (let i = 0; i < bytes.length; i++) c = crcTable[(c ^ bytes[i]) & 0xff] ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
}

const utf8 = 0x0800, stored = 0, needed = 10;
// 1980-01-01 00:00, the first date that a ZIP file can hold.
const dosTime = 0, dosDate = (1 << 5) | 1;
const tooLarge = () => new Error('The library is too large for one backup file.');

/** The fields from "version needed" to "extra length", which the local and the central header share. */
function shared(view: DataView, at: number, crc: number, size: number, nameLength: number): void {
    [needed, utf8, stored, dosTime, dosDate].forEach((value, i) => view.setUint16(at + i * 2, value, true));
    view.setUint32(at + 10, crc, true);
    view.setUint32(at + 14, size, true);
    view.setUint32(at + 18, size, true);
    view.setUint16(at + 22, nameLength, true);
    view.setUint16(at + 24, 0, true);
}

/** Reads each entry one time for its CRC-32. The result holds the entry blobs, not copies of their bytes. */
export async function zip(entries: Entry[]): Promise<Blob> {
    if (entries.length > 0xffff) throw tooLarge();
    const parts: BlobPart[] = [], central: Uint8Array<ArrayBuffer>[] = [];
    let offset = 0;
    for (const { name, data } of entries) {
        const path = new TextEncoder().encode(name), crc = await crc32(data);
        if (offset + 30 + path.length + data.size > 0xffffffff) throw tooLarge();
        const local = new Uint8Array(30 + path.length), view = new DataView(local.buffer);
        view.setUint32(0, 0x04034b50, true);
        shared(view, 4, crc, data.size, path.length);
        local.set(path, 30);
        const header = new Uint8Array(46 + path.length), entry = new DataView(header.buffer);
        entry.setUint32(0, 0x02014b50, true);
        entry.setUint16(4, 20, true);
        shared(entry, 6, crc, data.size, path.length);
        entry.setUint32(42, offset, true);
        header.set(path, 46);
        parts.push(local, data);
        central.push(header);
        offset += local.length + data.size;
    }
    const size = central.reduce((sum, header) => sum + header.length, 0);
    if (offset + size > 0xffffffff) throw tooLarge();
    const end = new DataView(new ArrayBuffer(22));
    end.setUint32(0, 0x06054b50, true);
    end.setUint16(8, entries.length, true);
    end.setUint16(10, entries.length, true);
    end.setUint32(12, size, true);
    end.setUint32(16, offset, true);
    return new Blob([...parts, ...central, end], { type: 'application/zip' });
}

const damaged = () => new Error('The file is damaged or is not a ZIP file of stored entries.');

/** Each entry of a ZIP file of stored entries, as a slice of `file`, after a check of its CRC-32. */
export async function unzip(file: Blob): Promise<Map<string, Blob>> {
    const tailStart = Math.max(0, file.size - 22 - 0xffff);
    const tail = new DataView(await file.slice(tailStart).arrayBuffer());
    let end = tail.byteLength - 22;
    while (end >= 0 && tail.getUint32(end, true) !== 0x06054b50) end--;
    if (end < 0) throw damaged();
    const count = tail.getUint16(end + 10, true), size = tail.getUint32(end + 12, true), start = tail.getUint32(end + 16, true);
    if (start + size > tailStart + end) throw damaged();
    const directory = new DataView(await file.slice(start, start + size).arrayBuffer()), decoder = new TextDecoder('utf-8', { fatal: true });
    const entries = new Map<string, Blob>();
    let at = 0;
    for (let i = 0; i < count; i++) {
        if (at + 46 > size || directory.getUint32(at, true) !== 0x02014b50) throw damaged();
        const method = directory.getUint16(at + 10, true), crc = directory.getUint32(at + 16, true);
        const length = directory.getUint32(at + 20, true), unpacked = directory.getUint32(at + 24, true);
        const nameLength = directory.getUint16(at + 28, true), next = at + 46 + nameLength + directory.getUint16(at + 30, true) + directory.getUint16(at + 32, true);
        const headerAt = directory.getUint32(at + 42, true);
        if (method !== stored || length !== unpacked || next > size) throw damaged();
        const name = decoder.decode(new Uint8Array(directory.buffer, at + 46, nameLength));
        const local = new DataView(await file.slice(headerAt, headerAt + 30).arrayBuffer());
        if (local.byteLength < 30 || local.getUint32(0, true) !== 0x04034b50) throw damaged();
        const dataAt = headerAt + 30 + local.getUint16(26, true) + local.getUint16(28, true);
        const data = file.slice(dataAt, dataAt + length);
        if (entries.has(name) || dataAt + length > start || await crc32(data) !== crc) throw damaged();
        entries.set(name, data);
        at = next;
    }
    return entries;
}
