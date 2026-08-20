/**
 * The native transport, driven end to end (D4, #909).
 *
 * There is no device in CI and none on the machine that wrote this, so the substitution has to be
 * chosen carefully. It is made at exactly one place: the **Tauri command boundary**. Everything
 * above it — `NativePipe`, `NativeWatcher`, `FlatStoreClient`, §5.2's record framing, the codecs,
 * the CRC — is the shipping code, and the fake backend below stands in for
 * `apps/obc-desktop/src/usb/`, forwarding to C3's simulated device.
 *
 * That means these tests are about the two things a fake *can* prove:
 *
 * 1. **The seam holds.** A real `specs/vectors/` object round-trips through the real client over
 *    the native pipes, byte for byte, with the device verifying the whole-object CRC — which is
 *    #909's first acceptance criterion and the entire claim that USB-over-Rust is a second
 *    transport rather than a second protocol.
 * 2. **The transport properties C3's contract names are honoured**: a read is not a record, a
 *    zero-length packet is a marker and not data, concurrent writes keep submission order,
 *    cancellation reaches the transport, and an unplug settles pending calls.
 *
 * There is a third thing, and it is an absence: this host has no EP0 vendor request, so §5.2.1's
 * device info is unreadable here. That is asserted rather than worked around — a connection that
 * invented a firmware revision would feed "an update is available" a lie.
 *
 * What this cannot prove is anything about `nusb`, the OS, or the descriptors — enumeration,
 * stalls, short-packet termination and the ZLP contract are hardware, and the PR body says how they
 * were checked on glass.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { FlatStoreClient } from "../usb/client";
import { Crc32 } from "../usb/crc32";
import { loopbackLink, MockDevice, type LoopbackLink, type LoopbackOptions, type MockDeviceOptions } from "../usb/loopback";
import { PipeError, type BytePipe } from "../usb/pipe";
import { MAX_HOST_STREAM_RECORD } from "../usb/records";
import { HEAD_REVISION, ObjectKind } from "../usb/protocol";

// --- the fake backend ----------------------------------------------------------

/** One `invoke()` the fake backend saw, for the wire assertions. */
interface Call {
    cmd: string;
    args: unknown;
    options?: { headers?: Record<string, string> };
}

class FakeChannel<T> {
    onmessage: ((message: T) => void) | null = null;
}

const calls: Call[] = [];
/** Set per test: what `usb_watch` / `usb_list` report is attached. */
let attached: Array<{
    id: string;
    vendorId: number;
    productId: number;
    product: string | null;
    serialNumber: string | null;
}> = [];
/** The hot-plug channel the watcher handed `usb_watch` — a test pushes plug events through it. */
let watchChannel: FakeChannel<import("./invoke").UsbEvent> | null = null;
let wire: LoopbackLink | null = null;
let device: MockDevice | null = null;
/** Whether `usb_open` should refuse, and with what. `onlyId` scopes the fault to one device. */
let openFault: { code: string; message: string; onlyId?: string } | null = null;
/** Gates shifted per `usb_open` call — an entry parks that open until its promise resolves,
 *  which is how a test freezes one connect flow mid-open while another overtakes it. */
let openGates: Array<(() => Promise<void>) | undefined> = [];
/** Per-call gates for `usb_write` on the stream plane, modelling the backend's free ordering of
 *  concurrent invokes. */
let writeGates: Array<(() => Promise<void>) | undefined> = [];
/** In-flight reads/writes, so `usb_cancel` can settle them the way a cancelled URB does. */
const inFlight = new Map<string, AbortController>();

/**
 * The backend's plane name → the channel §5 gives it.
 *
 * `"bulk"` is `DeviceLink.stream` under the Rust side's older name for the endpoint pair; the
 * mismatch is deliberate and lives in `usb.ts`, because renaming it is a Rust change.
 */
function planeOf(name: string): BytePipe {
    const link = wire?.host;
    if (!link) throw { code: "closed", message: "no link" };
    return name === "control" ? link.control : link.stream;
}

