// The archive fallback is what a Firefox user's map arrives as, so its bytes
// are pinned here at the structural level: real (small) archives are walked
// header by header, and the zip64 arithmetic is exercised through `zipLayout`
// with country-scale sizes — pure metadata, so no 4 GiB fixture exists.

import { describe, expect, it } from "vitest";
import { crc32, crc32OfBlob, storeZip, zipLayout } from "./zip";

const u16 = (b: Uint8Array, at: number) => b[at] | (b[at + 1] << 8);
const u32 = (b: Uint8Array, at: number) => (b[at] | (b[at + 1] << 8) | (b[at + 2] << 16) | (b[at + 3] << 24)) >>> 0;
const u64 = (b: Uint8Array, at: number) => u32(b, at) + u32(b, at + 4) * 0x100000000;

const bytes = (s: string) => new TextEncoder().encode(s);

describe("crc32", () => {
    it("matches the check value the polynomial is defined by", () => {
        // "123456789" → 0xCBF43926 is THE published CRC-32/ISO-HDLC vector.
        expect(crc32(bytes("123456789"))).toBe(0xcbf43926);
    });

    it("is 0 for no bytes and composes across chunks", () => {
        expect(crc32(new Uint8Array(0))).toBe(0);
        const whole = crc32(bytes("The quick brown fox"));
        const split = crc32(bytes("n fox"), crc32(bytes("The quick brow")));
        expect(split).toBe(whole);
    });

    it("streams a Blob to the same answer, reporting every byte", async () => {
        const payload = bytes("stream me in pieces");
        let reported = 0;
        const crc = await crc32OfBlob(new Blob([payload]), (n) => (reported += n));
        expect(crc).toBe(crc32(payload));
        expect(reported).toBe(payload.length);
    });
});

describe("storeZip", () => {
    async function archiveOf(entries: { name: string; body: string }[]): Promise<Uint8Array> {
        const blob = await storeZip(entries.map((e) => ({ name: e.name, blob: new Blob([bytes(e.body)]) })));
        return new Uint8Array(await blob.arrayBuffer());
    }

    it("writes a stored archive whose headers, payloads and directory agree", async () => {
        const files = [
            { name: "MS1.OBS", body: "manifest bytes" },
            { name: "MS1S00.OBM", body: "the core shard payload" },
        ];
        const zip = await archiveOf(files);

        // End of central directory: last 22 bytes, no comment.
        const eocd = zip.length - 22;
        expect(u32(zip, eocd)).toBe(0x06054b50);
        expect(u16(zip, eocd + 10)).toBe(files.length);
        const cdOffset = u32(zip, eocd + 16);
        const cdSize = u32(zip, eocd + 12);
        expect(cdOffset + cdSize).toBe(eocd);

        // Walk the central directory against each local header and its bytes.
        let at = cdOffset;
        for (const f of files) {
            const body = bytes(f.body);
            expect(u32(zip, at)).toBe(0x02014b50);
            expect(u16(zip, at + 10)).toBe(0); // STORE
            expect(u32(zip, at + 16)).toBe(crc32(body));
            expect(u32(zip, at + 20)).toBe(body.length); // compressed
            expect(u32(zip, at + 24)).toBe(body.length); // uncompressed
            const nameLen = u16(zip, at + 28);
            const name = new TextDecoder().decode(zip.slice(at + 46, at + 46 + nameLen));
            expect(name).toBe(f.name);

            const local = u32(zip, at + 42);
            expect(u32(zip, local)).toBe(0x04034b50);
            const localNameLen = u16(zip, local + 26);
            const localExtraLen = u16(zip, local + 28);
            const data = local + 30 + localNameLen + localExtraLen;
            expect(zip.slice(data, data + body.length)).toEqual(body);

            at += 46 + nameLen + u16(zip, at + 30) + u16(zip, at + 32);
        }
        expect(at).toBe(cdOffset + cdSize);
    });

    it("is deterministic — the same set zips to the same bytes", async () => {
        const files = [{ name: "A.OBM", body: "aaaa" }, { name: "B.OBM", body: "bb" }];
        expect(await archiveOf(files)).toEqual(await archiveOf(files));
    });

    it("composes the payload Blobs rather than copying them", async () => {
        // The parts of the returned Blob must include the entry Blobs by
        // reference — asserted through size arithmetic: the archive is exactly
        // headers + payloads, and `zipLayout` prices the headers.
        const entries = [
            { name: "x", blob: new Blob([bytes("0123456789")]) },
            { name: "yy", blob: new Blob([bytes("abcdef")]) },
        ];
        const metas = await Promise.all(
            entries.map(async (e) => ({ name: e.name, size: e.blob.size, crc: await crc32OfBlob(e.blob) })),
        );
        const layout = zipLayout(metas);
        const blob = await storeZip(entries);
        expect(blob.size).toBe(layout.totalBytes);
    });
});

