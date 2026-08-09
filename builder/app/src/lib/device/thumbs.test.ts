/** The host-specific thumbnail store, with device I/O represented by loader functions. */

import { describe, expect, it } from "vitest";

import { PREVIEW_POINTS } from "./library";
import type { RideScope } from "./rides";
import {
    DeviceThumbs,
    ThumbCache,
    memoryStorage,
    rideFingerprint,
    routeFingerprint,
    thumbKey,
    thumbnailStorageFor,
    type Thumb,
    type ThumbRequest,
    type ThumbStorage,
} from "./thumbs.svelte";

const SCOPE: RideScope = { serial: "OBC-0042", epoch: 7 };
const TRACK: Thumb = [
    [47.9950001234, 7.8420009876],
    [47.996, 7.843],
    [47.997, 7.844],
];

const request = (id: number, load: ThumbRequest["load"]): ThumbRequest => ({
    kind: "route",
    id,
    fingerprint: `c${id}`,
    load,
});

function recordedStorage(): { storage: ThumbStorage; map: Map<string, string> } {
    const map = new Map<string, string>();
    return {
        map,
        storage: {
            get: (key) => map.get(key) ?? null,
            set: (key, value) => void map.set(key, value),
            remove: (key) => void map.delete(key),
            keys: () => [...map.keys()],
        },
    };
}

function ticker(): { now: () => number; tick: () => void } {
    let value = 1_000;
    return { now: () => value, tick: () => void (value += 1) };
}

describe("thumb keys", () => {
    it("carry device scope, kind, id and fingerprint", () => {
        const key = thumbKey(SCOPE, "route", 12, "c123");
        expect(key).toContain("OBC-0042");
        expect(key).toContain(":7:route:12:c123");
        expect(thumbKey({ ...SCOPE, epoch: 8 }, "route", 12, "c123")).not.toBe(key);
    });

    it("uses the route CRC and does not invent a missing fingerprint", () => {
        expect(routeFingerprint({ crc32: 0xdeadbeef })).toBe(`c${0xdeadbeef}`);
        expect(routeFingerprint({ crc32: 0 })).toBeNull();
    });

    it("fingerprints an immutable ride by start time and length", () => {
        expect(rideFingerprint({ startTime: 1_700_000_000, byteLen: 4_242 })).toBe("t1700000000-l4242");
    });
});

describe("ThumbCache", () => {
    it("round-trips a track and rounds coordinates to six decimals", () => {
        const cache = new ThumbCache(recordedStorage().storage);
        const key = thumbKey(SCOPE, "route", 1, "c1");
        cache.put(key, TRACK);
        expect(cache.get(key)).toEqual([
            [47.995, 7.842001],
            [47.996, 7.843],
            [47.997, 7.844],
        ]);
    });

    it("removes corrupt entries so the caller can refetch", () => {
        const { storage, map } = recordedStorage();
        const cache = new ThumbCache(storage);
        const key = thumbKey(SCOPE, "route", 1, "c1");
        for (const junk of ["not json", "42", `{"at":"soon","track":[]}`, `{"at":1,"track":[[1]]}`]) {
            map.set(key, junk);
            expect(cache.get(key)).toBeNull();
            expect(map.has(key)).toBe(false);
        }
    });

    it("evicts the least recently used entry past the cap", () => {
        const { storage, map } = recordedStorage();
        const clock = ticker();
        const cache = new ThumbCache(storage, 3, clock.now);
        const keys = [1, 2, 3, 4].map((id) => thumbKey(SCOPE, "route", id, `c${id}`));
        for (const key of keys.slice(0, 3)) {
            cache.put(key, TRACK);
            clock.tick();
        }
        cache.get(keys[0]);
        clock.tick();
        cache.put(keys[3], TRACK);
        expect(map.has(keys[0])).toBe(true);
        expect(map.has(keys[1])).toBe(false);
        expect(map.has(keys[2])).toBe(true);
        expect(map.has(keys[3])).toBe(true);
    });

    it("treats storage quota failures as a missing optimization", () => {
        const { storage, map } = recordedStorage();
        const cache = new ThumbCache(
            {
                ...storage,
                set: (key, value) => {
                    if (map.size >= 1 && !map.has(key)) throw new DOMException("quota");
                    storage.set(key, value);
                },
            },
            1,
        );
        expect(() => cache.put(thumbKey(SCOPE, "route", 1, "c1"), TRACK)).not.toThrow();
        expect(() => cache.put(thumbKey(SCOPE, "route", 2, "c2"), TRACK)).not.toThrow();
    });
});

