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

/** Bytes handed to the bulk pipe at a time. A map is minutes long either way; this only decides
 *  how often progress moves and how much is in flight, so it stays modest. */
const MAP_CHUNK = 32 * 1024;

/** One file streamed out of the assembler worker, in `OBCA_Spec.md` §5.4's order: every OBCM shard,
 *  then the terrain raster if the set has one, then the manifest that makes them a map. */
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
    /** Whether the set's terrain shard has been sent (#1044). The manifest is one 56-byte record
     *  longer when it has, which is exactly what the device checks at the manifest's announce. */
    terrainSent: boolean;
    setId: number | null;
}

/** The coverage assembler's device sink, passed across the step-3/step-4 component seam. */
export type SendAssembledMap = (client: ProtocolClient, ctx: JobContext) => Promise<UploadResult>;

export function setSendState(shardCount: number, totalBytes: number): SetSendState {
    if (!Number.isInteger(shardCount) || shardCount < 1 || shardCount > 32) {
        throw new Error(`The assembled map contains ${shardCount} shards; a set must contain 1–32.`);
    }
    return { shardCount, totalBytes, committedBytes: 0, nextShard: 0, terrainSent: false, setId: null };
}

/**
 * Verify and send one worker-produced file, retrying one whole-file CRC refusal.
 *
 * **The one rule that is not local to a file** (#1044): the manifest's announced length is
 * `72 + 56 × Shard Count`, and `OBCA_Spec.md` §5.2's `Shard Count` counts every **record** — the
 * terrain one included. A device therefore derives the length it expects from what it has actually
 * received, so the raster must reach it *before* the manifest or the whole set is refused at its
 * last transfer. The assembler already emits shards → terrain → manifest; this asserts that order
 * rather than assuming it, because getting it wrong costs a multi-gigabyte upload.
 */
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
    const manifest = file.role === "manifest";
    const terrain = file.role === "terrain";
    // Everything but the manifest is content-addressed by the assembler, so its digest is checked
    // here — before the bytes cost minutes on the wire — rather than trusted.
    if (!manifest) {
        const digest = new Sha256().update(file.bytes).hex();
        if (digest !== file.sha256.toLowerCase()) {
            throw new Error(`${file.name} failed its SHA-256 check before it reached the device.`);
        }
    }
    if (terrain) {
        // The raster goes out under its own object type (#1044). It is **not** a shard: a shard's
        // object id is a `(count, index)` pair naming one of the OBCM files the manifest's leading
        // records describe, and sending the raster as one would consume an index the manifest never
        // names. Its place in the order is fixed by the device's manifest-length check — every
        // shard first, then this, then the manifest — which is also the order the assembler emits.
        if (state.nextShard !== state.shardCount) {
            throw new Error(
                `The terrain shard arrived after ${state.nextShard} of ${state.shardCount} shards; ` +
                    "a set's raster follows every shard and precedes the manifest.",
            );
        }
        if (state.terrainSent) throw new Error("The assembler produced a second terrain shard; a set carries one.");
        ctx.part?.(state.shardCount, state.shardCount, "elevation");
    } else if (!manifest) {
        if (state.nextShard >= state.shardCount) {
            throw new Error("The assembler produced more shards than its summary.");
        }
        ctx.part?.(state.nextShard + 1, state.shardCount);
    } else {
        if (state.nextShard !== state.shardCount) {
            throw new Error(`The set manifest arrived after ${state.nextShard} of ${state.shardCount} shards.`);
        }
        ctx.part?.(state.shardCount, state.shardCount, "sealing map");
    }

    const type = manifest ? ObjectType.MapSet : terrain ? ObjectType.TerrainShard : ObjectType.MapShard;
    const objectId = manifest || terrain ? NEW_OBJECT_ID : setPartId(state.shardCount, state.nextShard);
    let result: UploadResult | null = null;
    for (let attempt = 0; attempt < 2; attempt++) {
        try {
            result = await client.upload(type, objectId, bytesSource(file.bytes), {
                signal: ctx.signal,
                chunkSize: MAP_CHUNK,
                onProgress: (done) => ctx.progress(state.committedBytes + done, state.totalBytes),
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
    else if (terrain) state.terrainSent = true;
    else state.nextShard += 1;
}

/** Delete every shard staged for an incomplete set. Safe after active-transfer cancellation too. */
export async function abandonAssembledSet(client: ProtocolClient, state: SetSendState): Promise<void> {
    if (state.nextShard === 0 || state.setId !== null) return;
    try {
        await client.abandonMapSet(setPartId(state.shardCount, Math.min(state.nextShard, state.shardCount - 1)));
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
        chunkSize: MAP_CHUNK,
        onProgress: (done, total) => ctx.progress(done, total),
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
