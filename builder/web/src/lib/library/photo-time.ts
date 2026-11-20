/** Reads the time, the zone and the GPS place of a photo from the EXIF block of a JPEG file or a HEIC file. */

export interface PhotoFacts {
    /** The time on the clock of the camera, as the Unix seconds that this clock time has in UTC. */
    clock?: number;
    /** The zone of the clock: its seconds ahead of UTC. */
    zone?: number;
    /** Unix seconds of the GPS time. */
    gpsTime?: number;
    /** Longitude and latitude in degrees. */
    place?: [number, number];
}

/** The EXIF block of a JPEG file, and the box `meta` of a HEIC file, are at the start of the file. */
const HEAD_BYTES = 128 << 10;
const EXIF = 0x8769, GPS = 0x8825, TAKEN = 0x9003, ZONE = 0x9011;
const LAT_REF = 1, LAT = 2, LON_REF = 3, LON = 4, GPS_TIME = 7, GPS_DATE = 0x1d;
const ASCII = 2, RATIONAL = 5;

/** The facts of a photo. A file that is not a JPEG or a HEIC file, or that has no EXIF block, has none. */
export async function photoFacts(file: Blob): Promise<PhotoFacts> {
    // A block that is cut or damaged reads past the end of the view.
    try {
        const head = new DataView(await file.slice(0, HEAD_BYTES).arrayBuffer());
        if (head.getUint16(0) === 0xffd8) {
            const base = jpegExif(head);
            return base === undefined ? {} : tiff(head, base);
        }
        const item = heifExif(head);
        if (!item) return {};
        // The item starts with the offset of the TIFF header after these 4 bytes.
        const view = new DataView(await file.slice(item[0], item[0] + item[1]).arrayBuffer());
        return tiff(view, 4 + view.getUint32(0));
    } catch { return {}; }
}

/** The offset of the TIFF header of a JPEG file, in its segments before the image data: a marker, then a length that
 * counts itself. */
function jpegExif(view: DataView): number | undefined {
    let at = 2;
    while (view.getUint16(at) !== 0xffe1 || view.getUint32(at + 4) !== 0x45786966) {
        if (view.getUint8(at) !== 0xff || view.getUint16(at) === 0xffda) return undefined;
        at += 2 + view.getUint16(at + 2);
    }
    // The TIFF header follows "Exif" and two zero bytes.
    return at + 10;
}

/** The offset and the length of the EXIF item of a HEIF file such as HEIC: the box `meta` names the item in `iinf`
 * and gives its place in the file in `iloc`. Only an item in one piece of the file is read. */
function heifExif(view: DataView): [number, number] | undefined {
    const u8 = (at: number) => view.getUint8(at), u16 = (at: number) => view.getUint16(at), u32 = (at: number) => view.getUint32(at);
    const type = (at: number) => String.fromCharCode(u8(at), u8(at + 1), u8(at + 2), u8(at + 3));
    /** The boxes from `from` to `to`, each with its type and the start and the end of its content. A box of size 0
     * goes to the end; size 1 has a 64-bit size after the type. */
    const boxes = (from: number, to: number) => {
        const found: { type: string; start: number; end: number }[] = [];
        for (let at = from; at + 8 <= to;) {
            const size = u32(at), end = size === 1 ? at + Number(view.getBigUint64(at + 8)) : size ? at + size : to;
            if (end <= at) break;
            found.push({ type: type(at + 4), start: at + (size === 1 ? 16 : 8), end });
            at = end;
        }
        return found;
    };
    // `meta`, `iinf`, `infe` and `iloc` are full boxes: a version byte and 3 bytes of flags come first.
    const meta = type(4) === 'ftyp' ? boxes(0, view.byteLength).find(box => box.type === 'meta') : undefined;
    const inner = meta ? boxes(meta.start + 4, meta.end) : [], iinf = inner.find(box => box.type === 'iinf'), iloc = inner.find(box => box.type === 'iloc');
    if (!iinf || !iloc) return undefined;
    const id = boxes(iinf.start + (u8(iinf.start) ? 8 : 6), iinf.end).flatMap(({ type: kind, start }) => {
        const version = u8(start);
        return kind === 'infe' && version >= 2 && type(start + (version === 2 ? 8 : 10)) === 'Exif' ? [version === 2 ? u16(start + 4) : u32(start + 4)] : [];
    })[0];
    if (id === undefined) return undefined;
    const version = u8(iloc.start), sizes = u8(iloc.start + 4), more = u8(iloc.start + 5);
    const [offsetSize, lengthSize, baseSize, indexSize] = [sizes >> 4, sizes & 15, more >> 4, version ? more & 15 : 0];
    let at = iloc.start + 6;
    const next = (bytes: number) => {
        const value = bytes === 8 ? Number(view.getBigUint64(at)) : bytes === 4 ? u32(at) : bytes === 2 ? u16(at) : 0;
        at += bytes;
        return value;
    };
    for (let count = next(version < 2 ? 2 : 4); count > 0; count--) {
        const item = next(version < 2 ? 2 : 4), method = version ? next(2) & 15 : 0;
        at += 2;
        const base = next(baseSize), extents = next(2);
        at += indexSize;
        const offset = next(offsetSize), length = next(lengthSize);
        // Construction method 0: the offset is in the file.
        if (item === id) return extents === 1 && method === 0 ? [base + offset, length] : undefined;
        at += (extents - 1) * (indexSize + offsetSize + lengthSize);
    }
    return undefined;
}

