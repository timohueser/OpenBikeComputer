import { describe, expect, it } from "vitest";

import { RETENTION_LEVELS, expiryPhrase, expiryWarns, retentionLabel } from "./retention";

describe("retentionLabel", () => {
    it("names all six wire levels", () => {
        expect(RETENTION_LEVELS.map(retentionLabel)).toEqual([
            "forever",
            "1 day",
            "1 week",
            "2 weeks",
            "1 month",
            "2 months",
        ]);
    });

    it("names an unknown level rather than hiding it", () => {
        expect(retentionLabel(9)).toBe("level 9");
    });
});

describe("expiryPhrase", () => {
    const now = Date.UTC(2026, 6, 29, 12, 0, 0);
    const at = (secondsFromNow: number) => Math.floor(now / 1000) + secondsFromNow;

    it("reads level 0 as kept forever, whatever the clock says", () => {
        expect(expiryPhrase({ retention: 0, expiresAt: 0 }, now)).toBe("kept forever");
        expect(expiryPhrase({ retention: 0, expiresAt: at(86_400) }, now)).toBe("kept forever");
    });

    it("reads a level with no started countdown as not started", () => {
        expect(expiryPhrase({ retention: 2, expiresAt: 0 }, now)).toBe("expiry not started");
    });

    it("counts days, rounding up, with tomorrow special-cased", () => {
        expect(expiryPhrase({ retention: 1, expiresAt: at(3_600) }, now)).toBe("expires tomorrow");
        expect(expiryPhrase({ retention: 2, expiresAt: at(86_400 + 3_600) }, now)).toBe("expires in 2 days");
        expect(expiryPhrase({ retention: 5, expiresAt: at(30 * 86_400) }, now)).toBe("expires in 30 days");
    });

    it("reads a passed deadline as expiring — the device deletes on its own schedule", () => {
        expect(expiryPhrase({ retention: 1, expiresAt: at(-60) }, now)).toBe("expiring");
    });

    it("warns exactly on a running countdown", () => {
        expect(expiryWarns({ retention: 0, expiresAt: 0 })).toBe(false);
        expect(expiryWarns({ retention: 2, expiresAt: 0 })).toBe(false);
        expect(expiryWarns({ retention: 2, expiresAt: at(3_600) })).toBe(true);
        expect(expiryWarns({ retention: 1, expiresAt: at(-60) })).toBe(true);
    });
});
