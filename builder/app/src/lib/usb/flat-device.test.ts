/**
 * The round trip that proves the real device is wired up: a formatted card with nothing on it, then
 * one object pushed through `PUT` and read back through `GET`.
 *
 * Everything else about the protocol is tested by the suites that drive this device (and by
 * `firmware/obc-link`'s own, on the same engine and the same card). What is proved *here* is the
 * seam: the wasm module loads, records cross the loopback in both directions, the store assigns an
 * id, and the bytes come back byte-for-byte.
 */

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it } from "vitest";

import { flatDevice, initFlatDevice } from "./flat-device";
import { ObjectKind } from "./protocol";

beforeAll(async () => {
    const wasm = join(
        dirname(fileURLToPath(import.meta.url)),
        "../../../test-support/flat-device/pkg/obc_flat_device_bg.wasm",
    );
    await initFlatDevice(readFileSync(wasm));
});

describe("the flat device", () => {
    it("lists an empty formatted card", async () => {
        const rig = flatDevice();
        try {
            const catalog = await rig.client.list();
            expect(catalog.entries).toEqual([]);
            expect(catalog.storeId).toBe(rig.device.storeId);
            expect(catalog.commitSequence).toBe(rig.device.sequence);
            expect(rig.device.faults).toEqual([]);
        } finally {
            await rig.close();
        }
    });

    it("takes an object through PUT and serves the same bytes back", async () => {
        const rig = flatDevice();
        const bytes = new Uint8Array(20_000).map((_, at) => (at * 7 + 11) & 0xff);
        try {
            const put = await rig.client.put({ kind: ObjectKind.Route, displayName: "a route" }, bytes);
            expect(put.objectId).toBe(1n);
            expect(put.revision).toBe(1n);

            const got = await rig.client.get({ objectId: put.objectId, revision: 0n });
            expect(got.bytes).toEqual(bytes);

            const catalog = await rig.client.list();
            expect(catalog.entries.map((entry) => entry.displayName)).toEqual(["a route"]);
            expect(rig.device.payloadOf(put.objectId)).toEqual(bytes);
            expect(rig.device.faults).toEqual([]);
        } finally {
            await rig.close();
        }
    });
});