async function backend(cmd: string, args: unknown, options?: { headers?: Record<string, string> }): Promise<unknown> {
    const a = (args ?? {}) as Record<string, unknown>;
    switch (cmd) {
        case "usb_watch":
            watchChannel = a.onEvent as FakeChannel<import("./invoke").UsbEvent>;
            return attached;
        case "usb_list":
            return attached;
        case "usb_open": {
            const gate = openGates.shift();
            if (gate) await gate();
            if (openFault && (!openFault.onlyId || openFault.onlyId === a.deviceId)) throw openFault;
            const found = attached.find((d) => d.id === a.deviceId);
            if (!found) throw { code: "closed", message: "That device is no longer attached." };
            return {
                handle: 1,
                deviceId: found.id,
                interfaceNumber: 0,
                controlPacketSize: 512,
                bulkPacketSize: 512,
                product: found.product,
                serialNumber: found.serialNumber,
            };
        }
        case "usb_close":
            await wire?.host.close();
            return null;
        case "usb_read": {
            const key = `${a.plane}:in`;
            const controller = new AbortController();
            inFlight.set(key, controller);
            try {
                const bytes = await planeOf(a.plane as string).read(controller.signal);
                return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
            } catch (cause) {
                throw asFault(cause);
            } finally {
                inFlight.delete(key);
            }
        }
        case "usb_write": {
            // The real command takes the bytes as the whole invoke body, with the handle and plane
            // in headers — which is exactly what is asserted here rather than assumed.
            const plane = options?.headers?.plane ?? "";
            if (plane === "bulk") {
                const gate = writeGates.shift();
                if (gate) await gate();
            }
            const key = `${plane}:out`;
            const controller = new AbortController();
            inFlight.set(key, controller);
            try {
                await planeOf(plane).write(new Uint8Array(args as ArrayBufferLike), controller.signal);
                return null;
            } catch (cause) {
                throw asFault(cause);
            } finally {
                inFlight.delete(key);
            }
        }
        case "usb_cancel": {
            const dirs = a.dir ? [a.dir as string] : ["in", "out"];
            for (const dir of dirs) inFlight.get(`${a.plane}:${dir}`)?.abort();
            return null;
        }
        case "usb_reset":
            await planeOf(a.plane as string).reset();
            return null;
        default:
            throw new Error(`the fake backend has no command ${cmd}`);
    }
}

/** Translate the loopback's `PipeError` into the `{code, message}` the Rust side rejects with. */
function asFault(cause: unknown): { code: string; message: string } {
    if (cause instanceof PipeError) return { code: cause.code, message: cause.message };
    return { code: "device-error", message: String(cause) };
}

vi.mock("@tauri-apps/api/core", () => ({
    invoke: (cmd: string, args?: unknown, options?: { headers?: Record<string, string> }) => {
        calls.push({ cmd, args, options });
        return backend(cmd, args, options);
    },
    Channel: FakeChannel,
}));

const { NativeWatcher, openNativeLink } = await import("./usb");

// --- fixtures ------------------------------------------------------------------

function repoRoot(): string {
    let dir = dirname(fileURLToPath(import.meta.url));
    for (let up = 0; up < 12; up++) {
        if (existsSync(join(dir, "specs", "vectors", "manifest.json"))) return dir;
        dir = dirname(dir);
    }
    throw new Error("could not locate the repo root");
}
const ROOT = repoRoot();
const vector = (name: string): Uint8Array => new Uint8Array(readFileSync(join(ROOT, "specs/vectors", name)));

const DEVICE = {
    id: "usb#1",
    vendorId: 0x1209,
    productId: 0x0001,
    product: "OpenBikeComputer",
    serialNumber: "0011223344556677",
};
const DEVICE_B = {
    id: "usb#2",
    vendorId: 0x1209,
    productId: 0x0001,
    product: "OpenBikeComputer",
    serialNumber: "8877665544332211",
};

/** A live simulated device behind the fake backend, and a watcher already connected to it. */
async function connected(options: LoopbackOptions & MockDeviceOptions = {}) {
    wire = loopbackLink(options);
    device = new MockDevice(wire.device, options);
    void device.run();
    attached = [DEVICE];
    const watcher = new NativeWatcher();
    const ok = await watcher.start();
    return { watcher, ok };
}

beforeEach(() => {
    calls.length = 0;
    attached = [];
    watchChannel = null;
    openFault = null;
    openGates = [];
    writeGates = [];
    inFlight.clear();
    wire = null;
    device = null;
});

// --- the seam ------------------------------------------------------------------

