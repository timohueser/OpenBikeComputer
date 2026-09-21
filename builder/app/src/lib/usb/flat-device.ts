/**
 * The real device, on the other end of a {@link DeviceLink}.
 *
 * `obc_link::flat::Engine` over `obc_storage::flat`'s simulated card, compiled to wasm and driven
 * from here. It is the same assembly `firmware/obc-link`'s Rust suites run on, so a difference
 * between the browser's idea of the protocol and the device's shows up as a failing test rather than
 * on a rider's desk.
 *
 * **This module never ships.** The wasm package lives outside `src/` and `platform/bundle.test.ts`
 * asserts neither it nor this adapter reaches a built bundle.
 *
 * The device answers one bounded reaction at a time: hand it a record, get back one thing to send,
 * ask again. Everything else is this file's. Record framing is `RecordChannel`'s, the same class the
 * client uses on the other end. Ordering is one send queue per channel, so a channel's records leave
 * in the order the device produced them while the other channel is free to answer — which is what
 * keeps `CANCEL` serviceable when a download's stream write is parked on a full endpoint.
 * Backpressure is the pipe's: a write resolves when the reader has taken the bytes, and the device is
 * not polled for the next record until it does.
 */

import init, { FlatDevice as WasmDevice, type DeviceReaction, type InitInput } from "../../../test-support/flat-device/pkg/obc_flat_device.js";
import { FlatStoreClient } from "./client";
import { PipeError, type DeviceLink } from "./pipe";
import { MAX_DEVICE_RECORD, MAX_HOST_CONTROL_RECORD, MAX_HOST_STREAM_RECORD, RecordChannel, type DeviceInfo } from "./records";
import {
    EntryFlags,
    HEADER_LEN,
    LIST_ENTRY_LEN,
    LIST_PREFIX_LEN,
    STREAM_HEADER_LEN,
    ObjectKind,
    type CatalogEntry,
} from "./protocol";
import { loopbackLink, type LoopbackLink, type LoopbackOptions } from "./loopback";

/**
 * Load the device's wasm module. Call once before constructing a {@link FlatDevice}.
 *
 * Leave `source` out in the browser: the generated glue resolves the module next to itself, which is
 * the form the bundler rewrites to a hashed asset URL. Node has no `fetch` for `file:` URLs, so a
 * Vitest suite reads the `.wasm` and passes the bytes.
 */
export async function initFlatDevice(source?: InitInput): Promise<void> {
    await init(source === undefined ? undefined : { module_or_path: source });
}

/** How a {@link FlatDevice} starts out. Everything has a working default. */
export interface FlatDeviceOptions {
    /** Extents the card holds. Fewer is how a card with no room for the next object is made. */
    extents?: number;
    /** Whether the card arrives formatted. A blank one is `readOnly` until `FORMAT` recovers it. */
    formatted?: boolean;
    /** The sparse card's seed, so a scenario is reproducible. */
    seed?: number;
    /** The identity the card is formatted with, as 32 hex characters. */
    storeId?: string;
    /**
     * The control record ceiling. `LIST` pages at this number — it is the only thing that decides
     * how many entries fit in a page, so a test that wants two pages lowers it.
     */
    controlCeiling?: number;
    /** The stream record ceiling. */
    streamCeiling?: number;
    /** Whether `ARM` may succeed. `refuse` — the default — is the device's current policy. */
    armPolicy?: "refuse" | "allow";
    /** Record every request the device was handed. On by default; the dev harness turns it off. */
    trace?: boolean;
}

/** One record the device was asked to serve. */
export interface TracedRequest {
    opcode: number;
    requestId: number;
}

/** What to seed the card with. The store assigns the id, so the caller reads it off the answer. */
export interface Seed {
    kind: ObjectKind;
    displayName?: string;
    /** The payload. Omit it for an entry over a reserve with no bytes behind it. */
    bytes?: Uint8Array;
    /** Only for a reserved entry: bytes to hold, and the flag that says why. */
    reserve?: number;
    flags?: number;
}

/** The identity a card formats with unless a test names another. */
export const DEFAULT_STORE_ID = "11111111111111111111111111111111";

/**
 * The control ceiling at which `LIST` pages at exactly `entries` entries.
 *
 * The real device pages at its link's record ceiling and at nothing else — there is no page-size dial
 * — so a test that wants two pages says how wide the link is.
 */
export function controlCeilingFor(entries: number): number {
    return HEADER_LEN + LIST_PREFIX_LEN + entries * LIST_ENTRY_LEN;
}

/** The stream ceiling at which one `GET` record carries exactly `payload` bytes. */
export function streamCeilingFor(payload: number): number {
    return payload + STREAM_HEADER_LEN;
}

