/**
 * The thumbnail cache's contract, tested against a Map: keying, LRU, the storage round-trip, and
 * the store's one-at-a-time fill. No DOM, no device — the loaders are functions and the queue is a
 * recorder, which is exactly the seam `Device.svelte` plugs the real cable into.
 */

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
    type Thumb,
    type ThumbRequest,
    type ThumbStorage,
} from "./thumbs.svelte";

const SCOPE: RideScope = { serial: "OBC-0042", epoch: 7 };

/** A Map-backed storage that also exposes the Map, for direct assertions. */
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

/** A clock the tests advance by hand, so LRU order is deterministic. */
function ticker(): { now: () => number; tick: () => void } {
    let t = 1_000;
    return { now: () => t, tick: () => void (t += 1) };
}

const TRACK: Thumb = [
    [47.9950001234, 7.8420009876],
    [47.996, 7.843],
    [47.997, 7.844],
];

describe("thumb keys", () => {
    it("carry serial, epoch, kind, id and fingerprint — all five, in the key", () => {
        const key = thumbKey(SCOPE, "route", 12, "c123");
        expect(key).toContain("OBC-0042");
        expect(key).toContain(":7:");
        expect(key).toContain(":route:");
        expect(key).toContain(":12:");
        expect(key).toContain(":c123");
        // A different epoch is a different card era — never the same entry.
        expect(thumbKey({ ...SCOPE, epoch: 8 }, "route", 12, "c123")).not.toBe(key);
        expect(thumbKey({ ...SCOPE, epoch: null }, "route", 12, "c123")).toContain("no-store");
    });

    it("fingerprints a route by CRC, falling back to length only when the CRC is absent", () => {
        expect(routeFingerprint({ crc32: 0xdeadbeef, byteLen: 100 })).toBe(`c${0xdeadbeef}`);
        expect(routeFingerprint({ crc32: 0, byteLen: 100 })).toBe("l100");
    });

    it("fingerprints a ride by start time and length", () => {
        expect(rideFingerprint({ startTime: 1_700_000_000, byteLen: 4_242 })).toBe("t1700000000-l4242");
    });
});

describe("ThumbCache", () => {
    it("round-trips a track, rounded to six decimals on the way in", () => {
        const { storage } = recordedStorage();
        const cache = new ThumbCache(storage);
        const key = thumbKey(SCOPE, "route", 1, "c1");
        cache.put(key, TRACK);
        expect(cache.get(key)).toEqual([
            [47.995, 7.842001],
            [47.996, 7.843],
            [47.997, 7.844],
        ]);
    });

    it("answers null for an absent entry", () => {
        const cache = new ThumbCache(recordedStorage().storage);
        expect(cache.get(thumbKey(SCOPE, "ride", 9, "t0-l0"))).toBeNull();
    });

    it("removes a corrupt entry and answers null, so the caller just refetches", () => {
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
        // Reading key 1 freshens it, so key 2 is now the oldest.
        cache.get(keys[0]);
        clock.tick();
        cache.put(keys[3], TRACK);
        expect(map.has(keys[0])).toBe(true);
        expect(map.has(keys[1])).toBe(false);
        expect(map.has(keys[2])).toBe(true);
        expect(map.has(keys[3])).toBe(true);
    });

    it("survives a quota-throwing storage by evicting and retrying once", () => {
        const { storage, map } = recordedStorage();
        const clock = ticker();
        // A storage with room for two entries: the third `set` throws, like a full origin.
        const throwing: ThumbStorage = {
            ...storage,
            set: (key, value) => {
                if (map.size >= 2 && !map.has(key)) throw new DOMException("quota");
                storage.set(key, value);
            },
        };
        const cache = new ThumbCache(throwing, 2, clock.now);
        cache.put(thumbKey(SCOPE, "route", 2, "c2"), TRACK);
        clock.tick();
        cache.put(thumbKey(SCOPE, "route", 3, "c3"), TRACK);
        clock.tick();
        // The third put hits "quota", evicts, and lands on the retry — the caller never throws.
        cache.put(thumbKey(SCOPE, "route", 4, "c4"), TRACK);
        expect(map.has(thumbKey(SCOPE, "route", 4, "c4"))).toBe(true);
        expect(map.size).toBeLessThanOrEqual(2);
    });

    it("never throws even when the storage refuses everything", () => {
        const cache = new ThumbCache({
            get: () => null,
            set: () => {
                throw new DOMException("quota");
            },
            remove: () => undefined,
            keys: () => [],
        });
        expect(() => cache.put(thumbKey(SCOPE, "route", 1, "c1"), TRACK)).not.toThrow();
    });
});