describe("zipLayout zip64", () => {
    const FIVE_GIB = 5 * 1024 ** 3;

    it("stays plain below 4 GiB", () => {
        const layout = zipLayout([{ name: "small", size: 100, crc: 0x12345678 }]);
        const local = layout.locals[0];
        expect(u16(local, 4)).toBe(20); // version needed
        expect(u16(local, 28)).toBe(0); // no extra
        // No zip64 EOCD record anywhere in the tail — just directory + EOCD.
        expect(findSig(layout.tail, 0x06064b50)).toBe(-1);
        expect(u32(layout.tail, layout.tail.length - 22)).toBe(0x06054b50);
    });

    it("marks an oversized entry in its local header and carries the true size in the extra", () => {
        const layout = zipLayout([{ name: "big", size: FIVE_GIB, crc: 0 }]);
        const local = layout.locals[0];
        expect(u16(local, 4)).toBe(45);
        expect(u32(local, 18)).toBe(0xffffffff); // compressed
        expect(u32(local, 22)).toBe(0xffffffff); // uncompressed
        const extra = 30 + u16(local, 26);
        expect(u16(local, extra)).toBe(0x0001);
        expect(u16(local, extra + 2)).toBe(16);
        expect(u64(local, extra + 4)).toBe(FIVE_GIB);
        expect(u64(local, extra + 12)).toBe(FIVE_GIB);
    });

    it("carries an overflowed local offset in the central extra and writes the zip64 end records", () => {
        const layout = zipLayout([
            { name: "big", size: FIVE_GIB, crc: 0 },
            { name: "after", size: 10, crc: 0 },
        ]);
        expect(layout.offsets[1]).toBeGreaterThan(0xffffffff);

        const tail = layout.tail;
        // Second central entry: skip the first.
        const first = 0;
        expect(u32(tail, first)).toBe(0x02014b50);
        const second = 46 + u16(tail, first + 28) + u16(tail, first + 30);
        expect(u32(tail, second)).toBe(0x02014b50);
        expect(u32(tail, second + 42)).toBe(0xffffffff); // local offset slot
        const nameLen = u16(tail, second + 28);
        const extra = second + 46 + nameLen;
        expect(u16(tail, extra)).toBe(0x0001);
        expect(u16(tail, extra + 2)).toBe(8); // only the offset overflowed
        expect(u64(tail, extra + 4)).toBe(layout.offsets[1]);

        // Zip64 EOCD + locator + classic EOCD, in that order, at the tail's end.
        const eocd64 = findSig(tail, 0x06064b50);
        expect(eocd64).toBeGreaterThanOrEqual(0);
        expect(u64(tail, eocd64 + 24)).toBe(2); // entries
        const locator = findSig(tail, 0x07064b50);
        expect(locator).toBe(eocd64 + 56);
        const eocd = tail.length - 22;
        expect(u32(tail, eocd)).toBe(0x06054b50);
        expect(u32(tail, eocd + 16)).toBe(0xffffffff); // cd offset deferred to zip64
    });

    it("prices the archive: headers plus payloads, nothing else", () => {
        const metas = [
            { name: "a", size: 7, crc: 0 },
            { name: "bc", size: 11, crc: 0 },
        ];
        const layout = zipLayout(metas);
        const headers = layout.locals.reduce((n, h) => n + h.length, 0);
        expect(layout.totalBytes).toBe(headers + 7 + 11 + layout.tail.length);
        expect(layout.offsets[1]).toBe(layout.locals[0].length + 7);
    });
});

function findSig(b: Uint8Array, sig: number): number {
    for (let i = 0; i + 4 <= b.length; i++) if (u32(b, i) === sig) return i;
    return -1;
}
