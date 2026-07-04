// Pure edit operations over a pack config, ported from the legacy editor.
// Everything that rebuilds the features tree copies keys in order — the
// packer assigns style IDs from document order (see model.ts).

import { ENVELOPE_VERSION } from "./migrations";
import { deepCopy, normalizeConfig, type PackConfig, type StyleDef } from "./model";
import type { WorkingEnvelope } from "./storage.svelte";

/** Append a finer tier: half the previous ceiling, no simplification. */
export function addLodTier(config: PackConfig) {
    const last = config.lods[config.lods.length - 1];
    const prev = last.max_mpp != null ? last.max_mpp : 120;
    config.lods.push({ max_mpp: Math.max(1, Math.round(prev / 2)), simplify: 0 });
}

/**
 * Remove tier `k` and remap every feature's start tier: levels above the
 * removed one shift down by one; the removed level collapses into the tier
 * that took its index. The (new) coarsest tier is pinned back to +inf.
 */
export function removeLodTier(config: PackConfig, k: number) {
    if (config.lods.length <= 1) return;
    config.lods.splice(k, 1);
    config.lods[0].max_mpp = null;
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

/** The legacy editor's defaults for a freshly added feature type. */
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
 * Accept any historical shape — a builder export (`_meta` + features), a
 * legacy stylesheet / bare CLI config (`features`, maybe `disabled`), or a
 * raw localStorage envelope — and normalize it into a working envelope.
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

    // A stored envelope pasted whole: {schema_version, config: {...}}.
    const source =
        typeof obj.config === "object" && obj.config !== null && !("features" in obj)
            ? (obj.config as Record<string, unknown>)
            : obj;
    if (typeof source.features !== "object" || source.features === null) return null;

    const meta = (obj._meta ?? source._meta) as Record<string, unknown> | undefined;
    const basedOn = meta?.based_on as WorkingEnvelope["based_on"] | undefined;
    const { config, disabled } = normalizeConfig(source);
    const extraDisabled = Array.isArray(obj.disabled) ? (obj.disabled as string[]) : [];
    return {
        schema_version: ENVELOPE_VERSION,
        based_on: basedOn ?? null,
        modified: true, // an import is by definition not a pristine preset
        config,
        disabled: [...new Set([...disabled, ...extraDisabled])],
    };
}
