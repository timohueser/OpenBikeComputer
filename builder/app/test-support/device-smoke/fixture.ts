/**
 * The map the device smoke sends, and what the device must say about it afterwards.
 *
 * The map is `apps/obc-sim/assets/grimsel-demo.obcm`, already in the tree: an OBCM v18 file with an
 * embedded OBCT v3 surface terrain region, whose source packages, producer commit, output digest and
 * terrain digest are pinned in `fixtures/sources/ride-assistant/grimsel-demo-v18.json`. Nothing is
 * assembled here and no byte is appended: a surface terrain region starts on a 512-byte boundary and
 * is a whole number of 512-byte blocks long, so this map's own length is exactly 512 × 19,713. That
 * is what puts the transfer on the packet boundary — the case where a host that forgets the
 * zero-length terminating packet hangs and a device that miscounts commits a short object.
 *
 * The expectations the device is measured against come from the file's own header rather than from
 * the provenance record's `bounds_lon_lat`: the record pins the box the map was *cut* on, and the
 * header carries the extent of what survived packing. The device logs the header's numbers, so the
 * header is what they are compared with.
 */

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";

/** Where the map and its provenance record live, relative to the repository root. */
export const FIXTURE_MAP = "apps/obc-sim/assets/grimsel-demo.obcm";
export const FIXTURE_RECORD = "fixtures/sources/ride-assistant/grimsel-demo-v18.json";

/** The USB bulk packet size the transfer has to land on. */
export const PACKET_BYTES = 512;

/** A bounding box in microdegrees, in the order the OBCM header stores it. */
export interface BoundingBox {
    readonly minLat: number;
    readonly minLon: number;
    readonly maxLat: number;
    readonly maxLon: number;
}

/** The map to send, and every value the run is allowed to compare against. */
export interface SmokeFixture {
    /** The display name the object is committed under. */
    readonly name: string;
    readonly bytes: Uint8Array;
    /** The digest the provenance record pins, re-computed over the bytes read from disk. */
    readonly sha256: string;
    /** The producer commit the provenance record pins. Recorded, never compared. */
    readonly producerCommit: string;
    readonly bbox: BoundingBox;
    /** The embedded terrain region's length in bytes, from the header. */
    readonly terrainBytes: number;
}

/** What the OBCM header says, for a file the reader has already accepted as v18. */
interface Header {
    readonly bbox: BoundingBox;
    readonly terrainBytes: number;
}

const OBCM_VERSION = 18;

/**
 * Read the header fields the device echoes at boot.
 *
 * Only the two the observation needs: §1 offsets 5..21 are the bbox as lat/lon/lat/lon, and the
 * terrain region's length at offset 45 is a count of offset units whose base-2 logarithm is at
 * offset 40.
 */
export function readHeader(bytes: Uint8Array): Header {
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const magic = String.fromCharCode(...bytes.subarray(0, 4));
    if (magic !== "OBCM") throw new Error(`${FIXTURE_MAP} does not start with OBCM.`);
    const version = view.getUint8(4);
    if (version !== OBCM_VERSION) throw new Error(`${FIXTURE_MAP} is OBCM v${version}, not v${OBCM_VERSION}.`);
    const scale = view.getUint8(40);
    return {
        bbox: {
            minLat: view.getInt32(5, true),
            minLon: view.getInt32(9, true),
            maxLat: view.getInt32(13, true),
            maxLon: view.getInt32(17, true),
        },
        terrainBytes: view.getUint32(45, true) * 2 ** scale,
    };
}

/**
 * Load the map and refuse it unless it is still the file the provenance record describes and still
 * lands on the packet boundary.
 *
 * Both checks run before a byte moves, because either failure means the run would be measuring
 * something other than what its report claims.
 */
export function loadSmokeFixture(repoRoot: string): SmokeFixture {
    const bytes = new Uint8Array(readFileSync(join(repoRoot, FIXTURE_MAP)));
    const record = JSON.parse(readFileSync(join(repoRoot, FIXTURE_RECORD), "utf8")) as {
        map: { bytes: number; sha256: string };
        terrain: { bytes: number };
        producer_commit: string;
    };
    const sha256 = createHash("sha256").update(bytes).digest("hex");
    if (bytes.length !== record.map.bytes || sha256 !== record.map.sha256) {
        throw new Error(
            `${FIXTURE_MAP} is ${bytes.length} B / ${sha256}, and ${FIXTURE_RECORD} pins ` +
                `${record.map.bytes} B / ${record.map.sha256}. Rebuild the map or update its record.`,
        );
    }
    if (bytes.length % PACKET_BYTES !== 0) {
        throw new Error(
            `${FIXTURE_MAP} is ${bytes.length} B, which is not a whole number of ${PACKET_BYTES}-byte ` +
                "packets, so it no longer exercises the transfer boundary.",
        );
    }
    const header = readHeader(bytes);
    if (header.terrainBytes !== record.terrain.bytes) {
        throw new Error(
            `${FIXTURE_MAP} carries a ${header.terrainBytes} B terrain region and ${FIXTURE_RECORD} ` +
                `pins ${record.terrain.bytes} B.`,
        );
    }
    return {
        name: "grimsel-demo.obcm",
        bytes,
        sha256,
        producerCommit: record.producer_commit,
        bbox: header.bbox,
        terrainBytes: header.terrainBytes,
    };
}
