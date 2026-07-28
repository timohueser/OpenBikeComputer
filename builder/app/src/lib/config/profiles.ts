// Pure logic for the routing-profile editor. Everything here is driven by the
// config JSON Schema served by `obc-pack schema` (the class-name enums, the
// >= 1.0 multiplier bound, and the four shipped default profiles all come from
// the schema — never re-hardcoded in Svelte). The component in
// components/advanced/ProfilesTab.svelte is thin glue over these functions.

import {
    deepCopy,
    type Multiplier,
    type NavProfile,
    type PackConfig,
    type RoutingConfig,
    type SchemaEnvelope,
} from "./model";

export type ClassGroup = "highway" | "surface";

/** The profile-editor capabilities read out of the served schema. */
export interface ProfileSchema {
    /** Highway class names, in canonical order (schema propertyNames enum). */
    highwayClasses: string[];
    /** Surface class names, in canonical order. */
    surfaceClasses: string[];
    /** Lower bound for a numeric multiplier (schema $defs/multiplier). */
    multiplierMin: number;
    /** The per-profile `default` fallback multiplier (schema default: 2.0). */
    defaultMultiplier: number;
    /** Min / max profile count (schema profiles minItems / maxItems). */
    minProfiles: number;
    maxProfiles: number;
    /** Max name length on the wire, bytes (schema profile.name x-maxUtf8Bytes). */
    nameMaxBytes: number;
    /** The four shipped defaults (Road / Gravel / MTB / Touring), canonical. */
    defaultProfiles: NavProfile[];
}

// Loose views into the schema tree — the served envelope only strong-types
// $defs.style, so the routing bits are read defensively with fallbacks that
// match the spec (a schema that predates routing simply yields no editor).
interface SchemaTree {
    properties?: {
        routing?: {
            properties?: { profiles?: { minItems?: number; maxItems?: number } };
            default?: { profiles?: NavProfile[] };
        };
    };
    $defs?: {
        multiplier?: { oneOf?: { type?: string; minimum?: number }[] };
        profile?: {
            properties?: {
                name?: { maxLength?: number; "x-maxUtf8Bytes"?: number };
                default?: { default?: number };
                highway?: { propertyNames?: { enum?: string[] } };
                surface?: { propertyNames?: { enum?: string[] } };
            };
        };
    };
}

/**
 * Read the routing-profile capabilities out of the served schema, or null when
 * the schema doesn't describe routing (older obc-pack) — the editor hides
 * itself in that case rather than guessing.
 */
export function readProfileSchema(env: SchemaEnvelope | null): ProfileSchema | null {
    const s = (env?.schema ?? null) as SchemaTree | null;
    const profile = s?.$defs?.profile?.properties;
    const highwayClasses = profile?.highway?.propertyNames?.enum;
    const surfaceClasses = profile?.surface?.propertyNames?.enum;
    const profilesSchema = s?.properties?.routing?.properties?.profiles;
    const defaultProfiles = s?.properties?.routing?.default?.profiles;
    if (!highwayClasses?.length || !surfaceClasses?.length || !defaultProfiles?.length) {
        return null;
    }
    const numberVariant = s?.$defs?.multiplier?.oneOf?.find((o) => o.type === "number");
    return {
        highwayClasses,
        surfaceClasses,
        multiplierMin: numberVariant?.minimum ?? 1,
        defaultMultiplier: profile?.default?.default ?? 2,
        minProfiles: profilesSchema?.minItems ?? 1,
        maxProfiles: profilesSchema?.maxItems ?? 8,
        nameMaxBytes: profile?.name?.["x-maxUtf8Bytes"] ?? profile?.name?.maxLength ?? 12,
        defaultProfiles,
    };
}

/** The canonical shipped profiles (a fresh deep copy so edits never alias). */
export function defaultProfiles(ps: ProfileSchema): NavProfile[] {
    return deepCopy(ps.defaultProfiles);
}

/** `true` for the "forbidden" sentinel. */
export function isForbidden(v: Multiplier | undefined): v is "forbidden" {
    return v === "forbidden";
}

/** The class names for a group, in canonical (schema) order. */
export function classNames(ps: ProfileSchema, group: ClassGroup): string[] {
    return group === "highway" ? ps.highwayClasses : ps.surfaceClasses;
}

/** A profile's per-class map for a group (may be absent). */
function classMap(profile: NavProfile, group: ClassGroup): Record<string, Multiplier> | undefined {
    return group === "highway" ? profile.highway : profile.surface;
}

/** `true` when the class carries an explicit override (vs. inheriting `default`). */
export function isExplicit(profile: NavProfile, group: ClassGroup, cls: string): boolean {
    const map = classMap(profile, group);
    return !!map && Object.prototype.hasOwnProperty.call(map, cls);
}

/** A profile's effective `default` multiplier (falls back to the schema default). */
export function profileDefault(profile: NavProfile, ps: ProfileSchema): Multiplier {
    return profile.default ?? ps.defaultMultiplier;
}

/**
 * The effective multiplier shown in a cell: the explicit override if the class
 * carries one, otherwise the profile `default`. This mirrors the packer, which
 * fills every one of the 32/8 wire slots from `default` and overlays the map.
 */
