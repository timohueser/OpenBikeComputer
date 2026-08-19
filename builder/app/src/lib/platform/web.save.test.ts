// The web host's map output: a directory the rider picks, one file written into it.
//
// These are the pins for the two failures the FS7.5b2 review found in that path, and both
// are about **honesty after something went wrong** rather than about the happy path. A ~9 GiB
// write is the case that makes them reachable: it is long enough to be interrupted, and big
// enough that a card can fill partway through.

import { beforeEach, describe, expect, it, vi } from "vitest";

/** A `FileSystemWritableFileStream` that fails on the Nth `write`, like a card filling up. */
function tearingWritable(failOnWrite = true) {
    return {
        write: vi.fn(async () => {
            if (failOnWrite) throw new DOMException("quota", "QuotaExceededError");
        }),
        close: vi.fn(async () => {}),
    };
}

/** A directory handle that records what was created and what was removed. */
function mockDir(stream: { write: unknown; close: unknown }) {
    const created: string[] = [];
    const removed: string[] = [];
    return {
        created,
        removed,
        handle: {
            name: "OBC CARD",
            getFileHandle: vi.fn(async (filename: string) => {
                // The entry exists from here on, whatever happens to the stream after.
                created.push(filename);
                return { createWritable: vi.fn(async () => stream) };
            }),
            removeEntry: vi.fn(async (filename: string) => {
                removed.push(filename);
            }),
        },
    };
}

async function freshHost() {
    vi.resetModules();
    return (await import("./web")).platform;
}

beforeEach(() => {
    Object.assign(globalThis, { document: { baseURI: "https://maps.example.org/builder/" } });
});

describe("a torn write is still something that exists", () => {
    /**
     * **The boot-fault honesty rule, applied to the folder instead of the card.** A map-named
     * file that is not a map must never be reported as an absence.
     *
     * `getFileHandle(..., { create: true })` puts the entry in the rider's directory before a
     * single byte is written. So if the write then fails — a full card partway through a
     * country-sized map — an orphan is sitting there under the map's own name. Recording the
     * filename only *after* `close()` succeeded meant `discard()` had nothing to remove and the
     * UI said "Nothing was saved" beside a truncated file.
     */
    it("removes the orphan a failed write left behind", async () => {
        const stream = tearingWritable(true);
        const dir = mockDir(stream);
        vi.stubGlobal("window", { showDirectoryPicker: vi.fn(async () => dir.handle) });

        const host = await freshHost();
        expect(host.openMapOutput, "the picker host exposes the seam").not.toBeNull();
        const session = await host.openMapOutput!("OBC map.obcm");

        await expect(session.write("OBC map.obcm", new Uint8Array([1, 2, 3]))).rejects.toThrow(/quota/i);
        expect(dir.created, "the entry was created before the write failed").toEqual(["OBC map.obcm"]);

        await session.discard();
        expect(dir.removed, "…and discard knows about it").toEqual(["OBC map.obcm"]);
    });

    it("still removes a file whose write succeeded", async () => {
        const dir = mockDir(tearingWritable(false));
        vi.stubGlobal("window", { showDirectoryPicker: vi.fn(async () => dir.handle) });

        const host = await freshHost();
        const session = await host.openMapOutput!("OBC map.obcm");
        await session.write("OBC map.obcm", new Uint8Array([1, 2, 3]));
        await session.discard();

        expect(dir.removed).toEqual(["OBC map.obcm"]);
    });

    /** A second `discard` has nothing left to do — the list is cleared, not re-walked. */
    it("does not double-remove", async () => {
        const dir = mockDir(tearingWritable(false));
        vi.stubGlobal("window", { showDirectoryPicker: vi.fn(async () => dir.handle) });

        const host = await freshHost();
        const session = await host.openMapOutput!("OBC map.obcm");
        await session.write("OBC map.obcm", new Uint8Array([1]));
        await session.discard();
        await session.discard();

        expect(dir.removed).toEqual(["OBC map.obcm"]);
    });

    /**
     * A removal that refuses — a lock, a card pulled mid-discard — is surfaced rather than
     * swallowed, because the caller paints "some files could not be cleaned up" from it. Every
     * entry is still attempted first.
     */
    it("attempts every removal and reports a refusal", async () => {
        const dir = mockDir(tearingWritable(false));
        dir.handle.removeEntry = vi.fn(async () => {
            throw new DOMException("locked", "NoModificationAllowedError");
        });
        vi.stubGlobal("window", { showDirectoryPicker: vi.fn(async () => dir.handle) });

        const host = await freshHost();
        const session = await host.openMapOutput!("OBC map.obcm");
        await session.write("OBC map.obcm", new Uint8Array([1]));

        await expect(session.discard()).rejects.toThrow(/locked/i);
    });
});
