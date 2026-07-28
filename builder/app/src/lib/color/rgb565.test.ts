import { describe, expect, it } from "vitest";
import { formatRgb565, hexToRgb565, parseRgb565, rgb565ToDeviceHex, rgb565ToHex } from "./rgb565";

describe("rgb565", () => {
    it("parses hex strings with and without 0x, and numbers", () => {
        expect(parseRgb565("0xFAA0")).toBe(0xfaa0);
        expect(parseRgb565("FAA0")).toBe(0xfaa0);
        expect(parseRgb565(64160)).toBe(0xfaa0);
    });

    it("formats canonically", () => {
        expect(formatRgb565(0xfaa0)).toBe("0xFAA0");
        expect(formatRgb565(0x001f)).toBe("0x001F");
    });

    it("round-trips grid colors through CSS hex", () => {
        // Colors on the RGB222 grid survive 565 -> hex -> 565 unchanged.
        for (const c of ["0xFAA0", "0x555F", "0xAD55", "0x0000", "0xFFFF", "0x501F"]) {
            expect(hexToRgb565(rgb565ToHex(c))).toBe(c);
        }
    });

    it("quantizes to the device's 64-color gamut", () => {
        // The default preset's motorway orange lands on (255, 85, 0).
        expect(rgb565ToDeviceHex("0xFAA0")).toBe("#ff5500");
        expect(rgb565ToDeviceHex("0xFFFF")).toBe("#ffffff");
        expect(rgb565ToDeviceHex("0x0000")).toBe("#000000");
        // Marker red: pure R31 -> (255, 0, 0).
        expect(rgb565ToDeviceHex("0xF800")).toBe("#ff0000");
    });
});
