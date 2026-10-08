import { describe, expect, it } from "vitest";

import { projectedRunDiskBytes } from "./quota";

describe("projectedRunDiskBytes", () => {
    it("prices cells, output, and conservative live scratch for the published region shape", () => {
        expect(
            projectedRunDiskBytes({
                totalBytes: 889_017_984,
                core: { bytes: 305_599_488 },
                terrain: { bytes: 88_223_744 },
            }),
        ).toBe(2_453_810_944);
    });

    it("does not charge free quota again for cells already included in origin usage", () => {
        const ledger = { totalBytes: 100, core: { bytes: 20 }, terrain: { bytes: 10 } };
        expect(projectedRunDiskBytes(ledger, 30)).toBe(210);
        expect(projectedRunDiskBytes(ledger, 1_000)).toBe(150);
    });
});
