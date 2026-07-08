// The packer-config data model. The bare config shape is owned by obc-pack
// (see `obc-pack schema`); this module only normalizes it for editing and
// rebuilds it for submission. CRITICAL invariant: the `features` object's key
// insertion order assigns style IDs 1-based in the packer, so every function
// that rebuilds the tree must copy keys in order, never re-sort them.

export interface LodTier {
    max_mpp: number | null;
    simplify: number;
    // Drop area features (polygons) whose projected area is below this many square
    // pixels at this tier's finest on-screen scale; absent/0 ⇒ off. Lines are never
    // culled. Ignored by the packer on the finest tier (no coarser fallback).
    // Optional so a config without it stays byte-identical when submitted.
    min_area_px?: number;
}

export interface StyleDef {
    color: string | number;
    z_index?: number;
    weight?: number;
    priority?: number;
    min_lod?: number;
    // Schema-declared style fields (v10 line_style/color2) and any future ones
    // ride along untouched. `color2` is optional: its key is simply absent when
    // unset (never null / "0x0000" — black is a legit color, absence is not).
    [key: string]: unknown;
}

/** One routing edge-weight multiplier: a number >= 1.0 or the string "forbidden".
 * The bounds and the "forbidden" sentinel are owned by obc-pack's schema
 * ($defs/multiplier) — this type just mirrors the two-variant shape. */
export type Multiplier = number | "forbidden";

/** One bike profile (routing §8.6): a display name, a `default` multiplier for
 * unlisted classes, and per-class overrides keyed by the schema's class-name
 * enums. `highway`/`surface` are sparse — an absent class inherits `default`. */
export interface NavProfile {
    name: string;
    default?: Multiplier;
    highway?: Record<string, Multiplier>;
    surface?: Record<string, Multiplier>;
}

/** The `routing` config section (owned by obc-pack; see `obc-pack schema`). */
export interface RoutingConfig {
    min_component_edges?: number;
    profiles: NavProfile[];
}

export interface PackConfig {
    lods: LodTier[];
    features: Record<string, Record<string, StyleDef>>;
    marker: { color: string | number };
    chunk_size?: number;
    // Bike-type routing profiles (§8.6). Absent ⇒ the packer bakes in its four
    // shipped defaults; the profile editor materializes it on first edit.
    routing?: RoutingConfig;
}

export interface Preset {
    id: string;
    name: string;
    description: string;
    version: number;
    swatch: string[];
    config: PackConfig;
}

/** The schema envelope served by /api/schema (from `obc-pack schema`). */
export interface SchemaEnvelope {
    schema_version: number;
    format_version: number | null;
    schema: {
        $defs: { style: { properties: Record<string, unknown> } };
        [key: string]: unknown;
    };
    source: "binary" | "repo-file";
}

export function deepCopy<T>(v: T): T {
    return JSON.parse(JSON.stringify(v)) as T;
}

/**
 * Adopt a loaded config (preset, import, or stored working copy) as editable
 * state: guarantee lods/features/marker exist, pin the coarsest tier to +inf,
 * clamp per-style min_lod, and lift out the `disabled` list (a top-level
 * `["cat/name", …]` array the packer ignores).
 */
export function normalizeConfig(raw: Record<string, unknown>): {
    config: PackConfig;
    disabled: string[];
} {
    const cfg = deepCopy(raw) as unknown as PackConfig & { disabled?: unknown; _meta?: unknown };
    delete cfg._meta;
    if (!Array.isArray(cfg.lods) || cfg.lods.length === 0) {
        cfg.lods = [{ max_mpp: null, simplify: 0 }];
    }
    cfg.lods = cfg.lods.map((l, i) => ({
        max_mpp: i === 0 ? null : (l.max_mpp ?? null),
        simplify: l.simplify ?? 0,
        ...(l.min_area_px ? { min_area_px: l.min_area_px } : {}),
    }));
    cfg.features = cfg.features ?? {};
    cfg.marker = cfg.marker ?? { color: "0xF800" };
    const disabled = Array.isArray(cfg.disabled) ? (cfg.disabled as string[]) : [];
    delete cfg.disabled;
    const maxLod = cfg.lods.length - 1;
    for (const cat of Object.keys(cfg.features)) {
        for (const name of Object.keys(cfg.features[cat])) {
            const def = cfg.features[cat][name];
            if (typeof def.min_lod === "number") {
                def.min_lod = Math.max(0, Math.min(maxLod, def.min_lod | 0));
            }
        }
    }
    return { config: cfg, disabled };
}

/**
 * The config actually submitted to a build: disabled features dropped, min_lod
 * clamped to the LOD count, and — so the UI never sends styling the binary
 * would silently ignore — per-style keys not declared by the served schema
 * stripped. Returns the stripped key names alongside the config so the UI can
 * mention it. Key order is preserved throughout (style IDs!).
 */
export function buildConfigForSubmit(
    config: PackConfig,
    disabled: string[],
    schema: SchemaEnvelope | null,
): { config: PackConfig; strippedKeys: string[] } {
    const disabledSet = new Set(disabled);
    const known = schema ? new Set(Object.keys(schema.schema.$defs.style.properties)) : null;
    const stripped = new Set<string>();
    const n = config.lods.length;
    const out: PackConfig = {
        lods: config.lods.map((l, i) => {
            const tier: LodTier = { max_mpp: i === 0 ? null : (l.max_mpp ?? null), simplify: l.simplify || 0 };
            // Emit only a positive footprint floor; the finest tier's value is ignored
            // by the packer, so leaving it off keeps the submitted config clean.
            if (l.min_area_px && i < n - 1) tier.min_area_px = l.min_area_px;
            return tier;
        }),
        features: {},
        marker: config.marker,
    };
    if (config.chunk_size != null) out.chunk_size = config.chunk_size;
    // Routing profiles ride through untouched (validated by the packer). Absent
    // ⇒ the binary bakes in its four shipped defaults, so CLI parity holds.
    if (config.routing) out.routing = deepCopy(config.routing);
    for (const cat of Object.keys(config.features)) {
        for (const name of Object.keys(config.features[cat])) {
            if (disabledSet.has(`${cat}/${name}`)) continue;
            const def = config.features[cat][name];
            const copy: StyleDef = { ...def };
            copy.min_lod = Math.max(0, Math.min(n - 1, (def.min_lod ?? 0) | 0));
            copy.priority = def.priority || 3;
            for (const key of Object.keys(copy)) {
                // A cleared optional field (e.g. color2) is present-but-undefined
                // after a spread; drop it so JSON emits absence, not the key.
                if (copy[key] === undefined) {
                    delete copy[key];
                    continue;
                }
                // Keys the served schema doesn't declare would be silently
                // ignored by the binary — strip them and report which.
                if (known && !known.has(key)) {
                    stripped.add(key);
                    delete copy[key];
                }
            }
            out.features[cat] = out.features[cat] || {};
            out.features[cat][name] = copy;
        }
    }
    return { config: out, strippedKeys: [...stripped] };
}
