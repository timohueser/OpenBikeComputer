import { describe, expect, it } from "vitest";
import { EXAMPLE_ROOT } from "../catalog/v2/testdata";
import { peekSchemaVersion } from "./detect";

describe("peekSchemaVersion", () => {
    it("reads 2 off the real v2 example root", () => {
        expect(peekSchemaVersion(EXAMPLE_ROOT)).toBe(2);
    });

    it("reads 1 off a v1 manifest shell", () => {
        expect(peekSchemaVersion('{"schema_version": 1, "presets": [], "artifacts": []}')).toBe(1);
    });

    // Everything below routes to the v1 path on purpose: that parser owns the
    // error sentences for garbage, and detection must not invent its own.
    it("answers null for a body that is not JSON", () => {
        expect(peekSchemaVersion("<html>404</html>")).toBeNull();
    });

    it("answers null for JSON that is not an object", () => {
        expect(peekSchemaVersion("[2]")).toBeNull();
        expect(peekSchemaVersion("2")).toBeNull();
        expect(peekSchemaVersion("null")).toBeNull();
    });

    it("answers null when schema_version is missing or not a number", () => {
        expect(peekSchemaVersion("{}")).toBeNull();
        expect(peekSchemaVersion('{"schema_version": "2"}')).toBeNull();
    });
});
