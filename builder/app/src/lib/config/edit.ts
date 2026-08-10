import { deepCopy, normalizeConfig, type PackConfig, type StyleDef } from "./model";
import { WORKING_SCHEMA_VERSION, type WorkingEnvelope } from "./storage.svelte";

/**
 * The pixel-accurate simplify default for tier `i`: one pixel at the finest
 * scale the tier is drawn at, which is the next finer tier's ceiling. The
 * finest tier keeps full detail (0).
 */
export function autoSimplify(config: PackConfig, i: number): number {
    return config.lods[i + 1]?.max_mpp ?? 0;
}

/**
 * Set one tier field. A coarser neighbor whose simplify still sits at the
 * pixel-accurate default follows a ceiling change; a hand-set value stays.
 */
export function editLodTier(
    config: PackConfig,
    i: number,
    field: "max_mpp" | "simplify" | "min_area_px",
    value: number,
) {
    if (field === "max_mpp" && i > 0 && config.lods[i - 1].simplify === config.lods[i].max_mpp) {
        config.lods[i - 1].simplify = value;
    }
    config.lods[i][field] = value;
}

/**
 * Turn the shared-coverage fill simplify on or off for one tier. Off is stored as
 * absence, not `false`: a config that never asked for the pass submits the bytes it
 * always did.
 */
export function setLodCoverageSimplify(config: PackConfig, i: number, on: boolean) {
    if (on) config.lods[i].coverage_simplify = true;
    else delete config.lods[i].coverage_simplify;
}

/**
 * Append a finer tier at half the previous ceiling, with full detail. The
 * old finest tier is no longer finest: unless it was hand-set, its simplify
 * moves from 0 to the new pixel-accurate default.
 */
export function addLodTier(config: PackConfig) {
    const last = config.lods[config.lods.length - 1];
    const prev = last.max_mpp != null ? last.max_mpp : 120;
    const next = Math.max(1, Math.round(prev / 2));
    if (!last.simplify) last.simplify = next;
    config.lods.push({ max_mpp: next, simplify: 0 });
}

/**
 * Remove tier `k` and remap every feature's start tier: levels above the
 * removed one shift down by one; the removed level collapses into the tier
 * that took its index. The (new) coarsest tier is pinned back to +inf.
 */
export function removeLodTier(config: PackConfig, k: number) {
    if (config.lods.length <= 1) return;
    const removed = config.lods[k];
    config.lods.splice(k, 1);
    config.lods[0].max_mpp = null;
    // A coarser neighbor tracking the removed ceiling re-defaults to the
    // tier that took its place.
    if (k > 0 && config.lods[k - 1].simplify === removed.max_mpp) {
        config.lods[k - 1].simplify = autoSimplify(config, k - 1);
    }
    const n = config.lods.length;
    for (const cat of Object.keys(config.features)) {
        for (const name of Object.keys(config.features[cat])) {
            const def = config.features[cat][name];
            let m = typeof def.min_lod === "number" ? def.min_lod : 0;
            if (m > k) m -= 1;
            def.min_lod = Math.max(0, Math.min(n - 1, m));
        }
    }
}

/** Rebuild one category in the given name order (drag-reorder commit). */
export function reorderCategory(config: PackConfig, cat: string, orderedNames: string[]) {
    const entries = config.features[cat];
    if (!entries) return;
    const reordered: Record<string, StyleDef> = {};
    for (const name of orderedNames) {
        if (name in entries) reordered[name] = entries[name];
    }
    // Anything the order list missed (shouldn't happen) keeps its old position.
    for (const name of Object.keys(entries)) {
        if (!(name in reordered)) reordered[name] = entries[name];
    }
    config.features[cat] = reordered;
}

/** Defaults for a freshly added feature type. */
export function newStyleDef(config: PackConfig): StyleDef {
    return { z_index: 10, color: "0xFFFF", weight: 1, min_lod: config.lods.length - 1, priority: 3 };
}

/** Remove a category; returns its "cat/name" keys for disabled-list cleanup. */
export function removeCategory(config: PackConfig, cat: string): string[] {
    const keys = Object.keys(config.features[cat] ?? {}).map((n) => `${cat}/${n}`);
    delete config.features[cat];
    return keys;
}

// --- export / import ---------------------------------------------------------

/**
 * An exported file is a bare, CLI-usable packer config (the packer ignores
 * `_meta` and `disabled`) carrying provenance for re-import.
 */
export function exportFile(env: WorkingEnvelope): string {
    const out: Record<string, unknown> = {
        _meta: {
            app: "obcm-web-builder",
            schema_version: env.schema_version,
            based_on: env.based_on,
            exported: new Date().toISOString().slice(0, 10),
        },
        ...deepCopy(env.config),
    };
    if (env.disabled.length) out.disabled = [...env.disabled];
    return JSON.stringify(out, null, 2);
}

/**
 * Accept a builder export or bare CLI config and normalize it into a working envelope.
 * Returns null when the JSON isn't a config at all.
 */
export function importFile(text: string): WorkingEnvelope | null {
    let raw: unknown;
    try {
        raw = JSON.parse(text);
    } catch {
        return null;
    }
    if (typeof raw !== "object" || raw === null) return null;
    const obj = raw as Record<string, unknown>;

    if (typeof obj.features !== "object" || obj.features === null) return null;

    const metaValue = obj._meta;
    if (metaValue !== undefined && (typeof metaValue !== "object" || metaValue === null || Array.isArray(metaValue))) {
        return null;
    }
    const meta = metaValue as Record<string, unknown> | undefined;
    if (meta?.schema_version !== undefined && meta.schema_version !== WORKING_SCHEMA_VERSION) return null;
    const basedOnValue = meta?.based_on;
    let basedOn: WorkingEnvelope["based_on"] = null;
    if (basedOnValue !== undefined && basedOnValue !== null) {
        if (typeof basedOnValue !== "object" || Array.isArray(basedOnValue)) return null;
        const candidate = basedOnValue as Record<string, unknown>;
        if (typeof candidate.id !== "string" || !Number.isInteger(candidate.version) || Number(candidate.version) < 0) {
            return null;
        }
        basedOn = { id: candidate.id, version: Number(candidate.version) };
    }
    const { config, disabled } = normalizeConfig(obj);
    const extraDisabled = Array.isArray(obj.disabled) ? (obj.disabled as string[]) : [];
    return {
        schema_version: WORKING_SCHEMA_VERSION,
        based_on: basedOn,
        modified: true, // an import is by definition not a pristine preset
        config,
        disabled: [...new Set([...disabled, ...extraDisabled])],
    };
}
