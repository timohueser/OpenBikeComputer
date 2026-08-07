/**
 * The three things the builder writes to a device: a map file, a route, a firmware image.
 *
 * All three are the same six lines underneath — announce, stream, whole-object CRC, commit — which
 * is the point of the object model and the reason this file is short. What differs is what has to
 * be true *before* the first byte moves, and that is where the substance is:
 *
 * | | Where the bytes come from | Checked before sending |
 * | :-- | :-- | :-- |
 * | Map (file) | a file the rider picked | nothing to check it against; the device's CRC is the guarantee |
 * | Route | a dropped GPX, converted by wasm | the OBCR header is read back and shown before sending |
 * | Firmware | an `UPDATE.BIN` | the whole OBCU container: header CRC, image CRC, slot ceiling |
 *
 * **"Uploaded" means the device has a valid file, in every one of them.** The whole-object CRC-32
 * is announced up front and verified by the device at commit; anything else is a `transferResult`
 * that is not `committed`, and `ProtocolClient.upload` turns that into a throw. There is no path
 * through this file that reports success for a half-written object.
 *
 * ## Cancelling and unplugging
 *
 * Neither is special-cased here, and that is deliberate. Every await takes the job's signal, and
 * the client already holds the spec's §4.1 rule: an exchange that does not reach its correlated
 * close sends the device an abort, waits for it to say it has drained, and resets the pipe before
 * releasing the transfer slot. So a cancelled or unplugged write leaves *both* ends clean, and the
 * next attempt is an ordinary first attempt rather than a recovery path — which is exactly why
 * `flows.test.ts` retries on the same link and expects it to just work.
 */

import { DeviceError, bytesSource, type ProtocolClient, type UploadResult } from "../usb/client";
import { blobSource } from "../usb/client";
import { NEW_OBJECT_ID, ObjectType, SINGLETON_OBJECT_ID, setPartId } from "../usb/protocol";
import { readUpdateImage, type UpdateImage } from "../firmware/obcu";
import type { JobContext } from "./progress";
import type { PreparedRoute } from "./route";
import { Sha256 } from "./sha256";

// The chunk size is deliberately **not** overridden here any more. It used to be a local 32 KiB —
// half the client's own default since the upload retune, so a map (the one object where the number
// matters) was quietly getting the *smaller* chunk. There is one throughput dial and it lives with
// the transport that pays for it: `DEFAULT_CHUNK_SIZE` / `UPLOAD_WINDOW` in `../usb/client`.

/** One file streamed out of the assembler worker. Shards precede the manifest. */
export interface AssembledSetFile {
    readonly name: string;
    readonly role: "core" | "coarse" | "geometry" | "terrain" | "manifest";
    readonly sha256: string;
    readonly byteLength: number;
    readonly bytes: Uint8Array;
}

/** State kept across the independent whole-file transfers that form one set. */
export interface SetSendState {
    readonly shardCount: number;
    readonly totalBytes: number;
    committedBytes: number;
    nextShard: number;
    setId: number | null;
}

/** The coverage assembler's device sink, passed across the step-3/step-4 component seam. */
export type SendAssembledMap = (client: ProtocolClient, ctx: JobContext) => Promise<UploadResult>;

export function setSendState(shardCount: number, totalBytes: number): SetSendState {
    if (!Number.isInteger(shardCount) || shardCount < 1 || shardCount > 32) {
        throw new Error(`The assembled map contains ${shardCount} shards; a set must contain 1–32.`);
    }
    return { shardCount, totalBytes, committedBytes: 0, nextShard: 0, setId: null };
}

/** Verify and send one worker-produced file, retrying one whole-file CRC refusal. */
export async function sendAssembledSetFile(
    client: ProtocolClient,
    state: SetSendState,
    file: AssembledSetFile,
    ctx: JobContext,
): Promise<void> {
    if (file.bytes.byteLength !== file.byteLength) {
        throw new Error(
            `${file.name} arrived as ${file.bytes.byteLength} bytes; the assembler announced ${file.byteLength}.`,
        );
    }
    // The terrain shard has no `mapShard` object type yet — the device-side set
    // transfer (#1044) predates EL4's `terrain` role and knows nothing about a
    // raster. Sending it as an ordinary shard would consume a shard index the
    // manifest does not name and desynchronise the whole set, so it is skipped
    // **loudly** rather than misfiled: the map still arrives complete, without
    // elevation, which is exactly what a terrain-less set already is (§13).
    if (file.role === "terrain") {
        console.warn(
            `obc: skipping ${file.name} — sending a set's terrain shard to a device needs the transfer step ` +
                "(#1044) to learn the terrain role. The map is sent whole; its profiles will be flat.",
        );
        return;
    }
    const manifest = file.role === "manifest";
    if (!manifest) {
        if (state.nextShard >= state.shardCount) {
            throw new Error("The assembler produced more shards than its summary.");
        }
        const digest = new Sha256().update(file.bytes).hex();
        if (digest !== file.sha256.toLowerCase()) {
            throw new Error(`${file.name} failed its SHA-256 check before it reached the device.`);
        }
        ctx.part?.(state.nextShard + 1, state.shardCount);
    } else {
        if (state.nextShard !== state.shardCount) {
            throw new Error(`The set manifest arrived after ${state.nextShard} of ${state.shardCount} shards.`);
        }
        ctx.part?.(state.shardCount, state.shardCount, "sealing map");
    }

    const type = manifest ? ObjectType.MapSet : ObjectType.MapShard;
    const objectId = manifest ? NEW_OBJECT_ID : setPartId(state.shardCount, state.nextShard);
    let result: UploadResult | null = null;
    for (let attempt = 0; attempt < 2; attempt++) {
        try {
            // The manifest's first attempt leaves the phase at `committing`; a CRC refusal means the
            // bytes go again, so put the label back before they do. Without this the retry streams
            // under "Finishing on the device", which is the one phase that promises nothing is
            // moving.
            if (attempt > 0) ctx.phase("sending", state.totalBytes);
            result = await client.upload(type, objectId, bytesSource(file.bytes), {
                signal: ctx.signal,
                onProgress: (done) => ctx.progress(state.committedBytes + done, state.totalBytes),
                // **The manifest is the set's commit point, and it is tiny.** Committing it re-opens
                // and cross-checks every shard header already on the card, so its wait has to be
                // budgeted against the set rather than against the ~2 KB that just moved. Getting
                // this wrong loses data rather than time: a timed-out wait fires an `op = 3` abort,
                // and the device answers that by deleting the whole set — including one it may have
                // just committed successfully.
                //
                // A **shard** deliberately does not get this: its commit is a header check like any
                // other upload's, so it keeps the ordinary timeout and stays quick to fail.
                commitBytes: manifest ? state.totalBytes : undefined,
                onSent: manifest ? () => ctx.phase("committing") : undefined,
            });
            break;
        } catch (cause) {
            if (!(cause instanceof DeviceError) || cause.code !== "crc-mismatch" || attempt === 1) throw cause;
        }
    }
    if (!result) throw new Error(`${file.name} did not receive a transfer result.`);
    state.committedBytes += file.byteLength;
    ctx.progress(state.committedBytes, state.totalBytes);
    if (manifest) state.setId = result.objectId;
    else state.nextShard += 1;
}