export function cellValue(
    profile: NavProfile,
    group: ClassGroup,
    cls: string,
    ps: ProfileSchema,
): Multiplier {
    const map = classMap(profile, group);
    if (map && Object.prototype.hasOwnProperty.call(map, cls)) return map[cls];
    return profileDefault(profile, ps);
}

/**
 * Whether a raw numeric entry is admissible against the schema's minimum
 * (`ProfileSchema.multiplierMin`). A value below it is rejected with a hint
 * that mirrors the packer's error text, so the CLI and the web builder tell
 * the user the same thing. This is the single copy of that message — the
 * multiplier cells call it rather than re-wording it.
 */
export function checkMultiplier(n: number, min: number): { ok: boolean; hint: string | null } {
    if (!Number.isFinite(n)) return { ok: false, hint: "enter a number ≥ " + min.toFixed(1) + "." };
    if (n < min) {
        return {
            ok: false,
            hint:
                `a multiplier below ${min.toFixed(1)} breaks the router's ` +
                "shortest-path guarantee — every non-zero weight must stay ≥ 1.0 so the " +
                "great-circle A* heuristic remains admissible. Use “forbidden” to exclude " +
                "the class instead.",
        };
    }
    return { ok: true, hint: null };
}

/** Write an explicit override for a class (number already clamped, or "forbidden"). */
export function setCell(profile: NavProfile, group: ClassGroup, cls: string, v: Multiplier): void {
    if (group === "highway") (profile.highway ??= {})[cls] = v;
    else (profile.surface ??= {})[cls] = v;
}

/** Drop a class's explicit override so it inherits `default` again. */
export function clearCell(profile: NavProfile, group: ClassGroup, cls: string): void {
    const map = classMap(profile, group);
    if (!map) return;
    delete map[cls];
}

/** Set a profile's per-profile `default` multiplier. */
export function setProfileDefault(profile: NavProfile, v: Multiplier): void {
    profile.default = v;
}

/** Set a profile's display name (caller enforces the byte cap in the UI). */
export function setProfileName(profile: NavProfile, name: string): void {
    profile.name = name;
}

// --- routing-section lifecycle over a working config -------------------------

/**
 * The profiles the editor should display for a config: the config's own
 * `routing.profiles` if present, otherwise the schema's shipped defaults (shown
 * read-until-edited so an untouched CLI config stays `routing`-less).
 */
export function displayProfiles(config: PackConfig, ps: ProfileSchema): NavProfile[] {
    return config.routing?.profiles ?? defaultProfiles(ps);
}

/**
 * Materialize `config.routing` so it can be edited in place, seeding it from the
 * schema defaults when the config didn't carry a routing section. Returns the
 * (now guaranteed) routing object. Idempotent.
 */
export function ensureRouting(config: PackConfig, ps: ProfileSchema): RoutingConfig {
    if (!config.routing) {
        config.routing = { profiles: defaultProfiles(ps) };
    } else if (!Array.isArray(config.routing.profiles) || config.routing.profiles.length === 0) {
        config.routing.profiles = defaultProfiles(ps);
    }
    return config.routing;
}

/** A unique-ish name for an added profile ("Custom", "Custom 2", …). */
function uniqueName(existing: NavProfile[], base = "Custom", max = 12): string {
    const names = new Set(existing.map((p) => p.name));
    if (!names.has(base) && base.length <= max) return base;
    for (let i = 2; i < 100; i++) {
        const cand = `${base} ${i}`;
        if (!names.has(cand) && cand.length <= max) return cand;
    }
    return base.slice(0, max);
}

/**
 * Append a new profile (up to the schema max). The new profile carries only a
 * name + the schema default multiplier, so every class inherits `default` — a
 * neutral starting point the user then tunes. Returns the new profile or null
 * if the profile cap is already reached.
 */
export function addProfile(config: PackConfig, ps: ProfileSchema): NavProfile | null {
    const routing = ensureRouting(config, ps);
    if (routing.profiles.length >= ps.maxProfiles) return null;
    const profile: NavProfile = {
        name: uniqueName(routing.profiles, "Custom", ps.nameMaxBytes),
        default: ps.defaultMultiplier,
    };
    routing.profiles.push(profile);
    return profile;
}

/** Remove profile `i` (never below the schema minimum). Returns true on removal. */
export function removeProfile(config: PackConfig, i: number, ps: ProfileSchema): boolean {
    const routing = ensureRouting(config, ps);
    if (routing.profiles.length <= ps.minProfiles) return false;
    if (i < 0 || i >= routing.profiles.length) return false;
    routing.profiles.splice(i, 1);
    return true;
}

/**
 * Reset profile `i` to its canonical shipped default. Matches by name first
 * (so "Gravel" resets to the Gravel default wherever it sits), falling back to
 * the same-index default, then the first. Returns the replacement.
 */
export function resetProfile(config: PackConfig, i: number, ps: ProfileSchema): NavProfile | null {
    const routing = ensureRouting(config, ps);
    if (i < 0 || i >= routing.profiles.length) return null;
    const defaults = ps.defaultProfiles;
    const current = routing.profiles[i];
    const match =
        defaults.find((d) => d.name === current.name) ?? defaults[i] ?? defaults[0];
    const replacement = deepCopy(match);
    routing.profiles[i] = replacement;
    return replacement;
}
