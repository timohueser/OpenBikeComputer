import { IDBFactory } from 'fake-indexeddb';
import { crc32, deflateRawSync, gzipSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { readFiles, reasons, type Found, type Item, type Readers } from './import';
import { Library } from './store';

const encode = (value: unknown) => new TextEncoder().encode(typeof value === 'string' ? value : JSON.stringify(value));

/** Fake readers: a GPX file is the JSON of a track, a FIT file the JSON of a ride or of `{ code }`. */
const readers: Readers = {
    gpx: text => {
        try { return JSON.parse(text); } catch { throw Object.assign(new Error('no points'), { name: 'GpxError' }); }
    },
    fit: bytes => {
        const value = JSON.parse(new TextDecoder().decode(bytes));
        if (value.code) throw Object.assign(new Error(value.code), { name: 'FitError', code: value.code });
        return value;
    },
};

/** `size` is the declared size of the content. */
interface Part { name: string | Uint8Array; data: Uint8Array; method?: number; crc?: number; size?: number }

/** A ZIP file. A name in bytes has no UTF-8 flag. With `zip64`, the sizes and offsets are in ZIP64 records. */
function zipOf(parts: Part[], zip64 = false): Uint8Array<ArrayBuffer> {
    const files: Buffer[] = [], central: Buffer[] = [];
    let offset = 0;
    for (const { name, data, method = 8, crc = crc32(data), size = data.length } of parts) {
        const path = Buffer.from(name), packed = method === 8 ? deflateRawSync(data) : Buffer.from(data), flags = typeof name === 'string' ? 0x0800 : 0;
        const local = Buffer.alloc(30), header = Buffer.alloc(46), extra = Buffer.alloc(zip64 ? 28 : 0);
        local.writeUInt32LE(0x04034b50, 0);
        header.writeUInt32LE(0x02014b50, 0);
        for (const [buffer, at] of [[local, 4], [header, 6]] as const) {
            buffer.writeUInt16LE(flags, at + 2);
            buffer.writeUInt16LE(method, at + 4);
            buffer.writeUInt32LE(crc, at + 10);
            buffer.writeUInt32LE(zip64 ? 0xffffffff : packed.length, at + 14);
            buffer.writeUInt32LE(zip64 ? 0xffffffff : size, at + 18);
            buffer.writeUInt16LE(path.length, at + 22);
        }
        header.writeUInt16LE(extra.length, 30);
        header.writeUInt32LE(zip64 ? 0xffffffff : offset, 42);
        if (zip64) [1, 24].forEach((v, i) => extra.writeUInt16LE(v, i * 2));
        if (zip64) [size, packed.length, offset].forEach((v, i) => extra.writeBigUInt64LE(BigInt(v), 4 + i * 8));
        files.push(local, path, packed);
        central.push(header, path, extra);
        offset += 30 + path.length + packed.length;
    }
    const directory = Buffer.concat(central), end = Buffer.alloc(22), tail: Buffer[] = [];
    end.writeUInt32LE(0x06054b50, 0);
    if (zip64) {
        const record = Buffer.alloc(56), locator = Buffer.alloc(20);
        record.writeUInt32LE(0x06064b50, 0);
        [[4, 44], [24, parts.length], [32, parts.length], [40, directory.length], [48, offset]].forEach(([at, v]) => record.writeBigUInt64LE(BigInt(v), at));
        locator.writeUInt32LE(0x07064b50, 0);
        locator.writeBigUInt64LE(BigInt(offset + directory.length), 8);
        tail.push(record, locator);
        end.fill(0xff, 8, 20);
    } else {
        [[8, 2], [10, 2], [12, 4], [16, 4]].forEach(([at, size], i) => end.writeUIntLE([parts.length, parts.length, directory.length, offset][i], at, size));
    }
    return new Uint8Array(Buffer.concat([...files, directory, ...tail, end]));
}

const t = Date.UTC(2026, 7, 1, 7) / 1000;
const samples = (start: number, east = 0) => [0, 1, 2].map(i => ({ timeS: start + i * 60, lat: 47e6, lon: 8e6 + east + i * 1000, segmentStart: !i }));
const points = (start?: number, east = 0) => samples(start ?? 0, east).map(({ timeS, lat, lon }) => ({ lat, lon, ...start !== undefined && { time: timeS } }));
const file = (name: string, value: unknown) => new File([value instanceof Uint8Array ? value as Uint8Array<ArrayBuffer> : encode(value)], name);
const gz = (value: unknown) => new Uint8Array(gzipSync(encode(value)));

const evening = file('evening.gpx', { name: 'Evening', points: points(t), waypoints: [], kind: 'mountain_biking' });
const jura = file('jura.gpx', { name: 'Jura', points: points(undefined, 2000), waypoints: [] });
const morning = file('1.fit.gz', gz({ sport: 'bike', samples: samples(t + 3600) }));

/** A dropped folder with `files`, as `FileSystemDirectoryEntry` gives it. */
function folder(name: string, files: File[], onRead = () => {}): FileSystemEntry {
    const entry = (file: File) => ({ isFile: true, isDirectory: false, name: file.name, fullPath: `/${name}/${file.name}`, file: (ok: (f: File) => void) => ok(file) });
    let read = false;
    return { isFile: false, isDirectory: true, name, fullPath: `/${name}`,
        createReader: () => ({ readEntries: (ok: (entries: unknown[]) => void) => { onRead(); ok(read ? [] : (read = true, files.map(entry))); } }) } as unknown as FileSystemEntry;
}

async function collect(items: Item[], library: Library): Promise<Found[]> {
    const found: Found[] = [];
    for await (const item of readFiles(items, library, readers, new AbortController().signal)) found.push(item);
    return found;
}

const row = (found: Found) => [found.file, found.kind, found.name,
    ...found.kind === 'ride' ? [found.ride.summary.bike] : found.kind === 'route' ? [found.bike] : found.kind === 'unreadable' ? [found.reason] : []];

describe('readFiles', () => {
    it('reads a folder, deflated entries, a CP437 name, a ZIP in a ZIP, ZIP64 records and .gz files, with the names of a Strava CSV', async () => {
        const library = new Library(new IDBFactory());
        // A name in quotes with a comma, a quote and a line break; the CSV comes after the files of the ZIP.
        const csv = 'Activity ID,Activity Date,Activity Name,Activity Type,Filename\n1,"Aug 1, 2026","Loop, ""short""\nversion",Gravel Ride,activities/1.fit.gz\r\n';
        const exported = file('export.zip', zipOf([
            { name: 'activities/1.fit.gz', data: new Uint8Array(await morning.arrayBuffer()), method: 0 },
            { name: Uint8Array.from([...encode('Z'), 0x81, ...encode('rich.gpx')]), data: encode({ name: 'Zürich', points: points(undefined, 5000), waypoints: [] }) },
            // The macOS Finder writes UTF-8 names with no flag.
            { name: encode('Genève.gpx'), data: encode({ name: 'Genève', points: points(undefined, 9000), waypoints: [] }) },
            { name: 'DI_CONNECT/part.zip', data: zipOf([{ name: 'UploadedFiles/2.fit', data: encode({ sport: 'foot', samples: samples(t + 7200) }) }]) },
            { name: 'notes.txt', data: encode('not counted') },
            { name: 'activities.csv', data: encode(csv) },
        ]));
        const old = file('old.zip', zipOf([{ name: 'other.fit', data: encode({ sport: 'road', samples: samples(t + 10800) }) }], true));
        expect((await collect([folder('rides', [evening, jura]), exported, old], library)).map(row)).toEqual([
            ['rides/evening.gpx', 'ride', 'Evening', 'mtb'],
            ['rides/jura.gpx', 'route', 'Jura', 'touring'],
            ['export.zip/activities/1.fit.gz', 'ride', 'Loop, "short"\nversion', 'gravel'],
            ['export.zip/Zürich.gpx', 'route', 'Zürich', 'touring'],
            ['export.zip/Genève.gpx', 'route', 'Genève', 'touring'],
            ['export.zip/DI_CONNECT/part.zip/UploadedFiles/2.fit', 'ride', 'Hike, 1 Aug 2026', 'hiking'],
            ['old.zip/other.fit', 'ride', 'other', 'road'],
        ]);
        await library.close();
    });

    it('counts a ride as known when its key is stored, deleted or came before, and a route when its id is stored', async () => {
        const library = new Library(new IDBFactory());
        const first = await collect([evening, morning, jura, file('copy.gpx', await evening.text())], library);
        expect(first.map(found => found.kind)).toEqual(['ride', 'ride', 'route', 'known']);
        const [ride, other, route] = first as [Found & { kind: 'ride' }, Found & { kind: 'ride' }, Found & { kind: 'route' }];
        expect([ride.ride.key, ride.ride.summary.start]).toEqual(['20260801T070000Z', t]);
        expect(await (await other.open()).text()).toBe(JSON.stringify({ sport: 'bike', samples: samples(t + 3600) }));
        expect((await route.open()).name).toBe('Jura');
        expect(route.id).toMatch(/^file-[0-9a-f]{32}$/);
        await library.addRide(ride.ride, await ride.open());
        await library.addRide(other.ride, await other.open());
        await library.removeRide(other.ride.key);
        await library.addRoute({ id: route.id, revision: 0, name: route.name, updatedAt: 0, summary: null, trip: {} });
        expect((await collect([evening, morning, jura], library)).map(found => found.kind)).toEqual(['known', 'known', 'known']);
        await library.close();
    });

    it('gives the reason for each file that it cannot read, and counts no other file', async () => {
        const library = new Library(new IDBFactory());
        const packed = file('packed.zip', zipOf([
            { name: 'bzip2.fit', data: encode({ sport: 'bike', samples: samples(t) }), method: 12 },
            { name: 'crc.fit', data: encode({ sport: 'bike', samples: samples(t) }), crc: 1 },
            { name: 'photo.jpg', data: encode('not counted') },
            // 1 MB of zeros that declares 1 KB.
            { name: 'bomb.fit', data: new Uint8Array(1 << 20), size: 1024 },
        ]));
        const found = await collect([
            file('a.tcx', '<TrainingCenterDatabase/>'), file('b.tcx.gz', gz('<TrainingCenterDatabase/>')),
            file('swim.fit', { sport: 'other', samples: samples(t) }), file('cut.fit', { code: 'fit-damaged' }),
            file('empty.fit', { code: 'fit-no-positions' }), file('page.gpx', '<html/>'), file('broken.fit.gz', 'not gzip'),
            file('bad.zip', 'not a zip'), packed, file('readme.md', 'not counted'),
        ], library);
        expect(found.map(f => [f.name, f.kind === 'unreadable' && f.reason])).toEqual([
            ['bad.zip', reasons.damaged], ['a.tcx', reasons.tcx], ['b.tcx.gz', reasons.tcx], ['swim.fit', reasons.sport],
            ['cut.fit', reasons.damaged], ['empty.fit', reasons.track], ['page.gpx', reasons.track], ['broken.fit.gz', reasons.damaged],
            ['bzip2.fit', reasons.packing], ['crc.fit', reasons.damaged], ['bomb.fit', reasons.damaged],
        ]);
        await library.close();
    });

    it('takes a specific bike of Strava or else of the file, refuses a sport that is not by bike or on foot, and names a ride with no name by its date', async () => {
        const library = new Library(new IDBFactory());
        const csv = (header: string) => `${header}\n${[['1.fit', 'Ride'], ['2.gpx', 'Swim'], ['3.fit', 'Alpine Ski'], ['4.fit', 'Ride']]
            .map(([file, type], i) => `${i},"Aug 1, 2026",${i === 3 ? '' : `Name ${i}`},${type},,,,,,,,,activities/${file}`).join('\n')}\n`;
        const files = [
            file('1.fit', { sport: 'road', samples: samples(t) }),
            file('2.gpx', { name: 'Lake', points: points(t + 3600), waypoints: [] }),
            file('3.fit', { sport: 'bike', samples: samples(t + 7200) }),
            file('4.fit', { sport: 'bike', samples: samples(t + 10800) }),
            file('12345.fit', { sport: 'foot', samples: samples(t + 14400) }),
            file('me@example.org.gpx', { name: 'me@example.org', points: points(undefined, 7000), waypoints: [] }),
        ];
        const english = 'Activity ID,Activity Date,Activity Name,Activity Type,Activity Description,Elapsed Time,Distance,Max Heart Rate,Relative Effort,Commute,Activity Private Note,Activity Gear,Filename';
        expect((await collect([...files, file('activities.csv', csv(english))], library)).map(row)).toEqual([
            ['1.fit', 'ride', 'Name 0', 'road'],
            ['2.gpx', 'unreadable', '2.gpx', reasons.sport],
            ['3.fit', 'unreadable', '3.fit', reasons.sport],
            ['4.fit', 'ride', 'Ride, 1 Aug 2026', 'touring'],
            ['12345.fit', 'ride', 'Hike, 1 Aug 2026', 'hiking'],
            ['me@example.org.gpx', 'route', 'Planned route', 'touring'],
        ]);
        // German headers: the names come from the place of the columns; a type that names no bike leaves the sport of the file.
        const german = (await collect([...files.slice(0, 3), file('activities.csv', csv(english.replace('Activity Name', 'Name der Aktivität').replace('Activity Type', 'Aktivitätsart').replace('Filename', 'Dateiname')))], library)).map(row);
        expect(german).toEqual([['1.fit', 'ride', 'Name 0', 'road'], ['2.gpx', 'ride', 'Name 1', 'touring'], ['3.fit', 'ride', 'Name 2', 'touring']]);
        await library.close();
    });

    it('stops when the signal aborts, also while it lists, and opens no later ZIP', async () => {
        const controller = new AbortController();
        let reads = 0;
        const later = new (class extends File { slice(...args: Parameters<File['slice']>) { reads++; return super.slice(...args); } })(
            [zipOf([{ name: 'a.fit', data: encode({ sport: 'bike', samples: samples(t) }) }])], 'later.zip');
        const run = readFiles([folder('rides', [evening], () => controller.abort()), later], new Library(new IDBFactory()), readers, controller.signal);
        await expect(run.next()).rejects.toThrow(/abort/i);
        expect(reads).toBe(0);
    });

    it('finds a GPX file with times as a ride that can become a route, as a route when it starts later than now, and as known in either form once stored', async () => {
        const library = new Library(new IDBFactory());
        const tour = file('tour.gpx', { name: 'Tour', points: points(Date.now() / 1000 + 86400, 3000), waypoints: [] });
        const [ride, planned] = await collect([evening, tour], library) as [Found & { kind: 'ride' }, Found & { kind: 'route' }];
        expect([ride.kind, ride.other?.kind, planned.kind, planned.other?.kind]).toEqual(['ride', 'route', 'route', 'ride']);
        const route = ride.other as Found & { kind: 'route' }, recorded = planned.other as Found & { kind: 'ride' };
        expect(route).toMatchObject({ name: 'Evening', bike: 'mtb', lengthM: ride.ride.summary.lengthM });
        expect((await route.open()).points.map(p => p.time)).toEqual(points(t).map(p => p.time));
        await library.addRoute({ id: route.id, revision: 0, name: route.name, updatedAt: 0, summary: null, trip: {} });
        await library.addRide(recorded.ride, await recorded.open());
        expect((await collect([evening, tour], library)).map(found => found.kind)).toEqual(['known', 'known']);
        await library.close();
    });
});
