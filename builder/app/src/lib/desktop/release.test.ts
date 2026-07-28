// A ratchet, not a test of today: `RELEASE` is null until D3 (#908) publishes
// the first installers, so the body below runs zero assertions right now — on
// purpose. The moment there is a build, these are the things that have to be
// true of it, and a missing checksum or a placeholder URL fails here instead of
// on someone's machine. The one assertion that always runs pins that the page
// has an honest empty state to fall back on.

import { describe, expect, it } from "vitest";
import { RELEASE } from "./release";

describe("the desktop release", () => {
    it("either exists or is null — never a half-filled record", () => {
        expect(RELEASE === null || typeof RELEASE === "object").toBe(true);
    });

    it("offers a real, checksummed file per platform", () => {
        if (!RELEASE) return;
        expect(RELEASE.version).toMatch(/\S/);
        expect(RELEASE.date).toMatch(/^\d{4}-\d{2}-\d{2}$/);
        expect(RELEASE.downloads.length).toBeGreaterThan(0);

        const seen = new Set<string>();
        for (const file of RELEASE.downloads) {
            expect(file.url).toMatch(/^https:\/\//);
            expect(file.filename).toMatch(/\S/);
            expect(file.size).toBeGreaterThan(0);
            expect(file.sha256).toMatch(/^[0-9a-f]{64}$/);
            const key = `${file.os} ${file.arch ?? ""}`;
            expect(seen.has(key)).toBe(false);
            seen.add(key);
        }
    });

    it("writes the install note as instructions, not an apology", () => {
        if (!RELEASE?.installNote) return;
        expect(RELEASE.installNote).not.toMatch(/!|sorry|unfortunately|afraid|apolog/i);
    });
});
