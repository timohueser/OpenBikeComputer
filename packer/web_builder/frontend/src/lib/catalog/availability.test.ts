// OBCC §6(c), the consumer's half of the version law: with a known target
// firmware, an artifact it cannot read is shown as unsupported *with the
// reason* rather than hidden or silently offered; with no device attached the
// download is offered and the version stated. The hosted tier's normal case is
// the second one — nothing plugged in, and on most browsers nothing that could
// be.

import { describe, expect, it } from "vitest";
import { artifactState, regionState, stylingLagsPreset } from "./availability";
import type { CatalogArtifact } from "./manifest";
import type { RegionEntry } from "./regions";

function artifact(obcmVersion: number, presetId = "default"): CatalogArtifact {
    return {
        region_id: "europe/switzerland",
        region_name: "Switzerland",
        preset_id: presetId,
        preset_version: 2,
        obcm_version: obcmVersion,
        bytes: 1024,
        sha256: "0".repeat(64),
        bbox: { min_lat: 0, min_lon: 0, max_lat: 1, max_lon: 1 },
        built_at: "2026-07-20T02:14:07Z",
        source_snapshot: "2026-07-19",
        url: "https://maps.example.org/x.obcm",
    };
}

function entry(artifacts: CatalogArtifact[]): RegionEntry {
    return {
        id: "switzerland",
        path: "europe/switzerland",
        name: "Switzerland",
        parent: "europe",
        artifacts,
    };
}

describe("with no device attached", () => {
    it("offers the download whatever version it is", () => {
        expect(artifactState(artifact(10), null)).toEqual({ kind: "available" });
        expect(artifactState(artifact(11), null)).toEqual({ kind: "available" });
    });
});

describe("with a device attached", () => {
    it("offers what the firmware reads", () => {
        expect(artifactState(artifact(10), { obcmVersion: 10 })).toEqual({ kind: "available" });
    });

    it("refuses what it does not, and says both versions", () => {
        // The reader supports exactly one version, so newer *and* older are
        // equally unreadable — the state carries both numbers so the copy can
        // point at the right fix.
        expect(artifactState(artifact(11), { obcmVersion: 10 })).toEqual({
            kind: "unsupported",
            artifactObcm: 11,
            deviceObcm: 10,
        });
        expect(artifactState(artifact(9), { obcmVersion: 10 })).toMatchObject({
            kind: "unsupported",
        });
    });
});

describe("a region's state on the map", () => {
    it("is available when any of its presets can be read", () => {
        expect(regionState(entry([artifact(11), artifact(10, "minimal")]), { obcmVersion: 10 })).toBe(
            "available",
        );
    });

    it("is unsupported — not missing — when the maps exist but none can be read", () => {
        // "not baked" would send the rider to the desktop app to build a map
        // they already have; the fix is a firmware update.
        expect(regionState(entry([artifact(11)]), { obcmVersion: 10 })).toBe("unsupported");
    });

    it("is not-baked only when there is nothing at all", () => {
        expect(regionState(entry([]), { obcmVersion: 10 })).toBe("not-baked");
        expect(regionState(entry([]), null)).toBe("not-baked");
    });
});

describe("styling lag", () => {
    it("is worth a note and never a refusal", () => {
        // §3: an artifact behind its preset is valid, readable and complete.
        expect(stylingLagsPreset(artifact(10), { version: 3 })).toBe(true);
        expect(stylingLagsPreset(artifact(10), { version: 2 })).toBe(false);
        expect(artifactState(artifact(10), { obcmVersion: 10 })).toEqual({ kind: "available" });
    });
});