describe("the native pipes under the flat-store client", () => {
    it("round-trips a specs/vectors object, byte for byte", async () => {
        // 64-byte packets, so every record of any size spans several transfers in both directions.
        // A client that treated one `usb_read` as one record would pass on a mock that wrote whole
        // records and fail here, which is the whole reason the loopback re-slices (§5.2).
        const { watcher, ok } = await connected({ packetSize: 64, streamPayload: 256 });
        expect(ok).toBe(true);
        const client = watcher.current.client!;
        expect(watcher.current.status).toBe("ready");
        // The `LIST` every connection issues first (§3.3) is where the card's identity comes from.
        expect(watcher.current.store).toEqual({ storeId: device!.storeId, commitSequence: device!.sequence });

        // A `PUT` of a real OBCR fixture: the request over the control channel, the payload as §3.8
        // stream records, and the device verifying the whole-object CRC at commit. The device is
        // the one checking, so a mis-framed record fails here rather than being "uploaded".
        const obcr = vector("route-waypoints.obcr");
        const put = await client.put({ kind: ObjectKind.Route, displayName: "waypoints" }, obcr);
        expect(put.payloadLength).toBe(BigInt(obcr.length));
        expect(put.payloadCrc32).toBe(Crc32.of(obcr));
        expect(device!.payloadOf(put.objectId)).toEqual(obcr);

        // …and a `GET` in the other direction, whose length and CRC this side verifies.
        const got = await client.get({ objectId: put.objectId, revision: HEAD_REVISION });
        expect(got.bytes).toEqual(obcr);
        expect(got.revisionServed).toBe(put.revision);

        // The catalog, paged over the same channel, now names it.
        const catalog = await client.list();
        expect(catalog.entries.map((e) => e.objectId)).toContain(put.objectId);
        expect(catalog.entries.find((e) => e.objectId === put.objectId)?.displayName).toBe("waypoints");

        await watcher.close();
    });

    it("keeps concurrent stream writes in submission order across the bridge", async () => {
        // The backend gives concurrent invokes no ordering guarantee — each lands in its own task
        // racing for the endpoint lock. Delaying the first stream invoke a few ticks models the
        // race: without the pipe's submission chain the second batch reaches the wire first and the
        // object arrives with its middle swapped — right total length, wrong whole-object CRC, the
        // on-glass desktop shard rejections of 2026-08-07. With the chain, the delay just delays.
        const { watcher, ok } = await connected();
        expect(ok).toBe(true);
        const client = watcher.current.client!;
        // Big enough that the upload keeps several batches in its window, plus an odd tail.
        const bytes = new Uint8Array(200_003);
        for (let i = 0; i < bytes.length; i++) bytes[i] = (i * 31 + (i >> 9)) & 0xff;
        writeGates.push(() => new Promise<void>((resolve) => setTimeout(resolve, 10)));
        const put = await client.put({ kind: ObjectKind.MapShard, displayName: "black forest" }, bytes);
        expect(put.payloadCrc32).toBe(Crc32.of(bytes));
        expect(device!.payloadOf(put.objectId)).toEqual(bytes);
        await watcher.close();
    });
});

// --- the transport contract ----------------------------------------------------

