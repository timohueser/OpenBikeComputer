/** Reads the time, the zone and the GPS place of a photo from the EXIF block of a JPEG file. */

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

/** The EXIF block of a camera is at the start of the file, before the image data. */
const HEAD_BYTES = 128 << 10;
const EXIF = 0x8769, GPS = 0x8825, TAKEN = 0x9003, ZONE = 0x9011;
const LAT_REF = 1, LAT = 2, LON_REF = 3, LON = 4, GPS_TIME = 7, GPS_DATE = 0x1d;
const ASCII = 2, RATIONAL = 5;

/** The facts of a photo. A file that is not a JPEG, or that has no EXIF block, has none. */
export async function photoFacts(file: Blob): Promise<PhotoFacts> {
    // A block that is cut or damaged reads past the end of the view.
    try { return read(new DataView(await file.slice(0, HEAD_BYTES).arrayBuffer())); } catch { return {}; }
}

function read(view: DataView): PhotoFacts {
    if (view.getUint16(0) !== 0xffd8) return {};
    // The segments before the image data: a marker, then a length that counts itself.
    let at = 2;
    while (view.getUint16(at) !== 0xffe1 || view.getUint32(at + 4) !== 0x45786966) {
        if (view.getUint8(at) !== 0xff || view.getUint16(at) === 0xffda) return {};
        at += 2 + view.getUint16(at + 2);
    }
    // The TIFF header follows "Exif" and two zero bytes. Each offset counts from it.
    const base = at + 10, little = view.getUint16(base) === 0x4949;
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
