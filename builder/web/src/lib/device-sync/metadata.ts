/** Reads the rides that the card marks as taken from its Metadata object (`specs/ride-archive-metadata.md`). */

/** The exact source of a ride that a row of the Metadata object marks. */
export interface ArchivedRow {
    objectId: bigint;
    revision: bigint;
    payloadLength: bigint;
    payloadCrc32: number;
}

const HEADER = 32, ROW = 40, MAX_ROWS = 128;

/** The rows of an `OBRM` version 2 payload. It throws for another magic, version or layout. */
export function archivedRows(bytes: Uint8Array): ArchivedRow[] {
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    if (bytes.length < HEADER || String.fromCharCode(...bytes.subarray(0, 4)) !== 'OBRM' || view.getUint16(4, true) !== 2
        || view.getUint16(6, true) !== HEADER || view.getUint16(8, true) !== ROW) {
        throw new Error('The ride metadata of the device cannot be read.');
    }
    const count = view.getUint16(10, true);
    if (count > MAX_ROWS || bytes.length < HEADER + count * ROW) throw new Error('The ride metadata of the device cannot be read.');
    return Array.from({ length: count }, (_, i) => {
        const at = HEADER + i * ROW;
        return {
            objectId: view.getBigUint64(at, true),
            revision: view.getBigUint64(at + 8, true),
            payloadLength: view.getBigUint64(at + 16, true),
            payloadCrc32: view.getUint32(at + 24, true),
        };
    });
}
