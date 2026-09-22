/**
 * Disk needed by one streamed assembly.
 *
 * Removed scratch streams are truncated immediately, so the physical scratch area follows the
 * concurrent streams. The factor is the assembler memory model's conservative bound for those
 * streams (`SPILL_PER_NAV_BYTE` in `apps/obc-web-assemble/src/estimate.rs`).
 */
const SCRATCH_PER_CORE_BYTE = 2.5;

export interface DiskLedger {
    totalBytes: number;
    core: { bytes: number };
    terrain: { bytes: number } | null;
}

/** New downloaded cells, assembled output, and peak retained merge scratch. */
export function projectedRunDiskBytes(ledger: DiskLedger, storedCellBytes = 0): number {
    const terrain = ledger.terrain?.bytes ?? 0;
    const newCellBytes = Math.max(0, ledger.totalBytes - terrain - storedCellBytes);
    return newCellBytes + ledger.totalBytes + SCRATCH_PER_CORE_BYTE * ledger.core.bytes;
}