/** Delete every shard staged for an incomplete set. Safe after active-transfer cancellation too. */
export async function abandonAssembledSet(client: ProtocolClient, state: SetSendState): Promise<void> {
    if (state.nextShard === 0 || state.setId !== null) return;
    try {
        await client.abandonMapSet();
    } catch {
        // Best effort: preserve the original assembly/transport failure. A disconnect also makes
        // firmware delete the staged set when its USB plane tears down.
    }
}

/**
 * Send a `.obcm` the rider already has.
 *
 * No staging: a `File` is *already* a handle to bytes on disk, so it is read twice straight from
 * there — once for the CRC the descriptor announces, once to send — and nothing is copied anywhere.
 * That is what `blobSource` is for, and it is why this path costs nothing extra despite being the
 * one a 300 MB file is most likely to arrive on.
 */
export async function sendMapFile(client: ProtocolClient, file: File, ctx: JobContext): Promise<UploadResult> {
    ctx.phase("reading", file.size);
    const source = await blobSource(file);
    ctx.phase("sending", source.totalLen);
    return client.upload(ObjectType.Map, NEW_OBJECT_ID, source, {
        signal: ctx.signal,
        onProgress: (done, total) => ctx.progress(done, total),
        // No `commitBytes`: a map's commit is a close, an open, a 40-byte header read, a 4-byte
        // write and a flush (`Storage::map_upload_commit`) — bounded work the ordinary timeout
        // covers with room to spare. It does still take long enough to be worth naming, because the
        // device also has to land the last staging half before it starts.
        onSent: () => ctx.phase("committing"),
    });
}

/**
 * Send a converted route.
 *
 * Uploaded as a **new** object rather than replacing one: the device assigns the id, and a route
 * whose bytes it already holds dedups to the existing id instead of minting a twin (§4.1). So
 * dropping the same GPX twice is a no-op, not a duplicate in the rider's route list.
 */
export function sendRoute(client: ProtocolClient, route: PreparedRoute, ctx: JobContext): Promise<UploadResult> {
    ctx.phase("sending", route.obcr.length);
    return client.upload(ObjectType.Route, NEW_OBJECT_ID, route.obcr, {
        signal: ctx.signal,
        onProgress: (done, total) => ctx.progress(done, total),
    });
}

/** A verified update, and what the device will report once it is running. */
export interface StagedFirmware {
    readonly image: UpdateImage;
    readonly result: UploadResult;
}

/**
 * Verify an `UPDATE.BIN` and write it to the device's card as the staged image.
 *
 * **Staging only.** This is where C4 stops and #728's install semantics take over: the device
 * writes the container to `/UPDATE.BIN` verbatim, and nothing is armed, erased or rebooted by this
 * call. Installing is a separate, explicit ask ({@link askToInstall}) that the rider still has to
 * confirm on the glass — the spec's security posture is that a link may stage an image and can
 * never arm one, and there are no silent installs, ever.
 */
export async function stageFirmware(
    client: ProtocolClient,
    bytes: Uint8Array,
    ctx: JobContext,
): Promise<StagedFirmware> {
    ctx.phase("verifying", bytes.length);
    const { image, container } = readUpdateImage(bytes);
    ctx.phase("sending", container.length);
    const result = await client.upload(ObjectType.FwImage, SINGLETON_OBJECT_ID, container, {
        signal: ctx.signal,
        onProgress: (done, total) => ctx.progress(done, total),
    });
    return { image, result };
}

/**
 * Ask the device to install what is staged.
 *
 * `ok` means the *request* was accepted, and nothing more: the device runs its own scan, shows a
 * confirm card, and installs only on a physical Select press. Copy that says anything stronger than
 * "confirm it on the device" is wrong.
 */
export function askToInstall(client: ProtocolClient, signal?: AbortSignal): Promise<void> {
    return client.installFw(signal);
}
