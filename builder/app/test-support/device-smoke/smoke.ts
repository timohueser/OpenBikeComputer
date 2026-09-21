/**
 * The device upload smoke: send one map over USB, reboot the board, and prove the device opened
 * what was sent.
 *
 * Everything here is transport-free on purpose. It drives the shipping {@link FlatStoreClient} and
 * the shipping `sendMapBlob` family — there is no second transfer client and no second idea of the
 * protocol — and it takes the two things it cannot do itself as functions: `connect`, which opens a
 * session on the device, and `reboot`, which restarts the board and returns what it printed while
 * booting. `main.ts` supplies the real ones; the suite supplies the wasm device and a scripted
 * board, which is why every failure path below is reachable without hardware.
 *
 * Transfer completion is not a result. The run only passes when the device's own answers agree with
 * the bytes that were sent — `PUT`'s commit, `STATUS`'s head, the catalog entry and a full `GET`
 * read back — and then, after a power-cycle of the software, when the firmware says at boot that it
 * opened that object at that revision and parsed that map's header and embedded terrain region.
 *
 * Every phase is bounded by its own deadline and nothing waits on a sleep: the run either observes
 * what it is waiting for or fails with the phase that ran out of time.
 */

import { DeviceError, type Catalog, type FlatStoreClient, type DeviceInfo } from "../../src/lib/usb/client";
import { Crc32 } from "../../src/lib/usb/crc32";
import { ObjectKind, ObjectState, type CatalogEntry, type PutResponse } from "../../src/lib/usb/protocol";
import { sendMapBytes } from "../../src/lib/device/write";
import type { JobContext, JobPhase } from "../../src/lib/device/progress";
import type { BoundingBox, SmokeFixture } from "./fixture";

/** The phases, in the order they run. A failure names exactly one of them. */
export type SmokePhase = "connect" | "upload" | "verify" | "reboot" | "reverify";

/**
 * Why a run failed.
 *
 * `refused` is the device saying no and carries its refusal; `integrity` is the device's answer
 * disagreeing with the bytes that were sent; `stale` is a device that came back holding, or
 * opening, something other than what was committed; `data-loss` is another object that stopped
 * being what it was; `board` is the probe tooling or the boot itself.
 */
export type SmokeFailureReason = "timeout" | "refused" | "integrity" | "stale" | "data-loss" | "link" | "board";

/** A run that did not pass, with the phase it failed in. */
export class SmokeFailure extends Error {
    constructor(
        readonly phase: SmokePhase,
        readonly reason: SmokeFailureReason,
        message: string,
        options?: { cause?: unknown },
    ) {
        super(message, options);
        this.name = "SmokeFailure";
    }
}

/** What the firmware printed at boot, as `parseBootLog` reads it off the RTT log. */
export interface BootObservation {
    /** The map object the firmware opened, and the revision of it. */
    readonly objectId: bigint;
    readonly revision: bigint;
    readonly payloadLength: bigint;
    /** The bounding box the firmware parsed out of that object's OBCM header. */
    readonly bbox: BoundingBox;
    /** The embedded terrain region the firmware mounted through the OBCT reader, in bytes. */
    readonly terrainBytes: number;
}

/** The deadlines, in milliseconds. Every one of them is a real bound on a real wait. */
export interface SmokeTimeouts {
    readonly connect: number;
    readonly upload: number;
    readonly verify: number;
    readonly reboot: number;
    readonly reverify: number;
}

/** Generous enough for a ten-megabyte object on a slow card, short enough to be a failure. */
export const DEFAULT_TIMEOUTS: SmokeTimeouts = {
    connect: 20_000,
    upload: 300_000,
    verify: 300_000,
    reboot: 120_000,
    reverify: 60_000,
};

/**
 * One connection to the device. `release` is the caller's, because the transport is: the run has to
 * let go of the cable before the board resets, and only the host that opened it knows what that
 * means.
 */
export interface DeviceSession {
    readonly client: FlatStoreClient;
    release(): Promise<void>;
}