describe("the native pipe's transport contract", () => {
    it("puts the bytes in a raw body and the routing in headers", async () => {
        const { watcher } = await connected();
        const write = calls.find((c) => c.cmd === "usb_write");
        // A `Vec<u8>` argument would have been JSON — about four bytes of text per byte of payload,
        // which is fine for a 32-byte `LIST` and absurd for a 300 MB map.
        expect(write?.args).toBeInstanceOf(Uint8Array);
        expect(write?.options?.headers).toEqual({ handle: "1", plane: "control" });
        const read = calls.find((c) => c.cmd === "usb_read");
        expect(read?.args).toEqual({ handle: 1, plane: "control" });
        await watcher.close();
    });

    it("sends a record that spans packets instead of refusing it", async () => {
        // The rule this replaces: under the v1 envelope a frame was one transfer, so anything at or
        // above the endpoint's packet size was refused before it left the page. §5.2 makes a record
        // self-delimiting — `record_length u32`, frame bytes and alignment padding, across as many packets as
        // it takes — so the ordinary 8,208-byte stream frame (§3.8's header plus one 8,192-byte
        // write) has to go out, and so does a control record that lands on the 512-byte boundary.
        wire = loopbackLink();
        attached = [DEVICE];
        const link = await openNativeLink(DEVICE.id);
        const stream = Uint8Array.from({ length: MAX_HOST_STREAM_RECORD }, (_, i) => (i * 13) & 0xff);
        await link.stream.write(stream);
        expect(await drain(wire.device.stream, stream.length)).toEqual(stream);

        const control = new Uint8Array(512).fill(0xa5);
        await link.control.write(control);
        expect(await drain(wire.device.control, control.length)).toEqual(control);
        await link.close();
    });

    it("cancels at the transport rather than merely releasing the caller", async () => {
        const { watcher } = await connected();
        const link = await openNativeLink(DEVICE.id);
        const abort = new AbortController();
        const read = link.stream.read(abort.signal);
        // Nothing is queued on the stream plane, so this read is genuinely parked — which is the
        // case that wedges if a cancel only settles the promise: the backend would still hold the
        // endpoint and the next read would queue behind an orphan that never completes.
        await Promise.resolve();
        abort.abort();
        await expect(read).rejects.toMatchObject({ name: "PipeError", code: "aborted" });
        expect(calls.some((c) => c.cmd === "usb_cancel")).toBe(true);
        await watcher.close();
    });

    it("settles everything parked on it the moment the device is unplugged", async () => {
        const { watcher } = await connected();
        const link = await openNativeLink(DEVICE.id);
        const read = link.stream.read();
        link.disconnected();
        await expect(read).rejects.toMatchObject({ code: "closed" });
        // …and stays closed: a later call fails immediately instead of making another doomed round
        // trip, which is the difference between one error message and a stuck spinner.
        await expect(link.stream.read()).rejects.toMatchObject({ code: "closed" });

        // The abandoned command settles *after* the caller was failed by the unplug — `dead()` won
        // the race, so its rejection lands on nobody. It must stay harmless: no second error, no
        // resurrected pipe, and a `close()` afterwards that still works on a link whose device is
        // already gone. (`Promise.race` attaches to both arms, so the late rejection is handled by
        // construction; this is the path that proves it rather than a comment claiming it.)
        await wire!.host.close();
        await new Promise((resolve) => setTimeout(resolve, 10));
        expect(link.stream.open).toBe(false);
        await watcher.close();
    });
});

/** Read `total` bytes off a loopback end, which delivers them one packet at a time. */
async function drain(pipe: BytePipe, total: number): Promise<Uint8Array> {
    const out = new Uint8Array(total);
    let at = 0;
    while (at < total) {
        const slice = await pipe.read();
        out.set(slice, at);
        at += slice.length;
    }
    return out;
}

// --- §5.2.1, which this host cannot ask ----------------------------------------

describe("the device info this host cannot read", () => {
    it("says so rather than answering with a version nobody read off the device", async () => {
        // §5.2.1's `GET_DEVICE_INFO` is an EP0 vendor request and the Rust bridge exposes no
        // control-transfer command, so the link omits `vendorIn` entirely. The client turns that
        // absence into one specific code, which is what the connect flow catches.
        wire = loopbackLink();
        attached = [DEVICE];
        const link = await openNativeLink(DEVICE.id);
        expect(link.vendorIn, "a link that cannot issue EP0 must not pretend to").toBeUndefined();
        const client = new FlatStoreClient(link);
        await expect(client.deviceInfo()).rejects.toMatchObject({ name: "DeviceError", code: "unavailable" });
        await client.close();
    });

    it("connects anyway, publishing no firmware version at all", async () => {
        // The alternative — failing the connection, or filling the field with a plausible number —
        // would either strand the desktop tier or hand "an update is available" a lie. `null` is the
        // honest third answer, and the UI renders the absence.
        const { watcher, ok } = await connected();
        expect(ok).toBe(true);
        expect(watcher.current.status).toBe("ready");
        expect(watcher.current.info).toBeNull();
        expect(watcher.current.store).not.toBeNull();
        await watcher.close();
    });
});

// --- discovery -----------------------------------------------------------------

