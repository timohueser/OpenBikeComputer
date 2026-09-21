/**
 * The smoke's failure paths, over the real flat engine.
 *
 * The device here is `obc_link::flat::Engine` on a simulated card, the same assembly the firmware's
 * own Rust suites run on, reached through the same record framing the cable carries. So a refusal is
 * a real refusal and a commit is a real commit; what is scripted is the board, because a reboot and
 * an RTT log are the two things a host cannot produce.
 *
 * The payload is a whole number of 512-byte packets, which is the boundary case the run exists for.
 */

import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { flatDevice, type FlatDevice } from "../../src/lib/usb/flat-device";
import { ObjectKind } from "../../src/lib/usb/protocol";
import { loadFlatDevice } from "../flat-device/load";
import { PACKET_BYTES, loadSmokeFixture, type SmokeFixture } from "./fixture";
import { SmokeFailure, bootFault, parseBootLog, runSmoke, type BootObservation, type DeviceSession } from "./smoke";

const REPO_ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "..");

/** Nine whole packets of content-shaped bytes: long enough to span records, exact on the boundary. */
const PAYLOAD = Uint8Array.from({ length: PACKET_BYTES * 9 }, (_, at) => (at * 31 + 7) & 0xff);

const FIXTURE: SmokeFixture = {
    name: "smoke.obcm",
    bytes: PAYLOAD,
    sha256: "",
    producerCommit: "",
    bbox: { minLat: 46_369_864, minLon: 7_392_879, maxLat: 46_695_523, maxLon: 8_422_373 },
    terrainBytes: 7_807_488,
};

/** The three lines the firmware prints at boot, as probe-rs writes them to the RTT log. */
function bootLog(observation: { objectId: bigint; revision: bigint; length: bigint }): string {
    const { bbox, terrainBytes } = FIXTURE;
    return [
        "0.512345 INFO  flat: catalog holds 1 map object(s), 0 route(s), 0 ride(s), 0 other",
        `0.601234 INFO  flat: map object ${observation.objectId} revision ${observation.revision} ` +
            `open — ${observation.length} B, read direct (no channel)`,
        `0.712345 INFO  map: streaming from SD; bbox lon[${bbox.minLon}..${bbox.maxLon}] ` +
            `lat[${bbox.minLat}..${bbox.maxLat}]`,
        `0.804321 INFO  map: terrain mounted from the §1.3 region (${terrainBytes} B)`,
    ].join("\n");
}

const open: Array<() => Promise<void>> = [];

/** A device that lives until the test ends, and the `connect` that opens sessions on it. */
function device(options: Parameters<typeof flatDevice>[0] = {}): {
    device: FlatDevice;
    connect: () => Promise<DeviceSession>;
} {
    const started = flatDevice(options);
    open.push(started.close);
    // One simulated card serves both sessions, so releasing a session leaves the link up. On the
    // cable it is the opposite: the board takes the link down with it.
    return { device: started.device, connect: async () => ({ client: started.client, release: async () => {} }) };
}

afterEach(async () => {
    for (const close of open.splice(0)) await close();
});

beforeAll(async () => {
    await loadFlatDevice();
});

