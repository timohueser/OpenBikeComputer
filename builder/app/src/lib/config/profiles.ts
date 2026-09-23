// Pure logic for the routing-profile editor. Everything here is driven by the
// config JSON Schema served by `obc-pack schema` — the class-name enums, the
// multiplier bound and the four shipped default profiles all come from the schema
// and are never re-hardcoded in Svelte. The component in components/advanced/ is
// thin glue over these functions.

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
    /** Climb-weight bounds, `0..255` (schema profile.climb_weight minimum/maximum). */
    climbMin: number;
    climbMax: number;
    /** The climb weight an unstated field means — `0`, climb-blind (schema default). */
    climbDefault: number;
    /** The four bike types' shipped weights (Road / Gravel / MTB / Touring), in their fixed order. */
    defaultProfiles: NavProfile[];
}

// Loose views into the schema tree — the served envelope only strong-types
// $defs.style, so the routing bits are read defensively with fallbacks that match
// the spec (a schema that predates routing simply yields no editor).
interface SchemaTree {
    properties?: {
        routing?: {
            default?: { profiles?: NavProfile[] };
        };
    };
    $defs?: {
        multiplier?: { oneOf?: { type?: string; minimum?: number }[] };
        profile?: {
            properties?: {
                default?: { default?: number };
                climb_weight?: { minimum?: number; maximum?: number; default?: number };
                highway?: { propertyNames?: { enum?: string[] } };
                surface?: { propertyNames?: { enum?: string[] } };
            };
        };
    };
}

/**
 * Read the routing-profile capabilities out of the served schema, or null when the
 * schema does not describe routing — the editor hides itself rather than guessing.
 */
export function readProfileSchema(env: SchemaEnvelope | null): ProfileSchema | null {
    const s = (env?.schema ?? null) as SchemaTree | null;
    const profile = s?.$defs?.profile?.properties;
    const highwayClasses = profile?.highway?.propertyNames?.enum;
    const surfaceClasses = profile?.surface?.propertyNames?.enum;
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
            // An older schema states no climb bounds; the wire field is a `u8` either
            // way, so fall back to its full range and to climb-blind.
        climbMin: profile?.climb_weight?.minimum ?? 0,
        climbMax: profile?.climb_weight?.maximum ?? 255,
        climbDefault: profile?.climb_weight?.default ?? 0,
        defaultProfiles,
    };
}

/** The canonical shipped profiles (a fresh deep copy so edits never alias). */
export function defaultProfiles(ps: ProfileSchema): NavProfile[] {
    return deepCopy(ps.defaultProfiles);
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

/** `true` when the profile states a climb weight of its own, rather than being
 *  climb-blind by omission. */
export function hasClimbWeight(profile: NavProfile): boolean {
    return typeof profile.climb_weight === "number" && Number.isFinite(profile.climb_weight);
}

/**
 * A profile's effective climb weight — flat metres charged per metre of ascent.
 * Unlike a class multiplier this inherits nothing: an absent or non-numeric field is
 * the schema's `0`, climb-blind, which is exactly how the packer reads it.
 */
export function profileClimbWeight(profile: NavProfile, ps: ProfileSchema): number {
    return hasClimbWeight(profile) ? profile.climb_weight! : ps.climbDefault;
}

/**
 * Whether a raw climb-weight entry is admissible. The field is a `u8`, so the only
 * rules are "a whole number" and the schema's `0..255`. There is no `>= 1.0` floor,
 * because the climb term is *added* after the way-kind scaling and so can never make
 * an edge cheaper than the crow flies. `0` is a meaningful value, not an unset one.
 */
export function checkClimbWeight(n: number, min: number, max: number): { ok: boolean; hint: string | null } {
    if (!Number.isFinite(n) || !Number.isInteger(n)) {
        return { ok: false, hint: `enter a whole number between ${min} and ${max} (${min} = climb-blind).` };
    }
    if (n < min || n > max) {
        return {
            ok: false,
            hint:
                `a climb weight is a whole number between ${min} and ${max} — it's the byte the ` +
                `map carries per profile (OBCM §8.6). ${min} is climb-blind: the router costs ` +
                "ascent at nothing and plans exactly as it did before the map had terrain.",
        };
    }
    return { ok: true, hint: null };
}

/** Set a profile's climb weight (caller has already validated it). */
export function setClimbWeight(profile: NavProfile, v: number): void {
    profile.climb_weight = v;
}

/** Drop a profile's climb weight, leaving it climb-blind like an unstated field. */
export function clearClimbWeight(profile: NavProfile): void {
    delete profile.climb_weight;
}

/**
 * The effective multiplier shown in a cell: the explicit override if the class carries
 * one, otherwise the profile `default`. This mirrors the packer, which fills every wire
 * slot from `default` and overlays the map.
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
 * Whether a raw numeric entry is admissible against the schema's minimum. A value below
 * it is rejected with a hint that mirrors the packer's error text, so the CLI and the web
 * builder tell the user the same thing. This is the single copy of that message.
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

/**
 * The profiles the editor should display for a config: the config's own
 * `routing.profiles` if present, otherwise the schema's shipped defaults, shown
 * read-until-edited so an untouched CLI config stays `routing`-less.
 */
export function displayProfiles(config: PackConfig, ps: ProfileSchema): NavProfile[] {
    return config.routing?.profiles ?? defaultProfiles(ps);
}

/**
 * Materialize `config.routing` so it can be edited in place, seeding it from the schema
 * defaults when the config did not carry a routing section. Idempotent.
 */
export function ensureRouting(config: PackConfig, ps: ProfileSchema): RoutingConfig {
    if (!config.routing) {
        config.routing = { profiles: defaultProfiles(ps) };
    } else if (!Array.isArray(config.routing.profiles) || config.routing.profiles.length === 0) {
        config.routing.profiles = defaultProfiles(ps);
    }
    return config.routing;
}

/** Reset bike type `i` to its shipped weights. The order of the types is fixed. */
export function resetProfile(config: PackConfig, i: number, ps: ProfileSchema): NavProfile | null {
    const routing = ensureRouting(config, ps);
    if (i < 0 || i >= routing.profiles.length) return null;
    const replacement = deepCopy(ps.defaultProfiles[i]);
    routing.profiles[i] = replacement;
    return replacement;
}
