/**
 * The native transport, driven end to end (D4, #909).
 *
 * There is no device in CI and none on the machine that wrote this, so the substitution has to be
 * chosen carefully. It is made at exactly one place: the **Tauri command boundary**. Everything
 * above it — `NativePipe`, `NativeWatcher`, `nativeFileSource`, `ProtocolClient`, the codecs, the
 * CRC — is the shipping code, and the fake backend below stands in for
 * `apps/obc-desktop/src/usb/`, forwarding to C3's simulated device.
 *
 * That means these tests are about the two things a fake *can* prove:
 *
 * 1. **The seam holds.** The same `specs/vectors/` fixtures round-trip through the real client
 *    over the native pipe, byte for byte, with a real whole-object CRC — which is #909's first
 *    acceptance criterion and the entire claim that USB is a second transport rather than a second
 *    protocol.
 * 2. **The transport properties C3's contract names are honoured**: a read is not a message, a
 *    zero-length packet is a marker and not data, cancellation reaches the transport, an unplug
 *    settles pending calls, and a source that streams itself makes the same checks the chunk loop
 *    makes.
 *
 * What it cannot prove is anything about `nusb`, the OS, or the descriptors — enumeration, stalls,
 * short-packet termination and the ZLP contract are hardware, and the PR body says how they were
 * checked on glass.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { Crc32 } from "../usb/crc32";
import {
    loopbackLink,
    MockDevice,
    REFERENCE_OBCM_VERSION,
    type LoopbackLink,
    type LoopbackOptions,
    type MockDeviceOptions,
} from "../usb/loopback";
import { decodeRouteList } from "../usb/objects";
import { PipeError, type BytePipe } from "../usb/pipe";
import { NEW_OBJECT_ID, ObjectType, PROTOCOL_VERSION, SINGLETON_OBJECT_ID } from "../usb/protocol";
import type { DeviceState, DeviceWatcher } from "../usb/session";
import { WatchedDeviceSession } from "../usb/session.svelte";
import { builtMap } from "../device/built.svelte";
import { sendLocalMap } from "../device/write";

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
let attached: Array<{ id: string; vendorId: number; productId: number; product: string | null; serialNumber: string | null }> = [];
let wire: LoopbackLink | null = null;
let device: MockDevice | null = null;
/** Whether `usb_open` should refuse, and with what. */
let openFault: { code: string; message: string } | null = null;
/** In-flight reads/writes, so `usb_cancel` can settle them the way a cancelled URB does. */
const inFlight = new Map<string, AbortController>();
/**
 * A file the fake backend will stream, keyed by path (standing in for `sendable_path`'s allowlist).
 *
 * Read through a callback rather than held as bytes, so the flat-memory measurement below can use a
 * 300 MB one without the *fixture* being the thing that allocates 300 MB.
 */
interface FakeFile {
    len: number;
    slice(at: number, n: number): Uint8Array;
}

const heldFile = (bytes: Uint8Array): FakeFile => ({
    len: bytes.length,
    slice: (at, n) => bytes.subarray(at, at + n),
});

/** `len` bytes of `i & 0xff`, generated on demand into one reused buffer. */
function syntheticFile(len: number): FakeFile {
    let scratch = new Uint8Array(0);
    return {
        len,
        slice(at, n) {
            if (scratch.length < n) scratch = new Uint8Array(n);
            for (let i = 0; i < n; i++) scratch[i] = (at + i) & 0xff;
            return scratch.subarray(0, n);
        },
    };
}

let sendable = new Map<string, FakeFile>();
/** Bytes per transfer the fake file streamer uses — the Rust side's `CHUNK`, scaled down. */
let FAKE_SEND_CHUNK = 4096;
/** Set to stall the file send after this many bytes, so a cancel has something to interrupt. */
let sendStallAfter = Number.POSITIVE_INFINITY;

function planeOf(name: string): BytePipe {
    const link = wire?.host;
    if (!link) throw { code: "closed", message: "no link" };
    return name === "control" ? link.control : link.bulk;
}

