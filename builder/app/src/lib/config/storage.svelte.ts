import { migrateEnvelope, ENVELOPE_VERSION } from "./migrations";
import { deepCopy, normalizeConfig, type BuildablePreset, type PackConfig } from "./model";

// The working config lives in the browser (localStorage), never on the server
// — that keeps the backend stateless (no accounts needed) and survives
// reloads. Snapshot semantics: picking a preset COPIES it; edits mark the
// envelope modified ("Custom — based on X"); preset updates never silently
// change a user's maps — "Reset to preset" re-copies explicitly.

export interface WorkingEnvelope {
    schema_version: number;
    based_on: { id: string; version: number } | null;
    modified: boolean;
    config: PackConfig;
    disabled: string[];
}

const STORAGE_KEY = "obcm.working";

export class WorkingConfig {
    envelope = $state<WorkingEnvelope | null>(null);

    /** Restore from localStorage (migrating old envelopes); null if none. */
    restore(): boolean {
        try {
            const raw = localStorage.getItem(STORAGE_KEY);
            if (!raw) return false;
            const env = migrateEnvelope(JSON.parse(raw));
            if (!env) return false;
            const { config, disabled } = normalizeConfig(
                env.config as unknown as Record<string, unknown>,
            );
            this.envelope = { ...env, config, disabled: [...new Set([...env.disabled, ...disabled])] };
            return true;
        } catch {
            return false;
        }
    }

    /** Snapshot a preset into the working config (discards previous state).
     *  Takes a preset that carries a config — a catalog preset (hosted tier)
     *  names a baked artifact and has nothing to snapshot. */
    applyPreset(preset: BuildablePreset) {
        const { config, disabled } = normalizeConfig(
            preset.config as unknown as Record<string, unknown>,
        );
        this.envelope = {
            schema_version: ENVELOPE_VERSION,
            based_on: { id: preset.id, version: preset.version },
            modified: false,
            config: deepCopy(config),
            disabled,
        };
        this.persist();
    }

    /** Replace the whole envelope (import). */
    adopt(envelope: WorkingEnvelope) {
        this.envelope = envelope;
        this.persist();
    }

    /**
     * Call after any edit to the config tree (the advanced editor's hook).
     * Persisting is debounced: number inputs fire per keystroke and the
     * envelope is tens of KB of JSON.
     */
    markModified() {
        if (!this.envelope) return;
        this.envelope.modified = true;
        clearTimeout(this.saveTimer);
        this.saveTimer = setTimeout(() => this.persist(), 400);
    }

    private saveTimer: ReturnType<typeof setTimeout> | undefined;

    persist() {
        if (!this.envelope) return;
        clearTimeout(this.saveTimer);
        try {
            localStorage.setItem(STORAGE_KEY, JSON.stringify(this.envelope));
        } catch {
            // Quota/private-mode failures are non-fatal: the session still works.
        }
    }
}

export const working = new WorkingConfig();
