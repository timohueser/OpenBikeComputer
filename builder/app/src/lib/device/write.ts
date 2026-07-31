/**
 * The three things the hosted site writes to a device (C4, #903): a map, a route, a firmware image.
 *
 * All three are the same six lines underneath — announce, stream, whole-object CRC, commit — which
 * is the point of the object model and the reason this file is short. What differs is what has to
 * be true *before* the first byte moves, and that is where the substance is:
 *
 * | | Where the bytes come from | Checked before sending |
 * | :-- | :-- | :-- |
 * | Map (catalog) | the CDN, streamed into a scratch file | size + SHA-256 against the manifest (`OBCC_Spec.md` §7) |
 * | Map (file) | a file the rider picked | nothing to check it against; the device's CRC is the guarantee |
 * | Map (built here) | the app's own maps folder, read by Rust | nothing to check it against; it was produced by the packer in this process |
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
import type { LocalFileSource } from "../usb/session";
import { readUpdateImage, type UpdateImage } from "../firmware/obcu";
import type { JobContext } from "./progress";
import { stageStream, type StagingArea } from "./staging";
import type { PreparedRoute } from "./route";
import { Sha256 } from "./sha256";

/**
 * A catalog artifact, reduced to what sending it needs.
 *
 * Structurally a subset of `OBCC_Spec.md` §3's `ArtifactEntry`, so C1's parsed manifest entries
 * pass straight in — this file does not import the catalog, because a map file the rider picked
 * has no manifest behind it and must go down the same path.
 */
export interface MapArtifact {
    /** What to call the scratch file and what to show while it transfers. */
    readonly filename: string;
    readonly url: string;
    readonly bytes: number;
    /** Lowercase hex SHA-256, from the manifest. Verified before a byte reaches the device. */
    readonly sha256: string;
}

/** Bytes handed to the bulk pipe at a time. A map is minutes long either way; this only decides
 *  how often progress moves and how much is in flight, so it stays modest. */
const MAP_CHUNK = 32 * 1024;

/** One file streamed out of the assembler worker. Shards precede the manifest. */
export interface AssembledSetFile {
    readonly name: string;
    readonly role: "core" | "coarse" | "geometry" | "manifest";
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
 * Fetch a catalog artifact, verify it, and send it.
 *
 * The download is streamed into `area` — never into the tab's heap — while the CRC-32 and the
 * SHA-256 fold in on the way past. Only once the digest and the length match the manifest does
 * anything reach the device, which is `OBCC_Spec.md` §7's rule made structural: the scratch file is
 * deleted on a mismatch and the caller never gets a handle to it.
 *
 * The scratch copy is always cleaned up, including on a cancel — a map upload that leaves a
 * hundreds-of-megabyte orphan in the origin's storage every time the rider changes their mind is
 * its own bug.
 */
export async function sendCatalogMap(
    client: ProtocolClient,
    artifact: MapArtifact,
    area: StagingArea,
    ctx: JobContext,
): Promise<UploadResult> {
    ctx.phase("downloading", artifact.bytes);
    const response = await fetch(artifact.url, { signal: ctx.signal });
    if (!response.ok) {
        throw new Error(`The map could not be downloaded (HTTP ${response.status}).`);
    }
    if (!response.body) {
        throw new Error("This browser did not give the download a readable stream.");
    }
    const staged = await stageStream(response.body, {
        area,
        name: scratchName(artifact.filename),
        expect: { bytes: artifact.bytes, sha256: artifact.sha256 },
        signal: ctx.signal,
        onProgress: (done, total) => ctx.progress(done, total),
    });
    try {
        ctx.phase("sending", staged.bytes);
        return await client.upload(ObjectType.Map, NEW_OBJECT_ID, staged.source, {
            signal: ctx.signal,
            chunkSize: MAP_CHUNK,
            onProgress: (done, total) => ctx.progress(done, total),
        });
    } finally {
        await staged.discard();
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
 * Send a `.obcm` this app built, straight off the disk it was written to (E3 #913).
 *
 * The flow #894 is aiming at, and the only one where **no byte of the map is ever in this process**.
 * `open` hands back a source whose length and CRC-32 were computed in Rust and whose `sendTo`
 * streams the file into the bulk endpoint from there; `ProtocolClient.upload` prefers that over its
 * own chunk loop, so the webview writes a 12-byte descriptor and then watches a progress channel.
 *
 * "No intermediate file" in the acceptance sense means exactly that: nothing is staged, copied or
 * duplicated. The `.obcm` itself is not an intermediate — it is the build's product, and it lands
 * in the rider's maps folder whether or not a device is plugged in, which is most of why the
 * desktop tier exists. Streaming a *build* into the endpoint is not merely unimplemented but
 * unreachable: the transfer descriptor announces the whole object's length and CRC-32 before the
 * first byte moves (§4.2), and neither is known until the packer has written its last chunk.
 */
export async function sendLocalMap(
    client: ProtocolClient,
    map: { readonly path: string; readonly bytes: number },
    open: LocalFileSource,
    ctx: JobContext,
): Promise<UploadResult> {
    // Fingerprinting re-reads the file in Rust; on a several-hundred-megabyte map that is seconds
    // of disk, so it gets its own phase rather than looking like a stalled send.
    ctx.phase("reading", map.bytes);
    const source = await open(map.path);
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

/** A scratch-file name: one per artifact, so a re-run replaces rather than accumulates, and no
 *  path separators reach a file-system API. */
function scratchName(filename: string): string {
    const safe = filename.replace(/[^A-Za-z0-9._-]/g, "_").slice(-64);
    return `obc-staging-${safe || "map.obcm"}`;
}
