import type { WorkingEnvelope } from "./storage.svelte";

// Version of the localStorage envelope (and of exported files' _meta). Bump it
// when the envelope shape changes and add a migration below; stored configs
// from older app versions are upgraded on load. The bare pack-config shape is
// validated by obc-pack itself at pack time, not here.
export const ENVELOPE_VERSION = 1;

type Migration = (old: Record<string, unknown>) => Record<string, unknown>;

// index N upgrades version N -> N+1 (nothing to migrate yet at version 1).
const MIGRATIONS: Record<number, Migration> = {};

/** Upgrade a stored/imported envelope to ENVELOPE_VERSION, or null if hopeless. */
export function migrateEnvelope(raw: unknown): WorkingEnvelope | null {
    if (typeof raw !== "object" || raw === null) return null;
    let env = raw as Record<string, unknown>;
    let version = typeof env.schema_version === "number" ? env.schema_version : 1;
    while (version < ENVELOPE_VERSION) {
        const step = MIGRATIONS[version];
        if (!step) return null;
        env = step(env);
        version += 1;
    }
    if (typeof env.config !== "object" || env.config === null) return null;
    return {
        schema_version: ENVELOPE_VERSION,
        based_on: (env.based_on as WorkingEnvelope["based_on"]) ?? null,
        modified: env.modified === true,
        config: env.config as WorkingEnvelope["config"],
        disabled: Array.isArray(env.disabled) ? (env.disabled as string[]) : [],
    };
}
