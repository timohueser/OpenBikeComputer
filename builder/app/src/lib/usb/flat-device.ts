/**
 * The real device, on the other end of a {@link DeviceLink}.
 *
 * `obc_link::flat::Engine` over `obc_storage::flat`'s simulated card, compiled to wasm and driven
 * from here. It is the same assembly `firmware/obc-link`'s Rust suites run on, so a flow that works
 * in this file works against the code the board runs — and a difference between the browser's idea
 * of the protocol and the device's shows up as a failing test rather than on a rider's desk.
 *
 * **This module never ships.** The wasm package lives outside `src/` (`test-support/flat-device/`)
 * and `platform/bundle.test.ts` asserts neither it nor this adapter reaches a built bundle.
 *
 * ## What this adapter does, and what the device does
 *
 * The device answers **one bounded reaction at a time**: hand it a record, get back one thing to
 * send, ask again. Everything else is this file's:
 *
 * - **Record framing** is `RecordChannel`'s, the same class the client uses on the other end.
 * - **Ordering** is per channel. Each channel owns one pump, so its records leave in order, and a
 *   reaction for the other channel is handed over rather than waited on — which is what keeps
 *   `CANCEL` serviceable while a download's stream write is parked on a full endpoint.
 * - **Backpressure** is the pipe's: a write resolves when the reader has taken the bytes, and the
 *   device is not polled for the next record until it does. Nothing buffers a whole download.
 */

import init, { FlatDevice as WasmDevice, type DeviceReaction, type InitInput } from "../../../test-support/flat-device/pkg/obc_flat_device.js";
import { FlatStoreClient } from "./client";
import { PipeError, type DeviceLink } from "./pipe";
import { MAX_DEVICE_RECORD, MAX_HOST_CONTROL_RECORD, MAX_HOST_STREAM_RECORD, RecordChannel } from "./records";
import { EntryFlags, ObjectKind, type CatalogEntry } from "./protocol";
import { loopbackLink, type LoopbackLink, type LoopbackOptions } from "./loopback";

/**
 * Load the device's wasm module. Call once before constructing a {@link FlatDevice}.
 *
 * Node has no `fetch` for `file:` URLs, so a Vitest suite reads
 * `test-support/flat-device/pkg/obc_flat_device_bg.wasm` and passes the bytes, exactly as the
 * conversion bridge's suites do.
 */
export async function initFlatDevice(source: InitInput): Promise<void> {
    await init({ module_or_path: source });
}

/** How a {@link FlatDevice} starts out. Everything has a working default. */
export interface FlatDeviceOptions {
    /** Extents the card holds. Fewer is how a card with no room for the next object is made. */
    extents?: number;
    /** Whether the card arrives formatted. A blank one is `readOnly` until `FORMAT` recovers it. */
    formatted?: boolean;
    /** The sparse card's seed, so a scenario is reproducible. */
    seed?: number;
    /**
     * §5's control record ceiling. `LIST` pages at this number — it is the only thing that decides
     * how many entries fit in a page, so a test that wants two pages lowers it.
     */
    controlCeiling?: number;
    /** §5's stream record ceiling. */
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
    /** One pump per channel: what keeps records in order without making the channels wait on each other. */
    private readonly pumps = { control: Promise.resolve(), stream: Promise.resolve() };
    private readonly log: TracedRequest[] = [];
    private readonly tracing: boolean;
    private running = false;

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
        );
        this.channels = {
            control: new RecordChannel(link.control, MAX_DEVICE_RECORD, MAX_HOST_CONTROL_RECORD),
            stream: new RecordChannel(link.stream, MAX_DEVICE_RECORD, MAX_HOST_STREAM_RECORD),
        };
        this.tracing = options.trace ?? true;
        if (this.tracing) this.device.traceRequests();
    }

    // --- the control loop ------------------------------------------------------

    /** Serve until the link closes. Rejects only on a defect, never on a normal disconnect. */
    async run(): Promise<void> {
        this.running = true;
        await Promise.all([
            this.read("control", (record) => this.device.onControl(record)),
            this.read("stream", (record) => this.device.onStream(record)),
        ]);
    }

    stop(): void {
        this.running = false;
    }

    /**
     * Read whole records off one channel and hand each to the device.
     *
     * The reaction is handed to a pump and this loop goes straight back to reading, which is what
     * makes a `CANCEL` reach a device that is mid-download.
     */
    private async read(channel: "control" | "stream", feed: (record: Uint8Array) => DeviceReaction): Promise<void> {
        while (this.running) {
            let record: Uint8Array;
            try {
                record = await this.channels[channel].next();
            } catch {
                this.running = false;
                return;
            }
            let first: Reaction;
            try {
                first = taken(feed(record));
            } catch (cause) {
                // A wasm trap is never an expected answer: it is this adapter or the engine being
                // wrong, and a test must see it rather than a silent stall.
                this.faults.push(cause);
                this.running = false;
                return;
            }
            this.hand(first);
        }
    }

    /** Queue a reaction behind whatever its channel is already sending. */
    private hand(reaction: Reaction): void {
        const channel = reaction.channel === "stream" ? "stream" : "control";
        this.pumps[channel] = this.pumps[channel].then(() => this.pump(channel, reaction));
    }

    /** Send one reaction and keep asking the device for the next, while they are this channel's. */
    private async pump(channel: "control" | "stream", first: Reaction): Promise<void> {
        let reaction = first;
        for (;;) {
            if (reaction.kind === "idle") return;
            if (reaction.kind === "close") {
                // §3.1: an unanswerable record gets nothing at all and closes the record stream.
                this.running = false;
                return;
            }
            if (reaction.channel !== channel) {
                // The other channel's turn, and its pump's: waiting for it here would put a control
                // answer behind a stream write nobody is draining.
                this.hand(reaction);
                return;
            }
            try {
                await this.channels[channel].send(reaction.bytes);
            } catch (cause) {
                // The link went away mid-answer. That is the ordinary end of a session.
                if (!(cause instanceof PipeError)) this.faults.push(cause);
                this.running = false;
                return;
            }
            if (reaction.kind === "send-and-reboot") {
                this.device.reboot();
                return;
            }
            reaction = taken(this.device.poll());
        }
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
        if (this.tracing) this.log.push(...(JSON.parse(this.device.takeTrace()) as TracedRequest[]));
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
    options: LoopbackOptions & FlatDeviceOptions & { clientTimeoutMs?: number } = {},
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
