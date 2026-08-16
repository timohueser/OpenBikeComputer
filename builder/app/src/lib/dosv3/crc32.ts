/**
 * CRC-32/IEEE, Device_Object_Protocol_v3.md §1.
 *
 * Reflected polynomial `0xEDB88320`, initial and final XOR `0xFFFFFFFF`, with
 * `crc32("123456789") == 0xCBF43926`. §1 is careful about what it is for: "It detects accidental
 * corruption; it is not identity, authentication, authorization, or an idempotency proof."
 *
 * This codec never computes an object's CRC — payload bytes do not pass through it — so the only
 * caller is the §8.2/§8.3 page cursor, whose trailing word is a CRC over the store identity and the
 * cursor's own first twelve bytes. That is what stops a cursor minted against one store or one
 * draft parent from being replayed against another.
 *
 * The v1 USB client carries its own copy for the legacy wire; this contract's cursor rule is not
 * that client's, and coupling a v3 codec to a stack the cutover deletes would be the wrong seam.
 */

/** The reflected CRC-32/IEEE table, built once at module load (256 × 4 bytes). */
const TABLE: Uint32Array = (() => {
    const table = new Uint32Array(256);
    for (let i = 0; i < 256; i++) {
        let c = i;
        for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
        table[i] = c >>> 0;
    }
    return table;
})();

/** Folds every segment in order and returns the finalized CRC-32/IEEE. */
export function crc32(...segments: readonly Uint8Array[]): number {
    let c = 0xffffffff;
    for (const bytes of segments) {
        for (let i = 0; i < bytes.length; i++) c = TABLE[(c ^ bytes[i]) & 0xff] ^ (c >>> 8);
    }
    return (c ^ 0xffffffff) >>> 0;
}

/** §1's check value, the one constant that proves the parameterization. */
export const CRC32_CHECK_INPUT = "123456789";
export const CRC32_CHECK_VALUE = 0xcbf43926;
