import { describe, expect, it } from "vitest";

import { commonPrefixName, sortForTrip } from "./multidrop";

describe("sortForTrip", () => {
    it("orders naturally, so day 10 follows day 2", () => {
        const files = [{ name: "tmb-day10.gpx" }, { name: "tmb-day2.gpx" }, { name: "tmb-day1.gpx" }];
        expect(sortForTrip(files).map((f) => f.name)).toEqual([
            "tmb-day1.gpx",
            "tmb-day2.gpx",
            "tmb-day10.gpx",
        ]);
    });
});

describe("commonPrefixName", () => {
    it("suggests the shared stem, shorn of day numbering", () => {
        expect(commonPrefixName(["tour-mont-blanc-day1.gpx", "tour-mont-blanc-day2.gpx"])).toBe(
            "tour-mont-blanc",
        );
        expect(commonPrefixName(["Jura Crest 1.gpx", "Jura Crest 2.gpx"])).toBe("Jura Crest");
        expect(commonPrefixName(["Etappe_1.gpx", "Etappe_2.gpx"])).toBe("Etappe");
    });

    it("suggests nothing rather than something silly", () => {
        expect(commonPrefixName([])).toBe("");
        expect(commonPrefixName(["alps.gpx", "coast.gpx"])).toBe("");
        expect(commonPrefixName(["a1.gpx", "a2.gpx"])).toBe("");
    });

    it("stays inside the trip name's 48-byte cap", () => {
        const long = "Ü".repeat(40); // 80 UTF-8 bytes shared by both names
        const name = commonPrefixName([`${long} one.gpx`, `${long} two.gpx`]);
        expect(new TextEncoder().encode(name).length).toBeLessThanOrEqual(48);
        expect(name.length).toBeGreaterThan(0);
    });
});
