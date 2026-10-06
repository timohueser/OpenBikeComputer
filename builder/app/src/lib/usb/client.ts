/** The shared Rust store client over browser, desktop and loopback byte channels. */

import { ClientIO, type NativeOutcome } from "./client-io";
import { DeviceError, asDeviceError } from "./errors";
import { bytesSource, type ObjectSource } from "./source";
import type { DeviceLink } from "./pipe";
import { DEVICE_INFO_MAX, GET_DEVICE_INFO, decodeDeviceInfo, type DeviceInfo } from "./records";
import { NO_OBJECT, Opcode, hex, toSafeNumber, type ObjectRef, type ObjectKind,
    type ListPage, type CatalogEntry, type StatusResponse, type GetResponse, type PutResponse,
    type ArmResponse, type FormatResponse } from "./protocol";

export { DEFAULT_BATCH_BYTES, UPLOAD_WINDOW } from "./client-io";
export { blobSource, bytesSource } from "./source";
export type { ObjectSource } from "./source";
export { DeviceError, asDeviceError, refusalError, isFormatRecoveryState } from "./errors";
export type { DeviceErrorCode } from "./errors";
export type { DeviceInfo };

export const ZERO_STORE_ID = "00000000000000000000000000000000";

function mintStoreId(avoid: string): string {
    for (;;) {
        const bytes = new Uint8Array(16);
        globalThis.crypto.getRandomValues(bytes);
        const id = hex(bytes);
        if (id !== ZERO_STORE_ID && id !== avoid) return id;
    }
}

/**
 * How long to wait for a device answer before giving up. 15 s, a chosen value: a device that has
 * stopped answering must produce an error, never a spinner that outlives the ride. Progress refreshes the budget, so it bounds a stalled step rather than a whole transfer.
 */
export const DEFAULT_TIMEOUT_MS = 15_000;

/** Per-call knobs shared by both transfer directions. */
export interface TransferOptions {
    /** Cancel the transfer. A cancelled transfer sends `CANCEL` before it unwinds. */
    signal?: AbortSignal;
    /** Called as bytes move, for a progress bar. `done` and `total` are byte counts. */
    onProgress?: (done: number, total: number) => void;
    /** Override {@link DEFAULT_BATCH_BYTES} for one upload. Rounded up to whole stream records. */
    batchBytes?: number;
    /**
     * The payload length the caller already knows, used only to give a download's progress bar a
     * denominator. Without it a 300 MB map sits at zero for twenty minutes and then jumps. It never
     * decides when to stop reading or what to verify: that is always the device's own answer.
     */
    expectedLength?: number;
    /**
     * Called once the last byte has been handed to the transport, before the wait for the device's
     * verdict. That gap is invisible from the outside — the bar is at 100 %, nothing is moving — so
     * a caller that shows progress wants to say what is happening.
     */
    onSent?: () => void;
}

/** What a client asks a `PUT` to publish. The payload's length and CRC come from the source. */
export interface PutTarget {
    /** {@link NO_OBJECT} creates a new object; anything else replaces that one. */
    objectId?: bigint;
    /** The revision the device last reported for `objectId`. Omitted (zero) when creating. */
    expectedRevision?: bigint;
    kind: ObjectKind;
    /** Up to 48 UTF-8 bytes. The caller trims; this refuses a longer one. */
    displayName: string;
}

/** A downloaded object: its bytes, and what the device said it served. */
export interface GetResult extends GetResponse {
    readonly bytes: Uint8Array;
}

/** A whole catalog: the identity prefix and every page's entries, concatenated. */
export interface Catalog {
    readonly storeId: string;
    /** The sequence every page of this listing agreed on. A movement changes it. */
    readonly commitSequence: bigint;
    readonly entries: readonly CatalogEntry[];
}

/** Options for a {@link FlatStoreClient}. */
export interface ClientOptions {
    /** Bound on every wait for a device answer. Defaults to {@link DEFAULT_TIMEOUT_MS}. */
    timeoutMs?: number;
}

const WIDE_FIELDS = new Set(["objectId", "revision", "revisionServed", "payloadLength", "headRevision",
    "headPayloadLength", "commitSequence", "rollbackObjectId"]);

function body<T>(outcome: NativeOutcome): T {
    const { kind: _kind, ...value } = outcome;
    for (const key of WIDE_FIELDS) if (typeof value[key] === "string") value[key] = BigInt(value[key] as string);
    if (Array.isArray(value.entries)) value.entries = value.entries.map((entry) => {
        const copy = { ...entry } as Record<string, unknown>;
        for (const key of WIDE_FIELDS) if (typeof copy[key] === "string") copy[key] = BigInt(copy[key] as string);
        return copy;
    });
    return value as T;
}

export class FlatStoreClient {
    private readonly io: ClientIO;

