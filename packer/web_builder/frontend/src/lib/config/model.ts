// The packer-config data model. The bare config shape is owned by obc-pack
// (see `obc-pack schema`); this module only normalizes it for editing and
// rebuilds it for submission. CRITICAL invariant: the `features` object's key
// insertion order assigns style IDs 1-based in the packer, so every function
// that rebuilds the tree must copy keys in order, never re-sort them.

export interface LodTier {
    max_mpp: number | null;
    simplify: number;
}

export interface StyleDef {
    color: string | number;
    z_index?: number;
    weight?: number;
    priority?: number;
    min_lod?: number;
    // Future schema fields (v6 line_style/color2, …) ride along untouched.
    [key: string]: unknown;
}

export interface PackConfig {
    lods: LodTier[];
    features: Record<string, Record<string, StyleDef>>;
    marker: { color: string | number };
    chunk_size?: number;
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
        lods: config.lods.map((l, i) => ({
            max_mpp: i === 0 ? null : (l.max_mpp ?? null),
            simplify: l.simplify || 0,
        })),
        features: {},
        marker: config.marker,
    };
    if (config.chunk_size != null) out.chunk_size = config.chunk_size;
    for (const cat of Object.keys(config.features)) {
        for (const name of Object.keys(config.features[cat])) {
            if (disabledSet.has(`${cat}/${name}`)) continue;
            const def = config.features[cat][name];
            const copy: StyleDef = { ...def };
            copy.min_lod = Math.max(0, Math.min(n - 1, (def.min_lod ?? 0) | 0));
            copy.priority = def.priority || 3;
            if (known) {
                for (const key of Object.keys(copy)) {
                    if (!known.has(key)) {
                        stripped.add(key);
                        delete copy[key];
                    }
                }
            }
            out.features[cat] = out.features[cat] || {};
            out.features[cat][name] = copy;
        }
    }
    return { config: out, strippedKeys: [...stripped] };
}