type Reaction = { kind: string; channel: string; bytes: Uint8Array };

/** The reaction as plain JS, with the wasm object released. */
function taken(reaction: DeviceReaction): Reaction {
    const plain = { kind: reaction.kind, channel: reaction.channel, bytes: reaction.bytes };
    reaction.free();
    return plain;
}

export class FlatDevice {
    private readonly device: WasmDevice;
    private readonly channels: { control: RecordChannel; stream: RecordChannel };
    /** One send queue per channel: what keeps a channel's records in order. */
    private readonly sending = { control: Promise.resolve(), stream: Promise.resolve() };
    private readonly log: TracedRequest[] = [];
    private readonly tracing: boolean;
    private running = false;
    /** Set when something may be pollable; cleared by the pump when it has drained the device. */
    private owed = false;
    private wake: (() => void) | null = null;

    /** Anything that failed which is not a disconnect — a defect in the adapter or the device. */
    readonly faults: unknown[] = [];

    constructor(link: DeviceLink, options: FlatDeviceOptions = {}) {
        this.device = new WasmDevice(
            options.extents ?? 64,
            options.formatted ?? true,
            options.seed ?? 1,
            options.controlCeiling ?? MAX_DEVICE_RECORD,
            options.streamCeiling ?? MAX_DEVICE_RECORD,
            options.armPolicy === "allow",
            options.storeId ?? DEFAULT_STORE_ID,
        );
        this.channels = {
            control: new RecordChannel(link.control, MAX_DEVICE_RECORD, MAX_HOST_CONTROL_RECORD),
            stream: new RecordChannel(link.stream, MAX_DEVICE_RECORD, MAX_HOST_STREAM_RECORD),
        };
        this.tracing = options.trace ?? true;
        if (this.tracing) this.device.traceRequests();
    }

    /**
     * Serve until the link closes: two readers and one pump.
     *
     * **Exactly one task polls.** The device has one live transfer, and its next record is whatever
     * `poll` answers, so a second poller would take a record out of the middle of a download and send
     * it after the rest. The readers therefore never poll: they hand one record to the device, queue
     * whatever came back, and wake the pump.
     *
     * Rejects only on a defect, never on a normal disconnect.
     */
    async run(): Promise<void> {
        this.running = true;
        await Promise.all([
            this.read("control", (record) => this.device.onControl(record)),
            this.read("stream", (record) => this.device.onStream(record)),
            this.pump(),
        ]);
    }

    stop(): void {
        this.running = false;
        this.nudge();
    }

    /**
     * Read whole records off one channel and hand each to the device.
     *
     * The answer is queued rather than awaited, so this loop goes straight back to reading — which is
     * what makes a `CANCEL` reach a device whose download is parked on a full endpoint.
     */
    private async read(channel: "control" | "stream", feed: (record: Uint8Array) => DeviceReaction): Promise<void> {
        while (this.running) {
            let record: Uint8Array;
            try {
                record = await this.channels[channel].next();
            } catch {
                this.stop();
                return;
            }
            if (!this.running) return;
            let answer: Reaction;
            try {
                answer = taken(feed(record));
                this.drainTrace();
            } catch (cause) {
                // A wasm trap is never an expected answer: it is this adapter or the engine being
                // wrong, and a test must see it rather than a silent stall.
                this.faults.push(cause);
                this.stop();
                return;
            }
            void this.write(answer);
            this.nudge();
        }
    }

    /** The one poller: everything a live transfer still owes, one record at a time. */
    private async pump(): Promise<void> {
        while (this.running) {
            if (!this.owed) {
                await new Promise<void>((resume) => (this.wake = resume));
                this.wake = null;
                continue;
            }
            this.owed = false;
            for (;;) {
                let reaction: Reaction;
                try {
                    reaction = taken(this.device.poll());
                } catch (cause) {
                    this.faults.push(cause);
                    this.stop();
                    return;
                }
                if (reaction.kind === "idle") break;
                // Awaited, so the device is not asked for the next record until the transport has
                // taken this one. That await *is* the backpressure.
                await this.write(reaction);
                if (!this.running) return;
            }
        }
    }

    /** There may be something to poll for. */
    private nudge(): void {
        this.owed = true;
        this.wake?.();
    }

    /** Send one reaction on its own channel, behind whatever that channel is already sending. */
    private write(reaction: Reaction): Promise<void> {
        const channel = reaction.channel === "stream" ? "stream" : "control";
        const sent = this.sending[channel].then(() => this.send(channel, reaction));
        this.sending[channel] = sent.catch(() => undefined);
        return sent;
    }