export interface SmokeOptions {
    readonly fixture: SmokeFixture;
    /** Open a session on the device. Called once to upload and again after the reboot. */
    connect(signal: AbortSignal): Promise<DeviceSession>;
    /** Restart the board and return what the firmware printed while it came up. */
    reboot(signal: AbortSignal): Promise<BootObservation>;
    readonly timeouts?: Partial<SmokeTimeouts>;
    /** Called as each phase starts, and as bytes move inside it. For an operator watching. */
    onPhase?(phase: SmokePhase, note?: string): void;
    onProgress?(done: number, total: number): void;
}

/** The object reference the run committed and then proved, in the wire's own vocabulary. */
export interface CommittedObject {
    readonly storeId: string;
    readonly objectId: bigint;
    readonly revision: bigint;
    readonly kind: number;
    readonly payloadLength: bigint;
    readonly payloadCrc32: number;
    readonly displayName: string;
}

/** What one passing run observed. `main.ts` adds the build identities and writes it out. */
export interface SmokeReport {
    readonly device: DeviceInfo | null;
    readonly committed: CommittedObject;
    readonly boot: BootObservation;
    /** How long each phase took, in milliseconds. */
    readonly phaseMs: Readonly<Record<SmokePhase, number>>;
    /** The catalog entries that were on the card before the upload and still are, unchanged. */
    readonly preservedObjects: number;
}

/** Run every phase in order. Resolves only when the device agreed with all of it. */
export async function runSmoke(options: SmokeOptions): Promise<SmokeReport> {
    const timeouts = { ...DEFAULT_TIMEOUTS, ...options.timeouts };
    const { fixture } = options;
    const phaseMs: Record<string, number> = {};

    const phase = async <T>(name: SmokePhase, run: (signal: AbortSignal) => Promise<T>): Promise<T> => {
        options.onPhase?.(name);
        const started = Date.now();
        try {
            return await withDeadline(name, timeouts[name], run);
        } finally {
            phaseMs[name] = Date.now() - started;
        }
    };

    const expectedCrc = crc32Of(fixture.bytes);
    const expectedLength = BigInt(fixture.bytes.length);

    const opened = await phase("connect", async (signal) => {
        const session = await translate("connect", options.connect(signal));
        const client = session.client;
        return { session, device: await readDeviceInfo(client), before: await translate("connect", client.list({ signal })) };
    });
    const { device, before } = opened;
    const client = opened.session.client;
    let committed: CommittedObject;
    try {
        const put = await phase("upload", (signal) =>
            translate("upload", sendMapBytes(client, fixture.bytes, fixture.name, jobContext(signal, options))),
        );
        checkCommit(put, expectedLength, expectedCrc);
        committed = await phase("verify", (signal) =>
            verifyCommitted(client, put, fixture, expectedCrc, before, signal, options),
        );
    } finally {
        await opened.session.release();
    }

    // The cable goes down with the board, so the upload session is released before the reset.
    const boot = await phase("reboot", (signal) => translate("reboot", options.reboot(signal), "board"));
    checkBoot(boot, committed, fixture);

    await phase("reverify", async (signal) => {
        const session = await translate("reverify", options.connect(signal));
        try {
            const after = await translate("reverify", session.client.list({ signal }));
            if (after.storeId !== committed.storeId) {
                throw new SmokeFailure(
                    "reverify",
                    "stale",
                    `The device held store ${committed.storeId} before the reboot and ${after.storeId} after it.`,
                );
            }
            const entry = after.entries.find((candidate) => candidate.objectId === committed.objectId);
            if (!entry) {
                throw new SmokeFailure(
                    "reverify",
                    "data-loss",
                    `Object ${committed.objectId} is not in the catalog after the reboot.`,
                );
            }
            checkEntry("reverify", entry, committed);
        } finally {
            await session.release();
        }
    });

    return {
        device,
        committed,
        boot,
        phaseMs: phaseMs as Record<SmokePhase, number>,
        preservedObjects: before.entries.filter((entry) => entry.objectId !== committed.objectId).length,
    };
}