/** The facts of the TIFF structure of an EXIF block whose header is at `base`. Each offset counts from it. */
function tiff(view: DataView, base: number): PhotoFacts {
    const little = view.getUint16(base) === 0x4949;
    const u16 = (offset: number) => view.getUint16(base + offset, little), u32 = (offset: number) => view.getUint32(base + offset, little);
    type Field = { count: number; at: number };
    /** The fields of a directory by their tag. A value of more than 4 bytes is at the offset that the field holds. */
    const directory = (offset: number | undefined) => new Map<number, Field>(offset === undefined ? [] : Array.from({ length: u16(offset) }, (_, i) => {
        const field = offset + 2 + i * 12, type = u16(field + 2), count = u32(field + 4);
        return [u16(field), { count, at: type === RATIONAL || (type === ASCII && count > 4) ? u32(field + 8) : field + 8 }];
    }));
    /** A text ends with a zero byte. */
    const text = (field?: Field) => field ? Array.from({ length: field.count }, (_, i) => String.fromCharCode(view.getUint8(base + field.at + i))).join('').replace(/\0.*$/, '') : '';
    const rationals = (field?: Field) => Array.from({ length: field?.count ?? 0 }, (_, i) => u32(field!.at + i * 8) / u32(field!.at + i * 8 + 4));
    const pointer = (fields: Map<number, Field>, tag: number) => fields.has(tag) ? u32(fields.get(tag)!.at) : undefined;
    const utc = (date: string, [hours, minutes, seconds]: number[]) => {
        const [, year, month, day] = /^(\d{4}):(\d\d):(\d\d)/.exec(date) ?? [];
        const time = Date.UTC(+year, +month - 1, +day, hours, minutes, seconds) / 1000;
        return +year && Number.isFinite(time) ? Math.floor(time) : undefined;
    };

    const root = directory(u32(4)), exif = directory(pointer(root, EXIF)), gps = directory(pointer(root, GPS));
    const taken = text(exif.get(TAKEN)), zone = /^([+-])(\d\d):(\d\d)/.exec(text(exif.get(ZONE)));
    const degrees = (value: number, ref: number, negative: string) => {
        const [d, m, s] = rationals(gps.get(value)), sign = text(gps.get(ref))[0] === negative ? -1 : 1;
        return sign * (d + m / 60 + s / 3600);
    };
    const [lon, lat] = [degrees(LON, LON_REF, 'W'), degrees(LAT, LAT_REF, 'S')];
    const clock = utc(taken, taken.slice(11).split(':').map(Number)), gpsTime = utc(text(gps.get(GPS_DATE)), rationals(gps.get(GPS_TIME)));
    return {
        ...clock !== undefined && { clock },
        ...zone && { zone: (zone[1] === '-' ? -1 : 1) * (+zone[2] * 3600 + +zone[3] * 60) },
        ...gpsTime !== undefined && { gpsTime },
        // A camera with no fix writes no place, or the place 0, 0.
        ...Number.isFinite(lon) && Number.isFinite(lat) && (lon !== 0 || lat !== 0) && { place: [lon, lat] as [number, number] },
    };
}
