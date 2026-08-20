/**
 * The three things the builder writes to a device: a map, a route, a firmware image.
 *
 * **A map is one object.** There is no multi-file map upload here — no manifest, no shards, no
 * separate terrain file to order against them — so every write in this file is a single
 * `PUT` against one `.obcm`, `.obcr` or `UPDATE.BIN`, and there is no state that outlives a
 * transfer.
 *
 * All three are therefore the same shape underneath — declare the length, the whole-payload CRC, the
 * kind and a display name, stream the payload, read back what the commit published (§3.6) — which is
 * the point of the object model and the reason this file is short. What differs is what has to be
 * true *before* the first byte moves, and that is where the substance is:
 *
 * | | Where the bytes come from | Checked before sending |
 * | :-- | :-- | :-- |
 * | Map | a `.obcm` the rider picked | the selected map's current id + revision, when one exists |
 * | Route | a dropped GPX, converted by wasm | the OBCR header is read back and shown before sending |
 * | Firmware | an `UPDATE.BIN` | the whole OBCU container: header CRC, image CRC, slot ceiling |
 *
 * **"Uploaded" means the device has a valid object, in every one of them.** §3.6 has the device
 * verify the declared length and the whole-payload CRC, run the kind's validator, and only then
 * commit; anything else is an error response, which `FlatStoreClient.put` turns into a throw. There
 * is no path through this file that reports success for a half-written object.
 *
 * ## Cancelling and unplugging
 *
 * Neither is special-cased here, and that is deliberate. Every await takes the job's signal, and the
 * client already holds §3.6's rule: any break before the commit leaves the card as if nothing had
 * happened — the allocation is released, the written bytes are anonymous, the catalog is untouched.
 * So a cancelled or unplugged write leaves *both* ends clean, and the next attempt is an ordinary
 * first attempt rather than a recovery path.
 *
 * ## What is not here
 *
 * There is no free-space check before a map send. §5.2.2 retires the query, and §3.6 answers the
 * question at the point of decision: a `PUT` that does not fit is `noSpace`, whose context is the
 * bytes required. Asking in advance would be a second answer to a question the upload already
 * answers, and a stale one by the time the bytes arrive.
 */

import { blobSource, type FlatStoreClient } from "../usb/client";
import { EntryFlags, ObjectKind, type PutResponse } from "../usb/protocol";
import { truncateUtf8 } from "../format";
import { readUpdateImage, type UpdateImage } from "../firmware/obcu";
import type { JobContext } from "./progress";
import type { PreparedRoute } from "./route";

/** §3.6's display-name field: at most 48 UTF-8 bytes. */
export const DISPLAY_NAME_MAX = 48;

/** The coverage builder's one-click sink, kept as a type-only seam so the USB
 * implementation stays out of the builder entry chunk until a connected rider
 * actually presses Send. */
export type SendAssembledMap = (client: FlatStoreClient, ctx: JobContext) => Promise<PutResponse>;

/** A name the wire will take, trimmed on a byte boundary and never empty. */
export function displayName(name: string, fallback: string): string {
    return truncateUtf8(name.trim() || fallback, DISPLAY_NAME_MAX);
}

// The batch size is deliberately **not** overridden here. There is one throughput dial and it lives
// with the transport that pays for it: `DEFAULT_BATCH_BYTES` / `UPLOAD_WINDOW` in `../usb/client`.

/**
 * Send a `.obcm` Blob, either selected by the rider or produced by the assembler.
 *
 * No second staging copy: a picked `File` and the assembler's Blob are read twice in bounded
 * chunks — once for the CRC §3.6 declares, once to send. The assembler Blob is disk-backed when
 * writable OPFS had room for the run; its explicitly memory-priced fallback remains bounded by
 * the builder's preflight instead. That is what `blobSource` is for.
 *
 * Replaces the map the device will select: the active (non-retained) `MapShard` with the lowest
 * `ObjectId`. That is the firmware's deterministic selection rule, so a second send moves the map
 * the rider is actually using instead of accumulating an unreachable sibling. A card with no map
 * creates one. `LIST` supplies both compare-and-swap fields immediately before `PUT`.
 */