describe("DeviceThumbs", () => {
    it("fills strictly one at a time, in order, through the queue", async () => {
        const thumbs = new DeviceThumbs();
        const order: string[] = [];
        const queue = async <T>(op: () => Promise<T>): Promise<T> => {
            order.push("enter");
            const result = await op();
            order.push("leave");
            return result;
        };
        const requests = [1, 2, 3].map((id) =>
            request(id, async () => {
                order.push(`load ${id}`);
                return TRACK;
            }),
        );
        await thumbs.fill(SCOPE, requests, queue, new AbortController().signal);
        expect(order).toEqual(["enter", "load 1", "leave", "enter", "load 2", "leave", "enter", "load 3", "leave"]);
        expect(thumbs.get("route", 2)).not.toBeNull();
    });

    it("reuses a track within the session but refetches after a reload", async () => {
        const first = new DeviceThumbs();
        let loads = 0;
        const load = async (): Promise<Thumb> => {
            loads += 1;
            return TRACK;
        };
        const queue = <T>(op: () => Promise<T>) => op();
        const signal = new AbortController().signal;
        await first.fill(SCOPE, [request(1, load)], queue, signal);
        await first.fill(SCOPE, [request(1, load)], queue, signal);
        expect(loads).toBe(1);

        // A fresh store models a reload: no device-derived coordinates survive it.
        const second = new DeviceThumbs();
        await second.fill(SCOPE, [request(1, load)], queue, signal);
        expect(loads).toBe(2);
    });

    it("persists across a desktop restart but never across a web reload", async () => {
        const storage = memoryStorage();
        const queue = <T>(op: () => Promise<T>) => op();
        const signal = new AbortController().signal;
        let loads = 0;
        const load = async (): Promise<Thumb> => {
            loads += 1;
            return TRACK;
        };

        const desktopFirst = new DeviceThumbs(thumbnailStorageFor("desktop", storage));
        await desktopFirst.fill(SCOPE, [request(1, load)], queue, signal);
        const desktopRestart = new DeviceThumbs(thumbnailStorageFor("desktop", storage));
        await desktopRestart.fill(SCOPE, [request(1, load)], queue, signal);
        expect(loads).toBe(1);

        const webReload = new DeviceThumbs(thumbnailStorageFor("web", storage));
        await webReload.fill(SCOPE, [request(1, load)], queue, signal);
        expect(loads).toBe(2);
    });

    it("refetches an edited object in the same session and after restart", async () => {
        const storage = memoryStorage();
        const thumbs = new DeviceThumbs(storage);
        const queue = <T>(op: () => Promise<T>) => op();
        const signal = new AbortController().signal;
        let loads = 0;
        const load = async (): Promise<Thumb> => {
            loads += 1;
            return TRACK;
        };

        await thumbs.fill(SCOPE, [request(1, load)], queue, signal);
        await thumbs.fill(SCOPE, [{ ...request(1, load), fingerprint: "c-edited" }], queue, signal);
        const restarted = new DeviceThumbs(storage);
        await restarted.fill(SCOPE, [{ ...request(1, load), fingerprint: "c-edited" }], queue, signal);
        expect(loads).toBe(2);
    });

    it("can delete every durable preview without touching unrelated storage", async () => {
        const map = new Map<string, string>();
        const storage: ThumbStorage = {
            get: (key) => map.get(key) ?? null,
            set: (key, value) => void map.set(key, value),
            remove: (key) => void map.delete(key),
            keys: () => [...map.keys()],
        };
        map.set("unrelated", "keep");
        const thumbs = new DeviceThumbs(storage);
        await thumbs.fill(
            SCOPE,
            [request(1, async () => TRACK), request(2, async () => TRACK)],
            (op) => op(),
            new AbortController().signal,
        );
        expect(thumbs.clearPersistent()).toBe(2);
        expect(map).toEqual(new Map([["unrelated", "keep"]]));
    });

    it("downsamples an over-long load to the preview cap", async () => {
        const thumbs = new DeviceThumbs();
        const long: Thumb = Array.from({ length: 3 * PREVIEW_POINTS }, (_, i) => [47 + i * 1e-5, 7.8]);
        await thumbs.fill(SCOPE, [request(1, async () => long)], (op) => op(), new AbortController().signal);
        expect(thumbs.get("route", 1)!.length).toBeLessThanOrEqual(PREVIEW_POINTS);
    });

    it("stops the walk on abort, and skips a failed load", async () => {
        const thumbs = new DeviceThumbs();
        const controller = new AbortController();
        const loaded: number[] = [];
        const requests = [
            request(1, async () => {
                throw new Error("cable hiccup");
            }),
            request(2, async () => {
                loaded.push(2);
                return TRACK;
            }),
            request(3, async () => {
                controller.abort();
                throw new Error("unplugged");
            }),
            request(4, async () => {
                loaded.push(4);
                return TRACK;
            }),
        ];
        await thumbs.fill(SCOPE, requests, (op) => op(), controller.signal);
        expect(loaded).toEqual([2]);
        expect(thumbs.get("route", 1)).toBeNull();
        expect(thumbs.get("route", 4)).toBeNull();
    });

    it("drops a completion that crossed an abort and scope change", async () => {
        const thumbs = new DeviceThumbs();
        const scopeB: RideScope = { serial: "OBC-0042", epoch: 8 };
        let resolveA: ((track: Thumb) => void) | undefined;
        const pendingA = new Promise<Thumb>((resolve) => (resolveA = resolve));
        const aborter = new AbortController();

        const fillA = thumbs.fill(SCOPE, [request(1, () => pendingA)], (op) => op(), aborter.signal);
        aborter.abort();
        thumbs.ensureScope(scopeB);
        resolveA!(TRACK);
        await fillA;
        expect(thumbs.get("route", 1)).toBeNull();

        let loadsB = 0;
        await thumbs.fill(
            scopeB,
            [
                request(1, async () => {
                    loadsB += 1;
                    return TRACK;
                }),
            ],
            (op) => op(),
            new AbortController().signal,
        );
        expect(loadsB).toBe(1);
        expect(thumbs.get("route", 1)).not.toBeNull();
    });

    it("drops a completion that crossed a plain abort", async () => {
        const thumbs = new DeviceThumbs();
        const aborter = new AbortController();
        let resolveLoad: ((track: Thumb) => void) | undefined;
        const pending = new Promise<Thumb>((resolve) => (resolveLoad = resolve));
        const fill = thumbs.fill(SCOPE, [request(1, () => pending)], (op) => op(), aborter.signal);
        aborter.abort();
        resolveLoad!(TRACK);
        await fill;
        expect(thumbs.get("route", 1)).toBeNull();
    });

    it("forgets the in-memory map on a scope change", async () => {
        const thumbs = new DeviceThumbs();
        await thumbs.fill(SCOPE, [request(1, async () => TRACK)], (op) => op(), new AbortController().signal);
        expect(thumbs.get("route", 1)).not.toBeNull();
        thumbs.ensureScope({ serial: "OBC-0042", epoch: 8 });
        expect(thumbs.get("route", 1)).toBeNull();
    });
});
