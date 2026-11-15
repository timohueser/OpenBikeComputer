import { describe, expect, it } from 'vitest';
import { photoFacts } from './photo-time';

/** A tag with a pointer (a number), a text or a list of rationals. */
type Field = [tag: number, value: number | string | number[]];

/** A TIFF directory at the offset `at`, followed by the values that do not fit in a field. */
function directory(fields: Field[], at: number, big: boolean): number[] {
    const u16 = (v: number) => big ? [v >> 8, v & 255] : [v & 255, v >> 8];
    const u32 = (v: number) => big ? [v >>> 24, v >> 16 & 255, v >> 8 & 255, v & 255] : [v & 255, v >> 8 & 255, v >> 16 & 255, v >>> 24];
    const head = u16(fields.length), data: number[] = [], dataAt = at + 2 + fields.length * 12 + 4;
    for (const [tag, value] of fields) {
        if (typeof value === 'number') head.push(...u16(tag), ...u16(4), ...u32(1), ...u32(value));
        else if (typeof value === 'string') {
            const bytes = [...value].map(letter => letter.charCodeAt(0)).concat(0);
            head.push(...u16(tag), ...u16(2), ...u32(bytes.length));
            if (bytes.length <= 4) head.push(...bytes, ...Array<number>(4 - bytes.length).fill(0));
            else { head.push(...u32(dataAt + data.length)); data.push(...bytes); }
        } else {
            head.push(...u16(tag), ...u16(5), ...u32(value.length), ...u32(dataAt + data.length));
            for (const number of value) data.push(...u32(Math.round(number * 1000)), ...u32(1000));
        }
    }
    return [...head, ...u32(0), ...data];
}

/** A JPEG file with an EXIF block and no image data: an APP0 segment, then the block. */
function photo(exif: Field[], gps: Field[] = [], big = false): Blob {
    const exifAt = 8 + 2 + 2 * 12 + 4, exifBytes = directory(exif, exifAt, big), gpsAt = exifAt + exifBytes.length;
    const tiff = [...big ? [0x4d, 0x4d, 0, 42, 0, 0, 0, 8] : [0x49, 0x49, 42, 0, 8, 0, 0, 0],
        ...directory([[0x8769, exifAt], [0x8825, gpsAt]], 8, big), ...exifBytes, ...directory(gps, gpsAt, big)];
    const length = tiff.length + 8;
    return new Blob([new Uint8Array([0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xe1, length >> 8, length & 255, 0x45, 0x78, 0x69, 0x66, 0, 0, ...tiff, 0xff, 0xd9])]);
}

const clock = Date.UTC(2026, 9, 24, 10, 20, 5) / 1000;

describe('the facts of a photo', () => {
    it('reads the clock time with its zone', async () => {
        expect(await photoFacts(photo([[0x9003, '2026:10:24 10:20:05'], [0x9011, '+02:00']]))).toEqual({ clock, zone: 7200 });
        expect(await photoFacts(photo([[0x9003, '2026:10:24 10:20:05'], [0x9011, '-03:30']], [], true))).toEqual({ clock, zone: -12_600 });
    });

    it('reads a clock time with no zone, and no time from a clock that was never set', async () => {
        expect(await photoFacts(photo([[0x9003, '2026:10:24 10:20:05']], [], true))).toEqual({ clock });
        expect(await photoFacts(photo([[0x9003, '0000:00:00 00:00:00']]))).toEqual({});
    });

    it('reads the GPS time and the GPS place', async () => {
        const gps: Field[] = [[1, 'S'], [2, [46, 52, 30.6]], [3, 'W'], [4, [8, 39, 0]], [7, [8, 20, 5]], [0x1d, '2026:10:24']];
        const facts = await photoFacts(photo([], gps));
        expect(facts.gpsTime).toBe(clock - 7200);
        expect(facts.place![0]).toBeCloseTo(-8.65, 6);
        expect(facts.place![1]).toBeCloseTo(-46.8751667, 6);
        expect(facts.clock).toBeUndefined();
        // A camera with no fix.
        expect(await photoFacts(photo([], [[1, 'N'], [2, [0, 0, 0]], [3, 'E'], [4, [0, 0, 0]]]))).toEqual({});
    });

    it('reads nothing from a file with no EXIF block, a cut block or another kind of file', async () => {
        const whole = photo([[0x9003, '2026:10:24 10:20:05'], [0x9011, '+02:00']]);
        expect(await photoFacts(new Blob([new Uint8Array([0xff, 0xd8, 0xff, 0xdb, 0, 4, 0, 0, 0xff, 0xda, 0, 2])]))).toEqual({});
        expect(await photoFacts(whole.slice(0, 60))).toEqual({});
        expect(await photoFacts(new Blob(['<gpx></gpx>']))).toEqual({});
    });
});