export async function sendMapBlob(
    client: FlatStoreClient,
    blob: Blob,
    filename: string,
    ctx: JobContext,
): Promise<PutResponse> {
    ctx.phase("reading", blob.size);
    const source = await blobSource(blob, {
        signal: ctx.signal,
        onProgress: (done, total) => ctx.progress(done, total),
    });
    const maps = await client.list({ kind: ObjectKind.MapShard, signal: ctx.signal });
    const current = maps.entries
        .filter((entry) => (entry.flags & EntryFlags.Retained) === 0)
        .reduce<(typeof maps.entries)[number] | null>((best, entry) => {
            if (!best || entry.objectId < best.objectId) return entry;
            if (entry.objectId === best.objectId && entry.revision > best.revision) return entry;
            return best;
        }, null);
    ctx.phase("sending", source.totalLen);
    return client.put(
        {
            objectId: current?.objectId,
            expectedRevision: current?.revision,
            kind: ObjectKind.MapShard,
            displayName: displayName(filename.replace(/\.obcm$/i, ""), "Map"),
        },
        source,
        {
            signal: ctx.signal,
            onProgress: (done, total) => ctx.progress(done, total),
            // The commit is bounded work the ordinary timeout covers with room to spare, and it
            // still takes long enough to be worth naming: the device has to land the last staging
            // half before it starts.
            onSent: () => ctx.phase("committing"),
        },
    );
}

/** Send a `.obcm` the rider selected from disk. */
export function sendMapFile(client: FlatStoreClient, file: File, ctx: JobContext): Promise<PutResponse> {
    return sendMapBlob(client, file, file.name, ctx);
}

/**
 * Send a converted route.
 *
 * Uploaded as a **create**: the device assigns the `ObjectId` and reports it in the answer. Dropping
 * the same GPX twice therefore makes two objects, where the v1 wire deduped on (length, CRC) — that
 * dedupe is gone with the envelope, and §3.4 says why the honest replacement is the client's: the
 * catalog carries every object's payload length and CRC, so a caller that wants convergence looks
 * before it uploads. `FlatStoreClient.findCreated` is that lookup, and the drop flow uses it to
 * reconcile an upload whose answer was lost rather than to pre-empt an ordinary second drop.
 */
export function sendRoute(client: FlatStoreClient, route: PreparedRoute, ctx: JobContext): Promise<PutResponse> {
    ctx.phase("sending", route.obcr.length);
    return client.put(
        { kind: ObjectKind.Route, displayName: displayName(route.header.name, "Route") },
        route.obcr,
        {
            signal: ctx.signal,
            onProgress: (done, total) => ctx.progress(done, total),
        },
    );
}

/** A verified update, and what the device will report once it is running. */
export interface StagedFirmware {
    readonly image: UpdateImage;
    readonly result: PutResponse;
}

/**
 * Verify an `UPDATE.BIN` and write it to the card as an update-package object (§4's kind 7).
 *
 * **Staging only.** Uploading never installs; `ARM` is the separate, explicit step
 * ({@link armUpdate}) that makes an installed image the next boot, and the two are different
 * decisions precisely so delivery cannot arm anything.
 *
 * It **replaces** an update package the card already holds rather than adding a second one, and
 * that is a client-side policy rather than a wire rule: §3 has no singleton slot, so without this
 * every staging attempt would leave another multi-megabyte object on the card for the rider to find
 * and remove. The compare-and-swap on the existing revision is what makes it safe — a package
 * something else replaced in between fails rather than clobbering.
 */
export async function stageFirmware(
    client: FlatStoreClient,
    bytes: Uint8Array,
    ctx: JobContext,
): Promise<StagedFirmware> {
    ctx.phase("verifying", bytes.length);
    const { image, container } = readUpdateImage(bytes);

    const held = await client.list({ kind: ObjectKind.UpdatePackage, signal: ctx.signal });
    // The head of whatever package is there. More than one is not a state this client creates, and
    // the newest is the one a rider would mean.
    const previous = held.entries.reduce<(typeof held.entries)[number] | null>(
        (best, entry) => (!best || entry.objectId > best.objectId ? entry : best),
        null,
    );

    ctx.phase("sending", container.length);
    const result = await client.put(
        {
            objectId: previous?.objectId,
            expectedRevision: previous?.revision,
            kind: ObjectKind.UpdatePackage,
            displayName: displayName(image.version, "Update"),
        },
        container,
        {
            signal: ctx.signal,
            onProgress: (done, total) => ctx.progress(done, total),
        },
    );
    return { image, result };
}

/**
 * Ask the device to make a staged package the next boot (§4's `ARM`).
 *
 * **The device's current policy refuses this**, answering `rejected` — a stated dev-window gap, not
 * a failure of this call. It is wired because the shape is settled and because a refusal a rider can
 * read is better than an affordance that quietly does nothing; the caller surfaces the refusal as
 * itself rather than as "installing…".
 *
 * On success the device commits a rollback reserve, writes the boot handoff, answers, and reboots —
 * so the answer is the last thing this link will hear from it.
 */
export function armUpdate(
    client: FlatStoreClient,
    staged: { objectId: bigint; revision: bigint },
    signal?: AbortSignal,
): Promise<{ rollbackObjectId: bigint; commitSequence: bigint }> {
    return client.arm({ objectId: staged.objectId, expectedRevision: staged.revision }, signal);
}