    private async send(channel: "control" | "stream", reaction: Reaction): Promise<void> {
        if (reaction.kind === "idle") return;
        if (reaction.kind === "close") {
                // An unanswerable record gets nothing at all and closes the record stream.
            this.stop();
            return;
        }
        try {
            await this.channels[channel].send(reaction.bytes);
        } catch (cause) {
            // The link went away mid-answer. That is the ordinary end of a session.
            if (!(cause instanceof PipeError)) this.faults.push(cause);
            this.stop();
            return;
        }
        if (reaction.kind === "send-and-reboot") {
                // The answer reached the transport, and now the device restarts.
            this.device.reboot();
        }
    }

    /** Take what the device traced before anything can drop it — a reboot starts a new trace. */
    private drainTrace(): void {
        if (this.tracing) this.log.push(...(JSON.parse(this.device.takeTrace()) as TracedRequest[]));
    }

    // --- the card --------------------------------------------------------------

    /** Put an object on the card without a `PUT`. Returns the entry the store committed. */
    seed(object: Seed): CatalogEntry {
        const name = object.displayName ?? "";
        if (object.bytes !== undefined) {
            return entriesOf(this.device.seed(object.kind, name, object.bytes))[0];
        }
        // No bytes means an entry over a reserve: a ride mid-recording, or an update's rollback
        // reserve. Both are device-owned, and a `GET` of either is refused.
        const flags = object.flags ?? (object.kind === ObjectKind.Ride ? EntryFlags.Recording : EntryFlags.Reserved);
        const reserve = BigInt(object.reserve ?? 1024 * 1024);
        return entriesOf(this.device.seedReserved(object.kind, name, reserve, flags))[0];
    }

    /**
     * Publish a further revision of an object already on the card, keeping the previous one as
     * `RETAINED` — the one catalog state no opcode produces.
     */
    retain(previous: CatalogEntry, bytes: Uint8Array, displayName = ""): CatalogEntry {
        return entriesOf(this.device.seedRetained(previous.objectId, displayName, bytes))[0];
    }

    /** The whole catalog as the device would list it. */
    get entries(): readonly CatalogEntry[] {
        return entriesOf(this.device.catalog());
    }

    /** The bytes the card holds for one object's head revision, or `null`. */
    payloadOf(objectId: bigint, revision = 0n): Uint8Array | null {
        return this.device.readObject(objectId, revision) ?? null;
    }

    /** The card's commit sequence. */
    get sequence(): bigint {
        return this.device.commitSequence();
    }

    /** The card's identity, as `LIST` reports it. */
    get storeId(): string {
        return this.device.storeId();
    }

    // --- the two test hooks ----------------------------------------------------

    /** Every request the device was handed, in order. Lets a test assert what a flow did *not* send. */
    get requestLog(): readonly TracedRequest[] {
        this.drainTrace();
        return this.log;
    }

    /** An enumerated device that has hung: records arrive, nothing comes back. */
    stopAnswering(): void {
        this.device.stopAnswering();
    }
}

/** The catalog JSON the device produces, as the client's own entry shape. */
function entriesOf(json: string): CatalogEntry[] {
    const rows = JSON.parse(json) as Array<{
        objectId: string;
        revision: string;
        payloadLength: string;
        payloadCrc32: number;
        kind: number;
        flags: number;
        displayName: string;
    }>;
    return rows.map((row) => ({
        objectId: BigInt(row.objectId),
        revision: BigInt(row.revision),
        payloadLength: BigInt(row.payloadLength),
        payloadCrc32: row.payloadCrc32,
        kind: row.kind as ObjectKind,
        flags: row.flags,
        displayName: row.displayName,
    }));
}

/**
 * A client wired to a running {@link FlatDevice} — the one-liner every device test starts from.
 *
 * Seed the device, drive the client, `close()` when done. The device's loops run detached; closing
 * the client closes the link, which ends them.
 */
export function flatDevice(
    options: LoopbackOptions & FlatDeviceOptions & { deviceInfo?: DeviceInfo; clientTimeoutMs?: number } = {},
): {
    client: FlatStoreClient;
    device: FlatDevice;
    link: LoopbackLink;
    close: () => Promise<void>;
} {
    const link = loopbackLink(options);
    const device = new FlatDevice(link.device, options);
    void device.run();
    // `clientTimeoutMs` exists for one kind of test: a device that is enumerated but hung, where
    // the assertion is that a call *ends* rather than what it returns.
    const client = new FlatStoreClient(link.host, { timeoutMs: options.clientTimeoutMs });
    return {
        client,
        device,
        link,
        close: async () => {
            device.stop();
            await client.close();
            await link.device.close();
        },
    };
}