/**
 * Prove the committed object four ways: what `PUT` answered, what `STATUS` says the head is, what
 * the catalog lists, and what a whole `GET` reads back.
 *
 * The read-back is the only one of the four that is not the device repeating its own bookkeeping,
 * so it is not optional however long the object is.
 */
async function verifyCommitted(
    client: FlatStoreClient,
    put: PutResponse,
    fixture: SmokeFixture,
    expectedCrc: number,
    before: Catalog,
    signal: AbortSignal,
    options: SmokeOptions,
): Promise<CommittedObject> {
    const ref = { objectId: put.objectId, revision: put.revision };
    const status = await translate("verify", client.status(ref, signal));
    if (status.state !== ObjectState.Committed) {
        throw new SmokeFailure("verify", "integrity", `The device reports object state ${status.state}, not committed.`);
    }
    if (
        status.headRevision !== put.revision ||
        status.headPayloadLength !== put.payloadLength ||
        status.headPayloadCrc32 !== put.payloadCrc32
    ) {
        throw new SmokeFailure(
            "verify",
            "stale",
            `STATUS reports head revision ${status.headRevision}, ${status.headPayloadLength} B, ` +
                `CRC ${hex(status.headPayloadCrc32)}; PUT committed revision ${put.revision}, ` +
                `${put.payloadLength} B, CRC ${hex(put.payloadCrc32)}.`,
        );
    }

    const catalog = await translate("verify", client.list({ signal }));
    const entry = catalog.entries.find((candidate) => candidate.objectId === put.objectId);
    if (!entry) {
        throw new SmokeFailure("verify", "integrity", `The catalog does not list object ${put.objectId}.`);
    }
    const committed: CommittedObject = {
        storeId: catalog.storeId,
        objectId: entry.objectId,
        revision: entry.revision,
        kind: entry.kind,
        payloadLength: entry.payloadLength,
        payloadCrc32: entry.payloadCrc32,
        displayName: entry.displayName,
    };
    if (entry.kind !== ObjectKind.MapShard) {
        throw new SmokeFailure("verify", "integrity", `The catalog lists object ${entry.objectId} as kind ${entry.kind}.`);
    }
    checkEntry("verify", entry, {
        objectId: put.objectId,
        revision: put.revision,
        payloadLength: put.payloadLength,
        payloadCrc32: put.payloadCrc32,
    });
    checkPreserved(before, catalog, put.objectId);

    options.onPhase?.("verify", "reading the committed object back");
    const read = await translate(
        "verify",
        client.get(ref, {
            signal,
            expectedLength: fixture.bytes.length,
            onProgress: (done, total) => options.onProgress?.(done, total),
        }),
    );
    if (read.revisionServed !== put.revision) {
        throw new SmokeFailure(
            "verify",
            "stale",
            `GET served revision ${read.revisionServed} for the object committed at ${put.revision}.`,
        );
    }
    if (read.payloadCrc32 !== expectedCrc || !sameBytes(read.bytes, fixture.bytes)) {
        throw new SmokeFailure(
            "verify",
            "integrity",
            `The object read back is ${read.bytes.length} B / CRC ${hex(read.payloadCrc32)}; ` +
                `${fixture.bytes.length} B / CRC ${hex(expectedCrc)} was sent.`,
        );
    }
    return committed;
}

/** `PUT`'s own answer against the bytes the host holds — the first place a short object shows up. */
function checkCommit(put: PutResponse, expectedLength: bigint, expectedCrc: number): void {
    if (put.payloadLength !== expectedLength || put.payloadCrc32 !== expectedCrc) {
        throw new SmokeFailure(
            "upload",
            "integrity",
            `The device committed ${put.payloadLength} B / CRC ${hex(put.payloadCrc32)}; ` +
                `${expectedLength} B / CRC ${hex(expectedCrc)} was sent.`,
        );
    }
}

