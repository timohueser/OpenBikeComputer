/** The session-only thumbnail store, with device I/O represented by loader functions. */

import { describe, expect, it } from "vitest";

import { PREVIEW_POINTS } from "./library";
import type { RideScope } from "./rides";
import { DeviceThumbs, type Thumb, type ThumbRequest } from "./thumbs.svelte";

const SCOPE: RideScope = { serial: "OBC-0042", epoch: 7 };
const TRACK: Thumb = [
    [47.9950001234, 7.8420009876],
    [47.996, 7.843],
    [47.997, 7.844],
];

const request = (id: number, load: ThumbRequest["load"]): ThumbRequest => ({ kind: "route", id, load });

describe("DeviceThumbs", () => {
    it("fills strictly one at a time, in order, through the queue", async () => {
        const thumbs = new DeviceThumbs();
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

    it("reuses a track within the session but refetches after a reload", async () => {
        const first = new DeviceThumbs();
        let loads = 0;
        const load = async (): Promise<Thumb> => {
            loads += 1;
            return TRACK;
        };
        const queue = <T,>(op: () => Promise<T>) => op();
        const signal = new AbortController().signal;
        await first.fill(SCOPE, [request(1, load)], queue, signal);
        await first.fill(SCOPE, [request(1, load)], queue, signal);
        expect(loads).toBe(1);

        // A fresh store models a reload: no device-derived coordinates survive it.
        const second = new DeviceThumbs();
        await second.fill(SCOPE, [request(1, load)], queue, signal);
        expect(loads).toBe(2);
    });

    it("downsamples an over-long load to the preview cap", async () => {
        const thumbs = new DeviceThumbs();
        const long: Thumb = Array.from({ length: 3 * PREVIEW_POINTS }, (_, i) => [47 + i * 1e-5, 7.8]);
        await thumbs.fill(
            SCOPE,
            [request(1, async () => long)],
            (op) => op(),
            new AbortController().signal,
        );
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
