/**
 * The three things the builder writes to a device: a map, a route, a firmware image.
 *
 * **A map is one object.** There is no multi-file map upload here — no manifest, no shards, no
 * separate terrain file to order against them — so every write in this file is a single
 * announce/stream/commit against one `.obcm`, `.obcr` or `UPDATE.BIN`, and there is no state that
 * outlives a transfer.
 *
 * All three are therefore the same six lines underneath — announce, stream, whole-object CRC,
 * commit — which is the point of the object model and the reason this file is short. What differs
 * is what has to be true *before* the first byte moves, and that is where the substance is:
 *
 * | | Where the bytes come from | Checked before sending |
 * | :-- | :-- | :-- |
 * | Map | a `.obcm` the rider picked | nothing to check it against; the device's CRC is the guarantee |
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

import { blobSource, type ProtocolClient, type UploadResult } from "../usb/client";
import { NEW_OBJECT_ID, ObjectType, SINGLETON_OBJECT_ID } from "../usb/protocol";
import { readUpdateImage, type UpdateImage } from "../firmware/obcu";
import type { JobContext } from "./progress";
import type { PreparedRoute } from "./route";

// The chunk size is deliberately **not** overridden here any more. It used to be a local 32 KiB —
// half the client's own default since the upload retune, so a map (the one object where the number
// matters) was quietly getting the *smaller* chunk. There is one throughput dial and it lives with
// the transport that pays for it: `DEFAULT_CHUNK_SIZE` / `UPLOAD_WINDOW` in `../usb/client`.

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
        // A map's commit is a close, an open, a 40-byte header read, a 4-byte write and a flush
        // (`Storage::map_upload_commit`) — bounded work the ordinary timeout covers with room to
        // spare. It does still take long enough to be worth naming, because the device also has to
        // land the last staging half before it starts.
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