async function backend(cmd: string, args: unknown, options?: { headers?: Record<string, string> }): Promise<unknown> {
    const a = (args ?? {}) as Record<string, unknown>;
    switch (cmd) {
        case "usb_watch":
        case "usb_list":
            return attached;
        case "usb_open": {
            if (openFault) throw openFault;
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
        case "usb_file_digest": {
            const file = sendable.get(a.path as string);
            if (!file) throw { code: "device-error", message: `${a.path} is outside the folders this app streams from.` };
            // One streaming pass, exactly as `sendfile::digest` does it — nothing is held.
            const crc = new Crc32();
            for (let at = 0; at < file.len; at += FAKE_SEND_CHUNK) {
                crc.update(file.slice(at, Math.min(FAKE_SEND_CHUNK, file.len - at)));
            }
            return { len: file.len, crc32: crc.value() };
        }
        case "usb_send_file":
            return fakeSendFile(a.path as string, a.onProgress as FakeChannel<{ sent: number; total: number }>);
        default:
            throw new Error(`the fake backend has no command ${cmd}`);
    }
}

/** The Rust streamer's shape: read a chunk, write it, report, repeat — cancellable at the transport. */
async function fakeSendFile(path: string, progress: FakeChannel<{ sent: number; total: number }>): Promise<number> {
    const file = sendable.get(path);
    if (!file) throw { code: "device-error", message: `${path} is outside the folders this app streams from.` };
    const controller = new AbortController();
    inFlight.set("bulk:out", controller);
    let sent = 0;
    try {
        progress.onmessage?.({ sent: 0, total: file.len });
        while (sent < file.len) {
            if (sent >= sendStallAfter) {
                // Park until cancelled, standing in for a device that has stopped draining.
                await new Promise<void>((resolve) =>
                    controller.signal.addEventListener("abort", () => resolve(), { once: true }),
                );
            }
            const n = Math.min(FAKE_SEND_CHUNK, file.len - sent);
            await wire!.host.bulk.write(file.slice(sent, n), controller.signal);
            sent += n;
            progress.onmessage?.({ sent, total: file.len });
            // Yield, so a `check()` that throws inside the progress handler gets to run its cancel
            // before the next chunk — the same interleaving the real IPC produces.
            await Promise.resolve();
        }
        return sent;
    } catch (cause) {
        throw asFault(cause);
    } finally {
        inFlight.delete("bulk:out");
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

const { NativeWatcher, nativeFileSource, openNativeLink } = await import("./usb");

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

const DEVICE = { id: "usb#1", vendorId: 0x1209, productId: 0x0001, product: "OpenBikeComputer", serialNumber: "0011223344556677" };

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
    openFault = null;
    inFlight.clear();
    sendable = new Map();
    sendStallAfter = Number.POSITIVE_INFINITY;
    FAKE_SEND_CHUNK = 4096;
    wire = null;
    device = null;
});

// --- the seam ------------------------------------------------------------------

describe("the native pipe under C3's client", () => {
    it("round-trips specs/vectors objects, byte for byte", async () => {
        const { watcher, ok } = await connected();
        expect(ok).toBe(true);
        const state = watcher.current;
        expect(state.status).toBe("ready");
        const client = state.client!;

        // §1 identity and §3.1 device info: the two reads every connection makes before anything
        // else, and the ones that would fail first if the control frame envelope were wrong.
        expect(state.identity).toEqual({
            version: PROTOCOL_VERSION,
            storeEpoch: 0xa1b2c3d4,
            obcmVersion: REFERENCE_OBCM_VERSION,
        });
        expect(state.info?.firmwareRevision).toBe("0.4.0+abc1234");

        // An upload of a real OBCR fixture: descriptor, raw bytes over the bulk plane, whole-object
        // CRC verified by the device at commit. The device is the one checking the CRC, so a
        // mis-sliced transfer fails here rather than being "uploaded".
        const obcr = vector("route-waypoints.obcr");
        const result = await client.upload(ObjectType.Route, NEW_OBJECT_ID, obcr);
        expect(result.committedOffset).toBe(obcr.length);
        expect(device!.stored(ObjectType.Route, result.objectId)).toEqual(obcr);

        // …and a download in the other direction, over a bulk endpoint that hands over arbitrary
        // slices: the client must accumulate to the announced length rather than assume one read
        // is one object.
        const back = await client.download(ObjectType.Route, result.objectId);
        expect(back).toEqual(obcr);

        // A list object, decoded through the shared codec.
        const routes = await client.listRoutes();
        expect(routes.entries.map((e) => e.objectId)).toContain(result.objectId);
        expect(routes.truncated).toBe(false);

        // Config read and write — the longest control frame the protocol produces.
        const config = await client.readConfig();
        expect(config.name).toBe("OBC Tourer");
        await client.writeConfig({ name: "Alps", units: 1 });
        expect(await client.readConfig()).toEqual({ name: "Alps", units: 1 });

        await watcher.close();
    });

    it("serves a checked-in list object back unchanged", async () => {
        // `route-list.bin` is the fixture the firmware and iOS pin too. Seeding it and reading it
        // back over the native pipe is the strongest available statement that the transport is
        // transparent: any re-framing at all would show up as a byte difference.
        const { watcher } = await connected();
        const bytes = vector("route-list.bin");
        const { entries } = decodeRouteList(bytes);
        for (const entry of entries) device!.seedRoute(entry);
        const client = watcher.current.client!;
        const listed = await client.download(ObjectType.RouteList, SINGLETON_OBJECT_ID);
        expect(listed).toEqual(bytes);
        await watcher.close();
    });
});

// --- the transport contract ----------------------------------------------------

describe("the native pipe's transport contract", () => {
    it("puts the bytes in a raw body and the routing in headers", async () => {
        const { watcher } = await connected();
        const write = calls.find((c) => c.cmd === "usb_write");
        // A `Vec<u8>` argument would have been JSON — about four bytes of text per byte of payload,
        // which is fine for a 7-byte identity read and absurd for a firmware image.
        expect(write?.args).toBeInstanceOf(Uint8Array);
        expect(write?.options?.headers).toEqual({ handle: "1", plane: "control" });
        const read = calls.find((c) => c.cmd === "usb_read");
        expect(read?.args).toEqual({ handle: 1, plane: "control" });
        await watcher.close();
    });

    it("cancels at the transport rather than merely releasing the caller", async () => {
        const { watcher } = await connected();
        const link = await openNativeLink(DEVICE.id);
        const abort = new AbortController();
        const read = link.bulk.read(abort.signal);
        // Nothing is queued on the bulk plane, so this read is genuinely parked — which is the case
        // that wedges if a cancel only settles the promise: the backend would still hold the
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
        const read = link.bulk.read();
        link.disconnected();
        await expect(read).rejects.toMatchObject({ code: "closed" });
        // …and stays closed: a later call fails immediately instead of making another doomed round
        // trip, which is the difference between one error message and a stuck spinner.
        await expect(link.bulk.read()).rejects.toMatchObject({ code: "closed" });

        // The abandoned command settles *after* the caller was failed by the unplug — `dead()` won
        // the race, so its rejection lands on nobody. It must stay harmless: no second error, no
        // resurrected pipe, and a `close()` afterwards that still works on a link whose device is
        // already gone. (`Promise.race` attaches to both arms, so the late rejection is handled by
        // construction; this is the path that proves it rather than a comment claiming it.)
        await wire!.host.close();
        await new Promise((resolve) => setTimeout(resolve, 10));
        expect(link.bulk.open).toBe(false);
        await watcher.close();
    });

    it("refuses a control frame that would fill a whole packet", async () => {
        const { watcher } = await connected();
        const link = await openNativeLink(DEVICE.id);
        // At exactly the packet size the device cannot tell the frame ended without a zero-length
        // packet — the same rule the firmware asserts at compile time.
        await expect(link.control.write(new Uint8Array(512))).rejects.toMatchObject({ code: "device-error" });
        await link.control.write(new Uint8Array(511)).catch(() => undefined);
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
        expect(watcher.current).toMatchObject({ status: "idle", error: null, client: null });
        await watcher.close();
    });

    it("releases the interface when a connection fails partway", async () => {
        // A device claimed but never handshaken still holds its interface, and an interface can be
        // claimed once — so a failed connect that kept it would lock out every retry.
        wire = loopbackLink();
        attached = [DEVICE];
        const watcher = new NativeWatcher({ timeoutMs: 20 });
        // No MockDevice running, so the identity read times out.
        expect(await watcher.start()).toBe(false);
        expect(watcher.current.status).toBe("error");
        expect(calls.some((c) => c.cmd === "usb_close")).toBe(true);
        await watcher.close();
    });

    it("surfaces a refused open as the sentence the backend wrote", async () => {
        attached = [DEVICE];
        openFault = { code: "device-error", message: "Interface 0 could not be claimed: busy — something else has it open." };
        const watcher = new NativeWatcher();
        expect(await watcher.start()).toBe(false);
        expect(watcher.current.error).toContain("something else has it open");
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

// --- the bulk plane by file path ------------------------------------------------

describe("a file streamed natively", () => {
    it("uploads without its bytes ever entering the page", async () => {
        const { watcher } = await connected();
        const client = watcher.current.client!;
        const obcr = vector("route-plain.obcr");
        sendable.set("/maps/route.obcr", heldFile(obcr));

        const source = await nativeFileSource(1, "/maps/route.obcr");
        // The descriptor's two facts come from the backend's own pass over the file — the same CRC
        // the device will compute, which is the whole reason `obc-ble::Crc32` is linked in there.
        expect(source.totalLen).toBe(obcr.length);
        expect(source.crc32).toBe(Crc32.of(obcr));
        // And the fallback is a throw, not a quiet read over the IPC: a silent fallback to the very
        // thing the file plane exists to avoid would be worse than a loud failure.
        await expect((async () => { for await (const _ of source.chunks(1024)) break; })()).rejects.toThrow(
            /streams natively/,
        );

        const seen: number[] = [];
        const result = await client.upload(ObjectType.Route, NEW_OBJECT_ID, source, {
            onProgress: (done) => seen.push(done),
        });
        expect(result.committedOffset).toBe(obcr.length);
        expect(device!.stored(ObjectType.Route, result.objectId)).toEqual(obcr);
        expect(seen.at(-1)).toBe(obcr.length);
        // Not one `usb_write` for the object: the descriptor went over the control plane and the
        // bytes went disk → endpoint.
        expect(calls.filter((c) => c.cmd === "usb_write" && c.options?.headers?.plane === "bulk")).toEqual([]);
        expect(calls.filter((c) => c.cmd === "usb_send_file")).toHaveLength(1);
        await watcher.close();
    });

    it("stops the send when the device rejects the descriptor mid-stream", async () => {
        // The failure a naive native send gets wrong: a descriptor-open reject arrives on the
        // control plane while the bytes are already queued, and nothing in the JS chunk loop is
        // left to notice it. Here the progress reports are the only pulse, so `check()` runs on
        // each one — and a 2 MB "map" against a 1 KB ceiling has to stop early, not finish.
        const { watcher } = await connected({ maxFwImageLen: 1024 });
        const client = watcher.current.client!;
        const image = new Uint8Array(2 * 1024 * 1024).fill(7);
        sendable.set("/maps/UPDATE.BIN", heldFile(image));
        const source = await nativeFileSource(1, "/maps/UPDATE.BIN");

        let peak = 0;
        await expect(
            client.upload(ObjectType.FwImage, SINGLETON_OBJECT_ID, source, {
                onProgress: (done) => (peak = Math.max(peak, done)),
            }),
        ).rejects.toMatchObject({ name: "DeviceError" });
        expect(peak, "the send kept pushing at a device that had already said no").toBeLessThan(image.length);
        await watcher.close();
    });

    it("cancels a send that is stuck, at the transport", async () => {
        const { watcher } = await connected();
        const client = watcher.current.client!;
        const big = new Uint8Array(256 * 1024).fill(3);
        sendable.set("/maps/big.obcm", heldFile(big));
        sendStallAfter = FAKE_SEND_CHUNK * 2;
        const source = await nativeFileSource(1, "/maps/big.obcm");

        const abort = new AbortController();
        const upload = client.upload(ObjectType.Map, NEW_OBJECT_ID, source, { signal: abort.signal });
        // Let the send get going and then park, so there is a real in-flight transfer to interrupt.
        await new Promise((resolve) => setTimeout(resolve, 20));
        abort.abort();
        await expect(upload).rejects.toMatchObject({ name: "DeviceError", code: "aborted" });
        await watcher.close();
    });

    it("refuses a path the backend will not stream", async () => {
        await connected();
        await expect(nativeFileSource(1, "/etc/passwd")).rejects.toMatchObject({
            message: expect.stringContaining("outside the folders"),
        });
    });

    it("keeps this process's heap flat across a 300 MB object", async () => {
        // The web tier's own claim (`device/memory.test.ts`) is that a 300 MB map upload never
        // materialises the artifact — +4 to +5 MB of heap over the whole transfer. The native path
        // has to be *at least* as good, and for a stronger reason: none of those bytes should reach
        // this process at all. So this measures the same way and expects a much lower number, which
        // is the difference between "streaming" and "not even streaming, because it isn't here".
        //
        // The fake backend generates its bytes rather than holding them, so what grows here is only
        // the code under test.
        const TOTAL = 300 * 1024 * 1024;
        // A high-speed endpoint's shape, and the same numbers `device/memory.test.ts` uses: 64 KB
        // packets, 256 KB of writer credit. The device sinks its uploads, because a simulated
        // device that kept 300 MB would be the thing consuming the memory rather than the code.
        FAKE_SEND_CHUNK = 64 * 1024;
        const { watcher } = await connected({
            sinkUploads: true,
            bulkPacketSize: 64 * 1024,
            bulkHighWaterMark: 256 * 1024,
        });
        const client = watcher.current.client!;
        sendable.set("/maps/france.obcm", syntheticFile(TOTAL));

        const source = await nativeFileSource(1, "/maps/france.obcm");
        expect(source.totalLen).toBe(TOTAL);

        const base = process.memoryUsage().heapUsed;
        let peak = base;
        const result = await client.upload(ObjectType.Map, NEW_OBJECT_ID, source, {
            onProgress: () => (peak = Math.max(peak, process.memoryUsage().heapUsed)),
        });
        expect(result.committedOffset).toBe(TOTAL);

        const grew = peak - base;
        // Printed, not just asserted: "flat" should be a number someone can read in the CI log.
        console.log(`native 300 MB send: peak heap +${(grew / 1024 / 1024).toFixed(1)} MB`);
        expect(grew, `peak heap grew by ${grew} bytes over a 300 MB object`).toBeLessThan(16 * 1024 * 1024);
        await watcher.close();
    }, 300_000);
});

// --- build → send to device (E3, #913) -------------------------------------------

describe("the map this app just built", () => {
    it("goes from the maps folder to the device in one call", async () => {
        // The flow the whole epic aims at (#894): a build finishes, the rider plugs in, one click.
        // Everything here is the shipping path — `builtMap` is what the build card publishes,
        // `sendLocalMap` is what the Map surface calls, `localFileSource` is the session's, and the
        // object lands in C3's simulated device. Only the Tauri boundary is faked.
        const { watcher } = await connected();
        const session = new WatchedDeviceSession(watcher, "native");
        const map = new Uint8Array(96 * 1024).map((_, i) => (i * 7) & 0xff);
        sendable.set("/maps/black-forest.obcm", heldFile(map));
        builtMap.clear();
        builtMap.note({ path: "/maps/black-forest.obcm", filename: "black-forest.obcm", bytes: map.length });

        const open = session.localFileSource;
        expect(open, "a native session must offer the disk-to-endpoint path").toBeTruthy();
        const phases: string[] = [];
        const result = await sendLocalMap(
            watcher.current.client!,
            builtMap.current!,
            open!,
            {
                signal: new AbortController().signal,
                phase: (phase) => phases.push(phase),
                progress: () => undefined,
            },
        );

        expect(result.committedOffset).toBe(map.length);
        expect(device!.stored(ObjectType.Map, result.objectId)).toEqual(map);
        // "Reading" is the Rust-side fingerprint pass, which on a real map is seconds of disk and
        // would otherwise look like a stalled send.
        expect(phases).toEqual(["reading", "sending"]);
        // The acceptance criterion, stated as an assertion: no staging, no copy, and not one byte
        // of the map over the IPC. The webview wrote a descriptor and watched a progress channel.
        expect(calls.filter((c) => c.cmd === "usb_write" && c.options?.headers?.plane === "bulk")).toEqual([]);
        expect(calls.filter((c) => c.cmd === "usb_send_file")).toHaveLength(1);
        expect(calls.filter((c) => c.cmd === "usb_file_digest")).toHaveLength(1);
        await session.close();
    });

    it("addresses whatever link is open now, not the one it was built against", async () => {
        // Handles are per-open. A source bound to the handle a session had at page load would, after
        // an unplug and re-plug, stream into an endpoint that no longer exists — so the factory
        // reads the link at call time, and says so when there isn't one.
        attached = [];
        const watcher = new NativeWatcher();
        await watcher.start();
        await expect(watcher.localFileSource("/maps/black-forest.obcm")).rejects.toMatchObject({
            name: "PipeError",
            code: "closed",
        });

        wire = loopbackLink();
        device = new MockDevice(wire.device);
        void device.run();
        attached = [DEVICE];
        expect(await watcher.requestDevice()).toBe(true);
        sendable.set("/maps/black-forest.obcm", heldFile(new Uint8Array(64).fill(1)));
        await expect(watcher.localFileSource("/maps/black-forest.obcm")).resolves.toMatchObject({ totalLen: 64 });
        await watcher.close();
    });

    it("is absent on a watcher that has no disk under it", () => {
        // The gate is a property of the transport, not a host name — and this is the assertion that
        // keeps it that way, because `MapSend` renders the built-map row on exactly this check. A
        // browser's `WebUsbWatcher` has no `localFileSource`, so neither does the session over it.
        const idle: DeviceState = { status: "idle", client: null, identity: null, info: null, error: null };
        const pathless: DeviceWatcher = {
            current: idle,
            subscribe: (listener) => (listener(idle), () => undefined),
            requestDevice: async () => false,
            disconnect: async () => undefined,
            close: async () => undefined,
        };
        expect(new WatchedDeviceSession(pathless, "webusb").localFileSource).toBeUndefined();
    });
});

// --- the temp-file digest, for the record ---------------------------------------

describe("the digest a descriptor announces", () => {
    it("is the CRC-32 the whole toolchain agrees on", () => {
        // Pinned on this side too, so the Rust unit test and this one state the same constant:
        // crc32("123456789") == 0xCBF43926, which is also `specs/vectors/manifest.json`'s.
        const dir = mkdtempSync(join(tmpdir(), "obc-usb-digest-"));
        try {
            const path = join(dir, "check.bin");
            writeFileSync(path, "123456789");
            expect(Crc32.of(new Uint8Array(readFileSync(path)))).toBe(0xcbf43926);
        } finally {
            rmSync(dir, { recursive: true, force: true });
        }
    });
});
