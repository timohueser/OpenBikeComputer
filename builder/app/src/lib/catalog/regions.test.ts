// Nested regions are the subtle part of the join, and the two shapes that
// matter are opposites of each other:
//
//   * a country that isn't baked whose sub-regions are (Germany, with Bavaria
//     and Baden-Württemberg baked separately);
//   * a country that is baked whose sub-regions aren't (Switzerland, where
//     picking Ticino finds no artifact of its own).
//
// Neither may be answered by substitution — OBCC §3 forbids treating a parent's
// artifact as a child's or the other way round, because the two cover different
// areas at very different sizes. What the index does is *name* the alternatives
// so the UI can offer them and the rider chooses.

import { describe, expect, it } from "vitest";
import type { RegionFeature } from "../platform/types";
import type { Catalog, CatalogArtifact } from "./manifest";
import { CatalogIndex } from "./regions";

/** Geofabrik's own shape: a flat id plus a parent pointer, which is what the
 *  catalog's slash-separated `region_id` is the path of. */
function region(id: string, name: string, parent: string | null): RegionFeature {
    return {
        type: "Feature",
        properties: { id, name, parent, has_children: false },
        geometry: { type: "Polygon", coordinates: [] },
    };
}

function artifact(regionId: string, presetId: string, bytes: number): CatalogArtifact {
    return {
        region_id: regionId,
        region_name: regionId.split("/").pop()!,
        preset_id: presetId,
        preset_version: 3,
        obcm_version: 10,
        bytes,
        sha256: "0".repeat(64),
        bbox: { min_lat: 0, min_lon: 0, max_lat: 1, max_lon: 1 },
        built_at: "2026-07-20T02:14:07Z",
        source_snapshot: "2026-07-19",
        url: `https://maps.example.org/regions/${regionId}/${presetId}.obcm`,
    };
}

const REGIONS: RegionFeature[] = [
    region("europe", "Europe", null),
    region("switzerland", "Switzerland", "europe"),
    region("ticino", "Ticino", "switzerland"),
    region("germany", "Germany", "europe"),
    region("bayern", "Bayern", "germany"),
    region("baden-wuerttemberg", "Baden-Württemberg", "germany"),
    region("freiburg-regbez", "Freiburg", "baden-wuerttemberg"),
];

const CATALOG: Catalog = {
    schema_version: 1,
    generated_at: "2026-07-26T09:00:00Z",
    presets: [
        { id: "default", name: "Bikepacking", description: "…", version: 3 },
        { id: "minimal", name: "Minimal", description: "…", version: 2 },
    ],
    artifacts: [
        // Switzerland: the country is baked, its sub-region is not.
        artifact("europe/switzerland", "default", 214_000_000),
        artifact("europe/switzerland", "minimal", 96_000_000),
        // Germany: the country is not baked, two of its states are.
        artifact("europe/germany/bayern", "default", 88_000_000),
        artifact("europe/germany/baden-wuerttemberg", "default", 61_000_000),
        // And one artifact for a region the Geofabrik index has no polygon for.
        artifact("europe/atlantis", "default", 1_000),
    ],
};

const index = new CatalogIndex(REGIONS, CATALOG);

describe("region ids", () => {
    it("joins a flat Geofabrik id onto the catalog's slash-separated path", () => {
        expect(index.get("switzerland")!.path).toBe("europe/switzerland");
        expect(index.get("freiburg-regbez")!.path).toBe(
            "europe/germany/baden-wuerttemberg/freiburg-regbez",
        );
        expect(index.get("europe")!.path).toBe("europe");
    });

    it("paints exactly the regions with artifacts", () => {
        expect([...index.bakedIds].sort()).toEqual([
            "baden-wuerttemberg",
            "bayern",
            "switzerland",
        ]);
    });

    it("keeps a published artifact whose region it cannot draw visible as such", () => {
        // Silently dropping it would make the catalog look smaller than it is.
        expect(index.unmatchedRegionIds).toEqual(["europe/atlantis"]);
    });
});

describe("a baked parent whose children are not", () => {
    it("gives the child no artifact of its own", () => {
        expect(index.get("ticino")!.artifacts).toEqual([]);
    });

    it("names the containing region that is baked, nearest first", () => {
        const covering = index.ancestorsWithArtifacts("ticino");
        expect(covering.map((r) => r.name)).toEqual(["Switzerland"]);
        // …and it is offered as itself: a whole-country download, at its own
        // size, never as "Ticino".
        expect(covering[0].artifacts[0].region_id).toBe("europe/switzerland");
    });

    it("does not claim the parent's map covers the child in the catalog's terms", () => {
        // The join is by exact region id. `europe/switzerland/ticino` is simply
        // not in the manifest, and nothing here invents it.
        expect(
            CATALOG.artifacts.some((a) => a.region_id === "europe/switzerland/ticino"),
        ).toBe(false);
        expect(index.get("ticino")!.path).toBe("europe/switzerland/ticino");
    });
});

describe("an unbaked parent whose children are", () => {
    it("gives the parent no artifact of its own", () => {
        expect(index.get("germany")!.artifacts).toEqual([]);
        expect(index.ancestorsWithArtifacts("germany")).toEqual([]);
    });

    it("names the baked sub-regions", () => {
        expect(index.descendantsWithArtifacts("germany").map((r) => r.name)).toEqual([
            "Baden-Württemberg",
            "Bayern",
        ]);
    });

    it("stops at the outermost baked sub-region", () => {
        // Freiburg sits under Baden-Württemberg, which is baked; listing both
        // would offer the same coverage twice.
        expect(index.descendantsWithArtifacts("europe").map((r) => r.id)).toEqual([
            "baden-wuerttemberg",
            "bayern",
            "switzerland",
        ]);
    });

    it("offers a deep unbaked region both ways at once", () => {
        // Freiburg: covered by Baden-Württemberg above it, and nothing below.
        expect(index.ancestorsWithArtifacts("freiburg-regbez").map((r) => r.id)).toEqual([
            "baden-wuerttemberg",
        ]);
        expect(index.descendantsWithArtifacts("freiburg-regbez")).toEqual([]);
    });
});