describe("DeviceThumbs", () => {
    const request = (id: number, load: ThumbRequest["load"]): ThumbRequest => ({
        kind: "route",
        id,
        fingerprint: `c${id}`,
        load,
    });

    it("fills strictly one at a time, in order, through the queue", async () => {
        const thumbs = new DeviceThumbs(recordedStorage().storage);
        const order: string[] = [];
        const queue = async <T,>(op: () => Promise<T>): Promise<T> => {
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
        expect(order).toEqual([
            "enter", "load 1", "leave",
            "enter", "load 2", "leave",
            "enter", "load 3", "leave",
        ]);
        expect(thumbs.get("route", 2)).not.toBeNull();
    });

    it("skips entries the cache already holds — one download per object, ever", async () => {
        const { storage } = recordedStorage();
        const first = new DeviceThumbs(storage);
        let loads = 0;
        const load = async (): Promise<Thumb> => {
            loads += 1;
            return TRACK;
        };
        const queue = <T,>(op: () => Promise<T>) => op();
        const signal = new AbortController().signal;
        await first.fill(SCOPE, [request(1, load)], queue, signal);
        expect(loads).toBe(1);

        // A fresh store over the same storage — a page reload — finds the cache, not the cable.
        const second = new DeviceThumbs(storage);
        await second.fill(SCOPE, [request(1, load)], queue, signal);
        expect(loads).toBe(1);
        expect(second.get("route", 1)).toEqual(first.get("route", 1));
    });

    it("refetches when the fingerprint moved — an edited route is a different track", async () => {
        const { storage } = recordedStorage();
        const thumbs = new DeviceThumbs(storage);
        let loads = 0;
        const load = async (): Promise<Thumb> => {
            loads += 1;
            return TRACK;
        };
        const queue = <T,>(op: () => Promise<T>) => op();
        const signal = new AbortController().signal;
        await thumbs.fill(SCOPE, [request(1, load)], queue, signal);
        const edited = new DeviceThumbs(storage);
        await edited.fill(SCOPE, [{ ...request(1, load), fingerprint: "c-moved" }], queue, signal);
        expect(loads).toBe(2);
    });

    it("downsamples an over-long load to the preview cap", async () => {
        const thumbs = new DeviceThumbs(recordedStorage().storage);
        const long: Thumb = Array.from({ length: 3 * PREVIEW_POINTS }, (_, i) => [47 + i * 1e-5, 7.8]);
        await thumbs.fill(
            SCOPE,
            [request(1, async () => long)],
            (op) => op(),
            new AbortController().signal,
        );
        expect(thumbs.get("route", 1)!.length).toBeLessThanOrEqual(PREVIEW_POINTS);
    });

    it("stops the walk on abort, and skips (not stops) on a failed load", async () => {
        const thumbs = new DeviceThumbs(recordedStorage().storage);
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

    it("forgets the in-memory map on a scope change — ids are recycled across cards", async () => {
        const { storage } = recordedStorage();
        const thumbs = new DeviceThumbs(storage);
        const queue = <T,>(op: () => Promise<T>) => op();
        const signal = new AbortController().signal;
        await thumbs.fill(SCOPE, [request(1, async () => TRACK)], queue, signal);
        expect(thumbs.get("route", 1)).not.toBeNull();
        thumbs.ensureScope({ serial: "OBC-0042", epoch: 8 });
        expect(thumbs.get("route", 1)).toBeNull();
    });

    it("memoryStorage wears the interface", () => {
        const storage = memoryStorage();
        storage.set("a", "1");
        expect(storage.get("a")).toBe("1");
        expect(storage.keys()).toEqual(["a"]);
        storage.remove("a");
        expect(storage.get("a")).toBeNull();
    });
});
