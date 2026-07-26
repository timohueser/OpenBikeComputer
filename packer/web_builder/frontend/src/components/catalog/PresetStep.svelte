<script lang="ts">
    // Step 2 on a tier that downloads pre-baked maps: which of the baked styles.
    //
    // The list is the manifest's `presets[]`, so it is the same on every region;
    // what changes per region is whether that pair was baked (a partial bake is
    // legitimate — OBCC §8 reports a missing preset as a warning, not an error)
    // and whether the connected device can read the result.

    import { artifactState, stylingLagsPreset } from "../../lib/catalog/availability";
    import type { DeviceMapSupport } from "../../lib/catalog/availability";
    import type { RegionEntry } from "../../lib/catalog/regions";
    import type { Preset } from "../../lib/config/model";
    import { formatBytes } from "../../lib/format";

    let {
        presets,
        entry,
        presetId,
        device,
        onpick,
    }: {
        presets: Preset[];
        entry: RegionEntry | null;
        presetId: string | null;
        device: DeviceMapSupport | null;
        onpick: (id: string) => void;
    } = $props();

    /** What this preset means for the picked region: its artifact, if baked, and
     *  whether it can be handed to what's connected. */
    function statusOf(preset: Preset) {
        const artifact = entry?.artifacts.find((a) => a.preset_id === preset.id) ?? null;
        if (!entry) return { artifact, note: null, blocked: false };
        if (!artifact) return { artifact, note: `not baked for ${entry.name}`, blocked: true };
        const state = artifactState(artifact, device);
        if (state.kind === "unsupported") {
            return { artifact, note: `needs firmware that reads OBCM v${state.artifactObcm}`, blocked: true };
        }
        // A preset the artifact predates is styling one revision behind, and
        // nothing more: §3 says a consumer MUST NOT refuse it.
        const lag = stylingLagsPreset(artifact, preset);
        return {
            artifact,
            note: lag ? `${formatBytes(artifact.bytes)} · older styling` : formatBytes(artifact.bytes),
            blocked: false,
        };
    }
</script>

<div class="cards">
    {#each presets as preset (preset.id)}
        {@const status = statusOf(preset)}
        <button
            type="button"
            class="preset"
            class:selected={presetId === preset.id}
            class:blocked={status.blocked}
            onclick={() => onpick(preset.id)}
        >
            {#if preset.preview}
                <img class="preview" src={preset.preview} alt="" />
            {/if}
            <span class="name">{preset.name}</span>
            <span class="desc small muted">{preset.description}</span>
            {#if status.note}
                <span class="note small" class:warn={status.blocked}>{status.note}</span>
            {/if}
        </button>
    {/each}
</div>

<style>
    .cards {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(140px, 1fr));
        gap: 10px;
    }

    .preset {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 7px;
        text-align: left;
        background: var(--parchment);
        border: 1px solid var(--parchment-3);
        border-radius: 12px;
        padding: 11px 12px;
        transition:
            border-color 0.15s,
            box-shadow 0.15s;
    }

    .preset:hover {
        border-color: var(--wood);
    }

    .preset.selected {
        border: 2px solid var(--forest);
        padding: 10px 11px;
        box-shadow: 0 2px 10px rgba(60, 107, 57, 0.16);
    }

    /* Baked-but-not-here stays readable and pickable: picking it is how the
       download card gets to say what's missing and offer the alternative. */
    .preset.blocked .name,
    .preset.blocked .desc {
        opacity: 0.6;
    }

    .preview {
        width: 100%;
        aspect-ratio: 4 / 3;
        object-fit: cover;
        border-radius: 8px;
        border: 1px solid var(--line);
        background: var(--parchment-2);
    }

    .name {
        font-weight: 600;
        font-size: 14px;
        color: var(--ink);
    }

    .desc {
        line-height: 1.35;
    }

    .note {
        color: var(--ink-faint);
        font-family: var(--mono);
        font-size: 11.5px;
    }

    .note.warn {
        color: var(--coral);
    }
</style>
