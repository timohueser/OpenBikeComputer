/**
 * The update check: the version dialect, and what happens when the manifest isn't there yet.
 *
 * The one behaviour with a locked decision behind it is the *refusal*: #773 states that a device
 * reporting a git hash rather than a release version is never offered an auto-update. So the tests
 * that matter most here are the ones proving an unparseable version produces `unknown` and not
 * "you're out of date".
 */

import { describe, expect, it } from "vitest";

import {
    FirmwareManifestError,
    compareVersions,
    fetchFirmwareRelease,
    parseFirmwareManifest,
    parseVersion,
    updateStatus,
} from "./release";

const MANIFEST = JSON.stringify({
    version: "1.4.0",
    size: 812_345,
    sha256: "a".repeat(64),
    url: "https://github.com/timohueser/OpenBikeComputer/releases/download/v1.4.0/UPDATE.BIN",
    notes: "https://github.com/timohueser/OpenBikeComputer/releases/tag/v1.4.0",
    signature: "ignored-by-this-parser",
});

describe("parseFirmwareManifest", () => {
    it("reads the fields the check needs and ignores the rest", () => {
        const release = parseFirmwareManifest(MANIFEST);
        expect(release.version).toBe("1.4.0");
        expect(release.bytes).toBe(812_345);
        expect(release.sha256).toBe("a".repeat(64));
        expect(release.notes).toContain("releases/tag/v1.4.0");
    });

    it("rejects a manifest that is malformed rather than guessing at it", () => {
        const bad = [
            "not json",
            "[]",
            JSON.stringify({ version: "1.4.0", size: 1, sha256: "a".repeat(64) }), // no url
            JSON.stringify({ version: "abc1234", size: 1, sha256: "a".repeat(64), url: "https://x/" }),
            JSON.stringify({ version: "1.4.0", size: 0, sha256: "a".repeat(64), url: "https://x/" }),
            JSON.stringify({ version: "1.4.0", size: 1, sha256: "nope", url: "https://x/" }),
            JSON.stringify({ version: "1.4.0", size: 1, sha256: "a".repeat(64), url: "http://x/" }),
        ];
        for (const body of bad) {
            expect(() => parseFirmwareManifest(body), body.slice(0, 40)).toThrow(FirmwareManifestError);
        }
    });
});

describe("fetchFirmwareRelease", () => {
    const response = (status: number, body = "") =>
        ({ status, ok: status >= 200 && status < 300, text: async () => body }) as Response;

    it("treats a 404 as 'nothing published yet', not an error", async () => {
        const release = await fetchFirmwareRelease({ fetch: async () => response(404) });
        expect(release).toBeNull();
    });

    it("surfaces a server error", async () => {
        await expect(fetchFirmwareRelease({ fetch: async () => response(500) })).rejects.toBeInstanceOf(
            FirmwareManifestError,
        );
    });

    it("parses a published manifest", async () => {
        const release = await fetchFirmwareRelease({ fetch: async () => response(200, MANIFEST) });
        expect(release?.version).toBe("1.4.0");
    });
});

describe("the version dialect", () => {
    it("ignores build metadata, which is what DIS reports today", () => {
        expect(parseVersion("1.2.0+abc1234")).toEqual({ major: 1, minor: 2, patch: 0, pre: null });
        expect(compareVersions("1.2.0+abc1234", "1.2.0")).toBe(0);
        expect(compareVersions("v1.2.0", "1.2.0")).toBe(0);
    });

    it("refuses to compare a git hash", () => {
        expect(parseVersion("abc1234")).toBeNull();
        expect(compareVersions("abc1234", "1.4.0")).toBeNull();
    });

    it("orders releases and their pre-releases", () => {
        expect(compareVersions("1.3.0", "1.4.0")).toBeLessThan(0);
        expect(compareVersions("1.4.1", "1.4.0")).toBeGreaterThan(0);
        expect(compareVersions("2.0.0", "1.99.99")).toBeGreaterThan(0);
        expect(compareVersions("1.4.0-rc1", "1.4.0")).toBeLessThan(0);
        expect(compareVersions("1.4.0-rc1", "1.4.0-rc2")).toBeLessThan(0);
    });
});

describe("updateStatus", () => {
    it("never offers an update to a device running an unparseable version", () => {
        // #773's locked behaviour: a probe-flashed dev build reports a hash, and the answer is
        // "cannot say" — collapsing that into "older" would push firmware onto a dev device.
        expect(updateStatus("abc1234", "1.4.0")).toBe("unknown");
        expect(updateStatus(null, "1.4.0")).toBe("unknown");
    });

    it("says nothing at all when there is no published release", () => {
        expect(updateStatus("1.3.0", null)).toBe("no-release");
    });

    it("distinguishes older, current and ahead", () => {
        expect(updateStatus("1.3.0", "1.4.0")).toBe("available");
        expect(updateStatus("1.4.0+deadbee", "1.4.0")).toBe("current");
        expect(updateStatus("1.5.0", "1.4.0")).toBe("ahead");
    });
});