    constructor(private readonly link: DeviceLink, options: ClientOptions = {}) {
        this.io = new ClientIO(link, options.timeoutMs ?? DEFAULT_TIMEOUT_MS);
    }

    get liveTransfer(): number | null { return this.io.liveTransfer; }

    async deviceInfo(signal?: AbortSignal): Promise<DeviceInfo> {
        if (!this.link.vendorIn) throw new DeviceError("unavailable", "This host cannot read the device's firmware version: it cannot issue a USB control request.");
        try { return decodeDeviceInfo(await this.link.vendorIn(GET_DEVICE_INFO, 0, DEVICE_INFO_MAX, signal)); }
        catch (cause) { throw asDeviceError(cause); }
    }

    async listPage(request: { kind?: ObjectKind | null; cursor?: { objectId: bigint; revision: bigint; commitSequence: bigint } }, signal?: AbortSignal): Promise<ListPage> {
        const cursor = request.cursor;
        return body(await this.io.query({ op: "list", kind: request.kind ?? null,
            cursor: cursor ? { id: cursor.objectId.toString(), revision: cursor.revision.toString(), sequence: cursor.commitSequence.toString() } : null }, Opcode.List, signal));
    }

    async list(options: { kind?: ObjectKind; signal?: AbortSignal } = {}): Promise<Catalog> {
        return body(await this.io.query({ op: "catalog", kind: options.kind }, Opcode.List, options.signal));
    }

    async status(ref: ObjectRef, signal?: AbortSignal): Promise<StatusResponse> {
        return body(await this.io.query({ op: "status", id: ref.objectId.toString(), revision: ref.revision.toString() }, Opcode.Status, signal));
    }

    async findCreated(want: { kind: ObjectKind; payloadLength: bigint; payloadCrc32: number; displayName: string }, signal?: AbortSignal): Promise<CatalogEntry | null> {
        const catalog = await this.list({ kind: want.kind, signal });
        return catalog.entries.find((entry) => entry.kind === want.kind && entry.payloadLength === want.payloadLength &&
            entry.payloadCrc32 === want.payloadCrc32 && entry.displayName === want.displayName) ?? null;
    }

    async get(ref: ObjectRef, options: TransferOptions = {}): Promise<GetResult> {
        const result = body<GetResponse & { chunks: Uint8Array[] }>(await this.io.run({ op: "get", id: ref.objectId.toString(), revision: ref.revision.toString() }, Opcode.Get, options));
        const length = toSafeNumber(result.payloadLength, "the served payload length");
        const bytes = new Uint8Array(length);
        let at = 0;
        for (const chunk of result.chunks) { bytes.set(chunk, at); at += chunk.length; }
        const { chunks: _chunks, ...metadata } = result;
        options.onProgress?.(length, length);
        return { ...metadata, bytes };
    }

    async put(target: PutTarget, source: ObjectSource | Uint8Array, options: TransferOptions = {}): Promise<PutResponse> {
        const src = source instanceof Uint8Array ? bytesSource(source) : source;
        return body(await this.io.run({ op: "put", id: (target.objectId ?? NO_OBJECT).toString(), revision: (target.expectedRevision ?? 0n).toString(),
            length: BigInt(src.totalLen).toString(), crc: src.crc32 >>> 0, kind: target.kind, name: target.displayName }, Opcode.Put, options, src));
    }

    async remove(ref: ObjectRef, signal?: AbortSignal): Promise<bigint> {
        const result = body<{ commitSequence: bigint | null }>(await this.io.run({ op: "remove", id: ref.objectId.toString(), revision: ref.revision.toString() }, Opcode.Remove, { signal }));
        if (result.commitSequence === null) throw new DeviceError("protocol", "The removal needs a fresh catalog before its commit sequence is known.");
        return result.commitSequence;
    }

    async cancel(transferRequestId: number, signal?: AbortSignal): Promise<boolean> {
        return body<{ cancelled: boolean }>(await this.io.run({ op: "cancel", transfer: transferRequestId }, Opcode.Cancel, { signal })).cancelled;
    }

    async arm(ref: { objectId: bigint; expectedRevision: bigint }, signal?: AbortSignal): Promise<ArmResponse> {
        return body(await this.io.run({ op: "arm", id: ref.objectId.toString(), revision: ref.expectedRevision.toString() }, Opcode.Arm, { signal }));
    }

    async format(expectedStoreId: string | null, options: { signal?: AbortSignal; replacementStoreId?: string } = {}): Promise<FormatResponse> {
        const expected = expectedStoreId ?? ZERO_STORE_ID;
        const replacement = options.replacementStoreId ?? mintStoreId(expected);
        return body(await this.io.run({ op: "format", expected, replacement }, Opcode.Format, { signal: options.signal }));
    }

    close(): Promise<void> { return this.io.close(); }
}
