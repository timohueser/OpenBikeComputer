// Test fixtures for the v2 client. Not imported by any app code.
//
// The catalog here is the *real* one: `host/obc-pack/schema/catalog.v2.example.json`,
// the document the generator's own tests pin, read through `parseCatalogV2`. A
// producer-side change to the shape therefore fails in this suite rather than in
// a browser — which is the same trick `../manifest.test.ts` plays on v1, and the
// reason these tests are worth having at all.
//
// Cell indices are synthesised because the example ships only one band's, but
// they are synthesised *through the parser*: a fixture that skipped it could
// describe a document the client would refuse.

import { readFileSync } from "node:fs";
import { parseCatalogV2, type CatalogV2, type CellIndexRef } from "./manifest";
import { parseCellIndex, type CellIndexDocument } from "./satellites";

const EXAMPLE_DIR = new URL("../../../../../../host/obc-pack/schema/", import.meta.url);

export function readExample(name: string): string {
    return readFileSync(new URL(name, EXAMPLE_DIR), "utf8");
}

export const EXAMPLE_ROOT = readExample("catalog.v2.example.json");
export const EXAMPLE_CELL_INDEX = readExample("cell-index.v2.example.json");
export const EXAMPLE_REGION_CELLS = readExample("region-cells.v2.example.json");

export const exampleCatalog: CatalogV2 = parseCatalogV2(EXAMPLE_ROOT);

const ZERO_DIGEST = "0".repeat(64);

/** One synthetic cell, as a band index entry. */
export interface FixtureCell {
    id: string;
    bytes: number;
    partial?: boolean;
    /** The real digest, when a test serves real bytes for this cell. */
    sha256?: string;
}

/** A band's cell index, built through the real parser. */
export function fixtureIndex(
    catalog: CatalogV2,
    bandId: string,
    cells: FixtureCell[],
): CellIndexDocument {
    const band = catalog.schema.bands.find((b) => b.id === bandId);
    if (!band) throw new Error(`no band ${bandId}`);
    const doc = {
        schema_version: 2,
        schema_revision: catalog.schema.revision,
        band: bandId,
        cells: cells.map((c) => ({
            id: c.id,
            bytes: c.bytes,
            sha256: c.sha256 ?? ZERO_DIGEST,
            url: `https://maps.example.org/catalog/v2/cells/${bandId}/${c.id.split("/").slice(1).join("/")}.obcm`,
            built_at: "2026-07-30T02:12:55Z",
            sources: [{ extract_id: "europe/switzerland", snapshot: "2026-07-19" }],
            partial: c.partial ?? false,
        })),
    };
    const ref: CellIndexRef = {
        band: bandId,
        cell_log2: band.cell_log2,
        cell_count: cells.length,
        bytes: 0,
        sha256: ZERO_DIGEST,
        url: `/catalog/v2/cells/${bandId}/index.json`,
    };
    return parseCellIndex(JSON.stringify(doc), catalog, ref);
}

/** Indices for every band of the catalog, from one spec per band. */
export function fixtureIndices(
    catalog: CatalogV2,
    byBand: Record<string, FixtureCell[]>,
): Map<string, CellIndexDocument> {
    return new Map(
        Object.entries(byBand).map(([band, cells]) => [band, fixtureIndex(catalog, band, cells)]),
    );
}