describe("native discovery", () => {
    it("adopts an attached device with no prompt and no gesture", async () => {
        const { ok, watcher } = await connected();
        expect(ok).toBe(true);
        // The watch is started before the listing, which is the ordering that cannot miss a device
        // plugged in during the call.
        expect(calls[0].cmd).toBe("usb_watch");
        await watcher.close();
    });

    it("reports nothing attached as idle, which is not an error", async () => {
        attached = [];
        const watcher = new NativeWatcher();
        expect(await watcher.start()).toBe(false);
        expect(watcher.current).toMatchObject({ status: "idle", error: null, client: null, store: null });
        await watcher.close();
    });

    it("releases the interface when a connection fails partway", async () => {
        // A device claimed but never listed still holds its interface, and an interface can be
        // claimed once — so a failed connect that kept it would lock out every retry.
        wire = loopbackLink();
        attached = [DEVICE];
        const watcher = new NativeWatcher({ timeoutMs: 20, hotplugRetryDelayMs: 1 });
        // No MockDevice running, so the `LIST` times out.
        expect(await watcher.start()).toBe(false);
        expect(watcher.current.status).toBe("error");
        expect(calls.some((c) => c.cmd === "usb_close")).toBe(true);
        await watcher.close();
    });

    it("surfaces a refused open as the sentence the backend wrote", async () => {
        attached = [DEVICE];
        openFault = {
            code: "device-error",
            message: "Interface 0 could not be claimed: busy — something else has it open.",
        };
        const watcher = new NativeWatcher({ hotplugRetryDelayMs: 1 });
        expect(await watcher.start()).toBe(false);
        expect(watcher.current.error).toContain("something else has it open");
        await watcher.close();
    });

    it("connects by itself when the device is plugged in after start", async () => {
        // The hot-plug contract on hardware: app open, nothing attached, cable in — the window
        // lights up with no click. The event rides the channel `usb_watch` was given.
        attached = [];
        const watcher = new NativeWatcher();
        await watcher.start();
        expect(watcher.current.status).toBe("idle");

        wire = loopbackLink();
        device = new MockDevice(wire.device);
        void device.run();
        attached = [DEVICE];
        watchChannel!.onmessage!({ type: "connected", device: DEVICE });

        await vi.waitFor(() => expect(watcher.current.status).toBe("ready"));
        await watcher.close();
    });

    it("retries when the OS announces the device before it is claimable", async () => {
        // nusb's own hot-plug docs: a `Connected` event can precede the device being openable, so
        // retry after a short delay. The failure that motivated this: one transient claim failure
        // parked the session in `error` — which the chip renders as "No device" — forever.
        attached = [];
        const watcher = new NativeWatcher({ hotplugRetryDelayMs: 1 });
        await watcher.start();

        wire = loopbackLink();
        device = new MockDevice(wire.device);
        void device.run();
        attached = [DEVICE];
        openFault = { code: "device-error", message: "Interface 0 could not be claimed: busy." };
        watchChannel!.onmessage!({ type: "connected", device: DEVICE });

        // The first attempt has failed; between attempts the state stays `connecting`, not `error`.
        await vi.waitFor(() => expect(calls.filter((c) => c.cmd === "usb_open").length).toBeGreaterThanOrEqual(1));
        expect(watcher.current.status).toBe("connecting");

        openFault = null; // the OS finished setting the device up
        await vi.waitFor(() => expect(watcher.current.status).toBe("ready"));
        await watcher.close();
    });

    it("settles back to idle when the device vanishes during the retries", async () => {
        attached = [];
        const watcher = new NativeWatcher({ hotplugRetryDelayMs: 1 });
        await watcher.start();

        attached = [DEVICE];
        openFault = { code: "device-error", message: "busy" };
        watchChannel!.onmessage!({ type: "connected", device: DEVICE });
        await vi.waitFor(() => expect(calls.some((c) => c.cmd === "usb_open")).toBe(true));

        // Unplugged again before any attempt succeeded: the truthful end state is idle, not an
        // error about a device that is no longer there.
        attached = [];
        await vi.waitFor(() => expect(watcher.current.status).toBe("idle"));
        expect(watcher.current.error).toBeNull();
        await watcher.close();
    });

    it("gives up with the device's own error once the retries run out", async () => {
        attached = [];
        const watcher = new NativeWatcher({ hotplugRetryDelayMs: 1 });
        await watcher.start();

        attached = [DEVICE];
        openFault = {
            code: "device-error",
            message: "Interface 0 could not be claimed: busy — something else has it open.",
        };
        watchChannel!.onmessage!({ type: "connected", device: DEVICE });

        await vi.waitFor(() => expect(watcher.current.status).toBe("error"));
        expect(watcher.current.error).toContain("something else has it open");
        // Bounded: it tried its attempts and stopped, rather than polling forever.
        expect(calls.filter((c) => c.cmd === "usb_open")).toHaveLength(5);
        await watcher.close();
    });

    it("retries start()'s own initial adopt through the same window", async () => {
        // App launched moments after the cable went in: the initial probe hits the exact same
        // not-yet-claimable window a hot-plug event does, and used to park in `error` with no
        // event ever coming to rescue it.
        wire = loopbackLink();
        device = new MockDevice(wire.device);
        void device.run();
        attached = [DEVICE];
        openFault = { code: "device-error", message: "not ready yet" };
        const watcher = new NativeWatcher({ hotplugRetryDelayMs: 1 });

        let release1!: () => void;
        let release2!: () => void;
        openGates.push(() => new Promise<void>((resolve) => (release1 = resolve)));
        const started = watcher.start();
        await vi.waitFor(() => expect(calls.filter((c) => c.cmd === "usb_open")).toHaveLength(1));
        openGates.push(() => new Promise<void>((resolve) => (release2 = resolve)));
        release1(); // attempt 1 fails against the not-yet-claimable device
        await vi.waitFor(() => expect(calls.filter((c) => c.cmd === "usb_open")).toHaveLength(2));
        openFault = null; // the OS finished setting the device up
        release2();

        expect(await started).toBe(true);
        expect(watcher.current.status).toBe("ready");
        await watcher.close();
    });

    it("lets a Connect click win over an adopt loop parked between attempts", async () => {
        // The wedge this guards against: the click and the loop both open the same device, the
        // loop's late failure publishes `error` over the click's `ready`, and whichever link
        // loses is dropped still holding the interface claim. The click claims the flow, so the
        // loop must notice and stand down without another attempt.
        attached = [];
        const watcher = new NativeWatcher({ hotplugRetryDelayMs: 120 });
        await watcher.start();

        wire = loopbackLink();
        device = new MockDevice(wire.device);
        void device.run();
        attached = [DEVICE];
        openFault = { code: "device-error", message: "busy" };
        watchChannel!.onmessage!({ type: "connected", device: DEVICE });
        await vi.waitFor(() => expect(calls.filter((c) => c.cmd === "usb_open")).toHaveLength(1));

        // The rider clicks Connect while the loop sleeps out its retry delay.
        openFault = null;
        expect(await watcher.requestDevice()).toBe(true);
        expect(watcher.current.status).toBe("ready");

        // The loop wakes, sees it lost the flow, and goes quietly: no third open, no late error,
        // and the winner's client still works.
        await new Promise((resolve) => setTimeout(resolve, 200));
        expect(watcher.current.status).toBe("ready");
        expect(calls.filter((c) => c.cmd === "usb_open")).toHaveLength(2);
        const page = await watcher.current.client!.listPage({});
        expect(page.storeId).toBe(device!.storeId);
        await watcher.close();
    });

    it("closes, not orphans, a link whose flow lost while the handshake ran", async () => {
        // The expensive half of the same race: a flow that already *opened* the device loses
        // while its handshake runs. The Rust side holds the interface claim until the link is
        // closed, so dropping the client on the floor would make every later open fail "busy"
        // until a physical replug. The loser must close what it opened and publish nothing.
        attached = [];
        const watcher = new NativeWatcher({ hotplugRetryDelayMs: 1 });
        await watcher.start();

        wire = loopbackLink();
        device = new MockDevice(wire.device);
        void device.run();
        attached = [DEVICE];
        let release!: () => void;
        openGates.push(() => new Promise<void>((resolve) => (release = resolve)));
        watchChannel!.onmessage!({ type: "connected", device: DEVICE });
        await vi.waitFor(() => expect(calls.some((c) => c.cmd === "usb_open")).toBe(true));

        // Overtaken while parked inside the open: a deliberate disconnect claims the flow.
        await watcher.disconnect();
        const seen: string[] = [];
        const unsubscribe = watcher.subscribe((s) => seen.push(s.status));
        release();

        // The open and the handshake complete against the live device — and then the loser
        // notices it lost: the link is closed at the backend, nothing is published.
        await vi.waitFor(() => expect(calls.some((c) => c.cmd === "usb_close")).toBe(true));
        await new Promise((resolve) => setTimeout(resolve, 50));
        expect(watcher.current.status).toBe("idle");
        expect(seen).not.toContain("ready");
        expect(seen).not.toContain("error");
        expect(calls.filter((c) => c.cmd === "usb_open")).toHaveLength(1);
        unsubscribe();
        await watcher.close();
    });

    it("keeps a successor alive when a superseded loop's final attempt fails late", async () => {
        // A replugged device gets a new id and a new `Connected` event while the old id's loop is
        // still failing its last attempt. That final failure must not publish `error` over the
        // fresh loop's `connecting` — before the flow token, it did, and the fresh loop then saw
        // the wrong status and stood down: the freshly plugged device never connected.
        attached = [];
        const watcher = new NativeWatcher({ hotplugRetryDelayMs: 1 });
        await watcher.start();

        wire = loopbackLink();
        device = new MockDevice(wire.device);
        void device.run();
        attached = [DEVICE, DEVICE_B];
        openFault = { code: "device-error", message: "gone", onlyId: DEVICE.id };
        // Attempts 1–4 fail fast; the 5th parks in its open so the successor can overtake it.
        let release!: () => void;
        openGates = [
            undefined,
            undefined,
            undefined,
            undefined,
            () => new Promise<void>((resolve) => (release = resolve)),
        ];
        watchChannel!.onmessage!({ type: "connected", device: DEVICE });
        await vi.waitFor(() => expect(calls.filter((c) => c.cmd === "usb_open")).toHaveLength(5));

        watchChannel!.onmessage!({ type: "connected", device: DEVICE_B });
        await vi.waitFor(() => expect(watcher.current.status).toBe("ready"));
        release(); // the old loop's final attempt now fails, late and superseded

        await new Promise((resolve) => setTimeout(resolve, 50));
        expect(watcher.current.status).toBe("ready");
        expect(watcher.current.error).toBeNull();
        // …and the connection that stands is the successor's, not a resurrected old one.
        const opens = calls.filter((c) => c.cmd === "usb_open");
        expect((opens.at(-1)?.args as { deviceId: string }).deviceId).toBe(DEVICE_B.id);
        await watcher.close();
    });

    it("ends in idle when the unplug event lands mid-flow", async () => {
        // The Disconnected event arrives while the flow has no link yet (so the link-based
        // handler ignores it) and the device is even still *listed* by a stale probe. The flow
        // chasing exactly this id is ended in the honest state, and its in-flight attempt's
        // failure stays silent.
        attached = [];
        const watcher = new NativeWatcher({ hotplugRetryDelayMs: 1 });
        await watcher.start();

        attached = [DEVICE];
        openFault = { code: "device-error", message: "busy" };
        // Park the attempt inside its open, so the unplug event demonstrably lands mid-flight.
        let release!: () => void;
        openGates.push(() => new Promise<void>((resolve) => (release = resolve)));
        watchChannel!.onmessage!({ type: "connected", device: DEVICE });
        await vi.waitFor(() => expect(calls.some((c) => c.cmd === "usb_open")).toBe(true));

        watchChannel!.onmessage!({ type: "disconnected", id: DEVICE.id });
        await vi.waitFor(() => expect(watcher.current.status).toBe("idle"));
        release(); // the parked attempt now fails, sees a stale token, and says nothing
        await new Promise((resolve) => setTimeout(resolve, 60));
        expect(watcher.current.status).toBe("idle");
        expect(watcher.current.error).toBeNull();
        await watcher.close();
    });

    it("re-scans instead of opening a chooser", async () => {
        // The native host has no permission prompt, so `requestDevice()` is "look again now" — the
        // Connect button keeps working and no UI branches on which transport it got.
        attached = [];
        const watcher = new NativeWatcher();
        await watcher.start();
        wire = loopbackLink();
        device = new MockDevice(wire.device);
        void device.run();
        attached = [DEVICE];
        expect(await watcher.requestDevice()).toBe(true);
        expect(watcher.current.status).toBe("ready");
        await watcher.close();
    });
});

// A note on what is *not* here any more: there used to be a `nativeFileSource` suite — a map sent
// disk → endpoint inside Rust, its bytes never entering the page, with a flat-heap measurement over
// a 300 MB object. §3.8 requires every stream record to be framed by the protocol client, and the
// `usb_send_file` command writes raw bytes, so that path cannot exist until the Rust side frames
// records. The heap claim it made now belongs to the shared upload loop and is tested where that
// loop lives; the CRC constant it pinned is `usb/crc32.test.ts`'s.