function checkEntry(
    phase: SmokePhase,
    entry: CatalogEntry,
    expected: Pick<CommittedObject, "objectId" | "revision" | "payloadLength" | "payloadCrc32">,
): void {
    if (
        entry.revision !== expected.revision ||
        entry.payloadLength !== expected.payloadLength ||
        entry.payloadCrc32 !== expected.payloadCrc32
    ) {
        throw new SmokeFailure(
            phase,
            "stale",
            `The catalog lists object ${entry.objectId} at revision ${entry.revision}, ` +
                `${entry.payloadLength} B, CRC ${hex(entry.payloadCrc32)}; revision ${expected.revision}, ` +
                `${expected.payloadLength} B, CRC ${hex(expected.payloadCrc32)} was committed.`,
        );
    }
}

/** Every object the card already held, other than the one replaced, is still exactly itself. */
function checkPreserved(before: Catalog, after: Catalog, replaced: bigint): void {
    for (const was of before.entries) {
        if (was.objectId === replaced) continue;
        const now = after.entries.find((candidate) => candidate.objectId === was.objectId);
        if (!now) {
            throw new SmokeFailure("verify", "data-loss", `Object ${was.objectId} was on the card and is gone.`);
        }
        if (now.revision !== was.revision || now.payloadLength !== was.payloadLength || now.payloadCrc32 !== was.payloadCrc32) {
            throw new SmokeFailure(
                "verify",
                "data-loss",
                `Object ${was.objectId} changed: revision ${was.revision} → ${now.revision}, ` +
                    `${was.payloadLength} B → ${now.payloadLength} B, ` +
                    `CRC ${hex(was.payloadCrc32)} → ${hex(now.payloadCrc32)}.`,
            );
        }
    }
}

/**
 * The boot log against the commit and against the file's own header.
 *
 * The object id and revision are what rule out a stale selection: a device that came up on the map
 * it held before the upload reports that map's identity. The header numbers are what make it a
 * read rather than a mount — the firmware only prints them after `MapTables::parse` and
 * `TerrainElevation::parse` have taken them off the card.
 */
function checkBoot(boot: BootObservation, committed: CommittedObject, fixture: SmokeFixture): void {
    if (boot.objectId !== committed.objectId || boot.revision !== committed.revision) {
        throw new SmokeFailure(
            "reboot",
            "stale",
            `The device opened object ${boot.objectId} revision ${boot.revision}; ` +
                `object ${committed.objectId} revision ${committed.revision} was committed.`,
        );
    }
    if (boot.payloadLength !== committed.payloadLength) {
        throw new SmokeFailure(
            "reboot",
            "integrity",
            `The device opened ${boot.payloadLength} B of a ${committed.payloadLength} B object.`,
        );
    }
    if (!sameBox(boot.bbox, fixture.bbox)) {
        throw new SmokeFailure(
            "reboot",
            "integrity",
            `The device read the box ${box(boot.bbox)} out of the map header; the file carries ${box(fixture.bbox)}.`,
        );
    }
    if (boot.terrainBytes !== fixture.terrainBytes) {
        throw new SmokeFailure(
            "reboot",
            "integrity",
            `The device mounted a ${boot.terrainBytes} B terrain region; the file carries ${fixture.terrainBytes} B.`,
        );
    }
}

/**
 * Read the three boot lines out of an RTT log.
 *
 * The firmware prints them once each, in this order, and only when the step behind each one
 * succeeded: the flat store opened the selected map object, the OBCM tables parsed, and the
 * embedded OBCT container parsed. A log missing any of them is a failed boot, not a slow one, so
 * the caller keeps waiting until its deadline rather than deciding here.
 */
export function parseBootLog(text: string): BootObservation | null {
    const open = /flat: map object (\d+) revision (\d+) open — (\d+) B/.exec(text);
    const bbox = /map: streaming from SD; bbox lon\[(-?\d+)\.\.(-?\d+)\] lat\[(-?\d+)\.\.(-?\d+)\]/.exec(text);
    const terrain = /map: terrain mounted from the §1\.3 region \((\d+) B\)/.exec(text);
    if (!open || !bbox || !terrain) return null;
    return {
        objectId: BigInt(open[1]),
        revision: BigInt(open[2]),
        payloadLength: BigInt(open[3]),
        bbox: {
            minLon: Number(bbox[1]),
            maxLon: Number(bbox[2]),
            minLat: Number(bbox[3]),
            maxLat: Number(bbox[4]),
        },
        terrainBytes: Number(terrain[1]),
    };
}

