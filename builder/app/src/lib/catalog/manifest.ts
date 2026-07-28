// The catalog manifest (OBCC, schema_version 1) as this app reads it. The
// format is normative in OBCC_Spec.md and produced only by `obc-pack catalog`;
// this module is the consumer half of its §7 contract.
//
// §7 is the reason `parseCatalog` takes a *string* rather than a parsed object:
// a consumer must read the entire response body before parsing and parse it as
// one JSON document, because JSON is self-delimiting and no proper prefix of a
// valid document parses. Anything that streams or reconstructs the document
// incrementally forfeits that guarantee, so the seam is a whole body in, a
// whole catalog or an error out. There is no partial return value.

/** The envelope version this consumer implements. Checked before any other
 *  field; anything else is rejected outright (§7). */
export const CATALOG_SCHEMA_VERSION = 1;

/** Coverage box in microdegrees, copied from the artifact's OBCM header (§4).
 *  This is what the download *covers*, not the box it was cut from. */
export interface CatalogBbox {
    min_lat: number;
    min_lon: number;
    max_lat: number;
    max_lon: number;
}

/** A style preset the catalog is baked in (§2). */
export interface CatalogPreset {
    id: string;
    name: string;
    description: string;
    /** The preset's *current* version — what a fresh bake would produce. Not a
     *  claim about any artifact; see `CatalogArtifact.preset_version`. */
    version: number;
    /** A rendered preview asset (B2 #899), resolved like `CatalogArtifact.url`. */
    preview?: string;
}

/** One published `.obcm`: a (region, preset) pair (§3). */
export interface CatalogArtifact {
    /** Slash-separated, mirroring the Geofabrik hierarchy: `europe/switzerland`. */
    region_id: string;
    region_name: string;
    preset_id: string;
    /** The preset version *recorded by the bake job*. May lag the preset's own
     *  `version`, which means older styling — never a reason to refuse it (§3). */
    preset_version: number;
    /** OBCM format version, read from the artifact's own header (§6). */
    obcm_version: number;
    bytes: number;
    /** Lowercase hex SHA-256 of the artifact bytes. */
    sha256: string;
    bbox: CatalogBbox;
    built_at: string;
    source_snapshot: string;
    url: string;
}

export interface Catalog {
    schema_version: number;
    generated_at: string;
    presets: CatalogPreset[];
    artifacts: CatalogArtifact[];
}

/** A manifest this consumer will not use. Thrown whole: there is no state in
 *  which half a catalog is usable, so the caller's only choice is to keep
 *  whatever it had cached (§7). */
export class CatalogFormatError extends Error {
    constructor(message: string) {
        super(message);
        this.name = "CatalogFormatError";
    }
}

const KEBAB = /^[a-z0-9]+(-[a-z0-9]+)*$/;
const REGION_ID = /^[a-z0-9]+(-[a-z0-9]+)*(\/[a-z0-9]+(-[a-z0-9]+)*)*$/;
const SHA256 = /^[0-9a-f]{64}$/;
// §5: exactly one spelling — twenty characters, `Z`, no fractional seconds.
const INSTANT = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/;
const DATE = /^\d{4}-\d{2}-\d{2}$/;

type Obj = Record<string, unknown>;

function fail(what: string): never {
    throw new CatalogFormatError(what);
}

function obj(v: unknown, where: string): Obj {
    if (typeof v !== "object" || v === null || Array.isArray(v)) fail(`${where}: expected an object`);
    return v as Obj;
}

function str(o: Obj, key: string, where: string, pattern?: RegExp): string {
    const v = o[key];
    if (typeof v !== "string" || v.length === 0) fail(`${where}: ${key} must be a non-empty string`);
    if (pattern && !pattern.test(v)) fail(`${where}: ${key} is malformed (${JSON.stringify(v)})`);
    return v;
}

function int(o: Obj, key: string, where: string, min = 0): number {
    const v = o[key];
    if (typeof v !== "number" || !Number.isInteger(v) || v < min) {
        fail(`${where}: ${key} must be an integer >= ${min}`);
    }
    return v;
}

/** A calendar date that exists — §5 rejects `2026-02-30` and `2023-02-29`. */
function realDate(spelling: string, where: string): void {
    const [y, m, d] = spelling.slice(0, 10).split("-").map(Number);
    const probe = new Date(Date.UTC(y, m - 1, d));
    if (probe.getUTCFullYear() !== y || probe.getUTCMonth() !== m - 1 || probe.getUTCDate() !== d) {
        fail(`${where}: ${spelling} is not a real date`);
    }
}