describe("device upload smoke", () => {
    it("commits the boundary payload and proves it again after the reboot", async () => {
        const card = device();
        const report = await runSmoke({
            fixture: FIXTURE,
            connect: card.connect,
            reboot: async () => observationOf(card.device),
        });

        expect(report.committed.kind).toBe(ObjectKind.MapShard);
        expect(report.committed.payloadLength).toBe(BigInt(PAYLOAD.length));
        expect(report.committed.displayName).toBe("smoke");
        expect(card.device.payloadOf(report.committed.objectId, report.committed.revision)).toEqual(PAYLOAD);
        expect(report.boot.objectId).toBe(report.committed.objectId);
        expect(report.boot.revision).toBe(report.committed.revision);
        expect(report.phaseMs.reverify).toBeGreaterThanOrEqual(0);
    });

    it("fails the phase that ran out of time", async () => {
        const failure = await rejection(
            runSmoke({
                fixture: FIXTURE,
                connect: () => new Promise(() => {}),
                reboot: async () => {
                    throw new Error("unreachable");
                },
                timeouts: { connect: 20 },
            }),
        );
        expect(failure.phase).toBe("connect");
        expect(failure.reason).toBe("timeout");
    });

    it("fails on the device's own refusal rather than on a deadline", async () => {
        // One extent, already spent on a seeded object: the map has nowhere to go.
        const card = device({ extents: 1 });
        card.device.seed({ kind: ObjectKind.Route, displayName: "route", bytes: new Uint8Array(64) });
        const failure = await rejection(
            runSmoke({
                fixture: FIXTURE,
                connect: card.connect,
                reboot: async () => {
                    throw new Error("unreachable");
                },
            }),
        );
        expect(failure.phase).toBe("upload");
        expect(failure.reason).toBe("refused");
    });

    it("fails when the device boots on the revision the upload replaced", async () => {
        const card = device();
        const replaced = card.device.seed({ kind: ObjectKind.MapShard, displayName: "old", bytes: new Uint8Array(512) });
        const failure = await rejection(
            runSmoke({
                fixture: FIXTURE,
                connect: card.connect,
                reboot: async () =>
                    boot({ objectId: replaced.objectId, revision: replaced.revision, length: 512n }),
            }),
        );
        expect(failure.phase).toBe("reboot");
        expect(failure.reason).toBe("stale");
    });

    it("fails when the device mounts a terrain region the map does not carry", async () => {
        const card = device();
        const failure = await rejection(
            runSmoke({
                fixture: FIXTURE,
                connect: card.connect,
                reboot: async () => ({ ...(await observationOf(card.device)), terrainBytes: 4096 }),
            }),
        );
        expect(failure.phase).toBe("reboot");
        expect(failure.reason).toBe("integrity");
    });

    it("fails when the object is gone from the catalog after the reboot", async () => {
        const card = device();
        const fresh = device();
        let session = 0;
        const failure = await rejection(
            runSmoke({
                fixture: FIXTURE,
                connect: () => (session++ === 0 ? card.connect() : fresh.connect()),
                reboot: async () => observationOf(card.device),
            }),
        );
        expect(failure.phase).toBe("reverify");
        expect(failure.reason).toBe("data-loss");
    });
});

describe("boot log", () => {
    it("reads the object, the map header and the terrain region out of one log", () => {
        const observed = parseBootLog(bootLog({ objectId: 7n, revision: 3n, length: 4608n }));
        expect(observed).toEqual({
            objectId: 7n,
            revision: 3n,
            payloadLength: 4608n,
            bbox: FIXTURE.bbox,
            terrainBytes: FIXTURE.terrainBytes,
        });
    });

    it("is not readable until every line is there", () => {
        const lines = bootLog({ objectId: 7n, revision: 3n, length: 4608n }).split("\n");
        expect(parseBootLog(lines.slice(0, 3).join("\n"))).toBeNull();
    });

    it("names the firmware's own refusal to use the map", () => {
        const refusal = "0.4 ERROR map: not valid OBCM: Magic — showing MAP UNREADABLE with USB recovery";
        expect(bootFault(`0.1 INFO  flat: mounted\n${refusal}\n`)).toBe(refusal);
        expect(bootFault(bootLog({ objectId: 1n, revision: 1n, length: 4608n }))).toBeNull();
    });
});

describe("the shipped map", () => {
    it("is the file its provenance record pins and lands on the packet boundary", () => {
        const fixture = loadSmokeFixture(REPO_ROOT);
        expect(fixture.bytes.length % PACKET_BYTES).toBe(0);
        expect(fixture.terrainBytes % PACKET_BYTES).toBe(0);
        expect(fixture.bbox.maxLat).toBeGreaterThan(fixture.bbox.minLat);
    });
});

/** The boot the board would report for whatever the card now holds as its map. */
async function observationOf(card: FlatDevice): Promise<BootObservation> {
    const entry = card.entries
        .filter((candidate) => candidate.kind === ObjectKind.MapShard)
        .reduce((best, candidate) => (candidate.objectId < best.objectId ? candidate : best));
    return boot({ objectId: entry.objectId, revision: entry.revision, length: entry.payloadLength });
}

function boot(observation: { objectId: bigint; revision: bigint; length: bigint }): BootObservation {
    const parsed = parseBootLog(bootLog(observation));
    if (!parsed) throw new Error("the scripted boot log does not parse");
    return parsed;
}

async function rejection(work: Promise<unknown>): Promise<SmokeFailure> {
    try {
        await work;
    } catch (cause) {
        if (cause instanceof SmokeFailure) return cause;
        throw cause;
    }
    throw new Error("the run passed");
}