/**
 * The firmware's own word for a map it will not use: the catalog names an object that will not open,
 * or the object is not an OBCM file. Both print `MAP UNREADABLE`, and recognising it is what makes a
 * refused map fail on its reason instead of on the reboot deadline.
 */
export function bootFault(text: string): string | null {
    const fault = /^.*MAP UNREADABLE.*$/m.exec(text);
    return fault ? fault[0].trim() : null;
}

/**
 * Run `work` under a deadline of its own, and name the phase that ran out.
 *
 * The deadline both aborts the signal and releases the caller, because neither alone is enough: a
 * probe session or a USB enumeration that ignores the abort would hold the run forever, and a run
 * that only stopped waiting would leave a live transfer on the far end. What is left running after
 * a timeout is the operator's problem, and the run has already failed.
 */
async function withDeadline<T>(phase: SmokePhase, ms: number, work: (signal: AbortSignal) => Promise<T>): Promise<T> {
    const controller = new AbortController();
    const expiry = new SmokeFailure(phase, "timeout", `${phase} took longer than ${ms} ms.`);
    let timer: ReturnType<typeof setTimeout> | undefined;
    const deadline = new Promise<never>((_, reject) => {
        timer = setTimeout(() => {
            controller.abort(expiry);
            reject(expiry);
        }, ms);
    });
    try {
        return await Promise.race([work(controller.signal), deadline]);
    } catch (cause) {
        if (controller.signal.aborted && controller.signal.reason === expiry) throw expiry;
        throw cause;
    } finally {
        clearTimeout(timer);
        controller.abort();
    }
}

/** Turn a device refusal, a dead cable or a board failure into one that names the phase. */
async function translate<T>(phase: SmokePhase, work: Promise<T>, fallback: SmokeFailureReason = "link"): Promise<T> {
    try {
        return await work;
    } catch (cause) {
        if (cause instanceof SmokeFailure) throw cause;
        if (cause instanceof DeviceError) {
            const refusal = cause.refusal ? ` (detail ${cause.refusal.detail}, context ${cause.refusal.context})` : "";
            throw new SmokeFailure(phase, cause.code === "link" ? "link" : "refused", `${cause.message}${refusal}`, { cause });
        }
        throw new SmokeFailure(phase, fallback, describe(cause), { cause });
    }
}

/** The identity strings, when the transport can read them. A host that cannot is not a failure. */
async function readDeviceInfo(client: FlatStoreClient): Promise<DeviceInfo | null> {
    try {
        return await client.deviceInfo();
    } catch {
        return null;
    }
}

/** The shipping send path reports itself through this; the run only needs its cancellation. */
function jobContext(signal: AbortSignal, options: SmokeOptions): JobContext {
    return {
        signal,
        cancel: () => {},
        phase: (name: JobPhase) => options.onPhase?.("upload", name),
        progress: (done, total) => options.onProgress?.(done, total),
    };
}

function crc32Of(bytes: Uint8Array): number {
    const crc = new Crc32();
    crc.update(bytes);
    return crc.value();
}

function sameBytes(a: Uint8Array, b: Uint8Array): boolean {
    if (a.length !== b.length) return false;
    for (let at = 0; at < a.length; at += 1) if (a[at] !== b[at]) return false;
    return true;
}

function sameBox(a: BoundingBox, b: BoundingBox): boolean {
    return a.minLat === b.minLat && a.minLon === b.minLon && a.maxLat === b.maxLat && a.maxLon === b.maxLon;
}

function box(value: BoundingBox): string {
    return `lon[${value.minLon}..${value.maxLon}] lat[${value.minLat}..${value.maxLat}]`;
}

function hex(value: number): string {
    return `0x${(value >>> 0).toString(16).padStart(8, "0")}`;
}

function describe(cause: unknown): string {
    return cause instanceof Error ? cause.message : String(cause);
}
