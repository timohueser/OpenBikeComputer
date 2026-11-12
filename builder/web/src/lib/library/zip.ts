/** Writes ZIP files whose entries are stored with no compression, and reads ZIP files of stored or deflated entries. */

export interface Entry {
    name: string;
    data: Blob;
}

/** An entry of a ZIP file, as its central directory gives it. `open` gives its content. */
export interface ZipEntry {
    name: string;
    /** The ZIP file. */
    file: Blob;
    method: number;
    encrypted: boolean;
    crc: number;
    packed: number;
    size: number;
    /** The offset of the local header in `file`. */
    at: number;
}

/** An entry that is encrypted, or packed with a method other than stored or deflated. */
export class UnsupportedPacking extends Error {}

const crcTable = Uint32Array.from({ length: 256 }, (_, i) => {
    let c = i;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
});

/** The CRC-32 and the length of a stream. `keep` gets each chunk. */
async function measure(stream: ReadableStream<Uint8Array<ArrayBuffer>>, keep?: Uint8Array<ArrayBuffer>[]): Promise<{ crc: number; length: number }> {
    const reader = stream.getReader();
    let c = 0xffffffff, length = 0;
    for (;;) {
        const { done, value } = await reader.read();
        if (done) return { crc: (c ^ 0xffffffff) >>> 0, length };
        for (let i = 0; i < value.length; i++) c = crcTable[(c ^ value[i]) & 0xff] ^ (c >>> 8);
        length += value.length;
        keep?.push(value);
    }
}

const utf8 = 0x0800, stored = 0, deflated = 8, needed = 10, max32 = 0xffffffff;
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
        const path = new TextEncoder().encode(name), { crc } = await measure(data.stream());
        if (offset + 30 + path.length + data.size > max32) throw tooLarge();
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
    if (offset + size > max32) throw tooLarge();
    const end = new DataView(new ArrayBuffer(22));
    end.setUint32(0, 0x06054b50, true);
    end.setUint16(8, entries.length, true);
    end.setUint16(10, entries.length, true);
    end.setUint32(12, size, true);
    end.setUint32(16, offset, true);
    return new Blob([...parts, ...central, end], { type: 'application/zip' });
}

const damaged = () => new Error('The file is damaged or is not a ZIP file.');

async function view(file: Blob, at: number, length: number): Promise<DataView> {
    const bytes = await file.slice(at, at + length).arrayBuffer();
    if (bytes.byteLength !== length) throw damaged();
    return new DataView(bytes);
}

function uint64(view: DataView, at: number): number {
    const value = Number(view.getBigUint64(at, true));
    if (!Number.isSafeInteger(value)) throw damaged();
    return value;
}

// The characters 128 to 255 of code page 437, the encoding of a name without the UTF-8 flag.
const cp437 = 'ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■ ';

/** The entries of a ZIP file. It reads only the central directory, also of a ZIP64 file. */
export async function entries(file: Blob): Promise<ZipEntry[]> {
    const tailStart = Math.max(0, file.size - 22 - 0xffff);
    const tail = new DataView(await file.slice(tailStart).arrayBuffer());
    let end = tail.byteLength - 22;
    while (end >= 0 && tail.getUint32(end, true) !== 0x06054b50) end--;
    if (end < 0) throw damaged();
    let count = tail.getUint16(end + 10, true), size = tail.getUint32(end + 12, true), start = tail.getUint32(end + 16, true);
    // The ZIP64 locator comes just before the end record and points to the ZIP64 end record.
    if (end >= 20 && tail.getUint32(end - 20, true) === 0x07064b50) {
        const record = await view(file, uint64(tail, end - 12), 56);
        if (record.getUint32(0, true) !== 0x06064b50) throw damaged();
        [count, size, start] = [32, 40, 48].map(at => uint64(record, at));
    }
    const directory = await view(file, start, size), decoder = new TextDecoder();
    const list: ZipEntry[] = [];
    let at = 0;
    for (let i = 0; i < count; i++) {
        if (at + 46 > size || directory.getUint32(at, true) !== 0x02014b50) throw damaged();
        const flags = directory.getUint16(at + 8, true), method = directory.getUint16(at + 10, true), crc = directory.getUint32(at + 16, true);
        const nameLength = directory.getUint16(at + 28, true), extraAt = at + 46 + nameLength, extraEnd = extraAt + directory.getUint16(at + 30, true);
        const next = extraEnd + directory.getUint16(at + 32, true);
        if (next > size) throw damaged();
        let packed = directory.getUint32(at + 20, true), unpacked = directory.getUint32(at + 24, true), offset = directory.getUint32(at + 42, true);
        // The ZIP64 field holds 8 bytes for each of these three values that is 0xffffffff, in this order.
        for (let x = extraAt; x + 4 <= extraEnd; x += 4 + directory.getUint16(x + 2, true)) {
            if (directory.getUint16(x, true) !== 1) continue;
            let y = x + 4;
            const take = () => { if (y + 8 > extraEnd) throw damaged(); y += 8; return uint64(directory, y - 8); };
            if (unpacked === max32) unpacked = take();
            if (packed === max32) packed = take();
            if (offset === max32) offset = take();
        }
        const name = new Uint8Array(directory.buffer, at + 46, nameLength);
        list.push({ name: flags & utf8 ? decoder.decode(name) : Array.from(name, c => c < 128 ? String.fromCharCode(c) : cp437[c - 128]).join(''),
            file, method, encrypted: !!(flags & 1), crc, packed, size: unpacked, at: offset });
        at = next;
    }
    return list;
}

/** The content of an entry, after a check of its CRC-32. A stored entry is a slice of the ZIP file; a deflated one
 * is a new Blob, which the browser can keep on disk. */
export async function open(entry: ZipEntry): Promise<Blob> {
    if (entry.encrypted || (entry.method !== stored && entry.method !== deflated)) throw new UnsupportedPacking('This packing is not supported.');
    const local = await view(entry.file, entry.at, 30);
    if (local.getUint32(0, true) !== 0x04034b50) throw damaged();
    const dataAt = entry.at + 30 + local.getUint16(26, true) + local.getUint16(28, true);
    if (dataAt + entry.packed > entry.file.size) throw damaged();
    const data = entry.file.slice(dataAt, dataAt + entry.packed);
    const chunks: Uint8Array<ArrayBuffer>[] | undefined = entry.method === deflated ? [] : undefined;
    const content = chunks ? data.stream().pipeThrough(new DecompressionStream('deflate-raw')) : data.stream();
    const { crc, length } = await measure(content, chunks).catch(() => { throw damaged(); });
    if (crc !== entry.crc || length !== entry.size) throw damaged();
    return chunks ? new Blob(chunks) : data;
}

/** Each entry of a ZIP file by name, after a check of its CRC-32. */
export async function unzip(file: Blob): Promise<Map<string, Blob>> {
    const contents = new Map<string, Blob>();
    for (const entry of await entries(file)) {
        if (contents.has(entry.name)) throw damaged();
        contents.set(entry.name, await open(entry));
    }
    return contents;
}