function parseBbox(v: unknown, where: string): CatalogBbox {
    const o = obj(v, `${where}.bbox`);
    const box = {
        min_lat: intSigned(o, "min_lat", `${where}.bbox`, 90_000_000),
        min_lon: intSigned(o, "min_lon", `${where}.bbox`, 180_000_000),
        max_lat: intSigned(o, "max_lat", `${where}.bbox`, 90_000_000),
        max_lon: intSigned(o, "max_lon", `${where}.bbox`, 180_000_000),
    };
    if (box.min_lat > box.max_lat || box.min_lon > box.max_lon) {
        fail(`${where}.bbox: min is greater than max`);
    }
    return box;
}

function intSigned(o: Obj, key: string, where: string, limit: number): number {
    const v = o[key];
    if (typeof v !== "number" || !Number.isInteger(v) || v < -limit || v > limit) {
        fail(`${where}: ${key} must be an integer within ±${limit}`);
    }
    return v;
}

/**
 * Parse a manifest body, or throw. Every REQUIRED field is checked here and a
 * failure rejects the whole document — the caller keeps its cached copy rather
 * than showing a partially-populated catalog (§7). Fields this consumer does
 * not recognise ride through untouched: adding an OPTIONAL field is not a
 * breaking change (§1).
 */
export function parseCatalog(body: string): Catalog {
    let raw: unknown;
    try {
        raw = JSON.parse(body);
    } catch (e) {
        fail(`not a JSON document: ${e instanceof Error ? e.message : String(e)}`);
    }
    const root = obj(raw, "catalog");

    // Before any other field, per §7 — a document from a future envelope may
    // spell everything below differently, so nothing else is worth reading.
    const schemaVersion = root.schema_version;
    if (schemaVersion !== CATALOG_SCHEMA_VERSION) {
        fail(
            `schema_version ${JSON.stringify(schemaVersion)} is not supported ` +
                `(this app reads ${CATALOG_SCHEMA_VERSION})`,
        );
    }

    const generatedAt = str(root, "generated_at", "catalog", INSTANT);
    realDate(generatedAt, "catalog.generated_at");

    if (!Array.isArray(root.presets)) fail("catalog: presets must be an array");
    if (!Array.isArray(root.artifacts)) fail("catalog: artifacts must be an array");

    const presets: CatalogPreset[] = root.presets.map((entry, i) => {
        const where = `presets[${i}]`;
        const o = obj(entry, where);
        const preset: CatalogPreset = {
            id: str(o, "id", where, KEBAB),
            name: str(o, "name", where),
            description: str(o, "description", where),
            version: int(o, "version", where, 0),
        };
        // `preview` is optional and may be spelled as an explicit null (the
        // schema allows both); either way it means "no preview yet".
        if (o.preview !== undefined && o.preview !== null) {
            preset.preview = str(o, "preview", where);
        }
        return preset;
    });

    const presetById = new Map(presets.map((p) => [p.id, p]));
    if (presetById.size !== presets.length) fail("catalog: two presets share an id");

    const seen = new Set<string>();
    const artifacts: CatalogArtifact[] = root.artifacts.map((entry, i) => {
        const where = `artifacts[${i}]`;
        const o = obj(entry, where);
        const builtAt = str(o, "built_at", where, INSTANT);
        realDate(builtAt, `${where}.built_at`);
        const snapshot = str(o, "source_snapshot", where, DATE);
        realDate(snapshot, `${where}.source_snapshot`);
        const artifact: CatalogArtifact = {
            region_id: str(o, "region_id", where, REGION_ID),
            region_name: str(o, "region_name", where),
            preset_id: str(o, "preset_id", where, KEBAB),
            preset_version: int(o, "preset_version", where, 0),
            obcm_version: int(o, "obcm_version", where, 0),
            bytes: int(o, "bytes", where, 0),
            sha256: str(o, "sha256", where, SHA256),
            bbox: parseBbox(o.bbox, where),
            built_at: builtAt,
            source_snapshot: snapshot,
            url: str(o, "url", where),
        };

        // An artifact naming a preset the manifest doesn't list can't be shown
        // (no name, no description, nothing to compare its version against).
        const preset = presetById.get(artifact.preset_id);
        if (!preset) fail(`${where}: preset_id "${artifact.preset_id}" is not in presets[]`);
        // §3: an artifact ahead of the config it claims to be built from means
        // the catalog cannot describe its styling at all — malformed, reject.
        // (Behind is normal: a partial re-bake, and MUST NOT be refused.)
        if (artifact.preset_version > preset.version) {
            fail(
                `${where}: preset_version ${artifact.preset_version} is ahead of ` +
                    `preset "${preset.id}" version ${preset.version}`,
            );
        }

        // Two artifacts for one (region, preset) leave "the artifact" undefined,
        // and the picker's whole job is to name one file per choice.
        const key = `${artifact.region_id} ${artifact.preset_id}`;
        if (seen.has(key)) {
            fail(`${where}: duplicate artifact for ${artifact.region_id} / ${artifact.preset_id}`);
        }
        seen.add(key);
        return artifact;
    });

    return { schema_version: CATALOG_SCHEMA_VERSION, generated_at: generatedAt, presets, artifacts };
}
